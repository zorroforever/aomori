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

The node Docker smoke checked runtime UID, read-only root filesystem, dropped capabilities, writable snapshot and unchanged snapshot across container restart. The monitoring smoke checked a real node scrape (`up == 1`), four loaded rules, eight provisioned Grafana panels and a healthy datasource. After the node stopped, `AomoriNodeUnavailable` became `firing`. The extended smoke starts a temporary Alertmanager and loopback webhook; it verifies delivery of the firing notification and, after the same node/data directory restarts, delivery of the resolved notification. The local receiver accepts only bounded JSON requests and retains only alert name/status; four protocol tests cover valid transitions, malformed messages, oversized requests and unknown paths. No external notification channels are contacted. Temporary nodes, receivers, Compose containers and volumes were removed.

## Local TLS proxy runtime

`sudo -n env PATH="$PATH" bash scripts/tls-smoke.sh` passed with a temporary Caddy internal CA. Both curl and Node trusted the copied root certificate; certificate verification was never disabled. Checks covered HTTPS RPC, allowed-origin CORS, unauthenticated admin-write rejection, SPA routing, WSS handshake and HTTP 404 for both monitoring paths.

The first run caught a real routing-order defect: a standalone `respond @metrics` was bypassed by the SPA fallback and returned HTML with status 200. The metrics rejection now has its own `handle @metrics` branch; the runtime test passes and is part of CI. Public certificate issuance remains unverified.

## Real ENOSPC rollback and recovery

`sudo -n bash scripts/disk-full-smoke.sh` passed. The node ran non-root with no container network and an isolated 8 MiB tmpfs. Filling this filesystem produced an actual `No space left on device` error; a transaction then failed without changing snapshot hash, world info/root, admin nonce or event history. Removing the filler allowed the same transaction to commit. Restarting only the node process preserved world info, events, nonce and the full receipt. The container was removed on exit.

This fills no host disk and proves neither torn-write nor physical power-loss behavior. Container restart is intentionally not used because it discards tmpfs.

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

## Three-minute isolated concurrent read/write baseline

`python3 scripts/runtime-soak.py --seconds 180 --report /tmp/aomori-write-soak-final.json` passed using a fresh debug node/data directory, eight readers and a 0.5-second delay between write batches. It submitted 260 transactions, completed 2,080 concurrent reads, replayed 780 events over multiple pages, and verified final state/nonce/receipt after graceful restart. Observed maximum concurrent batch duration was 291.431 ms; this is not an individual RPC latency percentile.

35 resource samples were collected. Initial/final RSS was 11,100/82,176 KiB (maximum 82,176 KiB); snapshot size was 7,525/288,779 bytes; final cumulative node CPU was 52.17 seconds. The workload intentionally grows event/receipt history. This does not establish a memory leak or bounded long-term memory. It demonstrates why write workloads need separate capacity measurement from read-only soaks. Hour/day acceptance remains open. The script supports longer runs and emits an explicit passed/failed JSON report; CI retains its ten-second baseline report. Optional sampled RSS and primary-snapshot budgets now fail explicitly, preserving the offending sample and peak/growth summaries. Four integration tests verify successful restart/resource reporting, an actual 1 MiB RSS-budget failure, protection against report overwrite, and invalid-budget rejection. These are sampled acceptance budgets, not enforced node resource limits; temporary/backup/log storage is not included in snapshot size. Restart verification now compares a canonical SHA-256 digest of every event across all pages and verifies power-of-two receipt checkpoints plus the final receipt. Six Python tests cover the report semantics, pagination/key-order-independent digests, content changes and rejected event gaps/duplicates. A ten-second live baseline passed after the change; this digest is test evidence and not the protocol's state root.

## Interrupted snapshot artifact recovery

Four storage tests simulate incomplete temporary/backup/restore/rollback files, a complete but uncommitted temporary snapshot, completed primary replacement, and missing primary with valid/invalid backup. Valid primary snapshots win over temporary artifacts. Missing primary now restores a validated backup rather than silently initializing genesis; invalid backup fails closed. Restoring a backup may lose the newest commit, so this is not a zero-loss guarantee. These tests construct disk states deterministically; they do not terminate a process inside filesystem writes or simulate host power loss. Rust's 93 tests, clippy/build and restart/restore RPC smoke passed after the fix. Two additional real-binary startup tests now verify readiness and the exact recovered head/root across two starts after deleting the primary and leaving a partial temporary file, and a nonzero startup exit with no new primary when the backup is corrupt. The full Rust suite now contains 95 tests. Remote CI for b916192 completed successfully; newer commits still require matching runs.

## Read/write workload with confirmed-write SIGKILL recovery

`python3 scripts/runtime-soak.py --seconds 10 --restart-mode sigkill --report /tmp/aomori-kill-history-soak.json` passed with 18 writes and 144 concurrent reads. The node was killed after acknowledgment and full pre-crash history replay, exited with -9, and restarted with identical state/account, complete event-history digest and sampled receipt checkpoints. Seven Python tests now exercise both restart modes and resource/report/history validation. Remote CI for d4afca3 passed; this new revision requires its own run. This is not interruption during snapshot writes or physical power loss.

## Remaining external closure

GitHub's workflow API now reports CI as `active`, after the owner re-enabled it. Run 37912972291 passed Rust, Web build, Docker, deployment config and RPC/monitoring smoke, but failed Web E2E. No overall remote CI pass is claimed. CI-mode local E2E passed 48/48; native test stdout is now captured in failure artifacts for diagnosis. Subsequent investigation reproduced the network-recovery case failing on repeated execution because it reused a persisted account, and found a second assertion could read a prior SUCCESS receipt before the new transaction completed. Fixed account names are now unique per attempt, the independent cases no longer use serial retry grouping, and the final assertion waits for the real submission and nonce. The signed-transaction file passed twice consecutively (38 cases) in CI mode. Checkout/setup-node/upload-artifact were also upgraded to v6 after confirming their metadata uses Node 24; the project's Node 22 test version is unchanged. A new push or manual workflow dispatch can trigger that run. Run 37914860649 for commit 9069efc subsequently completed successfully, confirming the action-runtime upgrade and signed-transaction test fixes remotely. New commits still require matching CI results. Actual TLS/DNS, organization notification receivers, long-duration staging resource measurements and destructive fault drills remain separate acceptance items.
