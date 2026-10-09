#!/usr/bin/env bash
# Disposable Linux same-host monitoring acceptance; no public listeners.
set -Eeuo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
for command in docker curl jq openssl python3; do
  command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 1; }
done
docker info >/dev/null
docker compose version >/dev/null
binary=${AOMORI_BINARY:-$ROOT/target/debug/aomori}
[[ -x "$binary" ]] || { echo "Build the node with cargo build --locked first" >&2; exit 1; }
node_port=${AOMORI_MONITOR_NODE_PORT:-28095}
prom_port=${AOMORI_MONITOR_PROM_PORT:-29090}
graf_port=${AOMORI_MONITOR_GRAF_PORT:-23000}
python3 - "$node_port" "$prom_port" "$graf_port" <<'PY'
import socket,sys
ports=[int(x) for x in sys.argv[1:]]
assert len(set(ports)) == 3 and all(1024 <= p <= 65535 for p in ports), 'invalid ports'
sockets=[]
try:
    for p in ports:
        s=socket.socket(); sockets.append(s); s.bind(('127.0.0.1',p))
finally:
    for s in sockets: s.close()
PY
work=$(mktemp -d)
project="aomori-monitor-${$}"
pid=''
compose=(docker compose --env-file "$work/environment" -p "$project" -f "$ROOT/deploy/monitoring/compose.yaml" -f "$work/override.yaml")
cleanup() {
  local status=$?
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  if [[ -n "$pid" ]]; then kill -TERM "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# Secret goes in restricted temporary files, never command arguments/output.
password=$(openssl rand -hex 32)
printf 'GRAFANA_ADMIN_PASSWORD=%s\n' "$password" > "$work/environment"
printf 'user = "admin:%s"\n' "$password" > "$work/curl-auth"
unset password
chmod 600 "$work/environment" "$work/curl-auth"
# Keep tracked configs intact; override only isolated listeners and scrape target.
sed "s/127.0.0.1:8091/127.0.0.1:$node_port/" deploy/monitoring/prometheus.yml > "$work/prometheus.yml"
cat > "$work/override.yaml" <<EOF
services:
  prometheus:
    command:
      - --config.file=/etc/prometheus/prometheus.yml
      - --web.listen-address=127.0.0.1:$prom_port
      - --storage.tsdb.retention.time=1d
    volumes:
      - $work/prometheus.yml:/etc/prometheus/prometheus.yml:ro
  grafana:
    environment:
      GF_SERVER_HTTP_PORT: "$graf_port"
    volumes:
      - $work/datasource.yml:/etc/grafana/provisioning/datasources/prometheus.yml:ro
EOF
sed "s/127.0.0.1:9090/127.0.0.1:$prom_port/" deploy/monitoring/provisioning/datasources/prometheus.yml > "$work/datasource.yml"
"$binary" --listen "127.0.0.1:$node_port" --data-dir "$work/data" --demo > "$work/node.log" 2>&1 &
pid=$!
wait_http() {
  local url=$1
  for attempt in {1..90}; do
    if curl --fail --silent --max-time 2 "$url" >/dev/null; then return 0; fi
    sleep 1
  done
  echo "Timed out waiting for $url" >&2
  return 1
}
wait_http "http://127.0.0.1:$node_port/ready"
"${compose[@]}" up -d
wait_http "http://127.0.0.1:$prom_port/-/ready"
wait_http "http://127.0.0.1:$graf_port/api/health"
for attempt in {1..30}; do
  if curl --fail --silent --max-time 3 --get --data-urlencode 'query=up{job="aomori"}' "http://127.0.0.1:$prom_port/api/v1/query" | jq -e '.status == "success" and (.data.result | length) == 1 and .data.result[0].value[1] == "1"' >/dev/null; then break; fi
  [[ "$attempt" != 30 ]] || { echo 'Node scrape did not become healthy' >&2; exit 1; }
  sleep 1
done
curl --fail --silent --max-time 5 "http://127.0.0.1:$prom_port/api/v1/rules" | jq -e '[.data.groups[].rules[]] | length == 4' >/dev/null
curl --fail --silent --max-time 5 --config "$work/curl-auth" "http://127.0.0.1:$graf_port/api/dashboards/uid/aomori-node" | jq -e '.dashboard.panels | length == 8' >/dev/null
curl --fail --silent --max-time 5 --config "$work/curl-auth" "http://127.0.0.1:$graf_port/api/datasources/uid/aomori-prometheus/health" | jq -e '.status == "OK"' >/dev/null
# Verify the down rule actually fires, not just that it parses. This takes one
# minute plus scrape/evaluation intervals. Notification delivery is not tested.
kill -TERM "$pid"
wait "$pid"
pid=''
for attempt in {1..100}; do
  if curl --fail --silent --max-time 3 "http://127.0.0.1:$prom_port/api/v1/alerts" | jq -e 'any(.data.alerts[]; .labels.alertname == "AomoriNodeUnavailable" and .state == "firing")' >/dev/null; then break; fi
  [[ "$attempt" != 100 ]] || { echo 'Node-down alert did not fire' >&2; exit 1; }
  sleep 1
done
echo 'Monitoring smoke passed: live scrape, four rules, provisioned dashboard/datasource, node-down alert firing'
