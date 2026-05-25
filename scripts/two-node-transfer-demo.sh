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
NODE_A_PID=""
NODE_B_PID=""

cleanup() {
  for pid in "${NODE_A_PID}" "${NODE_B_PID}"; do
    if [[ -n "${pid}" ]] && kill -0 "${pid}" 2>/dev/null; then
      kill "${pid}" 2>/dev/null || true
      wait "${pid}" 2>/dev/null || true
    fi
  done
  rm -rf "${WORKDIR}"
}
trap cleanup EXIT

write_config() {
  local name="$1"
  local port="$2"
  local config="${WORKDIR}/${name}.toml"
  local data_dir="${WORKDIR}/${name}-data"
  cat > "${config}" <<EOF
[data]
dir = "${data_dir}"

[api]
bind_addr = "127.0.0.1:${port}"
auth_token_file = "${data_dir}/api.token"

[identity]
agent_key_path = "${data_dir}/agent.ed25519"
EOF
}

wait_for_node() {
  local name="$1"
  local port="$2"
  local pid="$3"
  local log_file="$4"
  local ready=0

  for _ in $(seq 1 600); do
    if curl --silent --fail "http://127.0.0.1:${port}/health" >/dev/null 2>&1; then
      ready=1
      break
    fi
    if ! kill -0 "${pid}" 2>/dev/null; then
      echo "${name} exited early" >&2
      cat "${log_file}" >&2
      exit 1
    fi
    sleep 0.1
  done

  if [[ "${ready}" != "1" ]]; then
    echo "${name} did not become ready" >&2
    cat "${log_file}" >&2
    exit 1
  fi
}

write_config node-a 17747
write_config node-b 17748

NODE_A_LOG="${WORKDIR}/node-a.log"
NODE_B_LOG="${WORKDIR}/node-b.log"

log "starting source node A on http://127.0.0.1:17747"
cargo run --quiet -p hivemind-node -- --config "${WORKDIR}/node-a.toml" >"${NODE_A_LOG}" 2>&1 &
NODE_A_PID="$!"

log "starting target node B on http://127.0.0.1:17748"
cargo run --quiet -p hivemind-node -- --config "${WORKDIR}/node-b.toml" >"${NODE_B_LOG}" 2>&1 &
NODE_B_PID="$!"

wait_for_node node-a 17747 "${NODE_A_PID}" "${NODE_A_LOG}"
wait_for_node node-b 17748 "${NODE_B_PID}" "${NODE_B_LOG}"
log "both nodes are healthy"

NODE_A_TOKEN="$(tr -d '\n' < "${WORKDIR}/node-a-data/api.token")"
NODE_B_TOKEN="$(tr -d '\n' < "${WORKDIR}/node-b-data/api.token")"

PUBLISH_BODY="$(python3 - <<'PY'
import base64
import json
payload = b"two node transfer payload:" + bytes([42]) * (16 * 1024 + 1)
print(json.dumps({
    "object_type": "fact",
    "mime_type": "application/octet-stream",
    "payload_base64": base64.b64encode(payload).decode("ascii"),
    "tags": ["two-node", "transfer"],
}))
PY
)"

log "publishing chunked object on node A"
PUBLISH_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${NODE_A_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${PUBLISH_BODY}" \
  http://127.0.0.1:17747/v1/objects)"

OBJECT_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["object_id"])' <<<"${PUBLISH_RESPONSE}")"
PUBLISHED_CHUNK_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["chunk_ids"][0])' <<<"${PUBLISH_RESPONSE}")"
log "node A published object ${OBJECT_ID} with chunk ${PUBLISHED_CHUNK_ID}"

log "verifying node B does not have the object before transfer"
PRE_TRANSFER_STATUS="$(curl --silent --output /dev/null --write-out "%{http_code}" \
  -H "Authorization: Bearer ${NODE_B_TOKEN}" \
  "http://127.0.0.1:17748/v1/objects/${OBJECT_ID}")"
if [[ "${PRE_TRANSFER_STATUS}" != "404" ]]; then
  echo "expected node B to return 404 before transfer, got ${PRE_TRANSFER_STATUS}" >&2
  exit 1
fi

