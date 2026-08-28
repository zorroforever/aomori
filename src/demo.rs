use crate::model::{EntityKind, QuestDefinition, WorldState};
use crate::runtime;
use anyhow::Result;
use serde_json::json;
use std::collections::BTreeMap;

const DEMO_SOURCE: &str = include_str!("../contracts/demo.lua");

pub fn initialize(state: &mut WorldState) -> Result<u64> {
    if !state.entities.is_empty() || !state.contracts.is_empty() {
        return Err(anyhow::anyhow!("demo requires an empty world"));
    }
    runtime::deploy(state, "demo".into(), DEMO_SOURCE.into())?;
    let village = zone(
        state,
        "Village",
        "A warm village at the edge of an old forest.",
        json!({"east": 2}),
    )?;
    let forest = zone(
        state,
        "Forest",
        "Tall trees hide a path toward the ruins.",
        json!({"west": village, "east": 3}),
    )?;
    let ruins = zone(
        state,
        "Ruins",
        "Broken stones surround a locked shrine.",
        json!({"west": forest}),
    )?;
    state
        .entities
        .get_mut(&village)
        .unwrap()
        .data
        .insert("exits".into(), json!({"east": forest}));
    let actor = runtime::create_entity(
        state,
        EntityKind::Actor,
        "admin".into(),
        Some("demo".into()),
        Some(village),
        BTreeMap::new(),
    )?;
    runtime::create_entity(
        state,
        EntityKind::Item,
        "admin".into(),
        None,
        Some(forest),
        map("name", "brass key"),
    )?;
    let mut npc_data = BTreeMap::new();
    npc_data.insert("name".into(), json!("Mira"));
    npc_data.insert(
        "dialogue".into(),
        json!("The ruins remember every traveler."),
    );
    let mira = runtime::create_entity(
        state,
        EntityKind::Actor,
        "admin".into(),
        Some("demo".into()),
        Some(village),
        npc_data,
    )?;
    let mut rowan_data = BTreeMap::new();
    rowan_data.insert("name".into(), json!("Rowan"));
    rowan_data.insert(
        "dialogue".into(),
        json!("A stone tablet in the ruins may preserve the old road."),
    );
    let rowan = runtime::create_entity(
        state,
        EntityKind::Actor,
        "admin".into(),
        Some("demo".into()),
        Some(village),
        rowan_data,
    )?;
    runtime::create_entity(
        state,
        EntityKind::Item,
        "admin".into(),
        None,
        Some(ruins),
        map("name", "stone tablet"),
    )?;
    state.quests.insert("lost_key".into(), lost_key_quest(mira));
    state
        .quests
        .insert("ruins_tablet".into(), ruins_tablet_quest(rowan));
    state
        .quests
        .insert("open_shrine".into(), open_shrine_quest(rowan));
    state.validate()?;
    Ok(actor)
}

