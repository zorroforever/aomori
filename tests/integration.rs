use aomori::demo;
use aomori::model::{
    Account, EntityKind, QuestStatus, Receipt, Transaction, WorldEvent, WorldState,
};
use aomori::runtime;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn look_go_look_and_failed_command_rolls_back() {
    let mut state = WorldState::genesis();
    let source = include_str!("../contracts/world.lua").to_string();
    runtime::deploy(&mut state, "world".into(), source).unwrap();
    let mut a = BTreeMap::new();
    a.insert("name".into(), json!("Village"));
    a.insert("exits".into(), json!({"north":2}));
    let z1 = runtime::create_entity(&mut state, EntityKind::Zone, "admin".into(), None, None, a)
        .unwrap();
    let mut b = BTreeMap::new();
    b.insert("name".into(), json!("Forest"));
    b.insert("exits".into(), json!({"south":z1}));
    let z2 = runtime::create_entity(&mut state, EntityKind::Zone, "admin".into(), None, None, b)
        .unwrap();
    state
        .entities
        .get_mut(&z1)
        .unwrap()
        .data
        .insert("exits".into(), json!({"north":z2}));
    let actor = runtime::create_entity(
        &mut state,
        EntityKind::Actor,
        "admin".into(),
        Some("world".into()),
        Some(z1),
        BTreeMap::new(),
    )
    .unwrap();
    let r = runtime::execute(&mut state, actor, "look", json!({}), false).unwrap();
    assert_eq!(r.result["location"], json!(z1));
    runtime::execute(&mut state, actor, "go", json!({"direction":"north"}), true).unwrap();
    assert_eq!(state.entities[&actor].location, Some(z2));
    let root = state.root();
    assert!(runtime::execute(&mut state, actor, "go", json!({"direction":"east"}), true).is_err());
    assert_eq!(state.root(), root);
    assert_eq!(state.entities[&actor].location, Some(z2));

    let receipt = runtime::execute_transaction(
        &mut state,
        Transaction {
            from: "admin".into(),
            nonce: 0,
            entity_id: actor,
            action: "go".into(),
            args: json!({"direction":"south"}),
            signature: None,
        },
    )
    .unwrap();
    assert!(!receipt.tx_id.is_empty());
    assert_eq!(state.entities[&actor].location, Some(z1));
    assert_eq!(state.accounts["admin"].nonce, 1);

    let root = state.root();
    assert!(runtime::execute_transaction(
        &mut state,
        Transaction {
            from: "admin".into(),
            nonce: 0,
            entity_id: actor,
            action: "go".into(),
            args: json!({"direction":"north"}),
            signature: None,
        },
    )
    .is_err());
    assert_eq!(state.root(), root);
}

#[test]
fn contract_versions_are_immutable_and_distinct() {
    let mut state = WorldState::genesis();
    let source = include_str!("../contracts/world.lua").to_string();
    runtime::deploy(&mut state, "world".into(), source.clone()).unwrap();
    runtime::deploy_version(&mut state, "world".into(), 2, source.clone()).unwrap();
    assert_eq!(state.contracts["world"].version, 1);
    assert_eq!(state.contracts["world@2"].version, 2);
    assert!(runtime::deploy_version(&mut state, "world".into(), 2, source).is_err());
}

