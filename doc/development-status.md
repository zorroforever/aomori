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
| Opt-in sustained concurrency | Local 60-second run with 100 simultaneous sockets passed; sockets established sequentially to avoid browser handshake throttling |
| Normal restart and acknowledged-write SIGKILL recovery | RPC smoke verifies root, account nonce, events, receipts and default auth policy |
| Offline full-directory backup restore | RPC smoke verifies matching state and exclusion of post-backup writes |
| Filesystem write failure and retry | RPC smoke blocks the snapshot temporary path in a disposable directory, verifies rollback and retry |
| README/documentation language navigation and accurate prototype positioning | English, Chinese and Japanese README/navigation |

## Delivered, environment acceptance still required

| Area | Delivered | Remaining acceptance |
| --- | --- | --- |
| TLS proxy | Caddy SPA/RPC/WebSocket configuration; forwarding chain overwrite; metrics exclusion | Validate Caddy and certificates on deployment host, public DNS, HTTPS/WebSocket connectivity |
| Monitoring | Prometheus scrape/rules, Grafana provisioning and eight-panel dashboard, loopback-only services | Run containers, inspect datasource/dashboard and configure/test organization-specific Alertmanager receivers |
| Docker | Existing hardened node image/Compose/smoke plus monitoring Compose | Local Docker daemon unavailable; runtime smoke and config validators cannot run here |
| Remote CI | Existing checks plus deployment-config validation job | GitHub API reported CI `disabled_manually`; owner must enable workflow, trigger a run and confirm the matching commit's checks |
| Long-duration/resource acceptance | Configurable soak (seconds/connections), run instructions | Hour/day runs and RSS/CPU/disk growth measurements on staging hardware |
| Disk/power-loss acceptance | Deterministic storage/RPC failure tests plus real temporary-path obstruction | ENOSPC, interrupted filesystem writes and host power loss on disposable infrastructure |

## Verification at this handoff

- Final full Web E2E: 48 tests passed locally, including sequential socket establishment. A separate 100-socket/60-second run also passed.
- Rust: 85 tests passed locally, including deployment configuration contracts.
- Rust fmt/check/clippy, Web build, shell syntax, RPC smoke, YAML/JSON parsing and monitoring Compose parse passed.
- Counts are the handoff baseline; use actual test output if more tests are added.
- Docker runtime/config validation and remote CI are **not** marked passed.

## Explicitly deferred

Blocks, consensus, P2P, multi-node sync, Gas and economic mechanisms are out of scope. RocksDB, snapshot compression, query coalescing and performance tuning are also future work rather than implied requirements: select them based on measured workloads. JSON world/event/receipt history grows with committed writes; Web DOM bounds do not make node storage bounded.

## Operator closure

1. Enable CI in GitHub Actions (or with an authenticated `gh workflow enable CI`), then trigger/verify a push or PR for the final commit. The workflow has no `workflow_dispatch` trigger.
2. Run Docker smoke and deployment validators on a Docker-capable host; do not bypass failures.
3. Follow [deployment steps](../deploy/README.md) for TLS, monitoring, offline backup and staging soak. Set unique credentials and verify expected firewall/loopback boundaries.
4. Record real measurements and environment-specific acceptance. Only then declare deployment acceptance complete.
