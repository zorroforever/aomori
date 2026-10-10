# Non-blockchain development and acceptance checklist

This checklist replaces informal percentage estimates. It covers the agreed single-node runtime/Web reliability and deployment-example work, not every possible future storage or performance feature. A local test pass does not imply production readiness.

## Implemented and locally verified

| Area | Evidence / scope |
| --- | --- |
| Runtime, Lua, transaction rollback, signatures, nonce, receipts, snapshot format/migration | Rust tests; fmt, check and clippy |
| Encrypted identities, import/export, refresh/multi-account recovery, key cleanup and key-rotation protection | Web E2E |
| Command busy guards, RPC generation cancellation, HTTP/network/timeouts, unknown receipt reconciliation | Web E2E, including real incomplete HTTP responses |
| Event duplicate/order handling, multi-page recovery, failure/reconnect/cursor rollback, bounded event buffers | Web E2E |
| Bounded visible logs and unsafe event cursor rejection | Logs retain 500 rows; visible events/buffer 200 entries |
| Browser connected to a real restarting node | Isolated node SIGKILL, automatic WebSocket reconnect, second signed transaction, cursor/root checks |
| Concurrent read/connection baseline | Five-second regular gate, 32 simultaneous sockets, 16-request batches |
| Opt-in sustained concurrency | Local 180-second run with 100 simultaneous sockets passed; isolated 180-second read/write soak also passed (260 writes, 2,080 concurrent reads, 780 replayed events and restart verification); samples in [local acceptance record](acceptance-local.md); not an hour/day guarantee |
| Normal restart and acknowledged-write SIGKILL recovery | RPC smoke verifies root, account nonce, events, receipts and default auth policy |
| Offline full-directory backup restore | RPC smoke verifies matching state and exclusion of post-backup writes |
| Interrupted snapshot artifact handling | Storage tests simulate partial and complete uncommitted temporary files; real binary startup verifies missing-primary backup recovery across two starts and rejects invalid backup without creating a new world |
| Filesystem write failure and retry | RPC smoke blocks the snapshot temporary path; Docker disk-full smoke tests real ENOSPC in isolated 8 MiB tmpfs, state/snapshot/nonce/event rollback, retry and receipt recovery |
| README/documentation language navigation and accurate prototype positioning | English, Chinese and Japanese README/navigation |
| Docker runtime | Smoke passed using existing sudo permission; non-root image, read-only root, capability removal, persistent restart verified |
| Deployment config validation | Caddy validate and Prometheus config/four-rule checks passed in containers |
| Local TLS proxy runtime | Trusted local CA, HTTPS RPC/CORS/auth, WSS handshake, SPA fallback and metrics 404 checked; smoke caught and fixed a routing-order defect |
| Monitoring runtime | Disposable monitoring smoke passed: live scrape, Grafana datasource/dashboard, Alertmanager firing and resolved webhook delivery; resources cleaned |

## Delivered, environment acceptance still required

| Area | Delivered | Remaining acceptance |
| --- | --- | --- |
| TLS proxy | Caddy SPA/RPC/WebSocket configuration; forwarding chain overwrite; metrics exclusion | Validate Caddy and certificates on deployment host, public DNS, HTTPS/WebSocket connectivity |
| Monitoring | Prometheus scrape/rules and Grafana provisioning verified in running containers | Configure/test organization-specific Alertmanager receivers and staging persistence |
| Docker | Hardened image/Compose/runtime smoke verified locally via sudo | Repeat on the target deployment host |
| Remote CI | Existing checks plus deployment-config validation job | Remote CI passed for commit b916192; confirm the matching run for subsequent commits |
| Long-duration/resource acceptance | Configurable soak (seconds/connections), run instructions | Hour/day runs and RSS/CPU/disk growth measurements on staging hardware |
| Disk/power-loss acceptance | Deterministic storage/RPC failure tests plus real temporary-path obstruction | Interrupted filesystem writes and host power loss on disposable infrastructure; ENOSPC already verified locally |

## Verification at this handoff

- Final full Web E2E: 48 tests passed locally, including sequential socket establishment. A separate 100-socket/60-second run also passed.
- Current Rust suite includes monitoring/TLS deployment contracts (104 tests total).
- Rust fmt/check/clippy, Web build, shell syntax, RPC smoke, YAML/JSON parsing and monitoring Compose parse passed.
- Counts are the handoff baseline; use actual test output if more tests are added.
- Docker runtime/config validation and monitoring smoke passed during this closure. Remote CI passed for commit b916192, including Web E2E and the accumulated deployment/fault checks after fixing account reuse/stale receipt assertions. Independent tests no longer share serial retry grouping; native stdout is preserved in failure artifacts. Subsequent commits require their own matching remote results.

## Explicitly deferred

Blocks, consensus, P2P, multi-node sync, Gas and economic mechanisms are out of scope. RocksDB, snapshot compression, query coalescing and performance tuning are also future work rather than implied requirements: select them based on measured workloads. JSON world/event/receipt history grows with committed writes; Web DOM bounds do not make node storage bounded.

## Operator closure

1. CI is enabled again. Trigger/verify a push, PR or manual Actions → CI → Run workflow for the final commit, and inspect all job results. Re-enabling a workflow does not retroactively run old pushes.
2. Run Docker smoke and deployment validators on a Docker-capable host; do not bypass failures.
3. Follow [deployment steps](../deploy/README.md) for TLS, monitoring, offline backup and staging soak. Set unique credentials and verify expected firewall/loopback boundaries.
4. Record real measurements and environment-specific acceptance. Only then declare deployment acceptance complete.
