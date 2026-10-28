use super::Result;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{Instant, timeout_at},
};

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
        let limit = OUTPUT.min(self.remaining / 2);
        if limit == 0 {
            return Err("Collection byte budget exhausted".into());
        }
        // Reserve both pipe capacities before launching. Neither raw stream is logged.
        let out = Vec::with_capacity(limit + 1);
        let err = Vec::with_capacity(limit + 1);
        self.remaining -= 2 * limit;
        let mut child = Command::new(executable)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Command unavailable ({executable}): {}", e.kind()))?;
        let stdout = child.stdout.take().ok_or("Missing command stdout")?;
        let stderr = child.stderr.take().ok_or("Missing command stderr")?;
        let deadline = self.deadline.min(Instant::now() + Duration::from_secs(10));
        let work = async {
            let (out, err, status) =
                tokio::try_join!(read(stdout, out, limit), read(stderr, err, limit), async {
                    child
                        .wait()
                        .await
                        .map_err(|_| "Command wait failed".to_owned())
                })?;
            self.remaining += 2 * limit - out.len() - err.len();
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
            Ok(Output {
                text: String::from_utf8(out).map_err(|_| "Command output is not UTF-8")?,
                warnings: !err.is_empty(),
            })
        };
        timeout_at(deadline, work)
            .await
            .map_err(|_| "Command deadline exceeded".to_owned())?
    }
}
async fn read(reader: impl AsyncRead + Unpin, mut bytes: Vec<u8>, limit: usize) -> Result<Vec<u8>> {
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Command pipe read failed")?;
    if bytes.len() > limit {
        return Err("Command output byte limit exceeded".into());
    }
    Ok(bytes)
}
