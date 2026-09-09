use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::agent::cli::output_filter;
use crate::config::{CliPromptVia, CliRunnerConfig, CliSecurityConfig};
use crate::error::{GatewayError, Result};
use crate::types::{AgentEvent, EventKind, EventStatus};

/// Per-session lock: only one CLI process at a time per session.
#[derive(Default)]
pub struct SessionLocks {
    locks: Mutex<HashMap<String, ArcLock>>,
}

type ArcLock = std::sync::Arc<tokio::sync::Mutex<()>>;

impl SessionLocks {
    pub fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }

    async fn lock_for(&self, session_id: &str) -> ArcLock {
        let mut guard = self.locks.lock().await;
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

pub fn session_workspace(security: &CliSecurityConfig, channel: &str, session_id: &str) -> PathBuf {
    let safe_session = sanitize_path_segment(session_id);
    let safe_channel = sanitize_path_segment(channel);
    PathBuf::from(&security.workspace_root)
        .join(safe_channel)
        .join(safe_session)
}

fn sanitize_path_segment(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "_".to_string()
    } else {
        out
    }
}

pub fn resolve_cwd(
    runner: &CliRunnerConfig,
    security: &CliSecurityConfig,
    channel: &str,
    session_id: &str,
) -> Result<PathBuf> {
    if !runner.cwd.is_empty() {
        return Ok(PathBuf::from(&runner.cwd));
    }
    Ok(session_workspace(security, channel, session_id))
}

pub async fn run_cli_agent(
    runner_name: &str,
    runner: &CliRunnerConfig,
    security: &CliSecurityConfig,
    channel: &str,
    session_id: &str,
    prompt: &str,
    session_locks: &SessionLocks,
) -> Result<Vec<AgentEvent>> {
    if !runner.enabled {
        return Err(GatewayError::Other(format!(
            "runner '{runner_name}' is disabled"
        )));
    }
    if runner.command.is_empty() {
        return Err(GatewayError::Other(format!(
            "runner '{runner_name}' has empty command"
        )));
    }

    let cwd = resolve_cwd(runner, security, channel, session_id)?;
    std::fs::create_dir_all(&cwd)?;
    tracing::info!(
        runner = %runner_name,
        command = %runner.command,
        args = ?runner.args,
        cwd = %cwd.display(),
        prompt_via = ?runner.prompt_via,
        prompt_len = prompt.len(),
        "cli: executing process"
    );

    let lock = session_locks.lock_for(session_id).await;
    let _guard = lock.lock().await;

    let max_bytes = security.max_output_kb.saturating_mul(1024);
    let timeout = std::time::Duration::from_secs(security.max_run_secs.max(1));

    let run_result = tokio::time::timeout(
        timeout,
        execute_process(runner_name, runner, &cwd, prompt, max_bytes),
    )
    .await;

    match run_result {
        Ok(Ok(events)) => Ok(events),
        Ok(Err(e)) => Err(e),
        Err(_) => Ok(timeout_events(runner_name, security.max_run_secs)),
    }
}

fn timeout_events(runner_name: &str, max_secs: u64) -> Vec<AgentEvent> {
    vec![
        AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::Failed,
            text: format!("CLI runner '{runner_name}' timed out after {max_secs}s"),
            ..Default::default()
        },
        AgentEvent::response_completed(),
    ]
}

async fn execute_process(
    runner_name: &str,
    runner: &CliRunnerConfig,
    cwd: &Path,
    prompt: &str,
    max_bytes: usize,
) -> Result<Vec<AgentEvent>> {
    let mut cmd = Command::new(&runner.command);
    cmd.args(&runner.args);
    cmd.current_dir(cwd);
    cmd.envs(&runner.env);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if runner.prompt_via == CliPromptVia::LastArg {
        cmd.arg(prompt);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| GatewayError::Other(format!("spawn {}: {e}", runner.command)))?;

    if runner.prompt_via == CliPromptVia::Stdin {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .await
                .map_err(|e| GatewayError::Other(format!("stdin write: {e}")))?;
            stdin
                .shutdown()
                .await
                .map_err(|e| GatewayError::Other(format!("stdin shutdown: {e}")))?;
        }
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| GatewayError::Other("stdout not captured".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| GatewayError::Other("stderr not captured".into()))?;

    let stdout_handle = tokio::spawn(read_lines(stdout));
    let stderr_handle = tokio::spawn(read_lines(stderr));

    let status = child
        .wait()
        .await
        .map_err(|e| GatewayError::Other(format!("wait: {e}")))?;

    let stdout_lines = stdout_handle
        .await
        .map_err(|e| GatewayError::Other(format!("stdout task: {e}")))??;
    let stderr_lines = stderr_handle
        .await
        .map_err(|e| GatewayError::Other(format!("stderr task: {e}")))??;

    let stdout_count = stdout_lines.len();
    let stderr_count = stderr_lines.len();
    let mut combined = String::new();
    let mut truncated = false;
    for line in stdout_lines {
        append_output(&mut combined, &mut truncated, &line, max_bytes);
    }
    for line in stderr_lines {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        append_output(&mut combined, &mut truncated, &line, max_bytes);
    }

    let mut final_text = combined.trim().to_string();
    if truncated {
        final_text.push_str("\n\n(output truncated)");
    }
    if final_text.is_empty() && !status.success() {
        final_text = format!("process exited with {}", status.code().unwrap_or(-1));
    }

    if tracing::enabled!(tracing::Level::DEBUG) {
        let preview: String = final_text.chars().take(4000).collect();
        tracing::debug!(
            runner = %runner_name,
            raw_len = final_text.len(),
            raw_preview = %preview,
            "cli: raw process output (not sent to IM)"
        );
    }
    let (im_text, _full) = output_filter::filter_for_im(&final_text, runner_name, runner);
    let im_text = if im_text.is_empty() {
        final_text.clone()
    } else {
        im_text
    };

    let mut events = Vec::new();
    tracing::info!(
        command = %runner.command,
        exit_success = status.success(),
        exit_code = ?status.code(),
        stdout_lines = stdout_count,
        stderr_lines = stderr_count,
        raw_output_len = final_text.len(),
        im_output_len = im_text.len(),
        truncated,
        "cli: process exited"
    );

    if !im_text.is_empty() {
        events.push(AgentEvent {
            kind: EventKind::Message,
            status: if status.success() {
                EventStatus::Completed
            } else {
                EventStatus::Failed
            },
            text: im_text,
            ..Default::default()
        });
    } else if !status.success() {
        events.push(AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::Failed,
            text: format!("process exited with {}", status.code().unwrap_or(-1)),
            ..Default::default()
        });
    }

    events.push(AgentEvent::response_completed());
    Ok(events)
}

async fn read_lines<R: tokio::io::AsyncRead + Unpin>(reader: R) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut buf = BufReader::new(reader);
    loop {
        let mut line = String::new();
        match buf.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                lines.push(line);
            }
            Err(e) => return Err(GatewayError::Other(format!("read output: {e}"))),
        }
    }
    Ok(lines)
}

fn append_output(buf: &mut String, truncated: &mut bool, line: &str, max_bytes: usize) {
    if *truncated {
        return;
    }
    let line_with_nl = format!("{line}\n");
    let new_len = buf.len() + line_with_nl.len();
    if new_len <= max_bytes {
        buf.push_str(&line_with_nl);
    } else {
        let remaining = max_bytes.saturating_sub(buf.len());
        if remaining > 0 {
            let take = line_with_nl.len().min(remaining);
            buf.push_str(&line_with_nl[..take]);
        }
        *truncated = true;
    }
}
