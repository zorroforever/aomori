#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"

for command in curl jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command command is required" >&2
    exit 1
  fi
done

binary="${AOMORI_BINARY:-$ROOT_DIR/target/debug/aomori}"
if [[ ! -x "$binary" ]]; then
  echo "executable AOMORI_BINARY is required: $binary" >&2
  exit 1
fi

port="${AOMORI_RPC_SMOKE_PORT:-28092}"
data_dir=$(mktemp -d)
log_file=$(mktemp)
token=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')
pid=""
cleanup() {
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$data_dir" "$log_file"
}
trap cleanup EXIT

start_node() {
  AOMORI_ADMIN_TOKEN="$token" \
  AOMORI_CORS_ORIGINS="http://127.0.0.1:5173" \
  "$binary" --listen "127.0.0.1:$port" --data-dir "$data_dir" --demo "$@" \
    >>"$log_file" 2>&1 &
  pid=$!

  for attempt in {1..60}; do
    if curl --fail --silent "http://127.0.0.1:$port/ready" >/dev/null; then
      break
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      cat "$log_file" >&2
      exit 1
    fi
    if [[ "$attempt" == 60 ]]; then
      cat "$log_file" >&2
      exit 1
    fi
    sleep 1
  done
}

start_node

curl --fail --silent "http://127.0.0.1:$port/health" | jq -e '.ok == true' >/dev/null
curl --fail --silent "http://127.0.0.1:$port/ready" | jq -e '.ready == true' >/dev/null
metrics=$(curl --fail --silent "http://127.0.0.1:$port/metrics")
if grep -Fq "$token" <<<"$metrics"; then
  echo "admin token leaked through JSON metrics" >&2
  exit 1
fi

rpc() {
  local auth_header="${1:-}"
  local payload="$2"
  if [[ -n "$auth_header" ]]; then
    curl --fail --silent "http://127.0.0.1:$port/rpc" \
      -H 'content-type: application/json' -H "$auth_header" -d "$payload"
  else
    curl --fail --silent "http://127.0.0.1:$port/rpc" \
      -H 'content-type: application/json' -d "$payload"
  fi
}

create_payload='{"jsonrpc":"2.0","id":1,"method":"aomori_create_account","params":{"name":"smoke-player"}}'
rpc '' "$create_payload" | jq -e '.error.code == -32002' >/dev/null
rpc 'authorization: Bearer incorrect-token' "$create_payload" | jq -e '.error.code == -32002' >/dev/null
rpc "authorization: Bearer $token" "$create_payload" | jq -e '.result.name == "smoke-player"' >/dev/null

command_payload='{"jsonrpc":"2.0","id":2,"method":"aomori_command","params":{"entity_id":4,"action":"accept","args":{"npc_id":6}}}'
rpc '' "$command_payload" | jq -e '.error.code == -32002' >/dev/null

metrics=$(curl --fail --silent "http://127.0.0.1:$port/metrics")
prometheus=$(curl --fail --silent "http://127.0.0.1:$port/metrics/prometheus")
for surface in "$metrics" "$prometheus"; do
  if grep -Fq "$token" <<<"$surface"; then
    echo "admin token leaked through metrics" >&2
    exit 1
  fi
done
if grep -Fq "$token" "$log_file"; then
  echo "admin token leaked through server logs" >&2
  exit 1
fi

test -f "$data_dir/state.json"
# Generate events on a temporary development-mode node only after verifying
# that the default configuration rejects unsigned commands.
kill -TERM "$pid"
wait "$pid"
pid=""
start_node --allow-unsigned-commands
talk_payload='{"jsonrpc":"2.0","id":7,"method":"aomori_submit_transaction","params":{"from":"admin","nonce":0,"entity_id":4,"action":"talk","args":{"npc_id":6},"signature":null}}'
talk_receipt=$(rpc '' "$talk_payload" | jq -ceS '.result | select(.ok == true)')
receipt_payload=$(jq -nc --arg tx_id "$(jq -r '.tx_id' <<<"$talk_receipt")" '{jsonrpc:"2.0",id:8,method:"aomori_get_receipt",params:{tx_id:$tx_id}}')
before_receipt=$(rpc '' "$receipt_payload" | jq -ceS '.result')
info_payload='{"jsonrpc":"2.0","id":3,"method":"aomori_get_info","params":{}}'
events_payload='{"jsonrpc":"2.0","id":4,"method":"aomori_get_events","params":{"since":0,"limit":500}}'
account_payload='{"jsonrpc":"2.0","id":5,"method":"aomori_get_account","params":{"name":"smoke-player"}}'
capture_state() {
  before_info=$(rpc '' "$info_payload" | jq -ceS '.result | {head, state_root}')
  before_events=$(rpc '' "$events_payload" | jq -ceS '.result')
  before_account=$(rpc '' "$account_payload" | jq -ceS '.result')
  before_receipt=$(rpc '' "$receipt_payload" | jq -ceS '.result')
  jq -e '.events | length > 0' <<<"$before_events" >/dev/null
}

