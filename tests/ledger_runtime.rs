use aomori::{
    ledger::{GenesisManifest, LedgerTransaction},
    ledger_runtime::MemoryLedger,
    model::WorldState,
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
fn setup(source: Option<&str>) -> (MemoryLedger, SigningKey) {
    let key = SigningKey::from_bytes(&[7; 32]);
    let mut world = WorldState::genesis();
    aomori::demo::initialize(&mut world).unwrap();
    for account in world.accounts.values_mut() {
        account.public_key = Some(hex::encode(key.verifying_key().to_bytes()));
    }
    if let Some(source) = source {
        for contract in world.contracts.values_mut() {
            contract.source = source.into();
            contract.source_hash = blake3::hash(source.as_bytes()).to_hex().to_string();
        }
    }
    let manifest = GenesisManifest {
        protocol_version: 1,
        chain_id: "test".into(),
        runtime_rules_version: 1,
        encoding_version: 1,
        initial_world: serde_json::to_value(world).unwrap(),
        execution_limits: json!({"instruction_limit":200000,"memory_limit":16777216}),
    };
    (MemoryLedger::new(manifest).unwrap(), key)
}
fn tx(ledger: &MemoryLedger, key: &SigningKey, nonce: u64) -> LedgerTransaction {
    let mut tx = LedgerTransaction {
        protocol_version: 1,
        chain_id: "test".into(),
        genesis_hash: ledger.genesis_hash().into(),
        from: "admin".into(),
        nonce,
        entity_id: 4,
        action: "talk".into(),
        args: json!({"npc_id":6}),
        signature: String::new(),
    };
    tx.signature = hex::encode(key.sign(&tx.signing_digest().unwrap()).to_bytes());
    tx
}
fn state(ledger: &MemoryLedger) -> Value {
    serde_json::to_value(ledger.world()).unwrap()
}
#[test]
fn independent_instances_replay_identically_and_duplicates_are_idempotent() {
    let (mut a, key) = setup(None);
    let mut b = MemoryLedger::new(a.genesis().clone()).unwrap();
    for nonce in 0..4 {
        let transaction = tx(&a, &key, nonce);
        let before = state(&a);
        let candidate = a.propose(transaction.clone()).unwrap();
        assert_eq!(state(&a), before);
        b.apply(candidate.clone()).unwrap();
        let block = a.submit(transaction.clone()).unwrap();
        assert_eq!(block.header.hash().unwrap(), b.tip_hash().unwrap());
        assert_eq!(state(&a), state(&b));
        let before = state(&a);
        assert_eq!(
            a.submit(transaction).unwrap().header.hash().unwrap(),
            block.header.hash().unwrap()
        );
        assert_eq!(state(&a), before);
    }
    assert_eq!(a.blocks().len(), 5);
    assert_eq!(a.world().accounts["admin"].nonce, 4);
}
#[test]
fn rejection_and_tampered_block_do_not_publish_state() {
    let (mut ledger, key) = setup(None);
    let original = state(&ledger);
    assert!(ledger.submit(tx(&ledger, &key, 1)).is_err());
    let mut bad = tx(&ledger, &key, 0);
    bad.signature = "00".repeat(64);
    assert!(ledger.submit(bad).is_err());
    let mut block = ledger.propose(tx(&ledger, &key, 0)).unwrap();
    block.header.post_state_digest = "00".repeat(32);
    assert!(ledger.apply(block).is_err());
    assert_eq!(state(&ledger), original);
    assert_eq!(ledger.blocks().len(), 1);
}
#[test]
fn execution_failure_budget_and_sandbox_rejections_are_atomic() {
    for source in [
        "function command_talk() error('fail') end",
        "function command_talk() while true do end end",
        "function command_talk() return math.random() end",
        "function command_talk() return os.time() end",
        "function command_talk() return 1.5 end",
    ] {
        let (mut ledger, key) = setup(Some(source));
        let original = state(&ledger);
        assert!(ledger.submit(tx(&ledger, &key, 0)).is_err(), "{source}");
        assert_eq!(state(&ledger), original);
        assert_eq!(ledger.blocks().len(), 1);
    }
}
#[test]
fn invalid_genesis_is_rejected() {
    let (ledger, _) = setup(None);
    let mut genesis = ledger.genesis().clone();
    genesis.execution_limits = json!({"instruction_limit":0,"memory_limit":16777216});
    assert!(MemoryLedger::new(genesis).is_err());
    let mut genesis = ledger.genesis().clone();
    genesis.initial_world["accounts"]["admin"]["public_key"] = Value::Null;
    assert!(MemoryLedger::new(genesis).is_err());
}
