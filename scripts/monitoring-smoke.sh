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
alert_port=${AOMORI_MONITOR_ALERT_PORT:-29093}
webhook_port=${AOMORI_MONITOR_WEBHOOK_PORT:-29094}
python3 - "$node_port" "$prom_port" "$graf_port" "$alert_port" "$webhook_port" <<'PY'
import socket,sys
ports=[int(x) for x in sys.argv[1:]]
assert len(set(ports)) == 5 and all(1024 <= p <= 65535 for p in ports), 'invalid ports'
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
webhook_pid=''
compose=(docker compose --env-file "$work/environment" -p "$project" -f "$ROOT/deploy/monitoring/compose.yaml" -f "$work/override.yaml")
cleanup() {
  local status=$?
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  if [[ -n "$pid" ]]; then kill -TERM "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  if [[ -n "$webhook_pid" ]]; then kill -TERM "$webhook_pid" 2>/dev/null || true; wait "$webhook_pid" 2>/dev/null || true; fi
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
cat >> "$work/prometheus.yml" <<EOF
alerting:
  alertmanagers:
    - static_configs:
        - targets: ['127.0.0.1:$alert_port']
EOF
cat > "$work/alertmanager.yml" <<EOF
route:
  receiver: local-smoke
  group_by: [alertname]
  group_wait: 1s
  group_interval: 1s
  repeat_interval: 1h
receivers:
  - name: local-smoke
    webhook_configs:
      - url: http://127.0.0.1:$webhook_port/alerts
        send_resolved: true
EOF
# The receiver is loopback-only, acknowledges valid payloads, and retains only
# alert identity/status. No external notification services are contacted.
python3 "$ROOT/scripts/alert-webhook.py" "$webhook_port" "$work/notifications.jsonl" > "$work/webhook.log" 2>&1 &
webhook_pid=$!
cat > "$work/override.yaml" <<EOF
services:
  prometheus:
    command:
      - --config.file=/etc/prometheus/prometheus.yml
      - --web.listen-address=127.0.0.1:$prom_port
      - --storage.tsdb.retention.time=1d
    volumes:
      - $work/prometheus.yml:/etc/prometheus/prometheus.yml:ro
  alertmanager:
    image: prom/alertmanager:v0.28.1
    network_mode: host
    command:
      - --config.file=/etc/alertmanager/alertmanager.yml
      - --web.listen-address=127.0.0.1:$alert_port
      - --cluster.listen-address=
    volumes:
      - $work/alertmanager.yml:/etc/alertmanager/alertmanager.yml:ro
    security_opt: [no-new-privileges:true]
    cap_drop: [ALL]
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
wait_http "http://127.0.0.1:$alert_port/-/ready"
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
# minute plus scrape/evaluation intervals. Verify delivery and recovery below.
kill -TERM "$pid"
wait "$pid"
pid=''
for attempt in {1..100}; do
  if curl --fail --silent --max-time 3 "http://127.0.0.1:$prom_port/api/v1/alerts" | jq -e 'any(.data.alerts[]; .labels.alertname == "AomoriNodeUnavailable" and .state == "firing")' >/dev/null; then break; fi
  [[ "$attempt" != 100 ]] || { echo 'Node-down alert did not fire' >&2; exit 1; }
  sleep 1
done
wait_notification() {
  local expected=$1
  for attempt in {1..90}; do
    if [[ -f "$work/notifications.jsonl" ]] && jq -se --arg status "$expected" 'any(.[]; .alertname == "AomoriNodeUnavailable" and .status == $status)' "$work/notifications.jsonl" >/dev/null; then return 0; fi
    sleep 1
  done
  echo "Node-down notification not delivered: $expected" >&2
  return 1
}
wait_notification firing
"$binary" --listen "127.0.0.1:$node_port" --data-dir "$work/data" --demo > "$work/node.log" 2>&1 &
pid=$!
wait_http "http://127.0.0.1:$node_port/ready"
wait_notification resolved
echo 'Monitoring smoke passed: live scrape, four rules, Grafana provisioning, firing and resolved webhook notifications'
