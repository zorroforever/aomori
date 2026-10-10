use std::collections::BTreeMap;

const SERVICE: &str = include_str!("../deploy/systemd/aomori.service");
const ENVIRONMENT: &str = include_str!("../deploy/systemd/aomori.env.example");
const DOCKERFILE: &str = include_str!("../Dockerfile");
const COMPOSE: &str = include_str!("../compose.yaml");
const DOCKER_SMOKE: &str = include_str!("../scripts/docker-smoke.sh");
const RPC_SMOKE: &str = include_str!("../scripts/rpc-smoke.sh");
const CI: &str = include_str!("../.github/workflows/ci.yml");

#[test]
fn systemd_service_keeps_runtime_and_secret_boundaries() {
    let directives = service_directives(SERVICE);
    assert_eq!(directive(&directives, "User"), "aomori");
    assert_eq!(directive(&directives, "Group"), "aomori");
    assert_eq!(directive(&directives, "StateDirectory"), "aomori");
    assert_eq!(directive(&directives, "StateDirectoryMode"), "0700");
    assert_eq!(
        directive(&directives, "EnvironmentFile"),
        "/etc/aomori/aomori.env"
    );

    let command = directive(&directives, "ExecStart");
    assert!(command.starts_with("/usr/local/bin/aomori "));
    assert!(command.contains("--listen 127.0.0.1:8091"));
    assert!(command.contains("--data-dir /var/lib/aomori"));
    assert!(!command.contains("--admin-token"));
    assert!(!SERVICE.contains("AOMORI_ADMIN_TOKEN"));
    assert!(!SERVICE.contains("0.0.0.0"));

    for key in [
        "NoNewPrivileges",
        "PrivateDevices",
        "PrivateTmp",
        "ProtectSystem",
        "ProtectHome",
        "ProtectKernelModules",
        "ProtectKernelTunables",
        "RestrictNamespaces",
        "RestrictSUIDSGID",
    ] {
        let value = directive(&directives, key);
        assert!(matches!(value, "true" | "strict"), "{key}={value}");
    }
    assert_eq!(directive(&directives, "CapabilityBoundingSet"), "");
    assert_eq!(directive(&directives, "AmbientCapabilities"), "");
    assert_eq!(
        directive(&directives, "RestrictAddressFamilies"),
        "AF_UNIX AF_INET AF_INET6"
    );
    assert_eq!(directive(&directives, "TimeoutStopSec"), "20s");
    assert_eq!(directive(&directives, "UMask"), "0077");
}

#[test]
fn systemd_environment_is_an_explicit_non_secret_template() {
    assert!(ENVIRONMENT.contains("AOMORI_ADMIN_TOKEN=replace-with-a-long-random-token"));
    assert!(ENVIRONMENT.contains("AOMORI_CORS_ORIGINS=https://mud.example.com"));
    assert!(ENVIRONMENT.contains("AOMORI_TRUSTED_PROXIES="));
    assert!(!ENVIRONMENT.contains("AOMORI_PUBLISH_ADDRESS"));
    assert!(!ENVIRONMENT.contains("AOMORI_PORT"));
}

#[test]
fn docker_runtime_and_compose_keep_security_boundaries() {
    for required in [
        "USER 10001:10001",
        "WORKDIR /data",
        "VOLUME [\"/data\"]",
        "HEALTHCHECK",
        "http://127.0.0.1:8091/ready",
        "CMD [\"--listen\", \"0.0.0.0:8091\", \"--data-dir\", \"/data\", \"--demo\"]",
    ] {
        assert!(
            DOCKERFILE.contains(required),
            "missing Dockerfile contract: {required}"
        );
    }
    assert!(!DOCKERFILE.contains("AOMORI_ADMIN_TOKEN"));

    for required in [
        "${AOMORI_PUBLISH_ADDRESS:-127.0.0.1}:${AOMORI_PORT:-8091}:8091",
        "AOMORI_ADMIN_TOKEN: \"${AOMORI_ADMIN_TOKEN:?set AOMORI_ADMIN_TOKEN in .env}\"",
        "read_only: true",
        "- ALL",
        "- no-new-privileges:true",
        "- aomori-data:/data",
        "- /tmp:size=16m,mode=1777",
        "stop_grace_period: 20s",
    ] {
        assert!(
            COMPOSE.contains(required),
            "missing Compose contract: {required}"
        );
    }
    assert!(!COMPOSE.contains("replace-with-a-long-random-token"));
}