log "exporting object envelope from node A"
ENVELOPE_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${NODE_A_TOKEN}" \
  "http://127.0.0.1:17747/v1/objects/${OBJECT_ID}/envelope")"
read -r CHUNK_ID CHUNK_SIZE < <(ENVELOPE_RESPONSE="${ENVELOPE_RESPONSE}" python3 - <<'PY'
import json
import os
envelope = json.loads(os.environ["ENVELOPE_RESPONSE"])
chunk = envelope["chunks"][0]
assert envelope["chunk_ids"] == [chunk["chunk_id"]]
print(chunk["chunk_id"], chunk["size"])
PY
)
if [[ "${CHUNK_ID}" != "${PUBLISHED_CHUNK_ID}" ]]; then
  echo "envelope chunk id ${CHUNK_ID} did not match publish response ${PUBLISHED_CHUNK_ID}" >&2
  exit 1
fi
log "using transfer chunk from envelope metadata: ${CHUNK_ID} (${CHUNK_SIZE} bytes)"

log "retrieving chunk from node A"
CHUNK_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${NODE_A_TOKEN}" \
  "http://127.0.0.1:17747/v1/chunks/${CHUNK_ID}")"

PUT_CHUNK_BODY="$(CHUNK_RESPONSE="${CHUNK_RESPONSE}" python3 - <<'PY'
import json
import os
chunk = json.loads(os.environ["CHUNK_RESPONSE"])
print(json.dumps({"bytes_base64": chunk["bytes_base64"]}))
PY
)"

log "importing chunk into node B"
curl --silent --fail \
  -X PUT \
  -H "Authorization: Bearer ${NODE_B_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${PUT_CHUNK_BODY}" \
  "http://127.0.0.1:17748/v1/chunks/${CHUNK_ID}" >/dev/null

IMPORT_ENVELOPE_BODY="$(ENVELOPE_RESPONSE="${ENVELOPE_RESPONSE}" python3 - <<'PY'
import json
import os
envelope = json.loads(os.environ["ENVELOPE_RESPONSE"])
print(json.dumps({"envelope_cbor_base64": envelope["envelope_cbor_base64"]}))
PY
)"

log "importing envelope into node B"
IMPORT_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${NODE_B_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${IMPORT_ENVELOPE_BODY}" \
  http://127.0.0.1:17748/v1/objects/envelope)"

log "retrieving transferred object from node B"
GET_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${NODE_B_TOKEN}" \
  "http://127.0.0.1:17748/v1/objects/${OBJECT_ID}")"

log "looking up transferred object by tag on node B"
TAG_RESPONSE="$(curl --silent --fail \
  -H "Authorization: Bearer ${NODE_B_TOKEN}" \
  http://127.0.0.1:17748/v1/tags/two-node)"

IMPORT_RESPONSE="${IMPORT_RESPONSE}" python3 - "${OBJECT_ID}" "${CHUNK_ID}" <<'PY'
import json
import os
import sys
object_id = sys.argv[1]
chunk_id = sys.argv[2]
body = json.loads(os.environ["IMPORT_RESPONSE"])
assert body["object_id"] == object_id
assert body["chunk_ids"] == [chunk_id]
PY

GET_RESPONSE="${GET_RESPONSE}" python3 - "${OBJECT_ID}" <<'PY'
import base64
import json
import os
import sys
object_id = sys.argv[1]
expected = b"two node transfer payload:" + bytes([42]) * (16 * 1024 + 1)
body = json.loads(os.environ["GET_RESPONSE"])
assert body["object_id"] == object_id
assert body["object_type"] == "fact"
assert body["mime_type"] == "application/octet-stream"
assert base64.b64decode(body["payload_base64"]) == expected
assert body["tags"] == ["two-node", "transfer"]
assert body["verified"] is True
PY

TAG_RESPONSE="${TAG_RESPONSE}" python3 - "${OBJECT_ID}" <<'PY'
import json
import os
import sys
object_id = sys.argv[1]
body = json.loads(os.environ["TAG_RESPONSE"])
assert body["tag"] == "two-node"
assert len(body["objects"]) == 1
assert body["objects"][0]["object_id"] == object_id
PY

log "verified node B can retrieve and discover transferred object"
echo "two-node transfer ok: ${OBJECT_ID} via chunk ${CHUNK_ID}"
