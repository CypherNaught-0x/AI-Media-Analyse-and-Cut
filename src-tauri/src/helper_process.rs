//! Out-of-process helpers that speak newline-delimited JSON.
//!
//! The CrisperWhisper Python runner and the Apple Speech helper share one
//! protocol: a single JSON request on stdin, then JSON lines on stdout:
//!
//! ```text
//! {"type": "progress", "message": "..."}
//! {"type": "result", ...}
//! {"type": "error", "message": "...", "detail": "...", "kind": "..."}
//! ```
//!
//! Anything else on stdout is logged and ignored, and stderr is drained
//! concurrently (a full pipe would deadlock the child) and kept for errors.

use anyhow::{anyhow, Context, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::run_control::RunControl;

/// Lines of stderr kept for the error message when no result arrives.
const STDERR_TAIL_LINES: usize = 40;

/// Run one request through a helper, forwarding `progress` lines and returning
/// the `result` object. `label` names the helper in errors and logs. With
/// `run`, the process belongs to that run: cancelling the run kills it.
pub(crate) async fn run_json_helper(
    label: &str,
    on_progress: &(dyn Fn(&str) + Sync),
    mut command: tokio::process::Command,
    request: serde_json::Value,
    run: Option<(u64, &RunControl)>,
) -> Result<serde_json::Value> {
    let program = command.as_std().get_program().to_string_lossy().to_string();
    command
        // A dropped run (e.g. the command future is abandoned) must not leave
        // a model running in the background.
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to start {label} ('{program}')"))?;
    // Captured now: `child.id()` is None once the child has been reaped.
    let pid = child.id();
    if let (Some((run_id, run_control)), Some(pid)) = (run, pid) {
        if let Err(error) = run_control.register_pid(run_id, pid) {
            let _ = child.kill().await;
            return Err(anyhow!(error));
        }
    }

    let payload = serde_json::to_vec(&request)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&payload).await?;
        stdin.shutdown().await?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Failed to capture {label} stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Failed to capture {label} stderr"))?;

    let log_label = label.to_string();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            log::debug!("{log_label}: {line}");
            tail.push(line);
            if tail.len() > STDERR_TAIL_LINES {
                tail.remove(0);
            }
        }
        tail
    });

    let mut result: Option<serde_json::Value> = None;
    let mut failure: Option<String> = None;
    let mut lines = BufReader::new(stdout).lines();

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(message) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            log::debug!("{label} (non-protocol stdout): {trimmed}");
            continue;
        };

        match message.get("type").and_then(|value| value.as_str()) {
            Some("progress") => {
                if let Some(text) = message.get("message").and_then(|value| value.as_str()) {
                    on_progress(text);
                }
            }
            Some("error") => {
                let text = message
                    .get("message")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{label} failed"));
                let detail = message.get("detail").and_then(|value| value.as_str());
                failure = Some(match detail {
                    Some(detail) => format!("{text} ({detail})"),
                    None => text,
                });
            }
            Some("result") => result = Some(message),
            _ => {}
        }
    }

    let status = child.wait().await?;
    let stderr_tail = stderr_task.await.unwrap_or_default();
    if let Some((run_id, run_control)) = run {
        if let Some(pid) = pid {
            run_control.clear_pid(run_id, pid);
        }
        run_control
            .ensure_active(run_id)
            .map_err(|error| anyhow!(error))?;
    }

    if let Some(failure) = failure {
        return Err(anyhow!(failure));
    }

    match result {
        Some(result) => Ok(result),
        None => {
            let tail = stderr_tail.join("\n");
            Err(anyhow!(
                "{label} produced no result (exit {}).{}",
                status.code().unwrap_or(-1),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(" Details: {tail}")
                }
            ))
        }
    }
}
