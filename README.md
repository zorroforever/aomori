<div align="center">

# Aomori

**A deterministic, programmable MUD world built with Rust and Lua.**

[![Rust](https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![JSON-RPC](https://img.shields.io/badge/API-JSON--RPC%202.0-2f855a)](doc/api.md)
[![WebSocket](https://img.shields.io/badge/events-WebSocket-7c3aed)](doc/api.md)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

</div>

Aomori is a single-node autonomous world MVP. It combines a Rust runtime, sandboxed Lua world scripts, deterministic state, transactional command execution, and a browser client into a small but complete MUD platform.

> Aomori is an experimental game runtime and protocol prototype. It is not a production blockchain or a public network.

## Highlights

- **Deterministic world state** — accounts, contracts, zones, actors, entities, quests, inventories, receipts, and events are persisted with a verifiable `state_root`.
- **Lua scripting with guardrails** — query and command scripts run in a sandbox with instruction and memory limits.
- **Transactional execution** — failed commands, invalid host calls, and script-limit violations automatically roll back world changes.
- **Programmable quests and entities** — Rust-managed quest chains, prerequisites, parallel progress, rewards, entity ownership, locations, and inventory indexes.
- **Signed transactions** — Ed25519 identities, nonce checks, deterministic transaction IDs, ownership validation, and execution receipts.
- **Live event delivery** — incremental event queries plus WebSocket push with reconnect and lag compensation.
- **Durable snapshots** — versioned JSON snapshots, integrity checks, migrations, atomic replacement, backups, and crash-conscious syncing.
- **Operational defaults** — JSON-RPC over HTTP, CORS controls, request limits, token-bucket rate limiting, admin authentication, health/readiness endpoints, metrics, Docker, and systemd deployment.
- **Playable demo world** — explore Village, Forest, and Ruins; complete quests; collect items; and earn rewards through the web client.

## Architecture

```text
Browser client
     │ HTTP JSON-RPC + WebSocket events
     ▼
Axum API / auth / rate limit / observability
     │
     ▼
Rust world runtime ───── Lua scripts + bounded Host API
     │
     ├── deterministic state + state_root
     ├── transaction validation + rollback
     ├── quests / entities / inventory / events
     └── versioned JSON snapshot store
```

## Quick Start

### Prerequisites

- Rust stable with Cargo
- Node.js and npm (only required for the web client)
- Docker (optional, for container deployment)

### Run the node

```bash
cargo run -- --listen 127.0.0.1:8091 --allow-unsigned-commands
```

Check the node:

```bash
curl -s http://127.0.0.1:8091/health | jq
curl -s http://127.0.0.1:8091/ready | jq
```

### Run the demo world

```bash
export AOMORI_ADMIN_TOKEN="replace-with-a-long-random-token"
export AOMORI_CORS_ORIGINS="http://127.0.0.1:5173,http://localhost:5173"

cargo run -- \
  --listen 127.0.0.1:8091 \
  --data-dir ./demo-data \
  --demo \
  --allow-unsigned-commands
```

The demo uses actor ID `4` by default. It includes Village, Forest, Ruins, a brass key, a stone tablet, and quest chains involving Mira and Rowan. The browser client connects to port `8091` by default.

### Run the web client

In a second terminal:

```bash
cd web
npm ci
npm run dev
```

Open <http://127.0.0.1:5173>.

Unsigned commands are intended for local development only. For the default secure mode, create an Ed25519 account and actor from the web client using an admin token.

## API Example

List entities in a location:

```bash
curl -s http://127.0.0.1:8091/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"aomori_list_entities","params":{"location":1}}' | jq
```

Subscribe to live events:

```text
ws://127.0.0.1:8091/events
```

Clients should keep the last received event ID. After reconnecting, call `aomori_get_events` to fill the gap before subscribing again. See the [API reference](doc/api.md) for RPC methods, transaction formats, errors, and event semantics.

## Lua World Scripts

Scripts can query the world and execute controlled commands through the Host API. A command can move actors, spawn entities, update owned entity data, manage inventory, and emit events. Query execution cannot mutate state; attempted mutations fail and roll back.

Example script entry points:

```lua
function query_look(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local zone = host.get_entity(actor.location)
  host.narrate(zone.data.name)
  return { location = actor.location, name = zone.data.name }
end

function command_go(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local target = host.get_exit(actor.location, args.direction)
  if not target then error("no exit") end
  host.move_actor(ctx.entity_id, target)
  return { location = target }
end
```

## Development

Run the full local verification suite:

```bash
cargo fmt --all -- --check
cargo test --locked
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings

cd web
npm ci
npm run build
npm run test:e2e
```

GitHub Actions validates Rust quality, the web build, browser E2E tests, and Docker smoke tests on pushes to `main` and pull requests.

## Deployment

Docker Compose provides a non-root deployment with a read-only root filesystem, a persistent named volume, and a `/ready` health check:

```bash
cp .env.example .env
# Put a long random token in .env

docker compose up --build -d
docker compose ps
curl --fail http://127.0.0.1:8091/ready
```

For host deployment, use [`deploy/systemd/aomori.service`](deploy/systemd/aomori.service) and the accompanying environment template. Before exposing the node beyond localhost, read the [operations guide](doc/operations.md) for listener, TLS, metrics, backup, and upgrade recommendations.

## Documentation

- [Documentation index](doc/README.md)
- [API reference](doc/api.md)
- [Operations guide](doc/operations.md)
- [MVP scope](doc/mvp.md)
- [Product requirements](doc/prd.md)
- [Contracts and demo scripts](contracts/)

## License

Aomori is released under the [MIT License](LICENSE).
