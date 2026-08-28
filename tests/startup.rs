use aomori::model::WorldState;
use aomori::storage::{SnapshotStore, FORMAT_VERSION};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }

    fn stop(mut self) -> Output {
        let mut child = self.0.take().unwrap();
        let _ = child.kill();
        child.wait_with_output().unwrap()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn binary_startup_migrates_legacy_demo_and_becomes_ready() {
    let dir = tempdir().unwrap();
    let store = SnapshotStore::new(dir.path()).unwrap();
    let mut state = WorldState::genesis();
    aomori::demo::initialize(&mut state).unwrap();
    state
        .entities
        .get_mut(&4)
        .unwrap()
        .data
        .insert("inventory".into(), json!([5]));
    let mut legacy = serde_json::to_value(state).unwrap();
    legacy["quests"]["lost_key"]
        .as_object_mut()
        .unwrap()
        .remove("giver_entity_id");
    fs::write(store.path(), serde_json::to_vec(&legacy).unwrap()).unwrap();

    let address = available_address();
    let child = Command::new(env!("CARGO_BIN_EXE_aomori"))
        .args([
            "--listen",
            &address.to_string(),
            "--data-dir",
            dir.path().to_str().unwrap(),
            "--demo",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = ChildGuard(Some(child));

    let response = wait_until_ready(&mut child, address);
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    let (_, body) = response.split_once("\r\n\r\n").unwrap();
    let readiness: Value = serde_json::from_str(body).unwrap();
    assert_eq!(readiness["ready"], json!(true));

    let output = child.stop();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let logs: Vec<Value> = stderr
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let migration = logs
        .iter()
        .find(|entry| entry["type"] == "snapshot_migrated")
        .unwrap_or_else(|| panic!("missing snapshot migration log: {stderr}"));
    assert_eq!(
        migration,
        &json!({
            "type":"snapshot_migrated",
            "from_format_version":null,
            "to_format_version":FORMAT_VERSION,
            "format_version":FORMAT_VERSION,
            "steps":[
                "pre_versioned_snapshot_import",
                "legacy_actor_inventory_to_inventories"
            ]
        })
    );
    assert!(
        logs.iter().any(|entry| entry["type"] == "state_loaded"),
        "{stderr}"
    );

    let snapshot: Value = serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
    assert_eq!(snapshot["format_version"], json!(FORMAT_VERSION));
    let loaded = store.load_with_status().unwrap();
    assert!(!loaded.needs_rewrite);
    assert_eq!(loaded.world.inventories[&4], vec![5]);
    assert_eq!(loaded.world.entities[&5].location, Some(4));
}

fn available_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

fn wait_until_ready(child: &mut ChildGuard, address: SocketAddr) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.child_mut().try_wait().unwrap() {
            panic!("aomori exited before readiness with {status}");
        }
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            stream
                .write_all(b"GET /ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            return response;
        }
        assert!(Instant::now() < deadline, "aomori readiness timed out");
        thread::sleep(Duration::from_millis(25));
    }
}