#[test]
fn signed_transaction_is_verified_before_execution() {
    let mut state = WorldState::genesis();
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let public_key = hex::encode(signing_key.verifying_key().to_bytes());
    state.accounts.insert(
        "player".into(),
        Account {
            name: "player".into(),
            public_key: Some(public_key),
            nonce: 0,
            balance: 0,
        },
    );
    let source = include_str!("../contracts/world.lua").to_string();
    runtime::deploy(&mut state, "world".into(), source).unwrap();
    let zone = runtime::create_entity(
        &mut state,
        EntityKind::Zone,
        "player".into(),
        None,
        None,
        BTreeMap::from([
            (String::from("name"), json!("Village")),
            (String::from("exits"), json!({"north": 1})),
        ]),
    )
    .unwrap();
    let actor = runtime::create_entity(
        &mut state,
        EntityKind::Actor,
        "player".into(),
        Some("world".into()),
        Some(zone),
        BTreeMap::new(),
    )
    .unwrap();
    let mut tx = Transaction {
        from: "player".into(),
        nonce: 0,
        entity_id: actor,
        action: "go".into(),
        args: json!({"direction":"north"}),
        signature: None,
    };
    let payload = serde_json::to_vec(&tx).unwrap();
    tx.signature = Some(hex::encode(signing_key.sign(&payload).to_bytes()));
    let receipt = runtime::execute_transaction(&mut state, tx).unwrap();
    assert!(receipt.ok);
    assert_eq!(state.accounts["player"].nonce, 1);
}

#[test]
fn lua_can_spawn_entities_update_data_and_emit_events() {
    let mut state = WorldState::genesis();
    let source = r#"
function command_create_item(ctx, args)
  local item = host.spawn_entity("item", nil, {name = args.name})
  host.update_entity_data(ctx.entity_id, "visited", true)
  local event = host.emit_event("item_created", item, {name = args.name})
  return {item = item, event = event}
end
"#;
    runtime::deploy(&mut state, "tools".into(), source.into()).unwrap();
    let actor = runtime::create_entity(
        &mut state,
        EntityKind::Actor,
        "admin".into(),
        Some("tools".into()),
        None,
        BTreeMap::new(),
    )
    .unwrap();
    let receipt = runtime::execute(
        &mut state,
        actor,
        "create_item",
        json!({"name":"key"}),
        true,
    )
    .unwrap();
    let item = receipt.result["item"].as_u64().unwrap();
    assert_eq!(state.entities[&item].data["name"], json!("key"));
    assert_eq!(state.entities[&actor].data["visited"], json!(true));
    assert_eq!(state.events[0].kind, "item_created");
    assert!(state
        .events
        .iter()
        .any(|event| event.kind == "entity_changed"));
    assert!(state
        .events
        .iter()
        .any(|event| event.kind == "command_executed"));
}

#[test]
fn lua_memory_limit_aborts_and_rolls_back() {
    let mut state = WorldState::genesis();
    let source = r#"
function command_allocate(ctx, args)
  host.update_entity_data(ctx.entity_id, "started", true)
  local values = {}
  while true do
    values[#values + 1] = string.rep("x", 8192)
  end
end
"#;
    runtime::deploy(&mut state, "allocate".into(), source.into()).unwrap();
    let actor = runtime::create_entity(
        &mut state,
        EntityKind::Actor,
        "admin".into(),
        Some("allocate".into()),
        None,
        BTreeMap::new(),
    )
    .unwrap();
    let root = state.root();
    let error = runtime::execute_with_limits(
        &mut state,
        actor,
        "allocate",
        json!({}),
        true,
        runtime::LuaLimits {
            instruction_limit: 5_000_000,
            memory_limit: 1024 * 1024,
        },
    )
    .unwrap_err();
    assert!(error.to_string().to_lowercase().contains("memory"));
    assert_eq!(state.root(), root);
    assert!(!state.entities[&actor].data.contains_key("started"));
}

#[test]
fn lua_instruction_limit_aborts_and_rolls_back() {
    let mut state = WorldState::genesis();
    let source = r#"
function command_loop(ctx, args)
  host.update_entity_data(ctx.entity_id, "started", true)
  host.emit_event("loop_started", ctx.entity_id, {})
  while true do end
end
"#;
    runtime::deploy(&mut state, "loop".into(), source.into()).unwrap();
    let actor = runtime::create_entity(
        &mut state,
        EntityKind::Actor,
        "admin".into(),
        Some("loop".into()),
        None,
        BTreeMap::new(),
    )
    .unwrap();
    let root = state.root();
    let event_count = state.events.len();

    let error = runtime::execute(&mut state, actor, "loop", json!({}), true).unwrap_err();
    assert!(error.to_string().contains("instruction limit exceeded"));
    assert_eq!(state.root(), root);
    assert_eq!(state.events.len(), event_count);
    assert!(!state.entities[&actor].data.contains_key("started"));
}

#[test]
fn failed_command_does_not_emit_system_events() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    let event_count = state.events.len();
    let root = state.root();
    assert!(runtime::execute(&mut state, actor, "complete", json!({}), true).is_err());
    assert_eq!(state.events.len(), event_count);
    assert_eq!(state.root(), root);
}

