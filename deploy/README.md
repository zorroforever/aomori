# Single-node deployment and acceptance

These are deployment examples for an experimental runtime, not a claim of production or blockchain readiness. Keep unsigned commands disabled outside temporary development tests.

## TLS and Web client (same Linux host)

1. Build the node and Web client, install the systemd service as described in [operations](../doc/operations.md), and copy `web/dist/` to `/srv/aomori/web`.
2. Build with `VITE_AOMORI_RPC=https://mud.example.com` (replace with your hostname); otherwise the Web default uses port 8091 rather than the TLS proxy.
3. Set `AOMORI_CORS_ORIGINS=https://mud.example.com` on the node. Keep the node bound to `127.0.0.1:8091`. With systemd, `AOMORI_TRUSTED_PROXIES=127.0.0.1` may be used with the supplied proxy configuration; verify the peer IP when using Docker NAT before trusting it.
4. Install Caddy. Set `AOMORI_DOMAIN=mud.example.com` in its service environment and use `deploy/Caddyfile`. Public DNS must resolve to the host; certificate issuance requires reachable ports 80/443. Do not use the example hostname unchanged.
5. Validate before reload: `AOMORI_DOMAIN=mud.example.com caddy validate --config deploy/Caddyfile --adapter caddyfile`.
6. Check HTTPS `/health`, `/ready`, JSON-RPC `/rpc`, and WebSocket `/events`. Public `/metrics` and `/metrics/prometheus` must return 404. Do not expose the direct node port.

Run `./scripts/tls-smoke.sh` after building the node, with Docker and Node 22+ available (or `sudo -n env PATH="$PATH" bash scripts/tls-smoke.sh` when Docker requires sudo). It uses the actual Caddy example with disposable local certificates, ports and static files. Certificate verification remains enabled. It checks HTTPS RPC/CORS/auth, SPA fallback, metrics 404 and WSS handshake; `AOMORI_TLS_NODE_PORT` and `AOMORI_TLS_PORT` override default loopback ports 28096/28443. This does not validate public DNS or public certificate issuance.

The proxy overwrites client forwarding headers, serves the SPA, and excludes monitoring endpoints. It does not add user authentication; admin RPC still requires the node token. Never embed the token in the Web build. No public DNS/certificate deployment has been verified in the development workspace.

## Monitoring (same Linux host)

The example uses Linux host networking and binds both monitoring UIs to loopback. It is not a Docker Desktop configuration.

```bash
export GRAFANA_ADMIN_PASSWORD='replace-with-a-unique-secret'
docker compose -f deploy/monitoring/compose.yaml config --quiet
docker compose -f deploy/monitoring/compose.yaml up -d
```

Prometheus scrapes `127.0.0.1:8091/metrics/prometheus`. Grafana provisions the datasource and the single-node dashboard automatically. Access via SSH tunnels, not public ports:

```bash
ssh -L 3000:127.0.0.1:3000 -L 9090:127.0.0.1:9090 user@host
```

Rules cover scrape failures, snapshot failures, event lag, and elevated RPC error ratio. They do **not** deliver notifications until an authenticated Alertmanager endpoint and receiver routing are configured for your organization. Scrape availability is not equivalent to application readiness; check `/ready` separately. Duration panels show means, not invented p95/p99 values. Review image versions/security updates before deployment.

Validate rules with `promtool check config` and `promtool check rules`, or the CI configuration-validation job. A local Compose parse does not verify running Grafana/Prometheus.

After `cargo build --locked`, run `./scripts/monitoring-smoke.sh` on a Docker-capable Linux host (use `sudo -n bash scripts/monitoring-smoke.sh` if required by local Docker permissions). It uses a disposable node, random Grafana credentials, dedicated Compose project/volumes and loopback ports 28095/29090/23000/29093/29094; override them via `AOMORI_MONITOR_NODE_PORT`, `AOMORI_MONITOR_PROM_PORT`, `AOMORI_MONITOR_GRAF_PORT`, `AOMORI_MONITOR_ALERT_PORT`, `AOMORI_MONITOR_WEBHOOK_PORT`. It checks live scraping, rule loading, datasource/dashboard provisioning, and Alertmanager firing/resolved notification delivery to a temporary local webhook, then cleans up. The test takes around two to three minutes once images are cached and is included in CI. It sends no external notifications and does not alter the deployment example's receiver configuration. Receiver input validation can also be checked with `python3 scripts/test_alert_webhook.py`. See [local evidence](../doc/acceptance-local.md).

## Restart, crash and offline restore drill

```bash
source "$HOME/.cargo/env"
cargo build --locked
./scripts/rpc-smoke.sh
```

