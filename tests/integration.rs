use aomori::model::{EntityKind, WorldState};
use aomori::runtime;
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
}
