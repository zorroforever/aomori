use crate::model::*;
use crate::runtime;
use crate::storage::SnapshotStore;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State, WebSocketUpgrade},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tower_http::cors::{AllowOrigin, CorsLayer};

pub struct RateLimiter {
    max_requests: u64,
    window_started: Instant,
    requests: u64,
}

impl RateLimiter {
    pub fn new(max_requests: u64) -> Self {
        Self {
            max_requests,
            window_started: Instant::now(),
            requests: 0,
        }
    }

    fn check(&mut self) -> Result<(), u64> {
        let elapsed = self.window_started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.window_started = Instant::now();
            self.requests = 0;
        }
        if self.requests >= self.max_requests {
            return Err(1000_u64.saturating_sub(elapsed.as_millis() as u64));
        }
        self.requests += 1;
        Ok(())
    }
}

pub struct AppState {
    pub world: WorldState,
    pub store: SnapshotStore,
    pub events: broadcast::Sender<WorldEvent>,
    pub lua_limits: runtime::LuaLimits,
    pub admin_token: Option<String>,
    pub allow_unsigned_commands: bool,
    pub cors_origins: Vec<String>,
    pub rate_limiter: RateLimiter,
    pub observability: Observability,
}

#[derive(Default, Serialize)]
pub struct Observability {
    pub next_request_id: u64,
    pub rpc_requests: u64,
    pub rpc_errors: u64,
    pub rpc_duration_micros: u128,
    pub rpc_max_duration_micros: u128,
    pub errors_by_code: BTreeMap<i64, u64>,
    pub methods: BTreeMap<String, MethodMetrics>,
    pub snapshot_saves: u64,
    pub snapshot_failures: u64,
    pub snapshot_duration_micros: u128,
    pub snapshot_max_duration_micros: u128,
    pub websocket_active: u64,
    pub websocket_connections: u64,
    pub websocket_lag_incidents: u64,
    pub websocket_missed_events: u64,
}

#[derive(Default, Serialize)]
pub struct MethodMetrics {
    pub requests: u64,
    pub errors: u64,
    pub duration_micros: u128,
    pub max_duration_micros: u128,
}

pub type SharedState = Arc<Mutex<AppState>>;

#[derive(Deserialize)]
pub struct Request {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Option<Value>,
}

pub fn router(state: SharedState) -> Router {
    let origins = Arc::new(state.lock().unwrap().cors_origins.clone());
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            origin
                .to_str()
                .map(|origin| origins.iter().any(|allowed| allowed == origin))
                .unwrap_or(false)
        }))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .expose_headers([
            header::RETRY_AFTER,
            header::HeaderName::from_static("x-request-id"),
        ]);
    Router::new()
        .route("/rpc", post(handle))
        .route("/events", get(events))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics))
        .route("/metrics/prometheus", get(prometheus_metrics))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(cors)
        .with_state(state)
}

async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.lock().unwrap();
    Json(json!({
        "ok": true,
        "head": state.world.head,
        "state_root": state.world.root()
    }))
}

async fn ready(State(state): State<SharedState>) -> Response {
    let state = state.lock().unwrap();
    let result = state.world.validate().and_then(|_| {
        let data_dir = state
            .store
            .path()
            .parent()
            .ok_or_else(|| anyhow::anyhow!("snapshot path has no parent"))?;
        let metadata = std::fs::metadata(data_dir)?;
        if !metadata.is_dir() {
            anyhow::bail!("snapshot data path is not a directory");
        }
        Ok(())
    });
    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"ready":true,"head":state.world.head})),
        )
            .into_response(),
        Err(error) => {
            eprintln!(
                "{}",
                json!({"type":"readiness_failed","error":error.to_string()})
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"ready":false,"error":"state unavailable"})),
            )
                .into_response()
        }
    }
}

async fn metrics(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.lock().unwrap();
    let metrics = &state.observability;
    Json(json!({
        "head":state.world.head,
        "events":state.world.events.len(),
        "entities":state.world.entities.len(),
        "accounts":state.world.accounts.len(),
        "rpc":{
            "next_request_id":metrics.next_request_id,
            "requests":metrics.rpc_requests,
            "errors":metrics.rpc_errors,
            "duration_micros":metrics.rpc_duration_micros,
            "max_duration_micros":metrics.rpc_max_duration_micros,
            "errors_by_code":&metrics.errors_by_code,
            "methods":&metrics.methods
        },
        "snapshot":{
            "saves":metrics.snapshot_saves,
            "failures":metrics.snapshot_failures,
            "duration_micros":metrics.snapshot_duration_micros,
            "max_duration_micros":metrics.snapshot_max_duration_micros
        },
        "websocket":{
            "active":metrics.websocket_active,
            "connections":metrics.websocket_connections,
            "lag_incidents":metrics.websocket_lag_incidents,
            "missed_events":metrics.websocket_missed_events
        }
    }))
}

