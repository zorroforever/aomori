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
    let mut world = store.load()?;
    if args.demo && aomori::demo::ensure_current(&mut world)? {
        store.save(&world)?;
        eprintln!(
            "{}",
            serde_json::json!({"type":"demo_initialized_or_upgraded"})
        );
    }
    world.validate()?;
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