#[test]
fn docker_smoke_covers_runtime_identity_storage_and_restart() {
    for required in [
        "docker info",
        "up -d aomori",
        "/ready",
        "/health",
        "{{.Config.User}}",
        "{{.HostConfig.ReadonlyRootfs}}",
        "{{json .HostConfig.CapDrop}}",
        "test -f /data/state.json",
        "down\n",
        "up -d aomori",
    ] {
        assert!(
            DOCKER_SMOKE.contains(required),
            "missing smoke check: {required}"
        );
    }
}

#[test]
fn rpc_smoke_covers_runtime_auth_and_secret_boundaries() {
    for required in [
        "AOMORI_ADMIN_TOKEN",
        "/health",
        "/ready",
        "/metrics",
        "/metrics/prometheus",
        "admin token leaked through server logs",
        "aomori_create_account",
        "aomori_command",
        "incorrect-token",
        "error.code == -32002",
        "test -f \"$data_dir/state.json\"",
    ] {
        assert!(
            RPC_SMOKE.contains(required),
            "missing RPC smoke check: {required}"
        );
    }
    assert!(!RPC_SMOKE.contains("echo \"$token\""));
}

#[test]
fn ci_runs_quality_web_e2e_and_container_checks_with_minimal_permissions() {
    for required in [
        "permissions:\n  contents: read",
        "name: Rust quality",
        "cargo test --locked",
        "cargo clippy --locked --all-targets -- -D warnings",
        "name: Web build",
        "npm ci",
        "npm run build",
        "name: Web E2E",
        "playwright install --with-deps chromium",
        "actions/upload-artifact@v6",
        "actions/checkout@v6",
        "actions/setup-node@v6",
        "name: RPC smoke",
        "cargo build --locked",
        "./scripts/rpc-smoke.sh",
        "name: Docker smoke",
        "./scripts/docker-smoke.sh",
    ] {
        assert!(CI.contains(required), "missing CI contract: {required}");
    }
    assert!(!CI.contains("AOMORI_ADMIN_TOKEN"));
    assert!(!CI.contains("permissions: write-all"));
}

#[test]
fn proxy_and_monitoring_examples_keep_private_endpoints_private() {
    let proxy = include_str!("../deploy/Caddyfile");
    for required in [
        "reverse_proxy 127.0.0.1:8091",
        "header_up X-Forwarded-For {remote_host}",
        "@metrics path /metrics /metrics/*",
        "handle @metrics",
        "respond 404",
    ] {
        assert!(
            proxy.contains(required),
            "missing proxy boundary: {required}"
        );
    }
    let monitoring = include_str!("../deploy/monitoring/compose.yaml");
    for required in [
        "--web.listen-address=127.0.0.1:9090",
        "GF_SERVER_HTTP_ADDR: 127.0.0.1",
        "GF_AUTH_ANONYMOUS_ENABLED: \"false\"",
        "${GRAFANA_ADMIN_PASSWORD:?set a unique Grafana password}",
    ] {
        assert!(
            monitoring.contains(required),
            "missing monitoring boundary: {required}"
        );
    }
    let dashboard: serde_json::Value =
        serde_json::from_str(include_str!("../deploy/monitoring/dashboards/aomori.json")).unwrap();
    assert_eq!(dashboard["panels"].as_array().unwrap().len(), 8);
    assert!(
        include_str!("../deploy/monitoring/alerts.yml").contains("aomori_snapshot_failures_total")
    );
    assert!(CI.contains("name: Deployment configuration"));
}

