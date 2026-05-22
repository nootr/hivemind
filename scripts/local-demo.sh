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

OBJECT_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["object_id"])' <<<"${PUBLISH_RESPONSE}")"
log "published object: ${OBJECT_ID}"

log "retrieving object by ID"
GET_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:17747/v1/objects/${OBJECT_ID}")"

log "looking up objects by exact tag: demo"
TAG_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${TOKEN}" \
  http://127.0.0.1:17747/v1/tags/demo)"

GET_RESPONSE="${GET_RESPONSE}" python3 - "${OBJECT_ID}" <<'PY'
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
assert body["verified"] is True
PY

TAG_RESPONSE="${TAG_RESPONSE}" python3 - "${OBJECT_ID}" <<'PY'
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

log "verified retrieved payload and tag lookup response"
echo "local demo ok: ${OBJECT_ID}"
