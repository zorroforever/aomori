use crate::model::*;
use crate::runtime;
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub type SharedState = Arc<Mutex<WorldState>>;

#[derive(Deserialize)]
pub struct Request {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

pub fn router(state: SharedState) -> Router {
    Router::new().route("/rpc", post(handle)).with_state(state)
}

async fn handle(State(state): State<SharedState>, Json(req): Json<Request>) -> impl IntoResponse {
    let id = req.id.unwrap_or(Value::Null);
    let params = req.params.unwrap_or_else(|| json!({}));
    let result = dispatch(&state, &req.method, params);
    match result {
        Ok(value) => (
            StatusCode::OK,
            Json(json!({"jsonrpc":"2.0","id":id,"result":value})),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":error.to_string()}}),
            ),
        ),
    }
}

fn dispatch(state: &SharedState, method: &str, p: Value) -> anyhow::Result<Value> {
    match method {
        "aomori_get_info" => {
            let s = state.lock().unwrap();
            Ok(
                json!({"head":s.head,"state_root":s.root(),"accounts":s.accounts.len(),"entities":s.entities.len(),"contracts":s.contracts.len()}),
            )
        }
        "aomori_get_entity" => {
            let id = p
                .get("entity_id")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("entity_id required"))?;
            let s = state.lock().unwrap();
            Ok(serde_json::to_value(s.entities.get(&id))?)
        }
        "aomori_deploy" => {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("name required"))?
                .to_string();
            let source = p
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("source required"))?
                .to_string();
            let mut s = state.lock().unwrap();
            runtime::deploy(&mut s, name.clone(), source)?;
            Ok(json!({"name":name,"state_root":s.root()}))
        }
        "aomori_create_entity" => {
            let kind: EntityKind = serde_json::from_value(
                p.get("kind")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("kind required"))?,
            )?;
            let owner = p
                .get("owner")
                .and_then(Value::as_str)
                .unwrap_or("admin")
                .to_string();
            let contract = p
                .get("contract")
                .and_then(Value::as_str)
                .map(str::to_string);
            let location = p.get("location").and_then(Value::as_u64);
            let data: BTreeMap<String, Value> =
                serde_json::from_value(p.get("data").cloned().unwrap_or(json!({})))?;
            let mut s = state.lock().unwrap();
            let id = runtime::create_entity(&mut s, kind, owner, contract, location, data)?;
            Ok(json!({"entity_id":id,"state_root":s.root()}))
        }
        "aomori_query" | "aomori_command" => {
            let id = p
                .get("entity_id")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("entity_id required"))?;
            let action = p
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("action required"))?;
            let args = p.get("args").cloned().unwrap_or(json!({}));
            let mut s = state.lock().unwrap();
            let receipt = runtime::execute(&mut s, id, action, args, method.ends_with("command"))?;
            Ok(serde_json::to_value(receipt)?)
        }
        _ => Err(anyhow::anyhow!("unknown method: {method}")),
    }
}
