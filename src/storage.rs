use crate::model::WorldState;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const LEGACY_FORMAT_VERSION: u32 = 1;
pub const FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatMigration {
    pub from_version: u32,
    pub to_version: u32,
    pub name: &'static str,
}

const FORMAT_MIGRATIONS: &[FormatMigration] = &[FormatMigration {
    from_version: LEGACY_FORMAT_VERSION,
    to_version: FORMAT_VERSION,
    name: "snapshot_format_1_to_2",
}];

#[derive(Serialize, Deserialize)]
struct Snapshot {
    format_version: u32,
    state_root: String,
    state: WorldState,
}

#[derive(Debug)]
pub struct LoadedSnapshot {
    pub world: WorldState,
    pub source_format_version: Option<u32>,
    pub format_migrations: Vec<FormatMigration>,
    pub needs_rewrite: bool,
}

struct DecodedSnapshot {
    world: WorldState,
    source_format_version: Option<u32>,
    format_migrations: Vec<FormatMigration>,
    needs_rewrite: bool,
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
        Ok(self.load_with_status()?.world)
    }

    pub fn load_with_status(&self) -> Result<LoadedSnapshot> {
        if !self.path.exists() {
            return Ok(LoadedSnapshot {
                world: WorldState::genesis(),
                source_format_version: None,
                format_migrations: Vec::new(),
                needs_rewrite: false,
            });
        }
        match decode(&self.path) {
            Ok(decoded) => Ok(LoadedSnapshot {
                world: decoded.world,
                source_format_version: decoded.source_format_version,
                format_migrations: decoded.format_migrations,
                needs_rewrite: decoded.needs_rewrite,
            }),
            Err(primary_error) => {
                let backup = self.backup_path();
                if !backup.exists() {
                    return Err(primary_error)
                        .with_context(|| format!("invalid snapshot {}", self.path.display()));
                }
                let decoded = decode(&backup).with_context(|| {
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
                Ok(LoadedSnapshot {
                    world: decoded.world,
                    source_format_version: decoded.source_format_version,
                    format_migrations: decoded.format_migrations,
                    needs_rewrite: decoded.needs_rewrite,
                })
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
        let snapshot = serde_json::to_value(snapshot)?;
        validate_current_schema(&snapshot).context("validate current snapshot schema")?;
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

fn decode(path: &Path) -> Result<DecodedSnapshot> {
    let bytes = fs::read(path).with_context(|| format!("read snapshot {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse snapshot {}", path.display()))?;
    let legacy_quest_schema = quest_definitions_missing_giver(&value);
    let legacy_inventory_schema = actors_contain_legacy_inventory(&value);
    if value.get("format_version").is_none() {
        let state: WorldState = serde_json::from_value(value)
            .with_context(|| format!("parse legacy snapshot {}", path.display()))?;
        validate_legacy_state(&state, legacy_quest_schema, legacy_inventory_schema)?;
        return Ok(DecodedSnapshot {
            world: state,
            source_format_version: None,
            format_migrations: Vec::new(),
            needs_rewrite: true,
        });
    }

    let format_version = value
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .filter(|version| *version > 0)
        .ok_or_else(|| anyhow!("snapshot format_version must be a positive integer"))?;
    let format_migrations = format_migration_path(format_version, FORMAT_VERSION)?;
    if format_version == FORMAT_VERSION {
        validate_current_schema(&value)?;
    }

    let snapshot: Snapshot = serde_json::from_value(value)
        .with_context(|| format!("parse versioned snapshot {}", path.display()))?;
    let actual_root = snapshot.state.root();
    if snapshot.state_root != actual_root {
        return Err(anyhow!(
            "snapshot state root mismatch: expected {}, got {}",
            snapshot.state_root,
            actual_root
        ));
    }

    if snapshot.format_version == LEGACY_FORMAT_VERSION {
        validate_legacy_state(
            &snapshot.state,
            legacy_quest_schema,
            legacy_inventory_schema,
        )?;
    } else {
        snapshot
            .state
            .validate()
            .context("validate current snapshot state")?;
    }
    Ok(DecodedSnapshot {
        world: snapshot.state,
        source_format_version: Some(snapshot.format_version),
        format_migrations,
        needs_rewrite: snapshot.format_version != FORMAT_VERSION,
    })
}

fn format_migration_path(from: u32, target: u32) -> Result<Vec<FormatMigration>> {
    let mut version = from;
    let mut path = Vec::new();
    while version != target {
        let mut migrations = FORMAT_MIGRATIONS
            .iter()
            .filter(|migration| migration.from_version == version);
        let Some(migration) = migrations.next() else {
            return Err(anyhow!(
                "unsupported snapshot format version: {from}; no migration from version {version} to {target}"
            ));
        };
        if migrations.next().is_some() {
            return Err(anyhow!(
                "ambiguous snapshot migrations from version {version}"
            ));
        }
        if migration.to_version <= version {
            return Err(anyhow!(
                "invalid snapshot migration: {version} to {}",
                migration.to_version
            ));
        }
        path.push(*migration);
        version = migration.to_version;
    }
    Ok(path)
}

fn validate_legacy_state(
    state: &WorldState,
    legacy_quest_schema: bool,
    legacy_inventory_schema: bool,
) -> Result<()> {
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
    Ok(())
}

fn validate_current_schema(value: &serde_json::Value) -> Result<()> {
    let envelope = value
        .as_object()
        .ok_or_else(|| anyhow!("snapshot must be an object"))?;
    require_object_fields(
        envelope,
        "envelope",
        &["format_version", "state_root", "state"],
    )?;
    let state = envelope
        .get("state")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("snapshot state must be an object"))?;
    require_object_fields(
        state,
        "state",
        &[
            "accounts",
            "entities",
            "contracts",
            "receipts",
            "quests",
            "quest_progress",
            "inventories",
            "events",
            "next_event_id",
            "next_entity_id",
            "head",
        ],
    )?;

    if actors_contain_legacy_inventory(value) {
        return Err(anyhow!(
            "snapshot format version {FORMAT_VERSION} contains legacy actor inventory data"
        ));
    }
    require_map_entry_fields(
        state.get("accounts"),
        "account",
        &["name", "public_key", "nonce", "balance"],
    )?;
    require_map_entry_fields(
        state.get("entities"),
        "entity",
        &["id", "kind", "owner", "location", "contract", "data"],
    )?;
    require_map_entry_fields(
        state.get("contracts"),
        "contract",
        &["name", "version", "source", "source_hash", "status"],
    )?;
    require_map_entry_fields(
        state.get("receipts"),
        "receipt",
        &[
            "tx_id",
            "from",
            "nonce",
            "ok",
            "messages",
            "result",
            "state_root",
        ],
    )?;
    require_map_entry_fields(
        state.get("quests"),
        "quest",
        &[
            "id",
            "title",
            "giver_entity_id",
            "prerequisite_quest_ids",
            "required_item",
            "completion_zone",
            "reward_balance",
            "consume_required_item",
        ],
    )?;
    require_map_entry_fields(
        state.get("quest_progress"),
        "quest progress",
        &["quest_id", "actor_id", "status"],
    )?;
    require_array_entry_fields(
        state.get("events"),
        "event",
        &["id", "head", "kind", "entity_id", "data"],
    )
}

fn require_object_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    label: &str,
    fields: &[&str],
) -> Result<()> {
    for field in fields {
        if !object.contains_key(*field) {
            return Err(anyhow!(
                "snapshot format version {FORMAT_VERSION} {label} is missing field: {field}"
            ));
        }
    }
    for field in object.keys() {
        if !fields.contains(&field.as_str()) {
            return Err(anyhow!(
                "snapshot format version {FORMAT_VERSION} {label} contains unknown field: {field}"
            ));
        }
    }
    Ok(())
}

fn require_map_entry_fields(
    entries: Option<&serde_json::Value>,
    label: &str,
    fields: &[&str],
) -> Result<()> {
    let entries = entries
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("snapshot {label}s must be an object"))?;
    for (key, entry) in entries {
        let entry = entry
            .as_object()
            .ok_or_else(|| anyhow!("snapshot {label} {key} must be an object"))?;
        require_object_fields(entry, &format!("{label} {key}"), fields)?;
    }
    Ok(())
}

fn require_array_entry_fields(
    entries: Option<&serde_json::Value>,
    label: &str,
    fields: &[&str],
) -> Result<()> {
    let entries = entries
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("snapshot {label}s must be an array"))?;
    for (index, entry) in entries.iter().enumerate() {
        let entry = entry
            .as_object()
            .ok_or_else(|| anyhow!("snapshot {label} {index} must be an object"))?;
        require_object_fields(entry, &format!("{label} {index}"), fields)?;
    }
    Ok(())
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
    use super::{
        format_migration_path, SnapshotStore, FORMAT_MIGRATIONS, FORMAT_VERSION,
        LEGACY_FORMAT_VERSION,
    };
    use crate::demo;
    use crate::model::{QuestDefinition, WorldState};
    use crate::runtime;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn registered_format_migrations_are_contiguous_to_current() {
        assert_eq!(
            format_migration_path(LEGACY_FORMAT_VERSION, FORMAT_VERSION).unwrap(),
            FORMAT_MIGRATIONS
        );
        assert!(format_migration_path(FORMAT_VERSION, FORMAT_VERSION)
            .unwrap()
            .is_empty());
        let error = format_migration_path(FORMAT_VERSION + 1, FORMAT_VERSION)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no migration from version"));
    }

    #[test]
    fn new_store_does_not_require_snapshot_rewrite() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let loaded = store.load_with_status().unwrap();
        assert!(!loaded.needs_rewrite);
        assert_eq!(loaded.source_format_version, None);
        assert!(loaded.format_migrations.is_empty());
        assert_eq!(loaded.world.root(), WorldState::genesis().root());
        assert!(!store.path().exists());
    }

    #[test]
    fn saves_and_loads_state() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 7;
        store.save(&state).unwrap();
        let loaded = store.load_with_status().unwrap();
        assert!(!loaded.needs_rewrite);
        assert_eq!(loaded.source_format_version, Some(FORMAT_VERSION));
        assert!(loaded.format_migrations.is_empty());
        assert_eq!(loaded.world.head, 7);
        assert_eq!(loaded.world.root(), state.root());
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(saved["format_version"], json!(FORMAT_VERSION));
        assert!(!store.path().with_extension("json.tmp").exists());
        assert!(!store.path().with_extension("json.bak.tmp").exists());
    }

    #[test]
    fn rewrites_previous_current_snapshot_as_current_version() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 3;
        fs::write(
            store.path(),
            serde_json::to_vec(&json!({
                "format_version": LEGACY_FORMAT_VERSION,
                "state_root": state.root(),
                "state": state
            }))
            .unwrap(),
        )
        .unwrap();

        let loaded = store.load_with_status().unwrap();
        assert!(loaded.needs_rewrite);
        assert_eq!(loaded.source_format_version, Some(LEGACY_FORMAT_VERSION));
        assert_eq!(loaded.format_migrations, FORMAT_MIGRATIONS);
        store.save(&loaded.world).unwrap();
        let rewritten: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(rewritten["format_version"], json!(FORMAT_VERSION));
        assert_eq!(rewritten["state_root"], json!(loaded.world.root()));
    }

    #[test]
    fn loads_previous_version_with_legacy_quest_defaults() {
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
        let state_root = state.root();
        let mut state_value = serde_json::to_value(state).unwrap();
        state_value["quests"]["legacy"]
            .as_object_mut()
            .unwrap()
            .remove("giver_entity_id");
        fs::write(
            store.path(),
            serde_json::to_vec(&json!({
                "format_version": LEGACY_FORMAT_VERSION,
                "state_root": state_root,
                "state": state_value
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(store.load().unwrap().quests["legacy"].giver_entity_id, 0);
    }

    #[test]
    fn current_version_rejects_missing_defaulted_state_fields() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let state = WorldState::genesis();
        let state_root = state.root();
        let mut state_value = serde_json::to_value(state).unwrap();
        state_value.as_object_mut().unwrap().remove("inventories");
        fs::write(
            store.path(),
            serde_json::to_vec(&json!({
                "format_version": FORMAT_VERSION,
                "state_root": state_root,
                "state": state_value
            }))
            .unwrap(),
        )
        .unwrap();

        let error = format!("{:#}", store.load().unwrap_err());
        assert!(error.contains("state is missing field: inventories"));
    }

    #[test]
    fn current_version_rejects_unknown_nested_fields() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        let state_root = state.root();
        let mut state_value = serde_json::to_value(state).unwrap();
        state_value["entities"]["4"]["ignored_admin_token"] = json!("must-not-survive");
        fs::write(
            store.path(),
            serde_json::to_vec(&json!({
                "format_version": FORMAT_VERSION,
                "state_root": state_root,
                "state": state_value
            }))
            .unwrap(),
        )
        .unwrap();

        let error = format!("{:#}", store.load().unwrap_err());
        assert!(error.contains("entity 4 contains unknown field: ignored_admin_token"));
    }

    #[test]
    fn current_version_rejects_missing_defaulted_quest_fields() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        let state_root = state.root();
        let mut state_value = serde_json::to_value(state).unwrap();
        state_value["quests"]["lost_key"]
            .as_object_mut()
            .unwrap()
            .remove("prerequisite_quest_ids");
        fs::write(
            store.path(),
            serde_json::to_vec(&json!({
                "format_version": FORMAT_VERSION,
                "state_root": state_root,
                "state": state_value
            }))
            .unwrap(),
        )
        .unwrap();

        let error = format!("{:#}", store.load().unwrap_err());
        assert!(error.contains("quest lost_key is missing field: prerequisite_quest_ids"));
    }

    #[test]
    fn current_version_rejects_legacy_actor_inventory_marker() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        demo::initialize(&mut state).unwrap();
        state
            .entities
            .get_mut(&4)
            .unwrap()
            .data
            .insert("inventory".into(), json!([]));
        let value = json!({
            "format_version": FORMAT_VERSION,
            "state_root": state.root(),
            "state": state
        });
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();

        let error = format!("{:#}", store.load().unwrap_err());
        assert!(error.contains("contains legacy actor inventory data"));
    }

    #[test]
    fn rejects_unknown_future_snapshot_version_before_loading_state() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        fs::write(
            store.path(),
            serde_json::to_vec(&json!({
                "format_version": FORMAT_VERSION + 1,
                "state_root": "ignored",
                "state": WorldState::genesis()
            }))
            .unwrap(),
        )
        .unwrap();

        let error = format!("{:#}", store.load().unwrap_err());
        assert!(error.contains("unsupported snapshot format version"));
    }

    #[test]
    fn legacy_actor_inventory_is_not_saved_as_current_schema() {
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

        let error = format!("{:#}", store.save(&state).unwrap_err());
        assert!(error.contains("contains legacy actor inventory data"));
        assert!(!store.path().exists());
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
        let loaded = store.load_with_status().unwrap();
        assert!(loaded.needs_rewrite);
        assert_eq!(loaded.source_format_version, None);
        assert!(loaded.format_migrations.is_empty());
        assert_eq!(loaded.world.head, 3);
    }

    #[test]
    fn legacy_backup_recovery_still_requires_rewrite() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let mut state = WorldState::genesis();
        state.head = 9;
        fs::write(
            store.backup_path(),
            serde_json::to_vec(&json!({
                "format_version": LEGACY_FORMAT_VERSION,
                "state_root": state.root(),
                "state": state
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(store.path(), b"corrupted").unwrap();

        let loaded = store.load_with_status().unwrap();
        assert!(loaded.needs_rewrite);
        assert_eq!(loaded.source_format_version, Some(LEGACY_FORMAT_VERSION));
        assert_eq!(loaded.format_migrations, FORMAT_MIGRATIONS);
        assert_eq!(loaded.world.head, 9);
        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(restored["format_version"], json!(LEGACY_FORMAT_VERSION));
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