async fn prometheus_metrics(State(state): State<SharedState>) -> Response {
    let state = state.lock().unwrap();
    let metrics = &state.observability;
    let mut output = String::new();
    macro_rules! metric {
        ($help:literal, $kind:literal, $name:literal, $value:expr) => {{
            let _ = writeln!(output, concat!("# HELP ", $name, " ", $help));
            let _ = writeln!(output, concat!("# TYPE ", $name, " ", $kind));
            let _ = writeln!(output, concat!($name, " {}"), $value);
        }};
    }
    metric!(
        "Current world head.",
        "gauge",
        "aomori_world_head",
        state.world.head
    );
    metric!(
        "Current persisted event count.",
        "gauge",
        "aomori_world_events",
        state.world.events.len()
    );
    metric!(
        "Current entity count.",
        "gauge",
        "aomori_world_entities",
        state.world.entities.len()
    );
    metric!(
        "Current account count.",
        "gauge",
        "aomori_world_accounts",
        state.world.accounts.len()
    );
    metric!(
        "Total RPC requests.",
        "counter",
        "aomori_rpc_requests_total",
        metrics.rpc_requests
    );
    metric!(
        "Total RPC errors.",
        "counter",
        "aomori_rpc_errors_total",
        metrics.rpc_errors
    );
    metric!(
        "Cumulative RPC duration in microseconds.",
        "counter",
        "aomori_rpc_duration_micros_total",
        metrics.rpc_duration_micros
    );
    metric!(
        "Maximum observed RPC duration in microseconds.",
        "gauge",
        "aomori_rpc_duration_micros_max",
        metrics.rpc_max_duration_micros
    );
    metric!(
        "Total snapshot save attempts from RPC mutations.",
        "counter",
        "aomori_snapshot_saves_total",
        metrics.snapshot_saves
    );
    metric!(
        "Total failed snapshot saves from RPC mutations.",
        "counter",
        "aomori_snapshot_failures_total",
        metrics.snapshot_failures
    );
    metric!(
        "Cumulative snapshot save duration in microseconds.",
        "counter",
        "aomori_snapshot_duration_micros_total",
        metrics.snapshot_duration_micros
    );
    metric!(
        "Maximum observed snapshot save duration in microseconds.",
        "gauge",
        "aomori_snapshot_duration_micros_max",
        metrics.snapshot_max_duration_micros
    );
    metric!(
        "Current WebSocket event stream connections.",
        "gauge",
        "aomori_websocket_active",
        metrics.websocket_active
    );
    metric!(
        "Total WebSocket event stream connections.",
        "counter",
        "aomori_websocket_connections_total",
        metrics.websocket_connections
    );
    metric!(
        "Total WebSocket lag incidents.",
        "counter",
        "aomori_websocket_lag_incidents_total",
        metrics.websocket_lag_incidents
    );
    metric!(
        "Total events missed by lagged WebSocket receivers.",
        "counter",
        "aomori_websocket_missed_events_total",
        metrics.websocket_missed_events
    );
    let _ = writeln!(
        output,
        "# HELP aomori_rpc_method_requests_total RPC requests by bounded method label."
    );
    let _ = writeln!(output, "# TYPE aomori_rpc_method_requests_total counter");
    let _ = writeln!(
        output,
        "# HELP aomori_rpc_method_errors_total RPC errors by bounded method label."
    );
    let _ = writeln!(output, "# TYPE aomori_rpc_method_errors_total counter");
    let _ = writeln!(output, "# HELP aomori_rpc_method_duration_micros_total Cumulative RPC duration by bounded method label.");
    let _ = writeln!(
        output,
        "# TYPE aomori_rpc_method_duration_micros_total counter"
    );
    let _ = writeln!(output, "# HELP aomori_rpc_method_duration_micros_max Maximum RPC duration by bounded method label.");
    let _ = writeln!(output, "# TYPE aomori_rpc_method_duration_micros_max gauge");
    for (method, values) in &metrics.methods {
        let method = prometheus_label(method);
        let _ = writeln!(
            output,
            "aomori_rpc_method_requests_total{{method=\"{method}\"}} {}",
            values.requests
        );
        let _ = writeln!(
            output,
            "aomori_rpc_method_errors_total{{method=\"{method}\"}} {}",
            values.errors
        );
        let _ = writeln!(
            output,
            "aomori_rpc_method_duration_micros_total{{method=\"{method}\"}} {}",
            values.duration_micros
        );
        let _ = writeln!(
            output,
            "aomori_rpc_method_duration_micros_max{{method=\"{method}\"}} {}",
            values.max_duration_micros
        );
    }
    let _ = writeln!(
        output,
        "# HELP aomori_rpc_error_code_total RPC errors by JSON-RPC error code."
    );
    let _ = writeln!(output, "# TYPE aomori_rpc_error_code_total counter");
    for (code, count) in &metrics.errors_by_code {
        let _ = writeln!(
            output,
            "aomori_rpc_error_code_total{{code=\"{code}\"}} {count}"
        );
    }
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        output,
    )
        .into_response()
}

