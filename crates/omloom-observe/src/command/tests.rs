use super::*;

// DOCKER_CONTEXT overrides DOCKER_HOST and conflicts with --host. A verified
// local endpoint cannot inherit a second connection selector or implicit TLS.
#[test]
fn pinned_docker_command_removes_inherited_connection_overrides() {
    let command = command("docker", &["--host", "unix:///test.sock", "info"]);
    let environment: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
    for key in [
        "DOCKER_CONTEXT",
        "DOCKER_HOST",
        "DOCKER_TLS",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
    ] {
        assert_eq!(
            environment.get(std::ffi::OsStr::new(key)),
            Some(&None),
            "{key}"
        );
    }
    assert_eq!(
        command.as_std().get_args().collect::<Vec<_>>(),
        ["--host", "unix:///test.sock", "info"]
    );
}

// A malicious source must be stopped after the first excess byte, without
// growing a buffer past the preallocated ceiling.
#[tokio::test]
async fn pipe_byte_limit_is_enforced_before_unbounded_growth() {
    let mut bytes = Vec::with_capacity(5);
    assert!(read(&b"12345"[..], &mut bytes, 4).await.is_err());
    assert_eq!(bytes, b"12345");
    bytes.clear();
    read(&b"1234"[..], &mut bytes, 4)
        .await
        .expect("bounded output");
    assert_eq!(bytes, b"1234");
}

// Exhaustion is checked before spawn. Even an unavailable executable must not
// be inspected once the source command budget is spent.
#[tokio::test]
async fn exhausted_call_budget_refuses_before_spawn() {
    let mut runner = Runner::new();
    runner.calls = 128;
    let error = runner
        .run("omloom-test-nonexistent-executable", &[])
        .await
        .err()
        .expect("refusal");
    assert_eq!(error, "Collection execution budget exhausted");
}

// Missing optional tools consume calls, but no output bytes. Repeated failures
// must leave the final installation/boot check its original byte budget.
#[tokio::test]
async fn unavailable_commands_do_not_consume_output_budget() {
    let mut runner = Runner::new();
    let initial = runner.remaining;
    for _ in 0..16 {
        let error = runner
            .run("omloom-test-nonexistent-executable", &[])
            .await
            .err()
            .expect("unavailable");
        assert!(error.starts_with("Command unavailable ("));
        assert_eq!(runner.remaining, initial);
    }
    assert_eq!(runner.calls, 16);
}

// A rejected stream still consumes the bytes actually read, while unused
// stderr capacity must be available to the next bounded command.
#[tokio::test]
async fn oversized_output_refunds_only_unused_pipe_capacity() {
    let mut runner = Runner::new();
    runner.remaining = 64;
    #[cfg(windows)]
    let result = runner
        .run(
            "cmd.exe",
            &[
                "/D",
                "/Q",
                "/C",
                "<nul set /p =12345678901234567890123456789012&exit /b 0",
            ],
        )
        .await;
    #[cfg(not(windows))]
    let result = runner
        .run(
            "/bin/sh",
            &["-c", "printf '12345678901234567890123456789012'"],
        )
        .await;
    assert_eq!(
        result.err().expect("output limit"),
        "Command output byte limit exceeded"
    );
    assert_eq!(runner.remaining, 32);
}

// The same deadline applies during startup, pipe reads and process wait. The
// owned child must be terminated and reaped before returning the failure.
#[tokio::test]
async fn deadline_terminates_the_owned_child() {
    let mut runner = Runner::new();
    let initial = runner.remaining;
    runner.deadline = Instant::now() + Duration::from_millis(50);
    #[cfg(windows)]
    let result = runner
        .run(
            "cmd.exe",
            &["/D", "/Q", "/C", "for /L %i in (1,1,2147483647) do @rem"],
        )
        .await;
    #[cfg(not(windows))]
    let result = runner.run("/bin/sleep", &["5"]).await;
    assert_eq!(
        result.err().expect("deadline failure"),
        "Command deadline exceeded"
    );
    assert_eq!(runner.remaining, initial);
}
