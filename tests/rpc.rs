use aomori::demo;
use aomori::model::{Transaction, WorldState};
use aomori::rpc::{self, AppState};
use aomori::storage::SnapshotStore;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::fs;
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::connect_async;
use tower::ServiceExt;

fn app_with_options(
    admin_token: Option<&str>,
    allow_unsigned_commands: bool,
) -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = SnapshotStore::new(dir.path()).unwrap();
    let mut world = WorldState::genesis();
    demo::initialize(&mut world).unwrap();
    let (events, _) = broadcast::channel(32);
    let state = Arc::new(Mutex::new(AppState {
        world,
        store,
        events,
        lua_limits: aomori::runtime::LuaLimits::default(),
        admin_token: admin_token.map(str::to_string),
        allow_unsigned_commands,
        cors_origins: vec!["http://127.0.0.1:5173".into()],
        rate_limiter: rpc::RateLimiter::new(10_000),
        observability: rpc::Observability::default(),
    }));
    (rpc::router(state), dir)
}

fn app_with_admin_token(admin_token: Option<&str>) -> (axum::Router, TempDir) {
    app_with_options(admin_token, true)
}

fn app() -> (axum::Router, TempDir) {
    app_with_options(Some("test-admin-token"), true)
}

fn app_with_rate_limit(max_requests: u64) -> (axum::Router, TempDir) {
    let (app, dir) = app_with_options(Some("test-admin-token"), true);
    // Rebuild with a low limit through the same initialized world helper is unnecessary here;
    // this router-level test uses a dedicated state below.
    drop(app);
    let store = SnapshotStore::new(dir.path()).unwrap();
    let mut world = WorldState::genesis();
    demo::initialize(&mut world).unwrap();
    let (events, _) = broadcast::channel(32);
    let state = Arc::new(Mutex::new(AppState {
        world,
        store,
        events,
        lua_limits: aomori::runtime::LuaLimits::default(),
        admin_token: None,
        allow_unsigned_commands: true,
        cors_origins: vec!["http://127.0.0.1:5173".into()],
        rate_limiter: rpc::RateLimiter::new(max_requests),
        observability: rpc::Observability::default(),
    }));
    (rpc::router(state), dir)
}

async fn raw_body(app: &axum::Router, body: impl Into<Body>) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/rpc")
                .header("content-type", "application/json")
                .body(body.into())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)}));
    (status, value)
}

