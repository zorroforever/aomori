use anyhow::Result;
use axum::Router;
use clap::Parser;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8091")]
    listen: SocketAddr,
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,
    #[arg(long, default_value_t = false)]
    demo: bool,
    #[arg(long, default_value_t = aomori::runtime::DEFAULT_LUA_INSTRUCTION_LIMIT)]
    lua_instruction_limit: u64,
    #[arg(long, default_value_t = aomori::runtime::DEFAULT_LUA_MEMORY_LIMIT)]
    lua_memory_limit: usize,
    #[arg(long, env = "AOMORI_ADMIN_TOKEN", hide_env_values = true)]
    admin_token: Option<String>,
    #[arg(long, default_value_t = false)]
    allow_unsigned_commands: bool,
    #[arg(
        long,
        env = "AOMORI_CORS_ORIGINS",
        value_delimiter = ',',
        default_value = "http://127.0.0.1:5173,http://localhost:5173"
    )]
    cors_origins: Vec<String>,
    #[arg(long, env = "AOMORI_RPC_RATE_LIMIT", default_value_t = 100)]
    rpc_rate_limit: u64,
    #[arg(
        long = "trusted-proxies",
        env = "AOMORI_TRUSTED_PROXIES",
        default_value = ""
    )]
    trusted_proxies: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.lua_instruction_limit == 0 || args.lua_memory_limit == 0 {
        anyhow::bail!("Lua instruction and memory limits must be greater than zero");
    }
    if args.admin_token.as_deref() == Some("") {
        anyhow::bail!("admin token must not be empty");
    }
    if args.rpc_rate_limit == 0 {
        anyhow::bail!("RPC rate limit must be greater than zero");
    }
    let trusted_proxies = args
        .trusted_proxies
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .trim()
                .parse::<IpAddr>()
                .map_err(|_| anyhow::anyhow!("invalid trusted proxy IP: {value}"))
        })
        .collect::<Result<Vec<_>>>()?;
    for origin in &args.cors_origins {
        let uri: axum::http::Uri = origin
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid CORS origin: {origin}"))?;
        if uri.scheme().is_none() || uri.authority().is_none() || uri.path() != "/" {
            anyhow::bail!("invalid CORS origin: {origin}");
        }
    }
    let store = aomori::storage::SnapshotStore::new(&args.data_dir)?;
    let world = load_world(&store, args.demo)?;
    eprintln!(
        "{}",
        serde_json::json!({
            "type":"state_loaded",
            "head":world.head,
            "state_root":world.root(),
            "snapshot":store.path().display().to_string()
        })
    );
    let (events, _) = tokio::sync::broadcast::channel(256);
    let state = Arc::new(Mutex::new(aomori::rpc::AppState {
        world,
        store,
        events,
        lua_limits: aomori::runtime::LuaLimits {
            instruction_limit: args.lua_instruction_limit,
            memory_limit: args.lua_memory_limit,
        },
        admin_token: args.admin_token,
        allow_unsigned_commands: args.allow_unsigned_commands,
        cors_origins: args.cors_origins.clone(),
        trusted_proxies: trusted_proxies.iter().copied().collect(),
        rate_limiter: aomori::rpc::RateLimiter::new(args.rpc_rate_limit),
        observability: aomori::rpc::Observability::default(),
    }));
    let app: Router = aomori::rpc::router(state);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    eprintln!(
        "{}",
        serde_json::json!({
            "type":"server_listening",
            "listen":args.listen.to_string(),
            "lua_instruction_limit":args.lua_instruction_limit,
            "lua_memory_limit":args.lua_memory_limit,
            "rpc_rate_limit_per_second":args.rpc_rate_limit,
            "trusted_proxy_count":trusted_proxies.len()
        })
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

fn load_world(
    store: &aomori::storage::SnapshotStore,
    demo: bool,
) -> Result<aomori::model::WorldState> {
    let loaded = store.load_with_status()?;
    let snapshot_migrated = loaded.needs_rewrite;
    let source_format_version = loaded.source_format_version;
    let mut migration_steps: Vec<String> = loaded
        .format_migrations
        .iter()
        .map(|migration| migration.name.into())
        .collect();
    if snapshot_migrated && source_format_version.is_none() {
        migration_steps.push("pre_versioned_snapshot_import".into());
    }
    let mut world = loaded.world;
    let inventory_migrated = aomori::migration::migrate_legacy_inventories(&mut world)?;
    if inventory_migrated {
        migration_steps.push("legacy_actor_inventory_to_inventories".into());
    }
    let mut needs_save = snapshot_migrated || inventory_migrated;
    if demo && aomori::demo::ensure_current(&mut world)? {
        needs_save = true;
        eprintln!(
            "{}",
            serde_json::json!({"type":"demo_initialized_or_upgraded"})
        );
    }
    world.validate()?;
    if needs_save {
        store.save(&world)?;
    }
    if snapshot_migrated {
        eprintln!(
            "{}",
            serde_json::json!({
                "type":"snapshot_migrated",
                "from_format_version":source_format_version,
                "to_format_version":aomori::storage::FORMAT_VERSION,
                "format_version":aomori::storage::FORMAT_VERSION,
                "steps":migration_steps
            })
        );
    }
    Ok(world)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    eprintln!("{}", serde_json::json!({"type":"shutdown_requested"}));
}

#[cfg(test)]
mod tests {
    use super::load_world;
    use aomori::model::{EntityKind, WorldState};
    use aomori::storage::{SnapshotStore, FORMAT_VERSION};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    fn legacy_demo_snapshot() -> Value {
        let mut state = WorldState::genesis();
        aomori::demo::initialize(&mut state).unwrap();
        state
            .entities
            .get_mut(&4)
            .unwrap()
            .data
            .insert("inventory".into(), json!([5]));
        let mut value = serde_json::to_value(state).unwrap();
        value["quests"]["lost_key"]
            .as_object_mut()
            .unwrap()
            .remove("giver_entity_id");
        value["quests"]["lost_key"]
            .as_object_mut()
            .unwrap()
            .remove("prerequisite_quest_ids");
        value
    }

    fn legacy_world_snapshot() -> Value {
        let mut state = WorldState::genesis();
        let zone = aomori::runtime::create_entity(
            &mut state,
            EntityKind::Zone,
            "admin".into(),
            None,
            None,
            BTreeMap::new(),
        )
        .unwrap();
        let actor = aomori::runtime::create_entity(
            &mut state,
            EntityKind::Actor,
            "admin".into(),
            None,
            Some(zone),
            BTreeMap::new(),
        )
        .unwrap();
        let item = aomori::runtime::create_entity(
            &mut state,
            EntityKind::Item,
            "admin".into(),
            None,
            Some(zone),
            BTreeMap::new(),
        )
        .unwrap();
        state
            .entities
            .get_mut(&actor)
            .unwrap()
            .data
            .insert("inventory".into(), json!([item]));
        serde_json::to_value(state).unwrap()
    }

    #[test]
    fn startup_migrates_inventory_without_demo_mode() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        fs::write(
            store.path(),
            serde_json::to_vec(&legacy_world_snapshot()).unwrap(),
        )
        .unwrap();

        let world = load_world(&store, false).unwrap();
        assert_eq!(world.inventories[&2], vec![3]);
        assert_eq!(world.entities[&3].location, Some(2));
        assert!(!world.entities[&2].data.contains_key("inventory"));
        world.validate().unwrap();
        let snapshot: Value = serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(snapshot["format_version"], json!(FORMAT_VERSION));
    }

    #[test]
    fn startup_migrates_legacy_demo_snapshot_and_rewrites_current_format() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        fs::write(
            store.path(),
            serde_json::to_vec(&legacy_demo_snapshot()).unwrap(),
        )
        .unwrap();

        let world = load_world(&store, true).unwrap();
        assert!(!world.entities[&4].data.contains_key("inventory"));
        assert_eq!(world.inventories[&4], vec![5]);
        assert_eq!(world.entities[&5].location, Some(4));
        assert_ne!(world.quests["lost_key"].giver_entity_id, 0);
        world.validate().unwrap();

        let rewritten: Value = serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(rewritten["format_version"], json!(FORMAT_VERSION));
        let loaded = store.load_with_status().unwrap();
        assert!(!loaded.needs_rewrite);
        assert_eq!(loaded.world.root(), world.root());
    }

    #[test]
    fn startup_does_not_rewrite_unmigrated_legacy_schema() {
        let dir = tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).unwrap();
        let bytes = serde_json::to_vec(&legacy_demo_snapshot()).unwrap();
        fs::write(store.path(), &bytes).unwrap();

        let error = format!("{:#}", load_world(&store, false).unwrap_err());
        assert!(
            error.contains("quest lost_key giver does not exist"),
            "{error}"
        );
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
    }
}