#[test]
fn demo_quest_requires_location_and_rewards_account() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    assert_eq!(actor, 4);
    runtime::execute(&mut state, actor, "accept", json!({"npc_id": 6}), true).unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction": "east"}), true).unwrap();
    runtime::execute(&mut state, actor, "take", json!({"item_id": 5}), true).unwrap();
    assert!(runtime::execute(&mut state, actor, "complete", json!({}), true).is_err());
    runtime::execute(&mut state, actor, "go", json!({"direction": "west"}), true).unwrap();
    runtime::execute(&mut state, actor, "complete", json!({}), true).unwrap();
    assert_eq!(state.accounts["admin"].balance, 10);
    assert!(state.inventories[&actor].is_empty());
    assert!(!state.entities.contains_key(&5));
    assert_eq!(
        state.quest_progress["4:lost_key"].status,
        QuestStatus::Completed
    );
}

#[test]
fn demo_supports_parallel_quests_with_distinct_givers_and_item_policies() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    runtime::execute(
        &mut state,
        actor,
        "accept",
        json!({"npc_id":6,"quest_id":"lost_key"}),
        true,
    )
    .unwrap();
    runtime::execute(
        &mut state,
        actor,
        "accept",
        json!({"npc_id":7,"quest_id":"ruins_tablet"}),
        true,
    )
    .unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"east"}), true).unwrap();
    runtime::execute(&mut state, actor, "take", json!({"item_id":5}), true).unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"east"}), true).unwrap();
    runtime::execute(&mut state, actor, "take", json!({"item_id":8}), true).unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"west"}), true).unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"west"}), true).unwrap();
    runtime::execute(
        &mut state,
        actor,
        "complete",
        json!({"quest_id":"lost_key"}),
        true,
    )
    .unwrap();
    runtime::execute(
        &mut state,
        actor,
        "complete",
        json!({"quest_id":"ruins_tablet"}),
        true,
    )
    .unwrap();

    assert_eq!(state.accounts["admin"].balance, 16);
    assert_eq!(state.inventories[&actor], vec![8]);
    assert!(!state.entities.contains_key(&5));
    assert!(state.entities.contains_key(&8));
    assert_eq!(
        state.quest_progress["4:lost_key"].status,
        QuestStatus::Completed
    );
    assert_eq!(
        state.quest_progress["4:ruins_tablet"].status,
        QuestStatus::Completed
    );
}

#[test]
fn quest_cannot_be_accepted_from_the_wrong_giver() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    let root = state.root();
    let result = runtime::execute(
        &mut state,
        actor,
        "accept",
        json!({"npc_id":7,"quest_id":"lost_key"}),
        true,
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("quest giver does not match"));
    assert_eq!(state.root(), root);
    assert!(state.quest_progress.is_empty());
}

