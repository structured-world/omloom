use super::*;

// A higher-priority context makes DOCKER_HOST irrelevant. Invalid or private
// unused host settings must not be read, validated or influence the chosen source.
#[test]
fn context_selection_does_not_read_an_overridden_host() {
    let selected = docker_selection(|name| {
        if name == "DOCKER_CONTEXT" {
            Ok(Some("selected".into()))
        } else {
            Err("Overridden host must not be read".into())
        }
    })
    .expect("context has precedence");
    assert!(matches!(selected, DockerSelection::Context(name) if name == "selected"));
}

struct Commands(std::collections::VecDeque<(String, String)>);
impl DockerCommands for Commands {
    async fn read(&mut self, args: &[&str]) -> Result<String> {
        let (expected, result) = self.0.pop_front().expect("expected Docker read");
        assert_eq!(args[0], "--host");
        assert_eq!(args[1], "npipe:////./pipe/docker_engine");
        assert_eq!(args[2], expected);
        Ok(result)
    }
}

// Recreating the daemon behind a stable local pipe during a batch cannot
// attribute its new containers to the previously inspected runtime identity.
#[tokio::test]
async fn changed_daemon_identity_invalidates_the_batch() {
    let mut commands = Commands(
        [
            (
                "info".into(),
                r#"{"id":"first","os":"Windows","type":"windows","version":"test"}"#.into(),
            ),
            ("ps".into(), String::new()),
            ("info".into(), r#""second""#.into()),
        ]
        .into(),
    );
    let mut report = api::HostReport {
        platform: "windows".into(),
        ..Default::default()
    };
    assert!(
        docker_at(&mut commands, "npipe:////./pipe/docker_engine", &mut report)
            .await
            .is_err()
    );
    assert!(commands.0.is_empty());
}

// Docker info can render empty server fields when the selected daemon pipe is
// absent. That is unavailable identity, never an adopted runtime or bad inventory.
#[tokio::test]
async fn absent_daemon_identity_is_reported_as_unavailable() {
    let mut commands = Commands(
        [(
            "info".into(),
            r#"{"id":"","os":"","type":"","version":""}"#.into(),
        )]
        .into(),
    );
    let mut report = api::HostReport {
        platform: "windows".into(),
        ..Default::default()
    };
    let error = docker_at(&mut commands, "npipe:////./pipe/docker_engine", &mut report)
        .await
        .expect_err("no daemon identity");
    assert_eq!(docker_status(&error), "unavailable");
    assert!(report.findings.is_empty());
    assert!(commands.0.is_empty());
}

// Named-pipe collection uses the same selected-field parser as Unix sockets.
// A Windows runtime/container/attachment keeps its real namespace and owned IP.
#[tokio::test]
async fn local_named_pipe_preserves_container_and_interface_context() {
    let id = "a".repeat(64);
    let endpoint = "b".repeat(64);
    let inspected = serde_json::json!({"id":id,"name":"/web","created":"2026-10-28T00:00:00Z","image":"sha256:test","project":null,"service":null,"working_directory":null,"networks":{"nat":{"EndpointID":endpoint,"IPAddress":"192.0.2.20","IPPrefixLen":24,"GlobalIPv6Address":"","GlobalIPv6PrefixLen":0}}});
    let mut commands = Commands(
        [
            (
                "info".into(),
                r#"{"id":"first","os":"Windows","type":"windows","version":"test"}"#.into(),
            ),
            ("ps".into(), id),
            ("inspect".into(), inspected.to_string()),
            ("info".into(), r#""first""#.into()),
        ]
        .into(),
    );
    let mut report = api::HostReport {
        platform: "windows".into(),
        ..Default::default()
    };
    docker_at(&mut commands, "npipe:////./pipe/docker_engine", &mut report)
        .await
        .expect("coherent Docker batch");
    assert!(commands.0.is_empty());
    assert_eq!(report.findings.len(), 4);
    assert_eq!(
        report.findings[2].parent.as_ref(),
        Some(&report.findings[1].key)
    );
    assert_eq!(
        report.findings[2].runtime.as_ref(),
        Some(&report.findings[0].key)
    );
    assert_eq!(
        report.findings[3].parent.as_ref(),
        Some(&report.findings[2].key)
    );
    assert_eq!(report.findings[3].addresses[0].address, "192.0.2.20");
    assert!(report.binds.is_empty());
}

// Named pipes can address another Windows machine. Only the local pipe syntax
// qualifies for host collection; a transport scheme alone cannot prove scope.
#[test]
fn remote_named_pipe_is_not_a_local_docker_daemon() {
    assert!(local_docker_endpoint("windows", "npipe://server/pipe/docker_engine").is_err());
    assert!(local_docker_endpoint("windows", "npipe:////server/pipe/docker_engine").is_err());
    assert!(local_docker_endpoint("windows", "npipe:////./pipe/docker_engine").is_ok());
    assert!(local_docker_endpoint("macos", "unix:///var/run/docker.sock").is_ok());
}

// Stage context helps diagnose unavailable engines without retaining stderr,
// local endpoint paths or credentials, and cannot turn denial into a failure.
#[test]
fn docker_stage_preserves_permission_and_availability_classification() {
    assert_eq!(
        docker_status(&docker_step(
            "daemon inspection",
            "Permission denied".into()
        )),
        "blocked"
    );
    assert_eq!(
        docker_status(&docker_step(
            "context selection",
            "Command unavailable (docker): NotFound".into()
        )),
        "unavailable"
    );
    assert_eq!(
        docker_status(&docker_step("daemon inspection", "Command failed".into())),
        "failed"
    );
}
