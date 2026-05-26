#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

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

cat >"${WORKDIR}/node.toml" <<EOF
data_dir = "${WORKDIR}/data"
bind_addr = "127.0.0.1:17847"
public_url = "http://127.0.0.1:17847"
EOF

cargo run --quiet -p hivemind-node -- --config "${WORKDIR}/node.toml" >"${WORKDIR}/node.log" 2>&1 &
PID="$!"

ready=0
for _ in $(seq 1 200); do
  if curl -fsS "http://127.0.0.1:17847/health" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
[[ "${ready}" == 1 ]] || { cat "${WORKDIR}/node.log" >&2; exit 1; }

FAKE_ID="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
python3 - <<PY
import socket
msg = b"HIVEMIND_NODE_V2 http://127.0.0.1:19999 ${FAKE_ID}"
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.sendto(msg, ("127.0.0.1", 7748))
sock.close()
PY

for _ in $(seq 1 40); do
  if curl -fsS http://127.0.0.1:17847/v1/peers | grep "${FAKE_ID}" | grep '"trusted":false' >/dev/null; then
    echo "discovery beacon ok"
    exit 0
  fi
  sleep 0.25
done

echo "fake discovery beacon was not stored" >&2
cat "${WORKDIR}/node.log" >&2
exit 1