#[test]
fn quest_prerequisite_unlocks_only_after_completion() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    let root = state.root();
    let locked = runtime::execute(
        &mut state,
        actor,
        "accept",
        json!({"npc_id":7,"quest_id":"open_shrine"}),
        true,
    );
    assert!(locked
        .unwrap_err()
        .to_string()
        .contains("quest prerequisite is not completed: lost_key"));
    assert_eq!(state.root(), root);

    runtime::execute(
        &mut state,
        actor,
        "accept",
        json!({"npc_id":6,"quest_id":"lost_key"}),
        true,
    )
    .unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"east"}), true).unwrap();
    runtime::execute(&mut state, actor, "take", json!({"item_id":5}), true).unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"west"}), true).unwrap();
    runtime::execute(
        &mut state,
        actor,
        "complete",
        json!({"quest_id":"lost_key"}),
        true,
    )
    .unwrap();
    runtime::execute(
        &mut state,
        actor,
        "accept",
        json!({"npc_id":7,"quest_id":"open_shrine"}),
        true,
    )
    .unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"east"}), true).unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction":"east"}), true).unwrap();
    runtime::execute(&mut state, actor, "take", json!({"item_id":8}), true).unwrap();
    runtime::execute(
        &mut state,
        actor,
        "complete",
        json!({"quest_id":"open_shrine"}),
        true,
    )
    .unwrap();

    assert_eq!(state.accounts["admin"].balance, 14);
    assert_eq!(
        state.quest_progress["4:open_shrine"].status,
        QuestStatus::Completed
    );
    assert_eq!(state.inventories[&actor], vec![8]);
}

#[test]
fn quest_definition_validation_rejects_invalid_givers_and_dependency_graphs() {
    let valid = || {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
    };

    let mut state = valid();
    state.quests.get_mut("lost_key").unwrap().giver_entity_id = 999;
    assert!(state
        .validate_quests()
        .unwrap_err()
        .to_string()
        .contains("giver does not exist"));

    let mut state = valid();
    state.quests.get_mut("lost_key").unwrap().giver_entity_id = 1;
    assert!(state
        .validate_quests()
        .unwrap_err()
        .to_string()
        .contains("giver is not an actor"));

    let mut state = valid();
    state.quests.get_mut("lost_key").unwrap().id = "wrong".into();
    assert!(state
        .validate_quests()
        .unwrap_err()
        .to_string()
        .contains("map key does not match"));

    let mut state = valid();
    state
        .quests
        .get_mut("lost_key")
        .unwrap()
        .prerequisite_quest_ids = vec!["missing".into()];
    assert!(state
        .validate_quests()
        .unwrap_err()
        .to_string()
        .contains("prerequisite does not exist"));

    let mut state = valid();
    state
        .quests
        .get_mut("lost_key")
        .unwrap()
        .prerequisite_quest_ids = vec!["lost_key".into()];
    assert!(state
        .validate_quests()
        .unwrap_err()
        .to_string()
        .contains("cannot require itself"));

    let mut state = valid();
    state
        .quests
        .get_mut("lost_key")
        .unwrap()
        .prerequisite_quest_ids = vec!["ruins_tablet".into()];
    state
        .quests
        .get_mut("ruins_tablet")
        .unwrap()
        .prerequisite_quest_ids = vec!["open_shrine".into()];
    state
        .quests
        .get_mut("open_shrine")
        .unwrap()
        .prerequisite_quest_ids = vec!["lost_key".into()];
    assert!(state
        .validate_quests()
        .unwrap_err()
        .to_string()
        .contains("cycle detected"));

    let mut state = valid();
    state
        .quests
        .get_mut("open_shrine")
        .unwrap()
        .prerequisite_quest_ids = vec!["lost_key".into(), "lost_key".into()];
    assert!(state
        .validate_quests()
        .unwrap_err()
        .to_string()
        .contains("duplicate prerequisite"));
}