async fn get_json(app: &axum::Router, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn raw_request_with_token(app: &axum::Router, payload: Value, token: Option<&str>) -> Value {
    let mut request = Request::post("/rpc").header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(payload.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn raw_request(app: &axum::Router, payload: Value) -> Value {
    raw_request_with_token(app, payload, None).await
}

async fn request(app: &axum::Router, method: &str, params: Value) -> Value {
    raw_request(
        app,
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
    )
    .await
}

#[tokio::test]
async fn cors_allows_only_configured_browser_origin() {
    let (app, _dir) = app();
    let allowed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/rpc")
                .header("origin", "http://127.0.0.1:5173")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        allowed.headers()["access-control-allow-origin"],
        "http://127.0.0.1:5173"
    );

    let denied = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/rpc")
                .header("origin", "http://evil.example")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(denied
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn rpc_rate_limit_returns_429_and_recovers_next_window() {
    let (app, _dir) = app_with_rate_limit(2);
    assert!(request(&app, "aomori_get_info", json!({}))
        .await
        .get("result")
        .is_some());
    assert!(request(&app, "aomori_get_info", json!({}))
        .await
        .get("result")
        .is_some());

    let response = app
        .clone()
        .oneshot(
            Request::post("/rpc")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":3,"method":"aomori_get_info","params":{}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "1");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let limited: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(limited["error"]["code"], json!(-32004));
    assert!(limited["error"]["data"]["retry_after_ms"]
        .as_u64()
        .is_some());

    tokio::time::sleep(Duration::from_millis(1_010)).await;
    assert!(request(&app, "aomori_get_info", json!({}))
        .await
        .get("result")
        .is_some());
}

#[tokio::test]
async fn health_reports_loaded_world() {
    let (app, _dir) = app();
    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["ok"], json!(true));
    assert!(body["state_root"].as_str().is_some());
}

#[tokio::test]
async fn readiness_and_metrics_report_rpc_activity_without_unbounded_method_labels() {
    let (app, _dir) = app();
    let ready = app
        .clone()
        .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    let ready_body: Value =
        serde_json::from_slice(&ready.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(ready_body["ready"], json!(true));

    let successful = app
        .clone()
        .oneshot(
            Request::post("/rpc")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":"aomori_get_info","params":{}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(successful.headers()["x-request-id"], "1");

    let unknown = app
        .clone()
        .oneshot(
            Request::post("/rpc")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":2,"method":"attacker-random-label","params":{}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.headers()["x-request-id"], "2");

    let invalid = app
        .clone()
        .oneshot(
            Request::post("/rpc")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.headers()["x-request-id"], "3");

    let metrics = app
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&metrics.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["rpc"]["requests"], json!(3));
    assert_eq!(body["rpc"]["errors"], json!(2));
    assert_eq!(body["rpc"]["errors_by_code"]["-32601"], json!(1));
    assert_eq!(body["rpc"]["errors_by_code"]["-32700"], json!(1));
    assert_eq!(
        body["rpc"]["methods"]["aomori_get_info"]["requests"],
        json!(1)
    );
    assert_eq!(body["rpc"]["methods"]["<unknown>"]["requests"], json!(1));
    assert_eq!(body["rpc"]["methods"]["<invalid>"]["requests"], json!(1));
    assert!(body["rpc"]["methods"]
        .get("attacker-random-label")
        .is_none());
}

#[tokio::test]
async fn prometheus_metrics_include_rpc_and_snapshot_series() {
    let (app, _dir) = app();
    let command = request(
        &app,
        "aomori_command",
        json!({"entity_id":4,"action":"accept","args":{"npc_id":6}}),
    )
    .await;
    assert_eq!(command["result"]["ok"], json!(true));

    let json_metrics = get_json(&app, "/metrics").await;
    assert_eq!(json_metrics["snapshot"]["saves"], json!(1));
    assert_eq!(json_metrics["snapshot"]["failures"], json!(0));
    assert!(json_metrics["snapshot"]["duration_micros"]
        .as_u64()
        .is_some());

    let response = app
        .oneshot(
            Request::get("/metrics/prometheus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/plain; version=0.0.4"));
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("aomori_snapshot_saves_total 1"));
    assert!(body.contains("aomori_rpc_method_requests_total{method=\"aomori_command\"} 1"));
    assert!(body.contains("# TYPE aomori_websocket_active gauge"));
}

#[tokio::test]
async fn rpc_returns_parse_error_for_invalid_json() {
    let (app, _dir) = app();
    let (status, body) = raw_body(&app, Body::from(br#"{"jsonrpc":"2.0","id":1,"#.to_vec())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], json!(-32700));
    assert_eq!(body["error"]["message"], json!("parse error"));
    assert_eq!(body["id"], Value::Null);

    let info = request(&app, "aomori_get_info", json!({})).await;
    assert_eq!(info["result"]["protocol_version"], json!(1));
}

#[tokio::test]
async fn rpc_rejects_valid_json_with_invalid_request_shape() {
    let (app, _dir) = app();
    let (status, body) = raw_body(&app, Body::from("[]")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], json!(-32600));
    assert_eq!(body["id"], Value::Null);
}

#[tokio::test]
async fn rpc_rejects_bodies_larger_than_one_mebibyte() {
    let (app, _dir) = app();
    let oversized = vec![b' '; 1024 * 1024 + 1];
    let response = app
        .oneshot(
            Request::post("/rpc")
                .header("content-type", "application/json")
                .body(Body::from(oversized))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn rpc_rejects_invalid_json_rpc_envelopes() {
    let (app, _dir) = app();
    let missing_version = raw_request(&app, json!({"id":1,"method":"aomori_get_info"})).await;
    let wrong_version = raw_request(
        &app,
        json!({"jsonrpc":"1.0","id":1,"method":"aomori_get_info"}),
    )
    .await;
    let missing_method = raw_request(&app, json!({"jsonrpc":"2.0","id":1})).await;
    assert_eq!(missing_version["error"]["code"], json!(-32600));
    assert_eq!(wrong_version["error"]["code"], json!(-32600));
    assert_eq!(missing_method["error"]["code"], json!(-32600));
}

#[tokio::test]
async fn rpc_classifies_unknown_methods() {
    let (app, _dir) = app();
    let body = request(&app, "not_a_method", json!({})).await;
    assert_eq!(body["error"]["code"], json!(-32601));
}

#[tokio::test]
async fn admin_rpc_requires_configured_bearer_token() {
    let (app, _dir) = app();
    let payload = json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"aomori_create_account",
        "params":{"name":"builder"}
    });
    let disabled_app = app_with_admin_token(None).0;
    let disabled = raw_request_with_token(&disabled_app, payload.clone(), Some("any-token")).await;
    assert_eq!(disabled["error"]["code"], json!(-32002));

    let missing = raw_request_with_token(&app, payload.clone(), None).await;
    let incorrect = raw_request_with_token(&app, payload.clone(), Some("wrong-token")).await;
    assert_eq!(missing["error"]["code"], json!(-32002));
    assert_eq!(incorrect["error"]["code"], json!(-32002));
    assert_eq!(
        missing["error"]["message"],
        json!("admin authorization required")
    );

    let created = raw_request_with_token(&app, payload, Some("test-admin-token")).await;
    assert_eq!(created["result"]["name"], json!("builder"));
    let account = request(&app, "aomori_get_account", json!({"name":"builder"})).await;
    assert_eq!(account["result"]["name"], json!("builder"));
}

#[tokio::test]
async fn unsigned_writes_are_disabled_while_signed_transactions_work() {
    let (app, _dir) = app_with_options(Some("test-admin-token"), false);
    let direct = request(
        &app,
        "aomori_command",
        json!({"entity_id":4,"action":"accept","args":{"npc_id":6}}),
    )
    .await;
    assert_eq!(direct["error"]["code"], json!(-32002));

    let unsigned = request(
        &app,
        "aomori_submit_transaction",
        json!({
            "from":"admin","nonce":0,"entity_id":4,"action":"accept",
            "args":{"npc_id":6},"signature":null
        }),
    )
    .await;
    assert_eq!(unsigned["error"]["code"], json!(-32002));

    let signing_key = SigningKey::from_bytes(&[7; 32]);
    let public_key = hex::encode(signing_key.verifying_key().as_bytes());
    let create_account = json!({
        "jsonrpc":"2.0","id":1,"method":"aomori_create_account",
        "params":{"name":"signed-player","public_key":public_key}
    });
    raw_request_with_token(&app, create_account, Some("test-admin-token")).await;
    let create_entity = json!({
        "jsonrpc":"2.0","id":2,"method":"aomori_create_entity",
        "params":{
            "kind":"actor","owner":"signed-player","contract":"demo",
            "location":1,"data":{}
        }
    });
    let created = raw_request_with_token(&app, create_entity, Some("test-admin-token")).await;
    let mut tx = Transaction {
        from: "signed-player".into(),
        nonce: 0,
        entity_id: created["result"]["entity_id"].as_u64().unwrap(),
        action: "accept".into(),
        args: json!({"npc_id":6}),
        signature: None,
    };
    tx.signature = Some(hex::encode(
        signing_key
            .sign(&serde_json::to_vec(&tx).unwrap())
            .to_bytes(),
    ));
    let signed = request(
        &app,
        "aomori_submit_transaction",
        serde_json::to_value(tx).unwrap(),
    )
    .await;
    assert_eq!(signed["result"]["ok"], json!(true));
    assert_eq!(signed["result"]["from"], json!("signed-player"));
}

#[tokio::test]
async fn quest_rpc_returns_definition_and_actor_progress() {
    let (app, _dir) = app();
    let available = request(&app, "aomori_get_quests", json!({"actor_id":4})).await;
    assert_eq!(available["result"]["quests"].as_array().unwrap().len(), 3);
    assert_eq!(available["result"]["quests"][0]["id"], json!("lost_key"));
    assert_eq!(
        available["result"]["quests"][0]["giver_entity_id"],
        json!(6)
    );
    assert_eq!(available["result"]["quests"][1]["id"], json!("open_shrine"));
    assert_eq!(available["result"]["quests"][1]["status"], json!("locked"));
    assert_eq!(
        available["result"]["quests"][1]["prerequisite_quest_ids"],
        json!(["lost_key"])
    );
    assert_eq!(
        available["result"]["quests"][0]["status"],
        json!("available")
    );
    assert_eq!(
        available["result"]["quests"][0]["reward_balance"],
        json!(10)
    );

    request(
        &app,
        "aomori_command",
        json!({"entity_id":4,"action":"accept","args":{"npc_id":6}}),
    )
    .await;
    let accepted = request(&app, "aomori_get_quests", json!({"actor_id":4})).await;
    assert_eq!(accepted["result"]["quests"][0]["status"], json!("accepted"));
}

#[tokio::test]
async fn successful_command_exposes_system_events() {
    let (app, _dir) = app();
    let command = request(
        &app,
        "aomori_command",
        json!({"entity_id":4,"action":"accept","args":{"npc_id":6}}),
    )
    .await;
    assert_eq!(command["result"]["ok"], json!(true));

    let events = request(&app, "aomori_get_events", json!({"since":0})).await;
    let kinds: Vec<&str> = events["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"quest_accepted"));
    assert!(kinds.contains(&"quest_progress_changed"));
    assert!(kinds.contains(&"command_executed"));
}

#[tokio::test]
async fn snapshot_failure_rolls_back_transaction_state() {
    let (app, dir) = app();
    fs::create_dir(dir.path().join("state.json.tmp")).unwrap();
    let before = request(&app, "aomori_get_info", json!({})).await;

    let failed = request(
        &app,
        "aomori_submit_transaction",
        json!({
            "from":"admin",
            "nonce":0,
            "entity_id":4,
            "action":"accept",
            "args":{"npc_id":6},
            "signature":null
        }),
    )
    .await;
    assert!(failed.get("error").is_some());

    let after = request(&app, "aomori_get_info", json!({})).await;
    let account = request(&app, "aomori_get_account", json!({"name":"admin"})).await;
    assert_eq!(after["result"], before["result"]);
    assert_eq!(account["result"]["nonce"], json!(0));
    let metrics = get_json(&app, "/metrics").await;
    assert_eq!(metrics["snapshot"]["saves"], json!(1));
    assert_eq!(metrics["snapshot"]["failures"], json!(1));
}

#[tokio::test]
async fn websocket_only_pushes_events_for_successful_commands() {
    let (app, _dir) = app();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
    let (mut socket, _) = connect_async(format!("ws://{address}/events"))
        .await
        .unwrap();
    let connected_metrics = get_json(&app, "/metrics").await;
    assert_eq!(connected_metrics["websocket"]["active"], json!(1));
    assert_eq!(connected_metrics["websocket"]["connections"], json!(1));

    let accepted = request(
        &app,
        "aomori_command",
        json!({"entity_id":4,"action":"accept","args":{"npc_id":6}}),
    )
    .await;
    assert_eq!(accepted["result"]["ok"], json!(true));

    let mut kinds = Vec::new();
    while !kinds.iter().any(|kind| kind == "command_executed") {
        let message = timeout(Duration::from_secs(1), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        kinds.push(event["kind"].as_str().unwrap().to_string());
    }
    assert!(kinds.iter().any(|kind| kind == "quest_accepted"));
    assert!(kinds.iter().any(|kind| kind == "quest_progress_changed"));

    let failed = request(
        &app,
        "aomori_command",
        json!({"entity_id":4,"action":"complete","args":{}}),
    )
    .await;
    assert!(failed.get("error").is_some());
    assert!(timeout(Duration::from_millis(100), socket.next())
        .await
        .is_err());
    socket.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let disconnected_metrics = get_json(&app, "/metrics").await;
    assert_eq!(disconnected_metrics["websocket"]["active"], json!(0));
    assert_eq!(disconnected_metrics["websocket"]["connections"], json!(1));
    server.abort();
}

#[tokio::test]
async fn failed_command_does_not_expose_events() {
    let (app, _dir) = app();
    let before = request(&app, "aomori_get_info", json!({})).await;
    let failed = request(
        &app,
        "aomori_command",
        json!({"entity_id":4,"action":"complete","args":{}}),
    )
    .await;
    assert!(failed.get("error").is_some());
    let after = request(&app, "aomori_get_info", json!({})).await;
    assert_eq!(after["result"]["events"], before["result"]["events"]);
    assert_eq!(
        after["result"]["state_root"],
        before["result"]["state_root"]
    );
}
