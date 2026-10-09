#!/usr/bin/env bash
# Local CA-backed HTTPS/WSS acceptance. Never disables certificate verification.
set -Eeuo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
for command in docker curl jq node python3; do command -v "$command" >/dev/null || exit 1; done
docker info >/dev/null
binary=${AOMORI_BINARY:-$ROOT/target/debug/aomori}
[[ -x "$binary" ]] || { echo 'Build the node first' >&2; exit 1; }
node_port=${AOMORI_TLS_NODE_PORT:-28096}
tls_port=${AOMORI_TLS_PORT:-28443}
python3 - "$node_port" "$tls_port" <<'PY'
import socket,sys
ports=[int(p) for p in sys.argv[1:]]
assert len(set(ports))==2 and all(1024<=p<=65535 for p in ports)
for p in ports:
    with socket.socket() as s: s.bind(('127.0.0.1',p))
PY
work=$(mktemp -d)
container="aomori-tls-${$}"
pid=''
cleanup() {
  local status=$?
  docker rm -f "$container" >/dev/null 2>&1 || true
  if [[ -n "$pid" ]]; then kill -TERM "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
mkdir "$work/web"
printf '<!doctype html><title>Aomori TLS smoke</title>' > "$work/web/index.html"
# Exercise the actual example, changing only test ports/root and local issuer.
python3 - "$work" "$node_port" "$tls_port" <<'PY'
from pathlib import Path
import sys
work,port,tls=sys.argv[1:]
x=Path('deploy/Caddyfile').read_text().replace('{$AOMORI_DOMAIN}',f'https://localhost:{tls}').replace('/srv/aomori/web','/srv/web').replace('127.0.0.1:8091',f'127.0.0.1:{port}')
x=x.replace(f'https://localhost:{tls} {{',f'https://localhost:{tls} {{\n\tbind 127.0.0.1\n\ttls internal')
Path(work,'Caddyfile').write_text('{\n admin off\n auto_https disable_redirects\n}\n'+x)
PY
AOMORI_CORS_ORIGINS="https://localhost:$tls_port" AOMORI_TRUSTED_PROXIES=127.0.0.1 "$binary" --listen "127.0.0.1:$node_port" --data-dir "$work/data" --demo > "$work/node.log" 2>&1 &
pid=$!
docker run -d --name "$container" --network host -v "$work/Caddyfile:/etc/caddy/Caddyfile:ro" -v "$work/web:/srv/web:ro" caddy:2.9.1 >/dev/null
for attempt in {1..30}; do
  if docker cp "$container:/data/caddy/pki/authorities/local/root.crt" "$work/root.crt" >/dev/null 2>&1; then break; fi
  [[ "$attempt" != 30 ]] || { echo 'Local CA unavailable' >&2; exit 1; }
  sleep 1
done
base="https://localhost:$tls_port"
for attempt in {1..30}; do
  if curl --fail --silent --cacert "$work/root.crt" --max-time 2 "$base/ready" >/dev/null; then break; fi
  [[ "$attempt" != 30 ]] || { echo 'HTTPS readiness failed' >&2; exit 1; }
  sleep 1
done
curl --fail --silent --cacert "$work/root.crt" "$base/some/spa/path" | grep -q 'Aomori TLS smoke'
for path in /metrics /metrics/prometheus; do
  [[ $(curl --silent --cacert "$work/root.crt" -o /dev/null -w '%{http_code}' "$base$path") == 404 ]]
done
curl --fail --silent --cacert "$work/root.crt" -H 'content-type: application/json' -H "Origin: $base" -D "$work/headers" -d '{"jsonrpc":"2.0","id":1,"method":"aomori_get_info","params":{}}' "$base/rpc" | jq -e '.result.state_root | type == "string"' >/dev/null
grep -qi "access-control-allow-origin: $base" "$work/headers"
curl --silent --cacert "$work/root.crt" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":2,"method":"aomori_create_account","params":{"name":"unauthorized"}}' "$base/rpc" | jq -e '.error != null' >/dev/null
# Node 22+ has a built-in WebSocket client. Trust only this disposable CA;
# do not set NODE_TLS_REJECT_UNAUTHORIZED=0 or use curl -k.
NODE_EXTRA_CA_CERTS="$work/root.crt" node --input-type=module - "$tls_port" <<'JS'
const ws = new WebSocket(`wss://localhost:${process.argv[2]}/events`);
await new Promise((resolve, reject) => {
  const timer = setTimeout(() => { ws.close(); reject(new Error('WSS timeout')); }, 5000);
  ws.onopen = () => { clearTimeout(timer); ws.close(); resolve(); };
  ws.onerror = () => { clearTimeout(timer); reject(new Error('WSS handshake failed')); };
});
JS
echo 'TLS smoke passed: trusted local CA, HTTPS RPC/CORS/auth, SPA fallback, private metrics, WSS handshake'