#[test]
fn inventory_and_location_validation_rejects_inconsistent_worlds() {
    let valid = || {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
    };

    let mut state = valid();
    state.inventories.insert(999, Vec::new());
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("inventory owner does not exist"));

    let mut state = valid();
    state.inventories.insert(1, Vec::new());
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("inventory owner is not an actor"));

    let mut state = valid();
    state.inventories.entry(4).or_default().push(999);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("item does not exist"));

    let mut state = valid();
    state.inventories.entry(4).or_default().push(1);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("entity is not an item"));

    let mut state = valid();
    state.entities.get_mut(&5).unwrap().location = Some(4);
    state.inventories.entry(4).or_default().extend([5, 5]);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("duplicate item"));

    let mut state = valid();
    state.entities.get_mut(&5).unwrap().location = Some(4);
    state.inventories.entry(4).or_default().push(5);
    state.inventories.entry(6).or_default().push(5);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("multiple actors"));

    let mut state = valid();
    state.inventories.entry(4).or_default().push(5);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("location does not match"));

    let mut state = valid();
    state.entities.get_mut(&5).unwrap().location = Some(4);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("missing from inventory"));

    let mut state = valid();
    state.entities.get_mut(&5).unwrap().location = Some(999);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("location does not exist"));

    let mut state = valid();
    state.entities.get_mut(&5).unwrap().location = Some(8);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("neither a zone nor an actor"));

    let mut state = valid();
    state.entities.get_mut(&4).unwrap().location = Some(6);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("location is not a zone"));

    let mut state = valid();
    state.entities.get_mut(&1).unwrap().location = Some(2);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("zone 1 must not have a location"));
}

#[test]
fn lua_cannot_emit_reserved_system_events() {
    let mut state = WorldState::genesis();
    runtime::deploy(
        &mut state,
        "reserved-events".into(),
        r#"
function command_spoof(ctx, args)
  host.emit_event("transaction_executed", ctx.entity_id, {tx_id = "fake"})
  return {}
end
"#
        .into(),
    )
    .unwrap();
    let actor = runtime::create_entity(
        &mut state,
        EntityKind::Actor,
        "admin".into(),
        Some("reserved-events".into()),
        None,
        BTreeMap::new(),
    )
    .unwrap();
    let original = state.clone();
    let error = runtime::execute(&mut state, actor, "spoof", json!({}), true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("event kind is reserved"));
    assert_eq!(state.root(), original.root());
    assert!(state.events.is_empty());
}

#[test]
fn event_validation_rejects_corrupt_log_and_transaction_links() {
    let with_events = || {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        runtime::execute(&mut state, 4, "go", json!({"direction":"east"}), true).unwrap();
        state
    };

    let mut state = with_events();
    state.events[1].id = state.events[0].id;
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("strictly increasing"));

    let mut state = with_events();
    state.next_event_id = state.events.last().unwrap().id;
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("next_event_id"));

    let mut state = with_events();
    state.events[0].head = state.head + 1;
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("exceeds world head"));

    let mut state = with_events();
    state.events[0].kind.clear();
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("kind must not be empty"));

    let mut state = with_events();
    let entity_event = state
        .events
        .iter_mut()
        .find(|event| event.kind == "entity_changed")
        .unwrap();
    entity_event.entity_id = None;
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("missing entity_id"));

    let mut state = with_events();
    let entity_event = state
        .events
        .iter_mut()
        .find(|event| event.kind == "entity_changed")
        .unwrap();
    entity_event.data = json!({"change":"unknown"});
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("invalid change"));

    let mut state = with_events();
    state.events.last_mut().unwrap().data = json!({});
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("missing action"));

    let mut state = WorldState::genesis();
    demo::initialize(&mut state).unwrap();
    let tx = Transaction {
        from: "admin".into(),
        nonce: 0,
        entity_id: 4,
        action: "go".into(),
        args: json!({"direction":"east"}),
        signature: None,
    };
    let receipt = runtime::execute_transaction(&mut state, tx).unwrap();
    state
        .receipts
        .insert(receipt.tx_id.clone(), receipt.clone());
    state.validate().unwrap();

    let tx_event_index = state
        .events
        .iter()
        .position(|event| event.kind == "transaction_executed")
        .unwrap();
    let mut missing_receipt = state.clone();
    missing_receipt.receipts.clear();
    assert!(missing_receipt
        .validate()
        .unwrap_err()
        .to_string()
        .contains("receipt does not exist"));

    let mut mismatch = state.clone();
    mismatch.events[tx_event_index].data["nonce"] = json!(99);
    assert!(mismatch
        .validate()
        .unwrap_err()
        .to_string()
        .contains("does not match receipt"));

    let tx_event = state.events[tx_event_index].clone();
    let mut duplicate = state;
    duplicate.events.push(WorldEvent {
        id: duplicate.next_event_id,
        head: duplicate.head,
        ..tx_event
    });
    duplicate.next_event_id += 1;
    assert!(duplicate
        .validate()
        .unwrap_err()
        .to_string()
        .contains("duplicate transaction event"));
}

