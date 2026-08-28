# Aomori RPC Protocol v1

## Transports

- HTTP JSON-RPC: `POST /rpc` (maximum request body: 1 MiB)
- Event WebSocket: `GET /events`
- Health check: `GET /health`

## HTTP Policy

`/rpc` uses a token bucket per client IP. The default refill rate is 100 requests per second with a burst capacity of 100, configurable with `AOMORI_RPC_RATE_LIMIT` or `--rpc-rate-limit`. Buckets that have been idle for ten minutes are removed, and the node bounds the number of tracked client buckets. An exhausted bucket returns HTTP 429:

```json
{"jsonrpc":"2.0","id":null,"error":{"code":-32004,"message":"rate limit exceeded","data":{"retry_after_ms":8}}}
```

The response also includes `Retry-After: 1` and `Cache-Control: no-store`; CORS exposes `Retry-After` to browser clients. `data.retry_after_ms` is the more precise delay until the client's bucket receives its next token.

By default the client identity is the TCP peer IP and `X-Forwarded-For` is ignored. Operators behind a reverse proxy may configure comma-separated exact proxy IPs with `AOMORI_TRUSTED_PROXIES` or `--trusted-proxies`. The node then walks the forwarding chain from right to left while addresses are trusted. The proxy must discard any client-supplied `X-Forwarded-For` value before constructing its forwarding chain. CIDR ranges are intentionally not accepted.

`/health` and an established `/events` WebSocket do not consume this quota.

Browser access uses an exact CORS origin allowlist. The default is `http://127.0.0.1:5173,http://localhost:5173`. Configure comma-separated origins with `AOMORI_CORS_ORIGINS` or `--cors-origins`; each value must include scheme, host, and port when non-default. Allowed request headers are `Content-Type` and `Authorization`. CORS is a browser policy and does not replace administrator authentication or transaction signatures.

Clients should read `protocol_version` from `aomori_get_info` and reject unsupported versions.

## JSON-RPC Envelope

Every request must use JSON-RPC 2.0:

```json
{"jsonrpc":"2.0","id":1,"method":"aomori_get_info","params":{}}
```

