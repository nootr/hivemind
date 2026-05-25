#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

log() {
  echo "==> $*"
}

require_command cargo
require_command curl
require_command python3

WORKDIR="$(mktemp -d)"
PID=""

cleanup() {
  if [[ -n "${PID}" ]] && kill -0 "${PID}" 2>/dev/null; then
    kill "${PID}" 2>/dev/null || true
    wait "${PID}" 2>/dev/null || true
  fi
  rm -rf "${WORKDIR}"
}
trap cleanup EXIT

CONFIG="${WORKDIR}/node.toml"
DATA_DIR="${WORKDIR}/data"
LOG="${WORKDIR}/node.log"
cat > "${CONFIG}" <<EOF
[data]
dir = "${DATA_DIR}"

[api]
bind_addr = "127.0.0.1:17747"
auth_token_file = "${DATA_DIR}/api.token"

[identity]
agent_key_path = "${DATA_DIR}/agent.ed25519"
EOF

log "created temporary node config at ${CONFIG}"
log "starting hivemind-node on http://127.0.0.1:17747"
cargo run --quiet -p hivemind-node -- --config "${CONFIG}" >"${LOG}" 2>&1 &
PID="$!"

READY=0
for _ in $(seq 1 600); do
  if curl --silent --fail http://127.0.0.1:17747/health >/dev/null 2>&1; then
    READY=1
    break
  fi
  if ! kill -0 "${PID}" 2>/dev/null; then
    echo "hivemind-node exited early" >&2
    cat "${LOG}" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ "${READY}" != "1" ]]; then
  echo "hivemind-node did not become ready" >&2
  cat "${LOG}" >&2
  exit 1
fi
log "node is healthy"

TOKEN="$(tr -d '\n' < "${DATA_DIR}/api.token")"
log "loaded bearer token from temporary data dir"

PUBLISH_BODY="$(python3 - <<'PY'
import base64
import json
print(json.dumps({
    "object_type": "fact",
    "mime_type": "text/plain",
    "payload_base64": base64.b64encode(b"hello from hivemind").decode("ascii"),
    "tags": ["demo", "rust"],
}))
PY
)"

log "publishing fact object with tags: demo, rust"
PUBLISH_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${PUBLISH_BODY}" \
  http://127.0.0.1:17747/v1/objects)"

PARENT_OBJECT_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["object_id"])' <<<"${PUBLISH_RESPONSE}")"
log "published parent object: ${PARENT_OBJECT_ID}"

CHILD_PUBLISH_BODY="$(PARENT_OBJECT_ID="${PARENT_OBJECT_ID}" python3 - <<'PY'
import base64
import json
import os
print(json.dumps({
    "object_type": "insight",
    "mime_type": "text/plain",
    "payload_base64": base64.b64encode(b"this insight references the parent fact").decode("ascii"),
    "tags": ["demo-child"],
    "references": [os.environ["PARENT_OBJECT_ID"]],
}))
PY
)"

log "publishing child insight that references parent"
CHILD_PUBLISH_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${CHILD_PUBLISH_BODY}" \
  http://127.0.0.1:17747/v1/objects)"

CHILD_OBJECT_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["object_id"])' <<<"${CHILD_PUBLISH_RESPONSE}")"
log "published child object: ${CHILD_OBJECT_ID}"

CHUNKED_PUBLISH_BODY="$(python3 - <<'PY'
import base64
import json
payload = bytes([9]) * (16 * 1024 + 1)
print(json.dumps({
    "object_type": "fact",
    "mime_type": "application/octet-stream",
    "payload_base64": base64.b64encode(payload).decode("ascii"),
    "tags": ["chunked"],
}))
PY
)"

log "publishing chunked object"
CHUNKED_PUBLISH_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${CHUNKED_PUBLISH_BODY}" \
  http://127.0.0.1:17747/v1/objects)"

CHUNK_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["chunk_ids"][0])' <<<"${CHUNKED_PUBLISH_RESPONSE}")"
log "published chunk: ${CHUNK_ID}"

log "retrieving parent object by ID"
GET_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:17747/v1/objects/${PARENT_OBJECT_ID}")"

log "retrieving child object by ID"
CHILD_GET_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:17747/v1/objects/${CHILD_OBJECT_ID}")"

log "exporting parent object envelope CBOR"
ENVELOPE_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:17747/v1/objects/${PARENT_OBJECT_ID}/envelope")"

IMPORT_ENVELOPE_BODY="$(ENVELOPE_RESPONSE="${ENVELOPE_RESPONSE}" python3 - <<'PY'
import json
import os
envelope = json.loads(os.environ["ENVELOPE_RESPONSE"])
print(json.dumps({"envelope_cbor_base64": envelope["envelope_cbor_base64"]}))
PY
)"

log "importing exported parent envelope idempotently"
IMPORT_ENVELOPE_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${IMPORT_ENVELOPE_BODY}" \
  http://127.0.0.1:17747/v1/objects/envelope)"

log "looking up objects by exact tag: demo"
TAG_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  http://127.0.0.1:17747/v1/tags/demo)"

log "looking up referrers for parent"
REFERRERS_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:17747/v1/objects/${PARENT_OBJECT_ID}/referrers")"