This uses disposable directories only. It verifies normal restart, SIGKILL immediately after an acknowledged transaction, and a full offline archive restore. The restore retains a separate copy of post-backup data and verifies the restored state excludes an account created after backup. It never restores over a running node. Follow [operations](../doc/operations.md) for real backups, permissions, and rollback; archives contain private world/account data and require restricted access. The script also obstructs the snapshot temporary-file path to verify rollback on a real filesystem write error and successful retry after the obstruction is removed. This is not ENOSPC, a power-loss or a torn-write simulation.

## Isolated disk-full drill

Run `./scripts/disk-full-smoke.sh` with Docker access (or `sudo -n bash scripts/disk-full-smoke.sh`). It builds the node image and fills an isolated 8 MiB container tmpfs until `dd` reports ENOSPC. It then verifies transaction failure preserves the snapshot hash, world info, account nonce and event history; freeing space permits the same transaction to succeed, with its receipt retained across a process restart. The temporary container has no network, runs as non-root and is removed on exit. No host filesystem is filled. Unsigned transactions are enabled only in this disposable test container.

The node process is restarted inside the same running container because restarting the container would discard tmpfs. This tests application behavior on real ENOSPC, not physical disk failure or power-loss durability. The script is included in the Docker CI job. Set `AOMORI_DISK_SMOKE_IMAGE` to override the test image tag.

## Browser restart and concurrency/soak

```bash
cd web
npm ci
npx playwright install chromium
npm run test:e2e -- e2e/node-lifecycle.spec.ts
AOMORI_SOAK_SECONDS=3600 AOMORI_SOAK_CONNECTIONS=100 npm run test:e2e -- e2e/node-lifecycle.spec.ts -g 'sustains concurrent'
```

The regular gate runs a five-second baseline with 32 simultaneous sockets and batches of 16 concurrent reads. Sockets are opened sequentially to avoid browser handshake throttling; this is a sustained-connection test, not a connection-storm benchmark. The opt-in soak supports 1–86,400 seconds and 1–1,000 sockets. It checks read-only state-root stability, rate-limit handling, connection continuity and cleanup; it is not a throughput benchmark or an unbounded-memory guarantee. Observe process RSS, CPU, snapshot latency, and resource limits separately during extended staging runs. Browser logs retain 500 rows; events and compensation buffers retain 200 entries. Node world event/receipt history still grows with committed writes in the JSON snapshot model; there is no automatic retention policy.

The browser restart case owns an isolated real node/data directory and verifies SIGKILL/reconnect, preserved cursor/state, and a second signed transaction. Ports are dynamically allocated, with a small bind race possible between reservation and node startup.

## Isolated sustained read/write and resource report

```bash
cargo build --locked
python3 scripts/runtime-soak.py --seconds 3600 --readers 8 --interval 0.5 --max-rss-mib 512 --max-snapshot-mib 64 --report /tmp/aomori-soak-hour.json
```

Linux `/proc` is required. The script owns a temporary node/data directory and free loopback port, submits transactions serially while querying concurrently, checks read snapshots against the before/after committed state, replays every event page, and restarts to verify acknowledged state, nonce, a canonical SHA-256 digest of the complete ordered event history, and logarithmically spaced receipt checkpoints including the first and final transactions. The digest is test evidence, not the protocol state root. History is hashed page-by-page without keeping all events in memory. Unsigned writes are enabled only on this temporary node. It does not connect to your deployed node. Supported settings: 1–86,400 seconds, 1–32 readers, 0.1–60 seconds between batches. The report path must not exist; failures retain `status: failed` and return nonzero. Optional `--max-rss-mib` and `--max-snapshot-mib` must be positive integers; sampled budget breaches fail the run and retain the offending sample plus peak/growth summaries. The example's 512/64 MiB budgets are illustrative operator choices, not measured production capacity. Sampling is every roughly five seconds plus start/end, so short spikes may be missed. Budgets do not enforce limits on the actual node and snapshot budgets exclude backups, temporary files and logs. Run `python3 scripts/test_runtime_soak.py` after building to verify success/failure report semantics. Reports contain RSS, cumulative process CPU seconds, snapshot sizes, counts and maximum observed batch duration (not an RPC latency percentile). CI uploads its 10-second baseline report even when a later step fails.

Observe write-driven RSS/snapshot growth rather than expecting constant memory: this runtime retains event and receipt history. A local three-minute run is documented in [local evidence](../doc/acceptance-local.md); an hour/day run has not yet been completed. Data is cleaned on exit and only the requested report remains.

## Release checks

Run Rust fmt/test/check/clippy, Web build/E2E, RPC smoke, Docker smoke and deployment-config validation. Require the matching remote commit's checks to complete before claiming CI acceptance. Local test success is not remote CI evidence. See [the development checklist](../doc/development-status.md) for remaining environment-dependent acceptance.