fn prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

async fn events(State(state): State<SharedState>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    let receiver = state.lock().unwrap().events.subscribe();
    upgrade.on_upgrade(move |socket| websocket_loop(socket, receiver, state))
}

async fn websocket_loop(
    mut socket: axum::extract::ws::WebSocket,
    mut receiver: broadcast::Receiver<WorldEvent>,
    state: SharedState,
) {
    {
        let mut state = state.lock().unwrap();
        state.observability.websocket_active += 1;
        state.observability.websocket_connections += 1;
    }
    let mut last_event_id = 0;
    loop {
        tokio::select! {
            client_message = socket.recv() => match client_message {
                Some(Ok(axum::extract::ws::Message::Close(_))) | Some(Err(_)) | None => break,
                _ => continue,
            },
            event = receiver.recv() => {
                let payload = match event {
                    Ok(event) => {
                        last_event_id = event.id;
                        serde_json::to_string(&event).ok()
                    }
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        let mut state = state.lock().unwrap();
                        state.observability.websocket_lag_incidents += 1;
                        state.observability.websocket_missed_events += missed;
                        Some(
                            json!({
                                "type":"event_stream_lagged",
                                "missed":missed,
                                "last_event_id":last_event_id
                            })
                            .to_string(),
                        )
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                };
                let Some(payload) = payload else { break };
                if socket
                    .send(axum::extract::ws::Message::Text(payload.into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    let mut state = state.lock().unwrap();
    state.observability.websocket_active = state.observability.websocket_active.saturating_sub(1);
}

#[cfg(test)]
fn event_stream_payload(
    result: Result<WorldEvent, broadcast::error::RecvError>,
    last_event_id: &mut u64,
) -> Option<String> {
    match result {
        Ok(event) => {
            *last_event_id = event.id;
            serde_json::to_string(&event).ok()
        }
        Err(broadcast::error::RecvError::Lagged(missed)) => Some(
            json!({
                "type":"event_stream_lagged",
                "missed":missed,
                "last_event_id":*last_event_id
            })
            .to_string(),
        ),
        Err(broadcast::error::RecvError::Closed) => None,
    }
}

async fn handle(State(state): State<SharedState>, headers: HeaderMap, body: Bytes) -> Response {
    let started = Instant::now();
    let request_id = {
        let mut state = state.lock().unwrap();
        state.observability.next_request_id += 1;
        state.observability.next_request_id
    };
    let (mut response, method, error_code) = process_rpc(&state, &headers, &body);
    let duration_micros = started.elapsed().as_micros();
    let status = response.status();
    response.headers_mut().insert(
        header::HeaderName::from_static("x-request-id"),
        HeaderValue::from_str(&request_id.to_string()).unwrap(),
    );
    record_rpc_metrics(&state, &method, error_code, duration_micros);
    eprintln!(
        "{}",
        json!({
            "type":"rpc_request",
            "request_id":request_id,
            "method":method,
            "http_status":status.as_u16(),
            "rpc_error_code":error_code,
            "duration_micros":duration_micros
        })
    );
    response
}

fn process_rpc(
    state: &SharedState,
    headers: &HeaderMap,
    body: &[u8],
) -> (Response, String, Option<i64>) {
    let rate_limit = state.lock().unwrap().rate_limiter.check();
    if let Err(retry_after_ms) = rate_limit {
        return (
            (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    (header::RETRY_AFTER, HeaderValue::from_static("1")),
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                ],
                Json(json!({
                    "jsonrpc":"2.0",
                    "id":Value::Null,
                    "error":{
                        "code":-32004,
                        "message":"rate limit exceeded",
                        "data":{"retry_after_ms":retry_after_ms}
                    }
                })),
            )
                .into_response(),
            "<rate_limited>".into(),
            Some(-32004),
        );
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return (
                rpc_response(Value::Null, Err((-32700, "parse error".to_string()))),
                "<invalid>".into(),
                Some(-32700),
            );
        }
    };
    let req: Request = match serde_json::from_value(value) {
        Ok(req) => req,
        Err(_) => {
            return (
                rpc_response(
                    Value::Null,
                    Err((-32600, "invalid JSON-RPC 2.0 request".to_string())),
                ),
                "<invalid>".into(),
                Some(-32600),
            );
        }
    };
    let id = req.id.unwrap_or(Value::Null);
    let method = match (req.jsonrpc.as_deref(), req.method.as_deref()) {
        (Some("2.0"), Some(method)) if !method.is_empty() => method,
        _ => {
            return (
                rpc_response(
                    id,
                    Err((-32600, "invalid JSON-RPC 2.0 request".to_string())),
                ),
                "<invalid>".into(),
                Some(-32600),
            );
        }
    };
    let metric_method = if is_known_method(method) {
        method.to_string()
    } else {
        "<unknown>".into()
    };
    if is_admin_method(method) {
        let supplied = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        let authorized = state
            .lock()
            .unwrap()
            .admin_token
            .as_deref()
            .zip(supplied)
            .map(|(expected, supplied)| expected == supplied)
            .unwrap_or(false);
        if !authorized {
            return (
                rpc_response(
                    id,
                    Err((-32002, "admin authorization required".to_string())),
                ),
                metric_method,
                Some(-32002),
            );
        }
    }
    let params = req.params.unwrap_or_else(|| json!({}));
    match dispatch(state, method, params) {
        Ok(value) => (rpc_response(id, Ok(value)), metric_method, None),
        Err(error) => {
            let code = error_code(&error);
            (
                rpc_response(id, Err((code, error.to_string()))),
                metric_method,
                Some(code),
            )
        }
    }
}

fn record_rpc_metrics(
    state: &SharedState,
    method: &str,
    error_code: Option<i64>,
    duration_micros: u128,
) {
    let mut state = state.lock().unwrap();
    let metrics = &mut state.observability;
    metrics.rpc_requests += 1;
    metrics.rpc_duration_micros += duration_micros;
    metrics.rpc_max_duration_micros = metrics.rpc_max_duration_micros.max(duration_micros);
    if let Some(code) = error_code {
        metrics.rpc_errors += 1;
        *metrics.errors_by_code.entry(code).or_default() += 1;
    }
    let method = metrics.methods.entry(method.to_string()).or_default();
    method.requests += 1;
    method.duration_micros += duration_micros;
    method.max_duration_micros = method.max_duration_micros.max(duration_micros);
    if error_code.is_some() {
        method.errors += 1;
    }
}

fn rpc_response(id: Value, result: Result<Value, (i64, String)>) -> Response {
    let body = match result {
        Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
        Err((code, message)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
        }
    };
    (StatusCode::OK, Json(body)).into_response()
}

fn is_known_method(method: &str) -> bool {
    matches!(
        method,
        "aomori_get_account"
            | "aomori_create_account"
            | "aomori_get_receipt"
            | "aomori_submit_transaction"
            | "aomori_list_entities"
            | "aomori_get_quests"
            | "aomori_get_events"
            | "aomori_get_info"
            | "aomori_get_entity"
            | "aomori_deploy"
            | "aomori_create_entity"
            | "aomori_query"
            | "aomori_command"
    )
}

fn is_admin_method(method: &str) -> bool {
    matches!(
        method,
        "aomori_create_account" | "aomori_deploy" | "aomori_create_entity"
    )
}

fn error_code(error: &anyhow::Error) -> i64 {
    let message = error.to_string();
    if message.starts_with("unknown method") {
        -32601
    } else if message.contains("required") || message.starts_with("invalid type") {
        -32602
    } else if message.starts_with("invalid nonce") {
        -32003
    } else if message.contains("owner")
        || message.starts_with("signature")
        || message.starts_with("invalid transaction signature")
        || message.starts_with("unsigned commands")
        || message.starts_with("unsigned transactions")
    {
        -32002
    } else {
        -32000
    }
}

fn mutate_and_save<T>(
    state: &mut AppState,
    mutate: impl FnOnce(&mut WorldState) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let original = state.world.clone();
    let value = match mutate(&mut state.world) {
        Ok(value) => value,
        Err(error) => {
            state.world = original;
            return Err(error);
        }
    };
    let save_started = Instant::now();
    let save_result = state.store.save(&state.world);
    let save_duration = save_started.elapsed().as_micros();
    state.observability.snapshot_saves += 1;
    state.observability.snapshot_duration_micros += save_duration;
    state.observability.snapshot_max_duration_micros = state
        .observability
        .snapshot_max_duration_micros
        .max(save_duration);
    if let Err(error) = save_result {
        state.observability.snapshot_failures += 1;
        state.world = original;
        return Err(error);
    }
    Ok(value)
}

fn publish_new_events(state: &AppState, previous_count: usize) {
    for event in state.world.events.iter().skip(previous_count) {
        let _ = state.events.send(event.clone());
    }
}

fn dispatch(state: &SharedState, method: &str, p: Value) -> anyhow::Result<Value> {
    let mut s = state.lock().unwrap();
    match method {
        "aomori_get_account" => {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("name required"))?;
            Ok(serde_json::to_value(s.world.accounts.get(name))?)
        }
        "aomori_create_account" => {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("name required"))?
                .to_string();
            let public_key = p
                .get("public_key")
                .and_then(Value::as_str)
                .map(str::to_string);
            let balance = p.get("balance").and_then(Value::as_u64).unwrap_or(0);
            mutate_and_save(&mut s, |world| {
                if world.accounts.contains_key(&name) {
                    return Err(anyhow::anyhow!("account already exists: {name}"));
                }
                world.accounts.insert(
                    name.clone(),
                    Account {
                        name: name.clone(),
                        public_key,
                        nonce: 0,
                        balance,
                    },
                );
                world.head += 1;
                Ok(())
            })?;
            Ok(json!({"name":name,"state_root":s.world.root()}))
        }
        "aomori_get_receipt" => {
            let tx_id = p
                .get("tx_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("tx_id required"))?;
            Ok(serde_json::to_value(s.world.receipts.get(tx_id))?)
        }
        "aomori_submit_transaction" => {
            let tx: Transaction = serde_json::from_value(p)?;
            if !s.allow_unsigned_commands
                && s.world
                    .accounts
                    .get(&tx.from)
                    .map(|account| account.public_key.is_none())
                    .unwrap_or(false)
            {
                return Err(anyhow::anyhow!("unsigned transactions are disabled"));
            }
            let previous_events = s.world.events.len();
            let limits = s.lua_limits;
            let receipt = mutate_and_save(&mut s, |world| {
                let receipt = runtime::execute_transaction_with_limits(world, tx, limits)?;
                world
                    .receipts
                    .insert(receipt.tx_id.clone(), receipt.clone());
                Ok(receipt)
            })?;
            publish_new_events(&s, previous_events);
            Ok(serde_json::to_value(receipt)?)
        }
        "aomori_list_entities" => {
            let location = p.get("location").and_then(Value::as_u64);
            let kind = p.get("kind").and_then(Value::as_str);
            let entities: Vec<&Entity> = s
                .world
                .entities
                .values()
                .filter(|entity| {
                    location
                        .map(|value| entity.location == Some(value))
                        .unwrap_or(true)
                })
                .filter(|entity| {
                    kind.map(|value| format!("{:?}", entity.kind).to_lowercase() == value)
                        .unwrap_or(true)
                })
                .collect();
            Ok(serde_json::to_value(entities)?)
        }
        "aomori_get_quests" => {
            let actor_id = p.get("actor_id").and_then(Value::as_u64);
            let quests: Vec<Value> = s
                .world
                .quests
                .values()
                .map(|quest| {
                    let status = actor_id
                        .and_then(|id| s.world.quest_progress.get(&format!("{id}:{}", quest.id)))
                        .map(|progress| match progress.status {
                            QuestStatus::Accepted => "accepted",
                            QuestStatus::Completed => "completed",
                        })
                        .unwrap_or_else(|| {
                            let unlocked = actor_id
                                .map(|id| {
                                    quest.prerequisite_quest_ids.iter().all(|prerequisite| {
                                        s.world
                                            .quest_progress
                                            .get(&format!("{id}:{prerequisite}"))
                                            .map(|progress| {
                                                progress.status == QuestStatus::Completed
                                            })
                                            .unwrap_or(false)
                                    })
                                })
                                .unwrap_or(true);
                            if unlocked {
                                "available"
                            } else {
                                "locked"
                            }
                        });
                    json!({
                        "id": quest.id,
                        "title": quest.title,
                        "giver_entity_id": quest.giver_entity_id,
                        "prerequisite_quest_ids": quest.prerequisite_quest_ids,
                        "required_item": quest.required_item,
                        "completion_zone": quest.completion_zone,
                        "reward_balance": quest.reward_balance,
                        "consume_required_item": quest.consume_required_item,
                        "status": status
                    })
                })
                .collect();
            Ok(json!({"quests": quests}))
        }
        "aomori_get_events" => {
            let since = p.get("since").and_then(Value::as_u64).unwrap_or(0);
            let limit = p
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(500) as usize;
            let events: Vec<&WorldEvent> = s
                .world
                .events
                .iter()
                .filter(|event| event.id > since)
                .take(limit)
                .collect();
            let next = events.last().map(|event| event.id).unwrap_or(since);
            Ok(json!({"events":events,"next":next}))
        }
        "aomori_get_info" => Ok(json!({
            "protocol_version":1,
            "head":s.world.head,
            "state_root":s.world.root(),
            "accounts":s.world.accounts.len(),
            "entities":s.world.entities.len(),
            "contracts":s.world.contracts.len(),
            "receipts":s.world.receipts.len(),
            "quests":s.world.quests.len(),
            "quest_progress":s.world.quest_progress.len(),
            "events":s.world.events.len(),
            "lua_instruction_limit":s.lua_limits.instruction_limit,
            "lua_memory_limit":s.lua_limits.memory_limit
        })),
        "aomori_get_entity" => {
            let id = p
                .get("entity_id")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("entity_id required"))?;
            Ok(serde_json::to_value(s.world.entities.get(&id))?)
        }
        "aomori_deploy" => {
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("name required"))?
                .to_string();
            let version = p.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
            let source = p
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("source required"))?
                .to_string();
            mutate_and_save(&mut s, |world| {
                runtime::deploy_version(world, name.clone(), version, source)
            })?;
            let contract = runtime::contract_key(&name, version);
            Ok(
                json!({"name":name,"version":version,"contract":contract,"state_root":s.world.root()}),
            )
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
            let id = mutate_and_save(&mut s, |world| {
                runtime::create_entity(world, kind, owner, contract, location, data)
            })?;
            Ok(json!({"entity_id":id,"state_root":s.world.root()}))
        }
        "aomori_query" | "aomori_command" => {
            let command = method.ends_with("command");
            if command && !s.allow_unsigned_commands {
                return Err(anyhow::anyhow!("unsigned commands are disabled"));
            }
            let id = p
                .get("entity_id")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("entity_id required"))?;
            let action = p
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("action required"))?;
            let args = p.get("args").cloned().unwrap_or(json!({}));
            let previous_events = s.world.events.len();
            let limits = s.lua_limits;
            let receipt = if command {
                mutate_and_save(&mut s, |world| {
                    runtime::execute_with_limits(world, id, action, args, true, limits)
                })?
            } else {
                runtime::execute_with_limits(&mut s.world, id, action, args, false, limits)?
            };
            if command {
                publish_new_events(&s, previous_events);
            }
            Ok(serde_json::to_value(receipt)?)
        }
        _ => Err(anyhow::anyhow!("unknown method: {method}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64) -> WorldEvent {
        WorldEvent {
            id,
            head: id,
            kind: "test".into(),
            entity_id: None,
            data: json!({}),
        }
    }

    #[tokio::test]
    async fn lagged_event_receiver_produces_recovery_control_message() {
        let (sender, mut receiver) = broadcast::channel(2);
        sender.send(event(1)).unwrap();
        sender.send(event(2)).unwrap();
        sender.send(event(3)).unwrap();
        let mut last_event_id = 0;

        let payload = event_stream_payload(receiver.recv().await, &mut last_event_id).unwrap();
        let control: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(control["type"], json!("event_stream_lagged"));
        assert_eq!(control["missed"], json!(1));
        assert_eq!(control["last_event_id"], json!(0));

        let payload = event_stream_payload(receiver.recv().await, &mut last_event_id).unwrap();
        let received: WorldEvent = serde_json::from_str(&payload).unwrap();
        assert_eq!(received.id, 2);
        assert_eq!(last_event_id, 2);
    }
}