pub fn ensure_current(state: &mut WorldState) -> Result<bool> {
    if state.entities.is_empty() && state.contracts.is_empty() {
        initialize(state)?;
        return Ok(true);
    }
    if !state
        .contracts
        .values()
        .any(|contract| contract.name == "demo")
    {
        return Err(anyhow::anyhow!(
            "existing world does not contain the demo contract"
        ));
    }

    let inventory_migrated = crate::migration::migrate_legacy_inventories(state)?;
    let mut changed = false;

    let source_hash = blake3::hash(DEMO_SOURCE.as_bytes()).to_hex().to_string();
    let target = state
        .contracts
        .iter()
        .filter(|(_, contract)| contract.name == "demo" && contract.source_hash == source_hash)
        .max_by_key(|(_, contract)| contract.version)
        .map(|(key, _)| key.clone());
    let target = match target {
        Some(target) => target,
        None => {
            let version = state
                .contracts
                .values()
                .filter(|contract| contract.name == "demo")
                .map(|contract| contract.version)
                .max()
                .unwrap_or(0)
                + 1;
            runtime::deploy_version(state, "demo".into(), version, DEMO_SOURCE.into())?;
            changed = true;
            runtime::contract_key("demo", version)
        }
    };

    let demo_contracts: std::collections::BTreeSet<String> = state
        .contracts
        .iter()
        .filter(|(_, contract)| contract.name == "demo")
        .map(|(key, _)| key.clone())
        .collect();
    let mut legacy_progress = Vec::new();
    for entity in state.entities.values_mut() {
        let is_demo_actor = entity
            .contract
            .as_ref()
            .map(|contract| demo_contracts.contains(contract))
            .unwrap_or(false)
            || (entity.kind == EntityKind::Actor
                && entity.data.get("name").and_then(|value| value.as_str()) == Some("Mira"));
        if is_demo_actor {
            if entity.contract.as_deref() != Some(target.as_str()) {
                entity.contract = Some(target.clone());
                changed = true;
            }
            if let Some(status) = entity.data.get("quest").and_then(|value| value.as_str()) {
                let status = match status {
                    "accepted" => Some(crate::model::QuestStatus::Accepted),
                    "completed" => Some(crate::model::QuestStatus::Completed),
                    _ => None,
                };
                if let Some(status) = status {
                    legacy_progress.push((entity.id, status));
                }
            }
        }
    }
    for (actor_id, status) in legacy_progress {
        let key = format!("{actor_id}:lost_key");
        if let std::collections::btree_map::Entry::Vacant(entry) = state.quest_progress.entry(key) {
            entry.insert(crate::model::QuestProgress {
                quest_id: "lost_key".into(),
                actor_id,
                status,
            });
            changed = true;
        }
    }
    let village = find_named_entity(state, EntityKind::Zone, "Village")
        .ok_or_else(|| anyhow::anyhow!("demo Village is missing"))?;
    let ruins = find_named_entity(state, EntityKind::Zone, "Ruins")
        .ok_or_else(|| anyhow::anyhow!("demo Ruins are missing"))?;
    let mira = find_named_entity(state, EntityKind::Actor, "Mira")
        .ok_or_else(|| anyhow::anyhow!("demo Mira is missing"))?;
    let rowan = match find_named_entity(state, EntityKind::Actor, "Rowan") {
        Some(id) => id,
        None => {
            let mut data = map("name", "Rowan");
            data.insert(
                "dialogue".into(),
                json!("A stone tablet in the ruins may preserve the old road."),
            );
            changed = true;
            runtime::create_entity(
                state,
                EntityKind::Actor,
                "admin".into(),
                Some(target.clone()),
                Some(village),
                data,
            )?
        }
    };
    if find_named_entity(state, EntityKind::Item, "stone tablet").is_none() {
        runtime::create_entity(
            state,
            EntityKind::Item,
            "admin".into(),
            None,
            Some(ruins),
            map("name", "stone tablet"),
        )?;
        changed = true;
    }
    for definition in [
        lost_key_quest(mira),
        ruins_tablet_quest(rowan),
        open_shrine_quest(rowan),
    ] {
        if state.quests.get(&definition.id) != Some(&definition) {
            state.quests.insert(definition.id.clone(), definition);
            changed = true;
        }
    }
    state.validate()?;
    if changed {
        state.head += 1;
    }
    Ok(changed || inventory_migrated)
}

fn lost_key_quest(giver_entity_id: u64) -> QuestDefinition {
    QuestDefinition {
        id: "lost_key".into(),
        title: "The Lost Key".into(),
        giver_entity_id,
        prerequisite_quest_ids: Vec::new(),
        required_item: "brass key".into(),
        completion_zone: "Village".into(),
        reward_balance: 10,
        consume_required_item: true,
    }
}

fn ruins_tablet_quest(giver_entity_id: u64) -> QuestDefinition {
    QuestDefinition {
        id: "ruins_tablet".into(),
        title: "Echoes in Stone".into(),
        giver_entity_id,
        prerequisite_quest_ids: Vec::new(),
        required_item: "stone tablet".into(),
        completion_zone: "Village".into(),
        reward_balance: 6,
        consume_required_item: false,
    }
}

fn open_shrine_quest(giver_entity_id: u64) -> QuestDefinition {
    QuestDefinition {
        id: "open_shrine".into(),
        title: "The Open Shrine".into(),
        giver_entity_id,
        prerequisite_quest_ids: vec!["lost_key".into()],
        required_item: "stone tablet".into(),
        completion_zone: "Ruins".into(),
        reward_balance: 4,
        consume_required_item: false,
    }
}

fn find_named_entity(state: &WorldState, kind: EntityKind, name: &str) -> Option<u64> {
    state.entities.values().find_map(|entity| {
        (entity.kind == kind
            && entity.data.get("name").and_then(|value| value.as_str()) == Some(name))
        .then_some(entity.id)
    })
}

fn zone(
    state: &mut WorldState,
    name: &str,
    description: &str,
    exits: serde_json::Value,
) -> Result<u64> {
    let mut data = BTreeMap::new();
    data.insert("name".into(), json!(name));
    data.insert("description".into(), json!(description));
    data.insert("exits".into(), exits);
    runtime::create_entity(state, EntityKind::Zone, "admin".into(), None, None, data)
}

fn map(key: &str, value: &str) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([(key.into(), json!(value))])
}