#[test]
fn restart_smoke_covers_crash_and_offline_restore_without_secret_output() {
    for required in [
        "kill -KILL",
        "crash_status",
        "verify_state",
        "capture_state",
        "state.tar.gz",
        "post-backup-data",
        "aomori_get_receipt",
        ".result.nonce == 2",
    ] {
        assert!(
            RPC_SMOKE.contains(required),
            "missing recovery check: {required}"
        );
    }
    assert!(!RPC_SMOKE.contains("set -x"));
}

#[test]
fn monitoring_smoke_checks_runtime_provisioning_and_alert_firing() {
    let smoke = include_str!("../scripts/monitoring-smoke.sh");
    for required in [
        "mktemp -d",
        "down --volumes --remove-orphans",
        "chmod 600",
        "127.0.0.1:$prom_port",
        "api/dashboards/uid/aomori-node",
        "api/datasources/uid/aomori-prometheus/health",
        "AomoriNodeUnavailable",
        ".state == \"firing\"",
        "wait_notification firing",
        "wait_notification resolved",
        "prom/alertmanager:v0.28.1",
        "send_resolved: true",
    ] {
        assert!(
            smoke.contains(required),
            "missing monitoring check: {required}"
        );
    }
    assert!(!smoke.contains("set -x"));
    assert!(CI.contains("./scripts/monitoring-smoke.sh"));
}

#[test]
fn tls_smoke_verifies_certificates_and_metrics_isolation() {
    let smoke = include_str!("../scripts/tls-smoke.sh");
    for required in [
        "--cacert",
        "tls internal",
        "NODE_EXTRA_CA_CERTS",
        "wss://localhost",
        "/metrics /metrics/prometheus",
        "== 404",
        "docker rm -f",
    ] {
        assert!(smoke.contains(required), "missing TLS check: {required}");
    }
    assert!(CI.contains("./scripts/tls-smoke.sh"));
    assert!(CI.contains("set -o pipefail"));
    assert!(CI.contains("web/e2e-output.log"));
}

#[test]
fn disk_full_smoke_is_isolated_and_checks_transaction_rollback() {
    let smoke = include_str!("../scripts/disk-full-smoke.sh");
    for required in [
        "--network none",
        "--tmpfs /data:rw,size=8m",
        "--cap-drop ALL",
        "No space left on device",
        "sha256sum /data/state.json",
        "before_account",
        "before_events",
        "aomori_get_receipt",
        "docker rm -f",
    ] {
        assert!(
            smoke.contains(required),
            "missing disk drill boundary: {required}"
        );
    }
    assert!(CI.contains("./scripts/disk-full-smoke.sh"));
}

#[test]
fn runtime_soak_checks_concurrent_reads_and_restart_evidence() {
    let soak = include_str!("../scripts/runtime-soak.py");
    for required in [
        "TemporaryDirectory",
        "ThreadPoolExecutor",
        "aomori_submit_transaction",
        "aomori_get_receipt",
        "aomori_get_events",
        "VmRSS:",
        "restart_verified=True",
        "args.report.open('x')",
        "'status': 'failed'",
        "Event replay count mismatch",
        "RSS budget exceeded",
        "Snapshot budget exceeded",
        "resource_summary",
        "--restart-mode",
        "Expected SIGKILL exit -9",
        "Restart changed event history",
        "Restart changed checkpoint receipt",
    ] {
        assert!(soak.contains(required), "missing soak check: {required}");
    }
    assert!(CI.contains("scripts/runtime-soak.py --seconds 10"));
    assert!(CI.contains("name: runtime-soak"));
}

fn service_directives(contents: &str) -> BTreeMap<&str, &str> {
    contents
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with(['#', '[']))
        .map(|line| line.split_once('=').unwrap())
        .collect()
}

fn directive<'a>(directives: &'a BTreeMap<&str, &str>, key: &str) -> &'a str {
    directives
        .get(key)
        .copied()
        .unwrap_or_else(|| panic!("missing systemd directive: {key}"))
}
