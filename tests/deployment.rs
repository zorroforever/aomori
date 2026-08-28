use std::collections::BTreeMap;

const SERVICE: &str = include_str!("../deploy/systemd/aomori.service");
const ENVIRONMENT: &str = include_str!("../deploy/systemd/aomori.env.example");
const DOCKERFILE: &str = include_str!("../Dockerfile");
const COMPOSE: &str = include_str!("../compose.yaml");
const DOCKER_SMOKE: &str = include_str!("../scripts/docker-smoke.sh");

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
