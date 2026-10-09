# Local non-blockchain closure evidence

This supplements the [development checklist](development-status.md). Measurements describe this development host and workload only, not production capacity or long-duration acceptance.

## Docker and configuration

Direct Docker access failed because the developer user is not in the socket's group. The existing `sudo -n` permission allowed these tests without changing group membership or daemon configuration:

```bash
sudo -n env AOMORI_SMOKE_PORT=28091 bash scripts/docker-smoke.sh
sudo -n docker run --rm -e AOMORI_DOMAIN=mud.example.com -v "$PWD/deploy/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2.9.1 caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
sudo -n docker run --rm --entrypoint promtool -v "$PWD/deploy/monitoring:/etc/prometheus:ro" prom/prometheus:v3.2.1 check config /etc/prometheus/prometheus.yml
sudo -n docker run --rm --entrypoint promtool -v "$PWD/deploy/monitoring:/etc/prometheus:ro" prom/prometheus:v3.2.1 check rules /etc/prometheus/alerts.yml
sudo -n bash scripts/monitoring-smoke.sh
```

All passed. Caddy reported an advisory about the explicit forwarding header; it is kept explicit to document the proxy boundary. Validation does not issue a public certificate or verify TLS traffic.

The node Docker smoke checked runtime UID, read-only root filesystem, dropped capabilities, writable snapshot and unchanged snapshot across container restart. The monitoring smoke checked a real node scrape (`up == 1`), four loaded rules, eight provisioned Grafana panels and a healthy datasource. After the node stopped, `AomoriNodeUnavailable` became `firing`. It did not send a notification. Temporary nodes, Compose containers and volumes were removed.

## Three-minute read/connection resource baseline

```bash
cd web
AOMORI_SOAK_SECONDS=180 AOMORI_SOAK_CONNECTIONS=100 npm run test:e2e -- e2e/node-lifecycle.spec.ts -g 'sustains concurrent'
```

Passed with 100 simultaneously held sockets and batches of 16 read-only RPCs. Socket establishment is sequential. World state root remained unchanged and socket cleanup completed.

The node PID was sampled using `ps -p PID -o rss=,%cpu=` every five seconds; snapshot size used `stat -c %s /tmp/aomori-e2e/state.json`. There were 36 samples spanning seconds 5–184 of test startup/run:

| Measurement | First | Last | Maximum |
| --- | ---: | ---: | ---: |
| Node RSS (KiB) | 25,648 | 26,304 | 26,304 |
| Node process CPU (%) | 15.0 | 19.5 | 19.5 |
| Snapshot size (bytes) | 7,525 | 7,525 | 7,525 |

`ps %cpu` is a process-lifetime average, not an instantaneous CPU gauge. RSS rose by 656 KiB over this short run; this does not establish a memory leak or prove bounded long-term memory. Snapshot stability is expected for a read-only workload. No write throughput, latency percentile, browser RSS, ENOSPC, torn-write or power-loss result is implied. Repeat hour/day read/write workloads and collect resource time series on staging before capacity sign-off.

## Remaining external closure

GitHub's workflow API still reports CI as `disabled_manually`. No remote CI pass is claimed. Enable the workflow with an authorized account, then run it for the final commit. Actual TLS/DNS, organization notification receivers, long-duration staging resource measurements and destructive fault drills remain separate acceptance items.
