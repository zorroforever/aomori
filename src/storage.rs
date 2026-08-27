use crate::model::WorldState;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Snapshot {
    format_version: u32,
    state_root: String,
    state: WorldState,
}

#[derive(Clone)]
pub struct SnapshotStore {
    path: PathBuf,
}

impl SnapshotStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        fs::create_dir_all(data_dir)?;
        Ok(Self {
            path: data_dir.join("state.json"),
        })
    }

    pub fn load(&self) -> Result<WorldState> {
        if !self.path.exists() {
            return Ok(WorldState::genesis());
        }
        match decode(&self.path) {
            Ok(state) => Ok(state),
            Err(primary_error) => {
                let backup = self.backup_path();
                if !backup.exists() {
                    return Err(primary_error)
                        .with_context(|| format!("invalid snapshot {}", self.path.display()));
                }
                let state = decode(&backup).with_context(|| {
                    format!(
                        "primary snapshot {} is invalid ({primary_error}); backup {} is also invalid",
                        self.path.display(),
                        backup.display()
                    )
                })?;
                restore_file(&self.path, &backup).with_context(|| {
                    format!(
                        "restore primary snapshot {} from {}",
                        self.path.display(),
                        backup.display()
                    )
                })?;
                Ok(state)
            }
        }
    }

    pub fn save(&self, state: &WorldState) -> Result<()> {
        state.validate().context("validate world state")?;
        let temporary = self.path.with_extension("json.tmp");
        let backup_temporary = self.path.with_extension("json.bak.tmp");
        let previous = if self.path.exists() {
            Some(
                fs::read(&self.path)
                    .with_context(|| format!("read existing snapshot {}", self.path.display()))?,
            )
        } else {
            None
        };
        let snapshot = Snapshot {
            format_version: FORMAT_VERSION,
            state_root: state.root(),
            state: state.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&snapshot)?;
        let result = (|| {
            write_synced(&temporary, &bytes)
                .with_context(|| format!("write temporary snapshot {}", temporary.display()))?;
            if previous.is_some() && decode(&self.path).is_ok() {
                let backup = self.backup_path();
                let primary_bytes = fs::read(&self.path).with_context(|| {
                    format!("read primary snapshot for backup {}", self.path.display())
                })?;
                write_synced(&backup_temporary, &primary_bytes).with_context(|| {
                    format!(
                        "write temporary snapshot backup {}",
                        backup_temporary.display()
                    )
                })?;
                fs::rename(&backup_temporary, &backup)
                    .with_context(|| format!("replace snapshot backup {}", backup.display()))?;
                sync_parent(&backup)?;
            }
            fs::rename(&temporary, &self.path)
                .with_context(|| format!("replace snapshot {}", self.path.display()))?;
            if let Err(sync_error) = sync_parent(&self.path) {
                rollback_primary(&self.path, previous.as_deref()).with_context(|| {
                    format!(
                        "sync snapshot directory after replacing {} failed ({sync_error}); rollback also failed",
                        self.path.display()
                    )
                })?;
                return Err(sync_error).context("sync snapshot directory");
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
            let _ = fs::remove_file(&backup_temporary);
        }
        result
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn backup_path(&self) -> PathBuf {
        self.path.with_extension("json.bak")
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", path.display()))?;
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    File::open(parent)
        .with_context(|| format!("open snapshot directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("sync snapshot directory {}", parent.display()))
}

fn restore_file(target: &Path, source: &Path) -> Result<()> {
    let temporary = target.with_extension("json.restore.tmp");
    let result = (|| {
        let bytes = fs::read(source)
            .with_context(|| format!("read recovery snapshot {}", source.display()))?;
        write_synced(&temporary, &bytes)?;
        fs::rename(&temporary, target)
            .with_context(|| format!("replace recovered snapshot {}", target.display()))?;
        sync_parent(target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn rollback_primary(target: &Path, previous: Option<&[u8]>) -> Result<()> {
    if let Some(bytes) = previous {
        let temporary = target.with_extension("json.rollback.tmp");
        let result = (|| {
            write_synced(&temporary, bytes)?;
            fs::rename(&temporary, target)
                .with_context(|| format!("restore previous snapshot {}", target.display()))?;
            sync_parent(target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    } else {
        if target.exists() {
            fs::remove_file(target)
                .with_context(|| format!("remove uncommitted snapshot {}", target.display()))?;
        }
        sync_parent(target)
    }
}

fn decode(path: &Path) -> Result<WorldState> {
    let bytes = fs::read(path).with_context(|| format!("read snapshot {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse snapshot {}", path.display()))?;
    let legacy_quest_schema = quest_definitions_missing_giver(&value);
    let legacy_inventory_schema = actors_contain_legacy_inventory(&value);
    if value.get("format_version").is_none() {
        let state: WorldState = serde_json::from_value(value)
            .with_context(|| format!("parse legacy snapshot {}", path.display()))?;
        state
            .validate_references()
            .context("validate core references")?;
        state.validate_events().context("validate event log")?;
        state
            .validate_locations()
            .context("validate entity locations")?;
        if !legacy_quest_schema {
            state
                .validate_quests()
                .context("validate quest definitions")?;
        }
        if !legacy_inventory_schema {
            state
                .validate_inventories()
                .context("validate inventories")?;
        }
        return Ok(state);
    }
    let snapshot: Snapshot = serde_json::from_value(value)
        .with_context(|| format!("parse versioned snapshot {}", path.display()))?;
    if snapshot.format_version != FORMAT_VERSION {
        return Err(anyhow!(
            "unsupported snapshot format version: {}",
            snapshot.format_version
        ));
    }
    let actual_root = snapshot.state.root();
    if snapshot.state_root != actual_root {
        return Err(anyhow!(
            "snapshot state root mismatch: expected {}, got {}",
            snapshot.state_root,
            actual_root
        ));
    }
    snapshot
        .state
        .validate_references()
        .context("validate core references")?;
    snapshot
        .state
        .validate_events()
        .context("validate event log")?;
    snapshot
        .state
        .validate_locations()
        .context("validate entity locations")?;
    if !legacy_quest_schema {
        snapshot
            .state
            .validate_quests()
            .context("validate quest definitions")?;
    }
    if !legacy_inventory_schema {
        snapshot
            .state
            .validate_inventories()
            .context("validate inventories")?;
    }
    Ok(snapshot.state)
}

fn actors_contain_legacy_inventory(value: &serde_json::Value) -> bool {
    let state = value.get("state").unwrap_or(value);
    state
        .get("entities")
        .and_then(serde_json::Value::as_object)
        .map(|entities| {
            entities.values().any(|entity| {
                entity.get("kind").and_then(serde_json::Value::as_str) == Some("actor")
                    && entity
                        .get("data")
                        .and_then(|data| data.get("inventory"))
                        .is_some()
            })
        })
        .unwrap_or(false)
}

fn quest_definitions_missing_giver(value: &serde_json::Value) -> bool {
    let state = value.get("state").unwrap_or(value);
    state
        .get("quests")
        .and_then(serde_json::Value::as_object)
        .map(|quests| {
            quests
                .values()
                .any(|quest| quest.get("giver_entity_id").is_none())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{SnapshotStore, FORMAT_VERSION};
    use crate::demo;
    use crate::model::{QuestDefinition, WorldState};
    use crate::runtime;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn saves_and_loads_state() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 7;
        store.save(&state).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.head, 7);
        assert_eq!(loaded.root(), state.root());
        assert!(!store.path().with_extension("json.tmp").exists());
        assert!(!store.path().with_extension("json.bak.tmp").exists());
    }

    #[test]
    fn invalid_quest_definition_is_not_saved() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.quests.insert(
            "broken".into(),
            QuestDefinition {
                id: "broken".into(),
                title: "Broken".into(),
                giver_entity_id: 99,
                prerequisite_quest_ids: Vec::new(),
                required_item: "nothing".into(),
                completion_zone: "Nowhere".into(),
                reward_balance: 0,
                consume_required_item: false,
            },
        );
        let error = store.save(&state).unwrap_err().to_string();
        assert!(error.contains("validate world state"));
        assert!(!store.path().exists());
    }

    #[test]
    fn invalid_event_log_is_rejected_even_with_matching_state_root() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        runtime::execute(&mut state, 4, "go", json!({"direction":"east"}), true).unwrap();
        state.next_event_id = state.events.last().unwrap().id;
        let value = json!({
            "format_version": FORMAT_VERSION,
            "state_root": state.root(),
            "state": state
        });
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("invalid snapshot"));
    }

    #[test]
    fn dangling_entity_owner_is_rejected_even_with_matching_state_root() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state.entities.get_mut(&4).unwrap().owner = "missing".into();
        let value = json!({
            "format_version": FORMAT_VERSION,
            "state_root": state.root(),
            "state": state
        });
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("invalid snapshot"));
    }

    #[test]
    fn invalid_inventory_is_rejected_even_with_matching_state_root() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state.entities.get_mut(&5).unwrap().location = Some(4);
        let value = json!({
            "format_version": FORMAT_VERSION,
            "state_root": state.root(),
            "state": state
        });
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("invalid snapshot"));
    }

    #[test]
    fn loads_legacy_actor_data_inventory_for_startup_migration() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
            .entities
            .get_mut(&4)
            .unwrap()
            .data
            .insert("inventory".into(), json!([5]));
        fs::write(store.path(), serde_json::to_vec(&state).unwrap()).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.entities[&4].data["inventory"], json!([5]));
    }

    #[test]
    fn invalid_quest_definition_is_rejected_even_with_matching_state_root() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.quests.insert(
            "broken".into(),
            QuestDefinition {
                id: "broken".into(),
                title: "Broken".into(),
                giver_entity_id: 99,
                prerequisite_quest_ids: Vec::new(),
                required_item: "nothing".into(),
                completion_zone: "Nowhere".into(),
                reward_balance: 0,
                consume_required_item: false,
            },
        );
        let value = json!({
            "format_version": FORMAT_VERSION,
            "state_root": state.root(),
            "state": state
        });
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("invalid snapshot"));
    }

    #[test]
    fn loads_legacy_quest_schema_for_startup_migration() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.quests.insert(
            "legacy".into(),
            QuestDefinition {
                id: "legacy".into(),
                title: "Legacy".into(),
                giver_entity_id: 0,
                prerequisite_quest_ids: Vec::new(),
                required_item: "item".into(),
                completion_zone: "zone".into(),
                reward_balance: 0,
                consume_required_item: false,
            },
        );
        let mut value = serde_json::to_value(state).unwrap();
        value["quests"]["legacy"]
            .as_object_mut()
            .unwrap()
            .remove("giver_entity_id");
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(store.load().unwrap().quests["legacy"].giver_entity_id, 0);
    }

    #[test]
    fn loads_legacy_state() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 3;
        fs::write(store.path(), serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(store.load().unwrap().head, 3);
    }

    #[test]
    fn corrupted_snapshot_is_rejected() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        fs::write(store.path(), b"not-json").unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn state_root_mismatch_is_rejected() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        fs::write(
            store.path(),
            serde_json::to_vec(&json!({
                "format_version": 1,
                "state_root": "wrong",
                "state": WorldState::genesis()
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn backup_replace_failure_keeps_primary_and_cleans_temporary_files() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 1;
        store.save(&state).unwrap();
        fs::create_dir(store.backup_path()).unwrap();

        state.head = 2;
        assert!(store.save(&state).is_err());
        assert_eq!(store.load().unwrap().head, 1);
        assert!(!store.path().with_extension("json.tmp").exists());
        assert!(!store.path().with_extension("json.bak.tmp").exists());
    }

    #[test]
    fn corrupt_primary_does_not_replace_a_valid_backup() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 1;
        store.save(&state).unwrap();
        state.head = 2;
        store.save(&state).unwrap();
        fs::write(store.path(), b"corrupted before save").unwrap();

        state.head = 3;
        store.save(&state).unwrap();
        fs::write(store.path(), b"corrupted after save").unwrap();
        assert_eq!(store.load().unwrap().head, 1);
    }

    #[test]
    fn recovers_previous_snapshot_from_backup() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 1;
        store.save(&state).unwrap();
        state.head = 2;
        store.save(&state).unwrap();
        fs::write(store.path(), b"corrupted").unwrap();
        assert_eq!(store.load().unwrap().head, 1);
        assert_eq!(store.load().unwrap().head, 1);
        assert!(!store.path().with_extension("json.restore.tmp").exists());
    }
}
