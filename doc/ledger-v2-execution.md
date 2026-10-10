# L2: experimental in-memory ledger execution

`src/ledger_runtime.rs` implements `MemoryLedger`, independent of the existing RPC and snapshot store. No blocks are written to disk and there is no externally enabled ledger mode.

## Implemented

- Genesis validates the existing WorldState invariants, empty receipts/events, initial event cursor, public keys for all accounts, zero account nonces and explicit execution limits. Contract source hashes are checked by world validation.
- Initial supported budgets: 1,000–10,000,000 Lua instructions and 1–64 MiB Lua memory. The manifest remains the authority for the selected values.
- `propose` verifies v1 signature against the account key and expected chain/genesis, nonce and entity ownership, then executes on a private world copy.
- Candidates increment nonce, add a distinct `ledger_transaction_executed` event, validate world invariants and protocol encoding, and commit event/result/state digests into a one-transaction block. Legacy receipts and transaction signature rules are not reused.
- `apply` validates commitments and parent/height, independently reexecutes the transaction, compares the entire expected block, then publishes state. Invalid input leaves the published world and tip unchanged.
- `submit` returns a previously included block for an exact duplicate tx ID, without reexecution. Different transactions still undergo signature/nonce validation.
- World access is immutable and there are no direct administrative mutation methods on MemoryLedger. Genesis predefines accounts, entities and contracts.

## Lua boundary and remaining gate

The ledger-only execution path removes os/io/package/require/debug, file/dynamic loaders, collectgarbage, pairs/next, tostring and math random/randomseed. Legacy runtime execution is unchanged. This is an intentionally restrictive experimental profile; some existing contract actions using tostring will fail until a deterministic replacement or contract update is designed. `ipairs` remains for ordered arrays.

This is **not a completed determinism audit**. Lua arithmetic, transcendental math, string formatting, implicit conversions, metatables and host operations still need review and cross-platform execution vectors. Rejecting floating-point persisted results does not prove intermediate arithmetic is deterministic. Host commands capable of dynamic entity/world changes also need a stable ledger policy before the mode is exposed. No untrusted deployment or cross-node consensus claim is made.

## Evidence

`cargo test --locked --test ledger_runtime` covers independent-instance replay, candidate nonmutation, duplicate idempotency, nonce/signature rejection, tampered post-state, explicit Lua errors, instruction exhaustion, unavailable randomness/time, floating-point result rejection and invalid genesis. These four tests run in the existing Rust CI job.

L2 currently supplies an executable candidate/replay skeleton. Determinism hardening remains open. L3 persistence must not publish a state before durable block commitment; MemoryLedger's in-memory apply alone is not that durability boundary. See [the design](single-node-ledger-design.md).