A successful response contains `result`. A failed response contains `error`:

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"entity_id required"}}
```

### Error Codes

| Code | Meaning |
| --- | --- |
| `-32700` | Malformed JSON; response `id` is `null` |
| `-32600` | Valid JSON with an invalid JSON-RPC 2.0 request or envelope |
| `-32601` | Unknown method |
| `-32602` | Missing or invalid parameters |
| `-32000` | Runtime or contract failure |
| `-32002` | Administrator authorization, owner, signature, or unsigned-write policy failure |
| `-32003` | Invalid nonce |
| `-32004` | Client RPC token bucket exhausted; inspect `data.retry_after_ms` |

Bodies larger than 1 MiB are rejected before JSON parsing with HTTP `413 Payload Too Large`. JSON-RPC application errors otherwise use HTTP 200, except rate limiting, which uses HTTP 429.

The bundled Web client retries a rate-limited read method once after a maximum delay of two seconds. Its retryable methods are `aomori_get_info`, `aomori_get_account`, `aomori_get_entity`, `aomori_list_entities`, `aomori_get_quests`, `aomori_get_events`, and the read-only `aomori_query`. Commands, transactions, deployments, and administrator mutations are never automatically replayed because a client may not know whether a write reached the node; the UI instead reports the suggested retry delay.

## Read Methods

### `aomori_get_info`

Parameters: `{}`

Returns `protocol_version`, `head`, `state_root`, and object counts.

### `aomori_get_account`

```json
{"name":"admin"}
```

### `aomori_get_entity`

```json
{"entity_id":4}
```

### `aomori_list_entities`

```json
{"location":1,"kind":"item"}
```

Both filters are optional. `kind` is `actor`, `zone`, or `item`.

### `aomori_get_receipt`

```json
{"tx_id":"hex transaction id"}
```

The result includes `protocol_version`, current state counts, and the active `lua_instruction_limit` and `lua_memory_limit` resource limits.

### `aomori_get_quests`

```json
{"actor_id":4}
```

Returns all Rust-managed quest definitions. When `actor_id` is provided, each quest includes that Actor's `locked`, `available`, `accepted`, or `completed` status. Every definition includes `giver_entity_id` and `prerequisite_quest_ids`. A quest is `locked` until every listed prerequisite has a completed `QuestProgress` for the same Actor. Existing accepted or completed progress takes precedence if a later definition upgrade adds prerequisites.

Clients should show an accept action only when the quest is `available` and its giver Entity is colocated with the Actor. The Rust Host API validates the requested quest/giver pair, colocation, and every prerequisite again during the command.

Quest definitions are validated as a complete directed acyclic graph during Demo initialization or upgrade, snapshot save, snapshot load, and node startup. Validation rejects map-key/ID mismatches, empty IDs, missing or non-Actor givers, missing prerequisites, duplicate prerequisites, self-dependencies, and dependency cycles. Cycle detection uses iterative topological sorting, so a deeply chained untrusted snapshot does not consume recursive stack space.

Legacy snapshots whose raw quest JSON predates `giver_entity_id` may load only far enough for Demo migration; the migrated world must then pass the same strict validation before startup or persistence.

## World State Invariants

Core references are validated before location, Quest, and Inventory checks:

- an Account map key equals `Account.name`, the name is non-empty, and an optional Ed25519 public key is exactly 32 bytes of hexadecimal;
- a Contract name is non-empty, its version is greater than zero, and its map key is `name` for v1 or `name@version` for later versions;
- a Contract `source_hash` equals the BLAKE3 hash of its stored Lua source;
- an Entity map key equals `Entity.id`, its owner Account exists, and an optional Contract binding exists;
- `next_entity_id` is greater than every existing Entity ID;
- a QuestProgress map key equals `<actor_id>:<quest_id>`, its actor exists and is an Actor, and its definition exists;
- a persisted Receipt map key equals `tx_id`, its Account exists, and `tx_id` plus `state_root` are 32-byte hexadecimal hashes;
- Event IDs are positive and strictly increasing, `next_event_id` cannot collide, event heads are valid, and transaction events match persisted Receipts.

These checks prevent a later write from overwriting an Entity, executing source that no longer matches its immutable Contract hash, or retaining authorization and progress records that reference deleted objects.

Entity locations and the Rust Inventory index are validated during Demo initialization or upgrade, snapshot save, snapshot load, and node startup:

- a Zone has no parent location;
- an Actor location, when present, references an existing Zone;
- an Item location, when present, references an existing Zone or Actor;
- every Inventory owner exists and is an Actor;
- every Inventory entry exists and is an Item;
- an Inventory contains no duplicate Item ID;
- an Item appears in at most one Actor Inventory;
- an inventoried Item's `location` equals its owner Actor ID;
- an Item whose `location` is an Actor appears in exactly that Actor's Inventory.

This bidirectional check rejects dangling Items, Item nesting, stale indexes, and double ownership even when a modified snapshot has a correctly recomputed `state_root`. Legacy `actor.data.inventory` is detected in raw snapshot JSON and may load only for Demo migration; migration moves those IDs into `WorldState.inventories`, updates Item locations, removes the legacy field, and then runs strict validation.

The Demo contains two independent quests and one chained quest:

| Quest | Giver | Prerequisites | Required item | Completion | Reward | Consume item |
| --- | --- | --- | --- | --- | ---: | --- |
| `lost_key` | Mira | — | brass key | Village | 10 | yes |
| `open_shrine` | Rowan | `lost_key` | stone tablet | Ruins | 4 | no |
| `ruins_tablet` | Rowan | — | stone tablet | Village | 6 | no |

### `aomori_get_events`

```json
{"since":0,"limit":100}
```

`since` is exclusive. `limit` defaults to 100 and is capped at 500. The response contains `events`, `next`, and `latest`. `next` is the last event in the returned page, or the requested `since` value for an empty page. `latest` is the node's current event ID, or zero when the log is empty; clients can reset a persisted cursor when it is greater than `latest`, which indicates that the world at that RPC URL was replaced.

The bundled Web client persists this non-sensitive cursor in `localStorage`, scoped by RPC URL. It does not persist event payloads.

### `aomori_query`

```json
{"entity_id":4,"action":"look","args":{}}
```

Queries are read-only. Any Lua Host API write fails and all temporary state is discarded.

## Write Methods

### Administrator authorization

`aomori_create_account`, `aomori_deploy`, and `aomori_create_entity` require:

```http
Authorization: Bearer <admin-token>
```

Configure the token with `AOMORI_ADMIN_TOKEN` or `--admin-token`. If no token is configured, administrator RPC methods are disabled. Missing or invalid credentials return JSON-RPC error `-32002`. The token is runtime configuration and is never stored in the world snapshot.

Example:

```bash
curl http://127.0.0.1:8091/rpc \
  -H 'content-type: application/json' \
  -H "authorization: Bearer $AOMORI_ADMIN_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"aomori_create_account","params":{"name":"player"}}'
