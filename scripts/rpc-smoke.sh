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

AOMORI_ADMIN_TOKEN="$token" \
AOMORI_CORS_ORIGINS="http://127.0.0.1:5173" \
"$binary" --listen "127.0.0.1:$port" --data-dir "$data_dir" \
  >"$log_file" 2>&1 &
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
echo "RPC smoke test passed: port=$port"
