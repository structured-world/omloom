use super::*;

// A malicious source must be stopped after the first excess byte, without
// growing a buffer past the preallocated ceiling.
#[tokio::test]
async fn pipe_byte_limit_is_enforced_before_unbounded_growth() {
    assert!(read(&b"12345"[..], Vec::with_capacity(5), 4).await.is_err());
    assert_eq!(
        read(&b"1234"[..], Vec::with_capacity(5), 4)
            .await
            .expect("bounded output"),
        b"1234"
    );
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

// The same deadline applies during startup, pipe reads and process wait. The
// owned child must be terminated and reaped before returning the failure.
#[tokio::test]
async fn deadline_terminates_the_owned_child() {
    let mut runner = Runner::new();
    runner.deadline = Instant::now() + Duration::from_millis(50);
    #[cfg(windows)]
    let result = runner
        .run(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 5",
            ],
        )
        .await;
    #[cfg(not(windows))]
    let result = runner.run("/bin/sleep", &["5"]).await;
    assert_eq!(
        result.err().expect("deadline failure"),
        "Command deadline exceeded"
    );
}