```

### `aomori_create_account`

```json
{"name":"player","public_key":null,"balance":0}
```

### `aomori_deploy`

```json
{"name":"world","version":1,"source":"Lua source"}
```

Version 1 is referenced as `world`; later versions are referenced as `world@2`, `world@3`, and so on. Published versions are immutable.

### `aomori_create_entity`

```json
{
  "kind":"actor",
  "owner":"admin",
  "contract":"demo",
  "location":1,
  "data":{}
}
```

### `aomori_command`

```json
{"entity_id":4,"action":"go","args":{"direction":"east"}}
```

This development method executes a command without a signed transaction and is disabled by default. Start the node with `--allow-unsigned-commands` to enable it for local Demo use. It remains subject to Lua transaction rollback and durable snapshot commit.

Quest acceptance identifies both the definition and its publisher:

```json
{"entity_id":4,"action":"accept","args":{"npc_id":6,"quest_id":"lost_key"}}
```

Completion identifies the accepted definition:

```json
{"entity_id":4,"action":"complete","args":{"quest_id":"lost_key"}}
```

### `aomori_submit_transaction`

```json
{
  "from":"player",
  "nonce":0,
  "entity_id":4,
  "action":"go",
  "args":{"direction":"east"},
  "signature":null
}
```

Accounts with an Ed25519 public key must sign the JSON serialization of the complete transaction after setting `signature` to `null`. Public keys and signatures use lowercase or uppercase hexadecimal encoding. When `--allow-unsigned-commands` is disabled (the default), accounts without a public key cannot submit transactions. The flag permits unsigned transactions only for local development.

When the Web client creates an identity, it generates an Ed25519 key pair locally and sends only the public key to `aomori_create_account`. The secret key is encrypted before persistent browser storage: the client derives a 256-bit key from the identity password using PBKDF2-SHA-256 with 210,000 iterations and encrypts with NaCl secretbox. The encrypted record is isolated by RPC URL and account name in `localStorage`; plaintext secret-key bytes exist only in the current page's memory. Reloading or explicitly locking the page clears that in-memory key and requires a password to unlock. The administrator token is used only for the two creation requests and is not persisted.

The encrypted local record and exported version 1 JSON backup use the same cryptographic envelope. Import verifies the backup public key and checks it against the account public key returned by the selected node before retaining the encrypted record. Passwords must contain at least eight characters and are never persisted. Existing 128-character hexadecimal plaintext records from earlier clients are detected but not automatically activated; the first explicit unlock asks for a new password and replaces the legacy value with an encrypted record.

This protects a copied browser profile or `localStorage` dump, but it does not protect an unlocked key from malicious script executing in the page origin. Operators must still serve the client over trusted HTTPS, control deployed assets and dependencies, and use a restrictive Content Security Policy. Deleting browser storage without an exported backup loses the identity. If concurrent browser actions race on the same account nonce, the client fetches the latest nonce, re-signs, and retries once.

## Receipt

Commands and queries return:

```json
{
  "tx_id":"",
  "from":"",
  "nonce":0,
  "ok":true,
  "messages":[],
  "result":{},
  "state_root":"hex state root"
}
```

Signed or submitted transactions populate `tx_id`, `from`, and `nonce`. Receipts are stored only for `aomori_submit_transaction`.

## Events

Each event has:

```json
{
  "id":1,
  "head":4,
  "kind":"entity_changed",
  "entity_id":4,
  "data":{"change":"updated"}
}
```

Rust system events include:

- `entity_changed`: `created`, `updated`, or `deleted`
- `quest_progress_changed`
- `command_executed`
- `transaction_executed`

Lua contracts may emit domain events such as `quest_accepted` and `quest_completed`. The four Rust system event names are reserved and `host.emit_event` rejects attempts to spoof them.

The persisted log is validated during snapshot load and save. Event IDs must be positive and strictly increasing, `next_event_id` must be greater than every persisted ID, event heads cannot exceed the current world head, and kinds cannot be empty. Rust system event payloads must contain their required fields. A `transaction_executed` event must match an existing Receipt by `tx_id`, `from`, and `nonce`, and duplicate transaction events for one Receipt are rejected. Event `entity_id` is intentionally historical and is not required to reference an Entity that still exists.

Events are published only after the state snapshot is durably saved. Failed commands and failed snapshot writes do not publish or persist events.

## Snapshot Durability

A validated snapshot commit uses the following local-filesystem protocol:

1. serialize the versioned state and write `state.json.tmp`;
2. call `sync_all` on the temporary file;
3. if the current `state.json` passes full decoding and semantic validation, write it to `state.json.bak.tmp`, sync it, rename it to `state.json.bak`, and sync the data directory;
4. rename `state.json.tmp` to `state.json` and sync the data directory again.

A corrupt primary is never copied over the last valid backup. Failed commits remove temporary files and return an error so the in-memory mutation is rolled back. If the final directory sync fails after rename, the store attempts to restore the previous primary before reporting failure.

Backup recovery follows the same pattern: copy the validated backup into `state.json.restore.tmp`, sync the file, rename it over the primary, and sync the parent directory. These guarantees assume the data directory and temporary files are on the same local filesystem; network filesystems may provide weaker rename and `fsync` semantics.

If a WebSocket consumer falls behind the bounded broadcast buffer, the connection remains open and the server sends a control message instead of silently ending the stream:

```json
{"type":"event_stream_lagged","missed":12,"last_event_id":41}
```

`missed` is the number of broadcast messages skipped by that receiver. `last_event_id` is the last event that this server connection successfully encoded before detecting lag; clients should recover from their own persisted cursor because it may be newer.

## Event Recovery

Clients should maintain the greatest processed event `id`:

1. Connect to `/events` for live events.
2. On WebSocket open, disconnect, reconnect, or `event_stream_lagged`, call `aomori_get_events` from the cursor captured at recovery start.
3. Request pages of up to 500 events until a short or empty page is returned.
4. Process results in ascending ID order and persist the cursor. If the server's `latest` value is lower than the persisted cursor, reset the cursor to zero and recover the replacement world's event log.
5. Keep the WebSocket open during lag recovery and deduplicate all HTTP and WebSocket events by ID.

## Health, Readiness, and Metrics

`GET /health` is a liveness endpoint and returns:

```json
{"ok":true,"head":4,"state_root":"hex state root"}
```

`GET /ready` performs read-only world semantic validation and verifies that the snapshot parent path is a directory. It returns HTTP 200 when ready:

```json
{"ready":true,"head":4}
```

A failed check returns HTTP 503 with a stable public error; internal details are written only to the node log:

```json
{"ready":false,"error":"state unavailable"}
```

`GET /metrics` exposes cumulative in-process JSON metrics for the current node lifetime. It has separate `rpc`, `snapshot`, and `websocket` objects plus current world object counts. RPC metrics include request/error totals, cumulative and maximum latency in microseconds, errors by JSON-RPC code, and per-method counters. Snapshot metrics cover RPC-triggered save attempts, failures, cumulative duration, and maximum duration. WebSocket metrics cover active and cumulative connections, lag incidents, and the total number of events missed by lagged receivers. Metrics reset on restart and are not a durable accounting source.

`GET /metrics/prometheus` exposes the same values in Prometheus text exposition format (`text/plain; version=0.0.4`). Series use the `aomori_` prefix, including `aomori_rpc_requests_total`, `aomori_snapshot_failures_total`, `aomori_websocket_active`, and bounded per-method/error-code labels. Snapshot counters currently cover commits passing through the RPC mutation path; startup Demo migration saves happen before the metrics state exists and are not counted.

Known RPC methods have fixed metric labels. Unknown methods are grouped under `<unknown>` and malformed requests under `<invalid>`, preventing attacker-controlled metric cardinality. Rate-limited requests use `<rate_limited>`.

Every request that enters the RPC handler receives a server-generated monotonic `X-Request-Id` response header. CORS exposes this header to allowed browser origins. The node emits one JSON log line with only:

```json
{
  "type":"rpc_request",
  "request_id":12,
  "method":"aomori_get_info",
  "http_status":200,
  "rpc_error_code":null,
  "duration_micros":83
}
```

RPC logs never include params, request bodies, `Authorization`, signatures, admin tokens, or private keys. Unknown client-supplied method names are logged as `<unknown>`. Startup and readiness failure messages are also emitted as JSON lines. `/metrics` and `/metrics/prometheus` are currently unauthenticated, so production operators should restrict them with the listening interface, firewall, or reverse proxy if world-level counts are considered sensitive.