log "retrieving chunk by ID"
CHUNK_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:17747/v1/chunks/${CHUNK_ID}")"

PUT_CHUNK_BODY="$(CHUNK_RESPONSE="${CHUNK_RESPONSE}" python3 - <<'PY'
import json
import os
chunk = json.loads(os.environ["CHUNK_RESPONSE"])
print(json.dumps({"bytes_base64": chunk["bytes_base64"]}))
PY
)"

log "importing retrieved chunk idempotently"
PUT_CHUNK_RESPONSE="$(curl --silent --fail \
  -X PUT \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${PUT_CHUNK_BODY}" \
  "http://127.0.0.1:17747/v1/chunks/${CHUNK_ID}")"

GET_RESPONSE="${GET_RESPONSE}" python3 - "${PARENT_OBJECT_ID}" <<'PY'
import base64
import json
import os
import sys
object_id = sys.argv[1]
body = json.loads(os.environ["GET_RESPONSE"])
assert body["object_id"] == object_id
assert body["object_type"] == "fact"
assert base64.b64decode(body["payload_base64"]) == b"hello from hivemind"
assert body["tags"] == ["demo", "rust"]
assert body["references"] == []
assert body["verified"] is True
PY

CHILD_GET_RESPONSE="${CHILD_GET_RESPONSE}" python3 - "${CHILD_OBJECT_ID}" "${PARENT_OBJECT_ID}" <<'PY'
import base64
import json
import os
import sys
child_id = sys.argv[1]
parent_id = sys.argv[2]
body = json.loads(os.environ["CHILD_GET_RESPONSE"])
assert body["object_id"] == child_id
assert body["object_type"] == "insight"
assert base64.b64decode(body["payload_base64"]) == b"this insight references the parent fact"
assert body["references"] == [parent_id]
assert body["verified"] is True
PY

ENVELOPE_RESPONSE="${ENVELOPE_RESPONSE}" python3 - "${PARENT_OBJECT_ID}" <<'PY'
import base64
import json
import os
import sys
object_id = sys.argv[1]
body = json.loads(os.environ["ENVELOPE_RESPONSE"])
assert body["object_id"] == object_id
assert body["object_type"] == "fact"
assert body["mime_type"] == "text/plain"
assert body["tags"] == ["demo", "rust"]
assert body["references"] == []
assert body["payload_size"] == len(b"hello from hivemind")
assert body["chunk_count"] == 0
assert len(base64.b64decode(body["envelope_cbor_base64"])) > 0
assert body["chunk_ids"] == []
assert body["chunks"] == []
assert body["verified"] is True
PY

IMPORT_ENVELOPE_RESPONSE="${IMPORT_ENVELOPE_RESPONSE}" python3 - "${PARENT_OBJECT_ID}" <<'PY'
import json
import os
import sys
object_id = sys.argv[1]
body = json.loads(os.environ["IMPORT_ENVELOPE_RESPONSE"])
assert body["object_id"] == object_id
assert body["chunk_ids"] == []
PY

TAG_RESPONSE="${TAG_RESPONSE}" python3 - "${PARENT_OBJECT_ID}" <<'PY'
import json
import os
import sys
object_id = sys.argv[1]
body = json.loads(os.environ["TAG_RESPONSE"])
assert body["tag"] == "demo"
assert len(body["objects"]) == 1
assert body["objects"][0]["object_id"] == object_id
assert body["objects"][0]["object_type"] == "fact"
PY

REFERRERS_RESPONSE="${REFERRERS_RESPONSE}" python3 - "${PARENT_OBJECT_ID}" "${CHILD_OBJECT_ID}" <<'PY'
import json
import os
import sys
parent_id = sys.argv[1]
child_id = sys.argv[2]
body = json.loads(os.environ["REFERRERS_RESPONSE"])
assert body["object_id"] == parent_id
assert len(body["objects"]) == 1
assert body["objects"][0]["object_id"] == child_id
assert body["objects"][0]["object_type"] == "insight"
PY

CHUNK_RESPONSE="${CHUNK_RESPONSE}" python3 - "${CHUNK_ID}" <<'PY'
import base64
import json
import os
import sys
chunk_id = sys.argv[1]
body = json.loads(os.environ["CHUNK_RESPONSE"])
assert body["chunk_id"] == chunk_id
assert body["size"] == 16 * 1024 + 1
assert base64.b64decode(body["bytes_base64"]) == bytes([9]) * (16 * 1024 + 1)
assert body["verified"] is True
PY

PUT_CHUNK_RESPONSE="${PUT_CHUNK_RESPONSE}" python3 - "${CHUNK_ID}" <<'PY'
import json
import os
import sys
chunk_id = sys.argv[1]
body = json.loads(os.environ["PUT_CHUNK_RESPONSE"])
assert body["chunk_id"] == chunk_id
assert body["size"] == 16 * 1024 + 1
assert body["verified"] is True
PY

log "verified retrieved payloads, envelope/chunk import, tag lookup, backlink response and chunk retrieval"
echo "local demo ok: ${PARENT_OBJECT_ID} <- ${CHILD_OBJECT_ID}; chunk ${CHUNK_ID}"
