use aomori::ledger::{
    canonical_json, digest, parse_json, Domain, LedgerTransaction, MAX_BYTES, MAX_DEPTH,
};
use serde_json::{json, Value};

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/ledger-v1.json")).unwrap()
}
#[test]
fn canonical_and_hash_golden_vectors() {
    for vector in vectors()["codec"].as_array().unwrap() {
        assert_eq!(
            String::from_utf8(canonical_json(&vector["value"]).unwrap()).unwrap(),
            vector["canonical"]
        );
        for domain in [
            Domain::Sign,
            Domain::Transaction,
            Domain::Genesis,
            Domain::Block,
            Domain::Transactions,
            Domain::Results,
            Domain::State,
        ] {
            let name = domain.label().split('/').nth(2).unwrap();
            assert_eq!(
                hex::encode(digest(domain, &vector["value"]).unwrap()),
                vector["hashes"][name]
            );
        }
    }
}
#[test]
fn signature_and_transaction_vectors_with_replay_rejection() {
    let vector = &vectors()["transaction"];
    let tx: LedgerTransaction = serde_json::from_value(vector["value"].clone()).unwrap();
    assert_eq!(
        hex::encode(tx.signing_digest().unwrap()),
        vector["signing_digest"]
    );
    assert_eq!(tx.tx_id().unwrap(), vector["tx_id"]);
    let key = vector["public_key"].as_str().unwrap();
    tx.verify(key, &tx.chain_id, &tx.genesis_hash).unwrap();
    assert!(tx.verify(key, "other-chain", &tx.genesis_hash).is_err());
    assert!(tx.verify(key, &tx.chain_id, &"01".repeat(32)).is_err());
    let mut changed = tx.clone();
    changed.nonce = 1;
    assert!(changed.verify(key, &tx.chain_id, &tx.genesis_hash).is_err());
    changed = tx.clone();
    changed.args = json!({"npc_id":7});
    assert_ne!(changed.tx_id().unwrap(), tx.tx_id().unwrap());
    assert!(changed.verify(key, &tx.chain_id, &tx.genesis_hash).is_err());
}
#[test]
fn invalid_json_and_limits_are_rejected() {
    for text in [
        r#"{"x":1,"x":2}"#,
        r#"{"x":{"a":1,"a":2}}"#,
        "1.0",
        "1e0",
        "-0",
        "9007199254740992",
        "-9007199254740992",
        r#""\ud800""#,
        "{} true",
    ] {
        assert!(parse_json(text.as_bytes()).is_err(), "{text}");
    }
    assert!(canonical_json(&json!(1.0)).is_err());
    let mut nested = json!(null);
    for _ in 0..MAX_DEPTH + 1 {
        nested = json!([nested]);
    }
    assert!(canonical_json(&nested).is_err());
    assert!(canonical_json(&json!("x".repeat(MAX_BYTES))).is_err());
    assert_eq!(
        parse_json(br#"{"b":2,"a":1}"#).unwrap(),
        json!({"a":1,"b":2})
    );
}
#[test]
fn header_and_genesis_hashes_validate_versions_and_change_with_content() {
    use aomori::ledger::{BlockHeader, GenesisManifest};
    let mut genesis = GenesisManifest {
        protocol_version: 1,
        chain_id: "test".into(),
        runtime_rules_version: 1,
        encoding_version: 1,
        initial_world: json!({}),
        execution_limits: json!({"instructions":1000}),
    };
    let first = genesis.hash().unwrap();
    genesis.chain_id = "other".into();
    assert_ne!(first, genesis.hash().unwrap());
    genesis.encoding_version = 2;
    assert!(genesis.hash().is_err());
    let mut header = BlockHeader {
        protocol_version: 1,
        chain_id: "test".into(),
        genesis_hash: first,
        runtime_rules_version: 1,
        height: 0,
        parent_hash: "0".repeat(64),
        tx_count: 0,
        transactions_digest: "0".repeat(64),
        results_digest: "0".repeat(64),
        post_state_digest: "0".repeat(64),
    };
    let hash = header.hash().unwrap();
    header.height = 1;
    header.tx_count = 1;
    assert_ne!(hash, header.hash().unwrap());
    header.tx_count = 2;
    assert!(header.hash().is_err());
}

#[test]
fn block_commitments_reject_mutated_body() {
    use aomori::ledger::{BlockHeader, ExecutionResult, LedgerBlock};
    let tx: LedgerTransaction =
        serde_json::from_value(vectors()["transaction"]["value"].clone()).unwrap();
    let id = tx.tx_id().unwrap();
    let result = ExecutionResult {
        tx_id: id.clone(),
        from: tx.from.clone(),
        nonce: 0,
        ok: true,
        messages: vec![],
        result: json!(null),
        first_event_id: None,
        last_event_id: None,
        events_digest: hex::encode(digest(Domain::Events, &json!([])).unwrap()),
    };
    let results = vec![result];
    let header = BlockHeader {
        protocol_version: 1,
        chain_id: tx.chain_id.clone(),
        genesis_hash: tx.genesis_hash.clone(),
        runtime_rules_version: 1,
        height: 1,
        parent_hash: "0".repeat(64),
        tx_count: 1,
        transactions_digest: hex::encode(digest(Domain::Transactions, &json!([id])).unwrap()),
        results_digest: hex::encode(
            digest(Domain::Results, &serde_json::to_value(&results).unwrap()).unwrap(),
        ),
        post_state_digest: "0".repeat(64),
    };
    let mut block = LedgerBlock {
        header,
        transactions: vec![tx],
        execution_results: results,
    };
    block.validate_commitments().unwrap();
    block.execution_results[0].result = json!("tampered");
    assert!(block.validate_commitments().is_err());
    block.execution_results.clear();
    assert!(block.validate_commitments().is_err());
}
