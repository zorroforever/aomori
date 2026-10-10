# Ledger v1 primitives (L1)

Implemented, but **not connected to Runtime execution, RPC, Web UI or persistence**. Legacy transactions and state roots are unchanged. This is not a running blockchain.

## Modules and evidence

- Rust: `src/ledger.rs`; TypeScript: `web/src/ledger.ts`.
- Shared fixed vectors: `tests/fixtures/ledger-v1.json` (including a public test key and signature, not a production identity).
- Rust tests: `cargo test --locked --test ledger`.
- TypeScript vectors: `cd web && npm run test:ledger` (Node 22.6+ with experimental type stripping; CI uses Node 22).

Canonical JSON sorts object keys by UTF-8 bytes, preserves array order, allows Unicode scalar strings without normalization, rejects floating-point values, and limits integers to ±9,007,199,254,740,991. Depth is at most 32 from root depth zero; canonical payload is at most 1 MiB. Strings use JSON escaping; no whitespace. These are initial protocol limits, not performance/capacity claims. Strict raw parsers reject duplicate keys, fractional/exponent numeric tokens and invalid Unicode; wire `-0` is rejected. Parsing into a generic JSON value before the strict parser loses duplicate-key evidence and is not an acceptable wire entry point.

Domains are fixed `aomori/ledger/{sign,tx,genesis,block,transactions,results,events,state}/v1`. Hash input is UTF-8 domain, one zero byte, then canonical JSON. Digest is 32-byte BLAKE3, displayed as lowercase hex.

Transaction signing excludes the signature **field**, rather than setting it to null. Ed25519 signs the binary signing digest, not hex text. Transaction ID includes the signature. Rust verifies against a supplied public key and expected chain/genesis; account lookup and nonce progression remain L2 work. Chain/genesis versions and lowercase hex encoding are checked.

Genesis, transaction, block header, execution result and block body types are defined. Header hashing validates protocol and per-height transaction count. `LedgerBlock::validate_commitments` checks body counts, chain association, result identity, event ranges and ordered transaction/result digests; it does not verify execution, parent ancestry or post-state correctness. Genesis `initial_world` and `execution_limits` remain generic JSON until L2 pins the world projection and deterministic execution rules. Do not interpret a successful manifest hash as a validated executable genesis.

## Remaining boundaries

L1 provides byte-level agreement and shape checks, not ledger acceptance. L2 must audit Lua determinism, validate genesis/world/limits, bind signer keys to accounts and replay successful transitions. L3 must implement authoritative block storage, directory locking and durable recovery. L4 connects explicit new APIs and Web mode. See [the design](single-node-ledger-design.md).
