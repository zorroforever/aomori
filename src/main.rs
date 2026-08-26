use anyhow::Result;
use axum::Router;
use clap::Parser;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8090")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let state = Arc::new(Mutex::new(aomori::model::WorldState::genesis()));
    let app: Router = aomori::rpc::router(state);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    println!("aomori single-node MVP listening on http://{}", args.listen);
    axum::serve(listener, app).await?;
    Ok(())
}
