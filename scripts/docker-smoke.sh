#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"

for command in docker curl openssl awk; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "$command command is required" >&2
    exit 1
  fi
done
if ! docker compose version >/dev/null 2>&1; then
  echo "Docker Compose plugin is required" >&2
  exit 1
fi
if ! docker info >/dev/null 2>&1; then
  echo "Docker daemon is unavailable or permission was denied" >&2
  exit 1
fi

project="aomori-smoke-${$}"
port="${AOMORI_SMOKE_PORT:-18091}"
tmp_env=$(mktemp)
trap 'docker compose -p "$project" down --volumes --remove-orphans >/dev/null 2>&1 || true; rm -f "$tmp_env"' EXIT

cat >"$tmp_env" <<EOF
AOMORI_ADMIN_TOKEN=$(openssl rand -hex 32)
AOMORI_PUBLISH_ADDRESS=127.0.0.1
AOMORI_PORT=$port
AOMORI_CORS_ORIGINS=http://127.0.0.1:5173
AOMORI_RPC_RATE_LIMIT=100
AOMORI_TRUSTED_PROXIES=
EOF

compose=(docker compose --env-file "$tmp_env" -p "$project")
"${compose[@]}" build aomori
"${compose[@]}" up -d aomori

wait_for_ready() {
  local attempt
  for attempt in {1..60}; do
    if curl --fail --silent "http://127.0.0.1:${port}/ready" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  "${compose[@]}" logs aomori >&2 || true
  return 1
}

wait_for_ready
curl --fail --silent "http://127.0.0.1:${port}/health" >/dev/null

container_id=$("${compose[@]}" ps -q aomori)
test -n "$container_id"
uid=$(docker inspect --format '{{.Config.User}}' "$container_id")
test "$uid" = "10001:10001"
readonly_root=$(docker inspect --format '{{.HostConfig.ReadonlyRootfs}}' "$container_id")
test "$readonly_root" = "true"
cap_drop=$(docker inspect --format '{{json .HostConfig.CapDrop}}' "$container_id")
test "$cap_drop" = '["ALL"]'

# Confirm the service can persist its normal snapshot under the only writable volume.
docker exec "$container_id" test -f /data/state.json
before=$(docker exec "$container_id" sha256sum /data/state.json | awk '{print $1}')
"${compose[@]}" down
"${compose[@]}" up -d aomori
wait_for_ready
container_id=$("${compose[@]}" ps -q aomori)
test -n "$container_id"
after=$(docker exec "$container_id" sha256sum /data/state.json | awk '{print $1}')
test "$before" = "$after"

echo "Docker smoke test passed: project=$project port=$port"
