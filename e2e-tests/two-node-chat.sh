#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

WORKDIR="$(mktemp -d)"
PID_A=""
PID_B=""
cleanup() {
  for pid in "${PID_A}" "${PID_B}"; do
    if [[ -n "${pid}" ]] && kill -0 "${pid}" 2>/dev/null; then
      kill "${pid}" 2>/dev/null || true
      wait "${pid}" 2>/dev/null || true
    fi
  done
  rm -rf "${WORKDIR}"
}
trap cleanup EXIT

cat >"${WORKDIR}/a.toml" <<EOF
data_dir = "${WORKDIR}/a-data"
bind_addr = "127.0.0.1:17747"
public_url = "http://127.0.0.1:17747"
EOF
cat >"${WORKDIR}/b.toml" <<EOF
data_dir = "${WORKDIR}/b-data"
bind_addr = "127.0.0.1:17748"
public_url = "http://127.0.0.1:17748"
EOF

cargo run --quiet -p hivemind-node -- --config "${WORKDIR}/a.toml" >"${WORKDIR}/a.log" 2>&1 &
PID_A="$!"
cargo run --quiet -p hivemind-node -- --config "${WORKDIR}/b.toml" >"${WORKDIR}/b.log" 2>&1 &
PID_B="$!"

for port in 17747 17748; do
  ready=0
  for _ in $(seq 1 200); do
    if curl -fsS "http://127.0.0.1:${port}/health" >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 0.1
  done
  [[ "${ready}" == 1 ]] || { cat "${WORKDIR}"/*.log >&2; exit 1; }
done

HIVEMIND_NODE_URL=http://127.0.0.1:17747 cargo run --quiet -p hivemind-cli -- join http://127.0.0.1:17748 >/dev/null
curl -fsS http://127.0.0.1:17747/v1/peers | grep 17748 >/dev/null
curl -fsS http://127.0.0.1:17748/v1/peers | grep 17747 >/dev/null
curl -fsS http://127.0.0.1:17747/v1/peers | grep '"trusted":false' >/dev/null

HIVEMIND_NODE_URL=http://127.0.0.1:17747 cargo run --quiet -p hivemind-cli -- say "untrusted route should not receive this" >/dev/null
sleep 0.5
if HIVEMIND_NODE_URL=http://127.0.0.1:17748 cargo run --quiet -p hivemind-cli -- chat | grep "untrusted route should not receive this" >/dev/null; then
  echo "untrusted peer received chat before trust" >&2
  exit 1
fi

NODE_A_ID="$(curl -fsS http://127.0.0.1:17747/v1/node | python3 -c 'import sys,json; print(json.load(sys.stdin)["node_id"])')"
NODE_B_ID="$(curl -fsS http://127.0.0.1:17748/v1/node | python3 -c 'import sys,json; print(json.load(sys.stdin)["node_id"])')"
HIVEMIND_NODE_URL=http://127.0.0.1:17747 cargo run --quiet -p hivemind-cli -- peer trust "${NODE_B_ID}" >/dev/null
HIVEMIND_NODE_URL=http://127.0.0.1:17748 cargo run --quiet -p hivemind-cli -- peer trust "${NODE_A_ID}" >/dev/null

HIVEMIND_NODE_URL=http://127.0.0.1:17747 cargo run --quiet -p hivemind-cli -- say "hello from mutually trusted node a" >/dev/null

for _ in $(seq 1 40); do
  if HIVEMIND_NODE_URL=http://127.0.0.1:17748 cargo run --quiet -p hivemind-cli -- chat | grep "hello from mutually trusted node a" >/dev/null; then
    echo "two-node chat ok"
    exit 0
  fi
  sleep 0.25
done

echo "message did not arrive" >&2
cat "${WORKDIR}"/*.log >&2
exit 1