#[test]
fn core_reference_validation_rejects_inconsistent_maps_and_links() {
    let valid = || {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
    };

    let mut state = valid();
    state.accounts.get_mut("admin").unwrap().name = "other".into();
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("account map key"));

    let mut state = valid();
    state.accounts.get_mut("admin").unwrap().public_key = Some("not-hex".into());
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("public key is not hex"));

    let mut state = valid();
    state.accounts.get_mut("admin").unwrap().public_key = Some("00".repeat(31));
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("public key must be 32 bytes"));

    let mut state = valid();
    let contract = state.contracts.remove("demo").unwrap();
    state.contracts.insert("wrong".into(), contract);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("contract map key"));

    let mut state = valid();
    state.contracts.get_mut("demo").unwrap().version = 0;
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("version must be greater than zero"));

    let mut state = valid();
    state.contracts.get_mut("demo").unwrap().source_hash = "00".repeat(32);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("source hash does not match"));

    let mut state = valid();
    let entity = state.entities.remove(&4).unwrap();
    state.entities.insert(99, entity);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("entity map key"));

    let mut state = valid();
    state.entities.get_mut(&4).unwrap().owner = "missing".into();
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("owner account does not exist"));

    let mut state = valid();
    state.entities.get_mut(&4).unwrap().contract = Some("missing".into());
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("contract does not exist"));

    let mut state = valid();
    state.next_entity_id = 8;
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("next_entity_id"));

    let progress_state = || {
        let mut state = valid();
        runtime::execute(
            &mut state,
            4,
            "accept",
            json!({"npc_id":6,"quest_id":"lost_key"}),
            true,
        )
        .unwrap();
        state
    };
    let mut state = progress_state();
    let progress = state.quest_progress.remove("4:lost_key").unwrap();
    state.quest_progress.insert("wrong".into(), progress);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("quest progress key"));

    let mut state = progress_state();
    state.quest_progress.get_mut("4:lost_key").unwrap().actor_id = 1;
    state
        .quest_progress
        .remove("4:lost_key")
        .map(|progress| state.quest_progress.insert("1:lost_key".into(), progress));
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("not an actor"));

    let mut state = progress_state();
    state.quest_progress.get_mut("4:lost_key").unwrap().quest_id = "missing".into();
    state
        .quest_progress
        .remove("4:lost_key")
        .map(|progress| state.quest_progress.insert("4:missing".into(), progress));
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("definition does not exist"));

    let receipt = Receipt {
        tx_id: "11".repeat(32),
        from: "admin".into(),
        nonce: 0,
        ok: true,
        messages: Vec::new(),
        result: json!({}),
        state_root: "22".repeat(32),
    };
    let mut state = valid();
    state.receipts.insert("wrong".into(), receipt.clone());
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("receipt map key"));

    let mut state = valid();
    let mut missing_account_receipt = receipt.clone();
    missing_account_receipt.from = "missing".into();
    state
        .receipts
        .insert(receipt.tx_id.clone(), missing_account_receipt);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("receipt account does not exist"));

    let mut state = valid();
    let mut invalid_hash_receipt = receipt;
    invalid_hash_receipt.state_root = "bad".into();
    state
        .receipts
        .insert(invalid_hash_receipt.tx_id.clone(), invalid_hash_receipt);
    assert!(state
        .validate()
        .unwrap_err()
        .to_string()
        .contains("receipt state root"));
}

