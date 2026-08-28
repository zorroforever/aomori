use crate::model::{EntityKind, WorldState};
use anyhow::{anyhow, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

pub fn migrate_legacy_inventories(state: &mut WorldState) -> Result<bool> {
    let migrations = collect_legacy_inventories(state)?;
    if migrations.is_empty() {
        return Ok(false);
    }

    let mut candidate = state.clone();
    for (actor_id, item_ids) in migrations {
        candidate
            .entities
            .get_mut(&actor_id)
            .expect("migration actor was prevalidated")
            .data
            .remove("inventory");
        let inventory = candidate.inventories.entry(actor_id).or_default();
        for item_id in item_ids {
            if !inventory.contains(&item_id) {
                inventory.push(item_id);
            }
            candidate
                .entities
                .get_mut(&item_id)
                .expect("migration item was prevalidated")
                .location = Some(actor_id);
        }
    }
    candidate.head = candidate
        .head
        .checked_add(1)
        .ok_or_else(|| anyhow!("world head overflow during inventory migration"))?;
    candidate
        .validate_locations()
        .context("validate locations after legacy inventory migration")?;
    candidate
        .validate_inventories()
        .context("validate inventories after legacy inventory migration")?;
    *state = candidate;
    Ok(true)
}

fn collect_legacy_inventories(state: &WorldState) -> Result<Vec<(u64, Vec<u64>)>> {
    let mut migrations = Vec::new();
    let mut claimed_by = BTreeMap::new();
    for actor in state
        .entities
        .values()
        .filter(|entity| entity.kind == EntityKind::Actor)
    {
        let Some(value) = actor.data.get("inventory") else {
            continue;
        };
        let values = value
            .as_array()
            .ok_or_else(|| anyhow!("legacy inventory for actor {} must be an array", actor.id))?;
        let mut seen = BTreeSet::new();
        let mut item_ids = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let item_id = value.as_u64().ok_or_else(|| {
                anyhow!(
                    "legacy inventory for actor {} contains a non-integer item at index {index}",
                    actor.id
                )
            })?;
            if !seen.insert(item_id) {
                return Err(anyhow!(
                    "legacy inventory for actor {} contains duplicate item: {item_id}",
                    actor.id
                ));
            }
            let item = state.entities.get(&item_id).ok_or_else(|| {
                anyhow!(
                    "legacy inventory for actor {} references missing item: {item_id}",
                    actor.id
                )
            })?;
            if item.kind != EntityKind::Item {
                return Err(anyhow!(
                    "legacy inventory for actor {} references non-item entity: {item_id}",
                    actor.id
                ));
            }
            if let Some(previous_actor) = claimed_by.insert(item_id, actor.id) {
                return Err(anyhow!(
                    "legacy item {item_id} is claimed by multiple actors: {previous_actor} and {}",
                    actor.id
                ));
            }
            item_ids.push(item_id);
        }
        migrations.push((actor.id, item_ids));
    }
    Ok(migrations)
}

#[cfg(test)]
mod tests {
    use super::migrate_legacy_inventories;
    use crate::demo;
    use crate::model::{EntityKind, WorldState};
    use crate::runtime;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn migrates_inventory_for_any_actor_and_is_idempotent() {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
            .entities
            .get_mut(&4)
            .unwrap()
            .data
            .insert("inventory".into(), json!([5]));
        let old_head = state.head;

        assert!(migrate_legacy_inventories(&mut state).unwrap());
        assert_eq!(state.head, old_head + 1);
        assert_eq!(state.inventories[&4], vec![5]);
        assert_eq!(state.entities[&5].location, Some(4));
        assert!(!state.entities[&4].data.contains_key("inventory"));
        assert!(!migrate_legacy_inventories(&mut state).unwrap());
        assert_eq!(state.head, old_head + 1);
    }

    #[test]
    fn malformed_inventory_rolls_back_without_dropping_values() {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
            .entities
            .get_mut(&4)
            .unwrap()
            .data
            .insert("inventory".into(), json!([5, "bad"]));
        let root = state.root();

        let error = migrate_legacy_inventories(&mut state)
            .unwrap_err()
            .to_string();
        assert!(error.contains("non-integer item at index 1"));
        assert_eq!(state.root(), root);
        assert_eq!(state.entities[&4].data["inventory"], json!([5, "bad"]));
    }

    #[test]
    fn duplicate_legacy_claims_fail_and_roll_back() {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        let second_actor = runtime::create_entity(
            &mut state,
            EntityKind::Actor,
            "admin".into(),
            Some("demo".into()),
            Some(1),
            BTreeMap::new(),
        )
        .unwrap();
        state
            .entities
            .get_mut(&4)
            .unwrap()
            .data
            .insert("inventory".into(), json!([5]));
        state
            .entities
            .get_mut(&second_actor)
            .unwrap()
            .data
            .insert("inventory".into(), json!([5]));
        let root = state.root();

        let error = format!("{:#}", migrate_legacy_inventories(&mut state).unwrap_err());
        assert!(error.contains("claimed by multiple actors"));
        assert_eq!(state.root(), root);
    }

    #[test]
    fn duplicate_and_missing_items_fail_before_mutation() {
        for (inventory, expected) in [
            (json!([5, 5]), "contains duplicate item: 5"),
            (json!([999]), "references missing item: 999"),
        ] {
            let mut state = WorldState::genesis();
            demo::initialize(&mut state).unwrap();
            state
                .entities
                .get_mut(&4)
                .unwrap()
                .data
                .insert("inventory".into(), inventory);
            let root = state.root();

            let error = migrate_legacy_inventories(&mut state)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
            assert_eq!(state.root(), root);
        }
    }

    #[test]
    fn non_item_reference_fails_before_mutation() {
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
            .entities
            .get_mut(&4)
            .unwrap()
            .data
            .insert("inventory".into(), json!([6]));
        let root = state.root();

        let error = migrate_legacy_inventories(&mut state)
            .unwrap_err()
            .to_string();
        assert!(error.contains("references non-item entity: 6"));
        assert_eq!(state.root(), root);
    }
}
