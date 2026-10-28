use super::Result;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{Instant, timeout_at},
};
#[cfg(test)]
mod tests;

const OUTPUT: usize = 1024 * 1024;
pub(super) struct Runner {
    deadline: Instant,
    remaining: usize,
    calls: usize,
}
pub(super) struct Output {
    pub text: String,
    pub warnings: bool,
}
fn command(executable: &str, args: &[&str]) -> Command {
    let mut command = Command::new(executable);
    command
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if executable == "docker" && args.first() == Some(&"--host") {
        // Docker CLI connection precedence must not override the endpoint
        // already checked by native collection (Docker CLI reference, Options).
        for name in [
            "DOCKER_CONTEXT",
            "DOCKER_HOST",
            "DOCKER_TLS",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
        ] {
            command.env_remove(name);
        }
    }
    command
}
impl Runner {
    pub fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(90),
            remaining: 16 * OUTPUT,
            calls: 0,
        }
    }
    pub async fn run(&mut self, executable: &str, args: &[&str]) -> Result<Output> {
        if self.calls == 128 || self.remaining == 0 || Instant::now() >= self.deadline {
            return Err("Collection execution budget exhausted".into());
        }
        self.calls += 1;
        if self.remaining < 4 {
            return Err("Collection byte budget exhausted".into());
        }
        let limit = OUTPUT.min(self.remaining / 2 - 1);
        if limit == 0 {
            return Err("Collection byte budget exhausted".into());
        }
        // Reserve both pipe capacities before launching. Neither raw stream is logged.
        let reserved = 2 * (limit + 1);
        self.remaining -= reserved;
        let mut out = Vec::with_capacity(limit + 1);
        let mut err = Vec::with_capacity(limit + 1);
        let result: Result<()> = async {
            let mut child = command(executable, args)
                .spawn()
                .map_err(|e| format!("Command unavailable ({executable}): {}", e.kind()))?;
            let stdout = child.stdout.take().ok_or("Missing command stdout")?;
            let stderr = child.stderr.take().ok_or("Missing command stderr")?;
            let deadline = self.deadline.min(Instant::now() + Duration::from_secs(10));
            let work = async {
                let (_, _, status) = tokio::try_join!(
                    read(stdout, &mut out, limit),
                    read(stderr, &mut err, limit),
                    async {
                        child
                            .wait()
                            .await
                            .map_err(|_| "Command wait failed".to_owned())
                    }
                )?;
                if !status.success() {
                    let denied = err.windows(6).any(|v| v == b"denied")
                        || err.windows(13).any(|v| v == b"not permitted");
                    return Err(if denied {
                        "Permission denied"
                    } else {
                        "Command failed"
                    }
                    .into());
                }
                Ok(())
            };
            let result = match timeout_at(deadline, work).await {
                Ok(result) => result,
                Err(_) => Err("Command deadline exceeded".into()),
            };
            if result.is_err() && child.id().is_some() {
                child
                    .start_kill()
                    .map_err(|_| "Cannot terminate failed command")?;
                tokio::time::timeout(Duration::from_secs(1), child.wait())
                    .await
                    .map_err(|_| "Failed command termination not confirmed")?
                    .map_err(|_| "Failed command wait failed")?;
            }
            result
        }
        .await;
        // The buffers survive cancellation and errors, so only bytes actually
        // read consume the cumulative budget, including rejected output.
        self.remaining += reserved - out.len() - err.len();
        result?;
        Ok(Output {
            text: String::from_utf8(out).map_err(|_| "Command output is not UTF-8")?,
            warnings: !err.is_empty(),
        })
    }
}
async fn read(reader: impl AsyncRead + Unpin, bytes: &mut Vec<u8>, limit: usize) -> Result<()> {
    debug_assert!(bytes.capacity() > limit);
    let mut reader = reader.take((limit + 1) as u64);
    loop {
        if bytes.len() > limit {
            return Err("Command output byte limit exceeded".into());
        }
        // Never poll a full Vec: even the EOF probe must stay within the
        // reserved capacity. Every completed read survives future cancellation.
        if reader
            .read_buf(bytes)
            .await
            .map_err(|_| "Command pipe read failed")?
            == 0
        {
            return Ok(());
        }
    }
}