verify_state() {
  test "$(rpc '' "$info_payload" | jq -ceS '.result | {head, state_root}')" = "$before_info"
  test "$(rpc '' "$events_payload" | jq -ceS '.result')" = "$before_events"
  test "$(rpc '' "$account_payload" | jq -ceS '.result')" = "$before_account"
  test "$(rpc '' "$receipt_payload" | jq -ceS '.result')" = "$before_receipt"
  local latest cursor_payload
  latest=$(jq -r '.latest' <<<"$before_events")
  cursor_payload=$(jq -nc --argjson latest "$latest" '{jsonrpc:"2.0",id:6,method:"aomori_get_events",params:{since:$latest,limit:500}}')
  rpc '' "$cursor_payload" | jq -e --argjson latest "$latest" '.result | (.events | length == 0) and .next == $latest and .latest == $latest' >/dev/null
}

capture_state

kill -TERM "$pid"
wait "$pid"
pid=""
start_node
rpc '' "$command_payload" | jq -e '.error.code == -32002' >/dev/null
rpc '' "$talk_payload" | jq -e '.error.code == -32002' >/dev/null

verify_state

# Acknowledge another write, then kill the development node without giving it
# a graceful shutdown opportunity. SIGKILL is not a power-loss simulation.
kill -TERM "$pid"
wait "$pid"
pid=""
start_node --allow-unsigned-commands
capture_state
crash_payload=$(jq -c '.params.nonce = 1' <<<"$talk_payload")
crash_receipt=$(rpc '' "$crash_payload" | jq -ceS '.result | select(.ok == true)')
kill -KILL "$pid"
crash_status=0
wait "$pid" 2>/dev/null || crash_status=$?
pid=""
test "$crash_status" -eq 137
start_node

# Read expectations come from the acknowledged response and pre-write state,
# rather than from a final snapshot taken after a graceful shutdown.
crash_receipt_payload=$(jq -nc --arg tx_id "$(jq -r '.tx_id' <<<"$crash_receipt")" '{jsonrpc:"2.0",id:9,method:"aomori_get_receipt",params:{tx_id:$tx_id}}')
test "$(rpc '' "$crash_receipt_payload" | jq -ceS '.result')" = "$crash_receipt"
admin_payload='{"jsonrpc":"2.0","id":11,"method":"aomori_get_account","params":{"name":"admin"}}'
rpc '' "$admin_payload" | jq -e '.result.nonce == 2' >/dev/null
old_head=$(jq -r '.head' <<<"$before_info")
rpc '' "$info_payload" | jq -e --argjson head "$old_head" --arg root "$(jq -r '.state_root' <<<"$crash_receipt")" '.result | .head == ($head + 1) and .state_root == $root' >/dev/null
test "$(rpc '' "$account_payload" | jq -ceS '.result')" = "$before_account"
test "$(rpc '' "$receipt_payload" | jq -ceS '.result')" = "$before_receipt"
latest=$(jq -r '.latest' <<<"$before_events")
crash_events=$(rpc '' "$events_payload" | jq -ceS '.result')
jq -e --argjson old "$before_events" '.events[0:($old.events | length)] == $old.events and .latest > $old.latest' <<<"$crash_events" >/dev/null
cursor_payload=$(jq -nc --argjson latest "$latest" '{jsonrpc:"2.0",id:10,method:"aomori_get_events",params:{since:$latest,limit:500}}')
rpc '' "$cursor_payload" | jq -e --argjson latest "$latest" '.result | (.events | length > 0) and all(.events[]; .id > $latest) and .next == .latest' >/dev/null
rpc '' "$command_payload" | jq -e '.error.code == -32002' >/dev/null
rpc '' "$crash_payload" | jq -e '.error.code == -32002' >/dev/null
if grep -Fq "$token" "$log_file"; then
  echo "admin token leaked through restart logs" >&2
  exit 1
fi
echo "RPC smoke test passed (including graceful restart and SIGKILL recovery): port=$port"
