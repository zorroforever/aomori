#!/usr/bin/env bash
# Real ENOSPC in an 8 MiB container tmpfs, never on the host filesystem.
set -Eeuo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
for command in docker jq; do command -v "$command" >/dev/null || exit 1; done
docker info >/dev/null
image=${AOMORI_DISK_SMOKE_IMAGE:-aomori:disk-smoke}
container="aomori-disk-${$}"
trap 'docker rm -f "$container" >/dev/null 2>&1 || true' EXIT
docker build -t "$image" . >/dev/null
docker run -d --name "$container" --read-only --cap-drop ALL --security-opt no-new-privileges \
  --network none --tmpfs /data:rw,size=8m,uid=10001,gid=10001,mode=0700 \
  --entrypoint /bin/sh "$image" -c 'sleep infinity' >/dev/null
start_node() {
  docker exec -d "$container" sh -c 'echo $$ > /data/node.pid; exec /usr/local/bin/aomori --listen 127.0.0.1:8091 --data-dir /data --demo --allow-unsigned-commands'
  for attempt in {1..30}; do
    if docker exec "$container" curl --fail --silent --max-time 2 http://127.0.0.1:8091/ready >/dev/null; then return 0; fi
    sleep 1
  done
  echo 'Node readiness timed out' >&2
  return 1
}
rpc() {
  docker exec "$container" curl --fail --silent --max-time 5 http://127.0.0.1:8091/rpc -H 'content-type: application/json' -d "$1"
}
info='{"jsonrpc":"2.0","id":1,"method":"aomori_get_info","params":{}}'
events='{"jsonrpc":"2.0","id":2,"method":"aomori_get_events","params":{"since":0,"limit":500}}'
account='{"jsonrpc":"2.0","id":3,"method":"aomori_get_account","params":{"name":"admin"}}'
transaction='{"jsonrpc":"2.0","id":4,"method":"aomori_submit_transaction","params":{"from":"admin","nonce":0,"entity_id":4,"action":"talk","args":{"npc_id":6},"signature":null}}'
start_node
before_info=$(rpc "$info" | jq -cS '.result')
before_events=$(rpc "$events" | jq -cS '.result')
before_account=$(rpc "$account" | jq -cS '.result')
before_hash=$(docker exec "$container" sha256sum /data/state.json)
# dd must specifically report ENOSPC; any other failure fails this drill.
fill_error=$(docker exec "$container" sh -c 'dd if=/dev/zero of=/data/fill bs=65536 2>&1' || true)
grep -q 'No space left on device' <<<"$fill_error"
rpc "$transaction" | jq -e '.error != null' >/dev/null
[[ $(rpc "$info" | jq -cS '.result') == "$before_info" ]]
[[ $(rpc "$events" | jq -cS '.result') == "$before_events" ]]
[[ $(rpc "$account" | jq -cS '.result') == "$before_account" ]]
[[ $(docker exec "$container" sha256sum /data/state.json) == "$before_hash" ]]
docker exec "$container" rm /data/fill
receipt=$(rpc "$transaction" | jq -ce '.result | select(.ok == true)')
tx_id=$(jq -r '.tx_id' <<<"$receipt")
rpc "$account" | jq -e '.result.nonce == 1' >/dev/null
after_info=$(rpc "$info" | jq -cS '.result')
after_events=$(rpc "$events" | jq -cS '.result')
# Restart the process, NOT the container: tmpfs vanishes on container restart.
docker exec "$container" sh -c 'kill -TERM "$(cat /data/node.pid)"'
for attempt in {1..30}; do
  if ! docker exec "$container" sh -c 'kill -0 "$(cat /data/node.pid)"' 2>/dev/null; then break; fi
  [[ "$attempt" != 30 ]] || { echo 'Node failed to stop' >&2; exit 1; }
  sleep 1
done
start_node
[[ $(rpc "$info" | jq -cS '.result') == "$after_info" ]]
[[ $(rpc "$events" | jq -cS '.result') == "$after_events" ]]
rpc "$account" | jq -e '.result.nonce == 1' >/dev/null
query=$(jq -nc --arg id "$tx_id" '{jsonrpc:"2.0",id:5,method:"aomori_get_receipt",params:{tx_id:$id}}')
[[ $(rpc "$query" | jq -cS '.result') == "$(jq -cS . <<<"$receipt")" ]]
echo 'Disk-full smoke passed: actual ENOSPC, snapshot/state/nonce/event rollback, retry and process-restart receipt recovery'