#[test]
fn demo_item_transfer_requires_colocation_and_updates_owner_inventory() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    let mut recipient_data = BTreeMap::new();
    recipient_data.insert("name".into(), json!("Rowan"));
    let recipient = runtime::create_entity(
        &mut state,
        EntityKind::Actor,
        "admin".into(),
        None,
        Some(2),
        recipient_data,
    )
    .unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction": "east"}), true).unwrap();
    runtime::execute(&mut state, actor, "take", json!({"item_id": 5}), true).unwrap();
    runtime::execute(
        &mut state,
        actor,
        "give",
        json!({"item_id": 5, "target_id": recipient}),
        true,
    )
    .unwrap();
    assert!(state.inventories[&actor].is_empty());
    assert_eq!(state.inventories[&recipient], vec![5]);
    assert_eq!(state.entities[&5].location, Some(recipient));
    assert!(state
        .events
        .iter()
        .any(|event| event.kind == "item_transferred"));

    state.entities.get_mut(&actor).unwrap().location = Some(1);
    let root = state.root();
    assert!(runtime::execute(
        &mut state,
        actor,
        "give",
        json!({"item_id": 5, "target_id": recipient}),
        true,
    )
    .is_err());
    assert_eq!(state.root(), root);
    assert_eq!(state.inventories[&recipient], vec![5]);
}

#[test]
fn demo_item_can_be_dropped_and_taken_again() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    runtime::execute(&mut state, actor, "go", json!({"direction": "east"}), true).unwrap();
    runtime::execute(&mut state, actor, "take", json!({"item_id": 5}), true).unwrap();
    runtime::execute(&mut state, actor, "drop", json!({"item_id": 5}), true).unwrap();
    assert!(state.inventories[&actor].is_empty());
    assert_eq!(state.entities[&5].location, Some(2));
    runtime::execute(&mut state, actor, "take", json!({"item_id": 5}), true).unwrap();
    assert_eq!(state.inventories[&actor], vec![5]);
}

#[test]
fn demo_upgrade_is_versioned_and_idempotent() {
    let mut state = WorldState::genesis();
    let actor = demo::initialize(&mut state).unwrap();
    let old_source = "function query_old() return {} end".to_string();
    let contract = state.contracts.get_mut("demo").unwrap();
    contract.source = old_source.clone();
    contract.source_hash = blake3::hash(old_source.as_bytes()).to_hex().to_string();
    state.quests.clear();
    state
        .entities
        .get_mut(&actor)
        .unwrap()
        .data
        .insert("quest".into(), json!("accepted"));
    state
        .entities
        .get_mut(&actor)
        .unwrap()
        .data
        .insert("inventory".into(), json!([5]));

    assert!(demo::ensure_current(&mut state).unwrap());
    assert_eq!(state.entities[&actor].contract.as_deref(), Some("demo@2"));
    assert!(state.quests.contains_key("lost_key"));
    assert!(state.quests.contains_key("ruins_tablet"));
    assert!(state.quests.contains_key("open_shrine"));
    assert_eq!(state.quests["lost_key"].giver_entity_id, 6);
    assert_eq!(state.quests["ruins_tablet"].giver_entity_id, 7);
    assert_eq!(
        state.quest_progress["4:lost_key"].status,
        QuestStatus::Accepted
    );
    assert_eq!(state.inventories[&actor], vec![5]);
    assert_eq!(state.entities[&5].location, Some(actor));
    assert!(!state.entities[&actor].data.contains_key("inventory"));
    assert!(!demo::ensure_current(&mut state).unwrap());
    assert_eq!(state.contracts.len(), 2);
}
