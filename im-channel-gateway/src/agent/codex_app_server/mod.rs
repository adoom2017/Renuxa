mod bindings;
mod client;
mod events;
mod router;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::agent::cli::{check_sender_allowed, error_events, help_events, request_text};
use crate::config::{AgentConfig, CodexAppServerConfig};
use crate::error::{GatewayError, Result};
use crate::gateway::{AgentBackend, AgentEventStream};
use crate::types::{AgentEvent, ChannelRequest};

use bindings::{BindingStore, ThreadBinding};
use client::{
    server_request_accept_result, server_request_answer_result, server_request_deny_result,
    server_request_question_ids, CodexAppServerClient, CodexThreadSummary, ServerRequest,
    ServerRequestHandler,
};
use router::{format_help, parse_command, ParsedCommand};

pub struct CodexAppServerBackend {
    cfg: CodexAppServerConfig,
    security: crate::config::CliSecurityConfig,
    client: CodexAppServerClient,
    bindings: Arc<BindingStore>,
    thread_list_cache: Arc<Mutex<HashMap<String, Vec<CodexThreadSummary>>>>,
    pending_server_requests: Arc<PendingServerRequests>,
}

#[derive(Default)]
struct PendingServerRequests {
    inner: Mutex<HashMap<String, PendingServerRequest>>,
}

struct PendingServerRequest {
    im_session_id: String,
    method: String,
    params: Value,
    responder: oneshot::Sender<Value>,
}

struct PendingRequestCleanup {
    pending: Arc<PendingServerRequests>,
    request_id: String,
}

enum PendingResponse {
    Approve,
    Deny,
    Answer(String),
}

impl Drop for PendingRequestCleanup {
    fn drop(&mut self) {
        self.pending.remove(&self.request_id);
    }
}

impl PendingServerRequests {
    fn insert(
        self: &Arc<Self>,
        request_id: String,
        request: PendingServerRequest,
    ) -> PendingRequestCleanup {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(request_id.clone(), request);
        }
        PendingRequestCleanup {
            pending: self.clone(),
            request_id,
        }
    }

    fn remove(&self, request_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(request_id);
        }
    }

    fn put_back(&self, request_id: String, request: PendingServerRequest) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(request_id, request);
        }
    }

    fn take_for_session(
        &self,
        im_session_id: &str,
        request_id: Option<&str>,
    ) -> Result<(String, PendingServerRequest)> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| GatewayError::Other("pending request lock poisoned".into()))?;
        if let Some(request_id) = request_id {
            let Some(request) = guard.remove(request_id) else {
                return Err(GatewayError::Other(format!(
                    "pending Codex request not found: {request_id}"
                )));
            };
            if request.im_session_id != im_session_id {
                guard.insert(request_id.to_string(), request);
                return Err(GatewayError::Other(format!(
                    "pending Codex request belongs to another IM session: {request_id}"
                )));
            }
            return Ok((request_id.to_string(), request));
        }

        let matching = guard
            .iter()
            .filter(|(_, request)| request.im_session_id == im_session_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        match matching.as_slice() {
            [] => Err(GatewayError::Other(
                "no pending Codex request for this IM session".into(),
            )),
            [id] => {
                let id = id.clone();
                let request = guard.remove(&id).expect("matching id exists");
                Ok((id, request))
            }
            _ => Err(GatewayError::Other(
                "multiple pending Codex requests; pass requestId".into(),
            )),
        }
    }
}

impl CodexAppServerBackend {
    pub fn from_config(agent: &AgentConfig, data_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            cfg: agent.codex_app_server.clone(),
            security: agent.security.clone(),
            client: CodexAppServerClient::new(agent.codex_app_server.clone())?,
            bindings: Arc::new(BindingStore::load(&data_dir)?),
            thread_list_cache: Arc::new(Mutex::new(HashMap::new())),
            pending_server_requests: Arc::new(PendingServerRequests::default()),
        })
    }

    fn cmd_thread(&self, im_session_id: &str) -> Vec<AgentEvent> {
        match self.bindings.get(im_session_id) {
            Some(b) => help_events(format!(
                "Bound Codex thread:\n  thread_id: {}\n  cwd: {}",
                b.thread_id,
                if b.cwd.is_empty() {
                    "(default)"
                } else {
                    &b.cwd
                }
            )),
            None => help_events(
                "No Codex thread bound for this IM session. Send /threads, then /bind <number>, or use /bind <threadId>."
                    .to_string(),
            ),
        }
    }

    fn cmd_session(&self, request: &ChannelRequest) -> Vec<AgentEvent> {
        help_events(format_session_info(
            &self.cfg.command_prefix,
            self.bindings.get(&request.session_id),
            request,
        ))
    }

    fn cmd_new(&self, im_session_id: &str) -> Vec<AgentEvent> {
        match self.bindings.clear(im_session_id) {
            Ok(()) => help_events(
                "Cleared binding. Next message will start a new Codex thread.".to_string(),
            ),
            Err(e) => error_events(e.to_string()),
        }
    }

    async fn cmd_threads(
        &self,
        im_session_id: &str,
        loaded_only: bool,
        limit: usize,
    ) -> Vec<AgentEvent> {
        let mut threads = match if loaded_only {
            self.client.thread_loaded_list(limit).await
        } else {
            self.client.thread_list(limit).await
        } {
            Ok(threads) => threads,
            Err(e) => return error_events(format!("thread/list failed: {e}")),
        };
        if !loaded_only {
            match self.client.thread_loaded_list(limit).await {
                Ok(loaded) => merge_loaded_threads(&mut threads, loaded, limit),
                Err(e) => tracing::debug!(
                    error = %e,
                    "codex app-server: thread/loaded/list failed while enriching thread list"
                ),
            }
        }
        if let Ok(mut guard) = self.thread_list_cache.lock() {
            guard.insert(im_session_id.to_string(), threads.clone());
        }
        help_events(format_thread_list(
            &self.cfg.command_prefix,
            &threads,
            loaded_only,
        ))
    }

    fn resolve_bind_selector(&self, im_session_id: &str, selector: &str) -> Result<ThreadBinding> {
        let trimmed = selector.trim();
        if let Ok(index) = trimmed.parse::<usize>() {
            if index == 0 {
                return Err(GatewayError::Other("bind number must start at 1".into()));
            }
            let guard = self
                .thread_list_cache
                .lock()
                .map_err(|_| GatewayError::Other("thread list cache lock poisoned".into()))?;
            let cached = guard.get(im_session_id).ok_or_else(|| {
                GatewayError::Other("no cached thread list; send /threads first".into())
            })?;
            let thread = cached.get(index - 1).ok_or_else(|| {
                GatewayError::Other(format!(
                    "thread number {index} is out of range; send /threads again"
                ))
            })?;
            return Ok(ThreadBinding {
                thread_id: thread.thread_id.clone(),
                cwd: thread.cwd.clone(),
            });
        }
        Ok(ThreadBinding {
            thread_id: trimmed.to_string(),
            cwd: self.cfg.default_cwd.clone(),
        })
    }

    async fn cmd_bind(&self, im_session_id: &str, selector: &str) -> Vec<AgentEvent> {
        let binding = match self.resolve_bind_selector(im_session_id, selector) {
            Ok(binding) => binding,
            Err(e) => return error_events(e.to_string()),
        };
        let thread_id = binding.thread_id.clone();
        if let Err(e) = self.client.thread_resume(&thread_id).await {
            return error_events(format!("thread/resume failed: {e}"));
        }
        match self.bindings.set(im_session_id, binding) {
            Ok(()) => help_events(format!("Bound to Codex thread `{thread_id}`.")),
            Err(e) => error_events(e.to_string()),
        }
    }

    async fn cmd_rename(&self, im_session_id: &str, name: &str) -> Vec<AgentEvent> {
        let Some(binding) = self.bindings.get(im_session_id) else {
            return error_events(
                "No Codex thread bound for this IM session. Send /threads, then /bind <number>, or send a prompt to start a new thread."
                    .to_string(),
            );
        };
        let name = name.trim();
        if name.is_empty() {
            return error_events("usage: /rename <name>".to_string());
        }
        match self.client.thread_set_name(&binding.thread_id, name).await {
            Ok(()) => {
                self.update_cached_thread_name(&binding.thread_id, name);
                help_events(format!(
                    "Renamed Codex thread `{}` to `{}`.",
                    binding.thread_id, name
                ))
            }
            Err(e) => error_events(format!("thread/name/set failed: {e}")),
        }
    }

    fn cmd_pending_response(
        &self,
        im_session_id: &str,
        request_id: Option<String>,
        response: PendingResponse,
    ) -> Vec<AgentEvent> {
        let (request_id, pending) = match self
            .pending_server_requests
            .take_for_session(im_session_id, request_id.as_deref())
        {
            Ok(pending) => pending,
            Err(e) => return error_events(e.to_string()),
        };
        let result = match response {
            PendingResponse::Approve => server_request_accept_result(&pending.method),
            PendingResponse::Deny => server_request_deny_result(&pending.method),
            PendingResponse::Answer(text) => {
                server_request_answer_result(&pending.method, &pending.params, &text)
            }
        };
        let Some(result) = result else {
            self.pending_server_requests
                .put_back(request_id.clone(), pending);
            return error_events(format!(
                "pending Codex request `{request_id}` does not accept this response"
            ));
        };
        if pending.responder.send(result).is_err() {
            return error_events(format!(
                "pending Codex request `{request_id}` is no longer waiting"
            ));
        }
        help_events(format!(
            "Submitted response for Codex request `{request_id}`; continuing turn."
        ))
    }

    fn update_cached_thread_name(&self, thread_id: &str, name: &str) {
        let Ok(mut guard) = self.thread_list_cache.lock() else {
            return;
        };
        for threads in guard.values_mut() {
            for thread in threads.iter_mut() {
                if thread.thread_id == thread_id {
                    thread.name = name.to_string();
                }
            }
        }
    }

    /// Streaming turn: events are pushed to `tx` as Codex app-server notifications arrive.
    async fn run_prompt_stream(
        &self,
        im_session_id: &str,
        prompt: &str,
        tx: mpsc::UnboundedSender<Result<AgentEvent>>,
    ) {
        tracing::info!(
            im_session = %im_session_id,
            prompt_len = prompt.len(),
            "codex app-server: resolving thread"
        );

        if let Some(binding) = self.bindings.get(im_session_id) {
            self.run_prompt_with_binding(im_session_id, binding, prompt, tx)
                .await;
            return;
        }

        let cwd = self.cfg.default_cwd.clone();
        if cwd.is_empty() {
            let error = GatewayError::Config(
                "no thread bound and codex_app_server.default_cwd is empty; set default_cwd or use /bind".into(),
            );
            tracing::error!(error = %error, "codex app-server: resolve thread failed");
            for ev in error_events(error.to_string()) {
                let _ = tx.send(Ok(ev));
            }
            return;
        }

        if let Err(e) = self
            .run_prompt_on_new_thread(im_session_id, cwd, prompt, &tx, "new IM binding")
            .await
        {
            tracing::error!(error = %e, "codex app-server turn failed on new thread");
            for ev in error_events(new_thread_error_message(&e)) {
                let _ = tx.send(Ok(ev));
            }
        }
    }

    async fn run_prompt_with_binding(
        &self,
        im_session_id: &str,
        binding: ThreadBinding,
        prompt: &str,
        tx: mpsc::UnboundedSender<Result<AgentEvent>>,
    ) {
        let thread_id = binding.thread_id.clone();
        tracing::info!(
            im_session = %im_session_id,
            thread_id = %thread_id,
            prompt_len = prompt.len(),
            "codex app-server: turn/start (streaming)"
        );

        let (result, event_count) = self.run_turn(im_session_id, &thread_id, prompt, &tx).await;

        match result {
            Ok(text) => tracing::info!(
                thread_id = %thread_id,
                event_count,
                reply_len = text.len(),
                "codex app-server: turn completed"
            ),
            Err(e) if is_missing_rollout_error(&e) => {
                tracing::warn!(
                    im_session = %im_session_id,
                    stale_thread_id = %thread_id,
                    error = %e,
                    "codex app-server thread binding is stale; recreating thread"
                );
                if let Err(clear_error) = self.bindings.clear(im_session_id) {
                    tracing::error!(
                        stale_thread_id = %thread_id,
                        error = %clear_error,
                        "codex app-server stale thread binding clear failed"
                    );
                    for ev in error_events(format!(
                        "Bound Codex thread is unavailable on app-server: {e}\nRecovery failed while clearing stale binding: {clear_error}"
                    )) {
                        let _ = tx.send(Ok(ev));
                    }
                    return;
                }
                let cwd = match replacement_cwd_for_stale_binding(&binding, &self.cfg.default_cwd) {
                    Some(cwd) => cwd,
                    None => {
                        let recovery_error = GatewayError::Config(
                            "bound Codex thread is unavailable on app-server and no cwd is available to start a replacement; set codex_app_server.default_cwd or use /bind <threadId>".into(),
                        );
                        tracing::error!(
                            stale_thread_id = %thread_id,
                            error = %recovery_error,
                            "codex app-server stale thread recovery failed"
                        );
                        for ev in error_events(format!(
                            "Bound Codex thread is unavailable on app-server: {e}\nRecovery failed: {recovery_error}"
                        )) {
                            let _ = tx.send(Ok(ev));
                        }
                        return;
                    }
                };
                if let Err(retry_error) = self
                    .run_prompt_on_new_thread(
                        im_session_id,
                        cwd,
                        prompt,
                        &tx,
                        "stale binding recovery",
                    )
                    .await
                {
                    tracing::error!(
                        stale_thread_id = %thread_id,
                        error = %retry_error,
                        "codex app-server turn failed after stale binding recovery"
                    );
                    for ev in error_events(stale_recovery_error_message(&retry_error)) {
                        let _ = tx.send(Ok(ev));
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "codex app-server turn failed");
                for ev in error_events(e.to_string()) {
                    let _ = tx.send(Ok(ev));
                }
            }
        }
    }

    async fn run_turn(
        &self,
        im_session_id: &str,
        thread_id: &str,
        prompt: &str,
        tx: &mpsc::UnboundedSender<Result<AgentEvent>>,
    ) -> (Result<String>, u32) {
        let mut event_count: u32 = 0;
        let mut forward_event = |ev| {
            event_count += 1;
            let _ = tx.send(Ok(ev));
        };
        let result = self
            .client
            .turn_start_stream_with_handler(
                thread_id,
                prompt,
                self.security.max_run_secs,
                &mut forward_event,
                Some(self.server_request_handler(im_session_id.to_string(), tx.clone())),
            )
            .await;
        (result, event_count)
    }

    fn server_request_handler(
        &self,
        im_session_id: String,
        tx: mpsc::UnboundedSender<Result<AgentEvent>>,
    ) -> ServerRequestHandler {
        let pending = self.pending_server_requests.clone();
        Arc::new(move |request: ServerRequest| {
            let pending = pending.clone();
            let tx = tx.clone();
            let im_session_id = im_session_id.clone();
            Box::pin(async move {
                let request_id = format!("req_{}", Uuid::new_v4().simple());
                let (responder, response) = oneshot::channel();
                let cleanup = pending.insert(
                    request_id.clone(),
                    PendingServerRequest {
                        im_session_id,
                        method: request.method.clone(),
                        params: request.params.clone(),
                        responder,
                    },
                );
                tx.send(Ok(AgentEvent::assistant_text(
                    format_server_request_prompt(&request_id, &request),
                )))
                .map_err(|_| GatewayError::Other("IM response channel closed".into()))?;
                let result = response.await.map_err(|_| {
                    GatewayError::Other(format!(
                        "pending Codex request `{request_id}` was cancelled"
                    ))
                })?;
                drop(cleanup);
                Ok(result)
            })
        })
    }

    async fn run_prompt_on_new_thread(
        &self,
        im_session_id: &str,
        cwd: String,
        prompt: &str,
        tx: &mpsc::UnboundedSender<Result<AgentEvent>>,
        reason: &str,
    ) -> Result<()> {
        tracing::info!(
            im_session = %im_session_id,
            cwd = %cwd,
            reason,
            "codex app-server: thread/start + turn/start on one connection"
        );
        let mut event_count: u32 = 0;
        let mut forward_event = |ev| {
            event_count += 1;
            let _ = tx.send(Ok(ev));
        };
        let (thread_id, text) = self
            .client
            .thread_start_turn_stream_with_handler(
                &cwd,
                prompt,
                self.security.max_run_secs,
                &mut forward_event,
                Some(self.server_request_handler(im_session_id.to_string(), tx.clone())),
            )
            .await?;
        let binding = ThreadBinding { thread_id, cwd };
        self.bindings.set(im_session_id, binding.clone())?;
        tracing::info!(
            im_session = %im_session_id,
            thread_id = %binding.thread_id,
            event_count,
            reply_len = text.len(),
            reason,
            "codex app-server: turn completed on new thread"
        );
        Ok(())
    }
}

fn is_missing_rollout_error(error: &GatewayError) -> bool {
    error.to_string().contains("no rollout found for thread id")
}

fn replacement_cwd_for_stale_binding(stale: &ThreadBinding, default_cwd: &str) -> Option<String> {
    if !stale.cwd.is_empty() {
        Some(stale.cwd.clone())
    } else if !default_cwd.is_empty() {
        Some(default_cwd.to_string())
    } else {
        None
    }
}

fn new_thread_error_message(error: &GatewayError) -> String {
    if is_missing_rollout_error(error) {
        format!(
            "Codex app-server created a thread but could not load its rollout. Restart codex app-server and try again. Details: {error}"
        )
    } else {
        error.to_string()
    }
}

fn stale_recovery_error_message(error: &GatewayError) -> String {
    if is_missing_rollout_error(error) {
        format!(
            "The bound Codex thread was stale, and the replacement thread also could not load its rollout. The stale binding was cleared; restart codex app-server and send the message again. Details: {error}"
        )
    } else {
        error.to_string()
    }
}

fn merge_loaded_threads(
    threads: &mut Vec<CodexThreadSummary>,
    loaded: Vec<CodexThreadSummary>,
    limit: usize,
) {
    let limit = limit.clamp(1, 20);
    for thread in loaded {
        if threads.len() >= limit {
            break;
        }
        if !threads
            .iter()
            .any(|item| item.thread_id == thread.thread_id)
        {
            threads.push(thread);
        }
    }
}

fn format_server_request_prompt(request_id: &str, request: &ServerRequest) -> String {
    let mut lines = vec![
        format!(
            "Codex is waiting for IM confirmation: {}",
            server_request_title(&request.method)
        ),
        format!("  request_id: {request_id}"),
        format!("  method: {}", request.method),
    ];
    for key in [
        "command",
        "cwd",
        "reason",
        "description",
        "toolName",
        "tool_name",
    ] {
        if let Some(value) = request_param_preview(&request.params, key) {
            lines.push(format!("  {key}: {value}"));
        }
    }
    let question_ids = server_request_question_ids(&request.params);
    if !question_ids.is_empty() {
        lines.push(format!("  question_ids: {}", question_ids.join(", ")));
    }
    lines.push(format!(
        "  params: {}",
        preview_text(
            &serde_json::to_string(&request.params).unwrap_or_default(),
            600
        )
    ));
    lines.push(String::new());
    if request.method == "item/tool/requestUserInput" {
        lines.push(format!(
            "Reply with `/answer {request_id} <answer>` to continue."
        ));
        lines.push(format!(
            "For multiple questions: `/answer {request_id} id1=value; id2=value`."
        ));
        lines.push(format!(
            "Use `/approve {request_id}` to submit empty/default answers, or `/deny {request_id}` to skip."
        ));
    } else {
        lines.push(format!(
            "Reply with `/approve {request_id}` to allow, or `/deny {request_id}` to reject."
        ));
    }
    lines.join("\n")
}

fn server_request_title(method: &str) -> &'static str {
    match method {
        "item/commandExecution/requestApproval" | "execCommandApproval" => "command execution",
        "item/fileChange/requestApproval" | "applyPatchApproval" => "file change",
        "item/permissions/requestApproval" => "permission request",
        "item/tool/requestUserInput" => "user input",
        _ => "server request",
    }
}

fn request_param_preview(params: &Value, key: &str) -> Option<String> {
    let value = params.get(key)?;
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(preview_text(text, 300))
    }
}

fn format_session_info(
    command_prefix: &str,
    binding: Option<ThreadBinding>,
    request: &ChannelRequest,
) -> String {
    let is_group = request
        .meta
        .get("is_group")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let group_id = request
        .meta
        .get("group_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let from_user_id = request
        .meta
        .get("from_user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut lines = vec![
        "IM session:".to_string(),
        format!("  channel: {}", request.channel),
        format!("  session_id: {}", request.session_id),
        format!("  user_id: {}", request.user_id),
        format!("  is_group: {}", is_group),
    ];
    if !group_id.is_empty() {
        lines.push(format!("  group_id: {group_id}"));
    }
    if !from_user_id.is_empty() && from_user_id != request.user_id {
        lines.push(format!("  from_user_id: {from_user_id}"));
    }

    lines.push("Codex binding:".to_string());
    match binding {
        Some(binding) => {
            lines.push(format!("  thread_id: {}", binding.thread_id));
            lines.push(format!(
                "  cwd: {}",
                if binding.cwd.is_empty() {
                    "(default)"
                } else {
                    &binding.cwd
                }
            ));
        }
        None => {
            lines.push("  thread_id: (none)".to_string());
            lines.push("  cwd: (none)".to_string());
        }
    }
    lines.push("Binding key:".to_string());
    lines.push(format!("  {}", request.session_id));
    lines.push("Bind command:".to_string());
    let prefix = if command_prefix.is_empty() {
        "/"
    } else {
        command_prefix
    };
    lines.push(format!("  {prefix}threads"));
    lines.push(format!("  {prefix}bind <threadId|number>"));

    lines.join("\n")
}

fn format_thread_list(
    command_prefix: &str,
    threads: &[CodexThreadSummary],
    loaded_only: bool,
) -> String {
    let prefix = if command_prefix.is_empty() {
        "/"
    } else {
        command_prefix
    };
    if threads.is_empty() {
        return if loaded_only {
            "No loaded Codex threads found on app-server.".to_string()
        } else {
            "No Codex threads found on app-server.".to_string()
        };
    }

    let mut lines = vec![if loaded_only {
        "Loaded Codex threads:".to_string()
    } else {
        "Recent Codex threads:".to_string()
    }];
    for (idx, thread) in threads.iter().enumerate() {
        if idx > 0 {
            lines.push(String::new());
        }
        lines.push(format!("{}. {}", idx + 1, thread.thread_id));
        let title = if thread.name.is_empty() {
            preview_text(&thread.preview, 80)
        } else {
            preview_text(&thread.name, 80)
        };
        if !title.is_empty() {
            lines.push(format!("   title: {title}"));
        }
        if !thread.cwd.is_empty() {
            lines.push(format!("   cwd: {}", thread.cwd));
        }
        if !thread.session_id.is_empty() {
            lines.push(format!("   session_id: {}", thread.session_id));
        }
        if !thread.status.is_empty() {
            lines.push(format!("   status: {}", thread.status));
        }
        if !thread.source.is_empty() {
            lines.push(format!("   source: {}", thread.source));
        }
        if thread.updated_at > 0 {
            lines.push(format!(
                "   updated_at: {}",
                format_updated_at(thread.updated_at)
            ));
        }
    }
    lines.push("".to_string());
    lines.push(format!(
        "Bind one with `{prefix}bind <number>`, for example `{prefix}bind 1`."
    ));
    lines.push(format!(
        "You can still bind an explicit id with `{prefix}bind <threadId>`."
    ));
    lines.join("\n")
}

fn format_updated_at(epoch_secs: i64) -> String {
    let Some(utc) = DateTime::from_timestamp(epoch_secs, 0) else {
        return epoch_secs.to_string();
    };
    let offset = FixedOffset::east_opt(8 * 60 * 60).expect("valid UTC+08 offset");
    utc.with_timezone(&offset)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push_str("...");
    out
}

#[async_trait]
impl AgentBackend for CodexAppServerBackend {
    async fn stream(&self, request: ChannelRequest) -> AgentEventStream {
        let this = self.clone_for_stream();
        Box::pin(async_stream::stream! {
            if let Err(msg) = check_sender_allowed(&this.security, &request.user_id) {
                for ev in error_events(msg) {
                    yield Ok(ev);
                }
                return;
            }

            let text = request_text(&request);
            let parsed = match parse_command(&text, &this.cfg.command_prefix) {
                Ok(p) => p,
                Err(e) => {
                    for ev in error_events(format!(
                        "{e}\n\n{}",
                        format_help(&this.cfg.command_prefix)
                    )) {
                        yield Ok(ev);
                    }
                    return;
                }
            };

            match parsed {
                ParsedCommand::Prompt { text } => {
                    let (tx, mut rx) = mpsc::unbounded_channel();
                    let worker = this.clone_for_stream();
                    let session_id = request.session_id.clone();
                    tokio::spawn(async move {
                        worker.run_prompt_stream(&session_id, &text, tx).await;
                    });
                    while let Some(item) = rx.recv().await {
                        yield item;
                    }
                }
                ParsedCommand::Help => {
                    for ev in help_events(format_help(&this.cfg.command_prefix)) {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Session => {
                    for ev in this.cmd_session(&request) {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Thread => {
                    for ev in this.cmd_thread(&request.session_id) {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Threads { loaded_only, limit } => {
                    for ev in this.cmd_threads(&request.session_id, loaded_only, limit).await {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::New => {
                    for ev in this.cmd_new(&request.session_id) {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Bind { thread_id } => {
                    for ev in this.cmd_bind(&request.session_id, &thread_id).await {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Rename { name } => {
                    for ev in this.cmd_rename(&request.session_id, &name).await {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Approve { request_id } => {
                    for ev in this.cmd_pending_response(
                        &request.session_id,
                        request_id,
                        PendingResponse::Approve,
                    ) {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Deny { request_id } => {
                    for ev in this.cmd_pending_response(
                        &request.session_id,
                        request_id,
                        PendingResponse::Deny,
                    ) {
                        yield Ok(ev);
                    }
                }
                ParsedCommand::Answer { request_id, text } => {
                    for ev in this.cmd_pending_response(
                        &request.session_id,
                        request_id,
                        PendingResponse::Answer(text),
                    ) {
                        yield Ok(ev);
                    }
                }
            }
        })
    }
}

impl CodexAppServerBackend {
    fn clone_for_stream(&self) -> Self {
        Self {
            cfg: self.cfg.clone(),
            security: self.security.clone(),
            client: CodexAppServerClient::new(self.cfg.clone()).expect("client clone"),
            bindings: self.bindings.clone(),
            thread_list_cache: self.thread_list_cache.clone(),
            pending_server_requests: self.pending_server_requests.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn formats_wechat_session_info_for_binding() {
        let mut meta = HashMap::new();
        meta.insert("is_group".into(), json!(true));
        meta.insert("group_id".into(), json!("group_1"));
        meta.insert("from_user_id".into(), json!("wx_user"));
        let request = ChannelRequest {
            channel: "wechat".into(),
            session_id: "wechat:group:group_1".into(),
            user_id: "wx_user".into(),
            content: Vec::new(),
            meta,
        };
        let binding = ThreadBinding {
            thread_id: "thr_123".into(),
            cwd: "/work".into(),
        };

        let text = format_session_info("/", Some(binding), &request);

        assert!(text.contains("channel: wechat"));
        assert!(text.contains("session_id: wechat:group:group_1"));
        assert!(text.contains("group_id: group_1"));
        assert!(text.contains("thread_id: thr_123"));
        assert!(text.contains("/bind <threadId|number>"));
    }

    #[test]
    fn formats_thread_list_with_bind_hint() {
        let text = format_thread_list(
            "/",
            &[CodexThreadSummary {
                thread_id: "thr_1".into(),
                session_id: "sess_1".into(),
                name: "Review code".into(),
                preview: String::new(),
                cwd: "/repo".into(),
                status: "idle".into(),
                source: "cli".into(),
                updated_at: 123,
            }],
            false,
        );

        assert!(text.contains("1. thr_1"));
        assert!(text.contains("title: Review code"));
        assert!(text.contains("session_id: sess_1"));
        assert!(text.contains("status: idle"));
        assert!(text.contains("source: cli"));
        assert!(text.contains("updated_at: 1970-01-01 08:02:03"));
        assert!(!text.contains("session_id=sess_1 status=idle"));
        assert!(text.contains("/bind 1"));
    }

    #[test]
    fn formats_multiple_threads_with_blank_line_between_items() {
        let text = format_thread_list(
            "/",
            &[
                CodexThreadSummary {
                    thread_id: "thr_1".into(),
                    session_id: String::new(),
                    name: "One".into(),
                    preview: String::new(),
                    cwd: String::new(),
                    status: String::new(),
                    source: String::new(),
                    updated_at: 0,
                },
                CodexThreadSummary {
                    thread_id: "thr_2".into(),
                    session_id: String::new(),
                    name: "Two".into(),
                    preview: String::new(),
                    cwd: String::new(),
                    status: String::new(),
                    source: String::new(),
                    updated_at: 0,
                },
            ],
            false,
        );

        assert!(text.contains("   title: One\n\n2. thr_2"));
    }

    #[test]
    fn resolves_numeric_bind_selector_from_cached_threads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend =
            CodexAppServerBackend::from_config(&AgentConfig::default(), dir.path().into())
                .expect("backend");
        backend.thread_list_cache.lock().expect("cache").insert(
            "im_session".into(),
            vec![CodexThreadSummary {
                thread_id: "thr_1".into(),
                session_id: String::new(),
                name: String::new(),
                preview: String::new(),
                cwd: "/repo".into(),
                status: String::new(),
                source: String::new(),
                updated_at: 0,
            }],
        );

        let resolved = backend
            .resolve_bind_selector("im_session", "1")
            .expect("selector");
        assert_eq!(resolved.thread_id, "thr_1");
        assert_eq!(resolved.cwd, "/repo");
    }

    #[test]
    fn update_cached_thread_name_updates_all_cached_lists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend =
            CodexAppServerBackend::from_config(&AgentConfig::default(), dir.path().into())
                .expect("backend");
        {
            let mut cache = backend.thread_list_cache.lock().expect("cache");
            cache.insert(
                "im_session_1".into(),
                vec![CodexThreadSummary {
                    thread_id: "thr_1".into(),
                    session_id: String::new(),
                    name: "Old".into(),
                    preview: String::new(),
                    cwd: String::new(),
                    status: String::new(),
                    source: String::new(),
                    updated_at: 0,
                }],
            );
            cache.insert(
                "im_session_2".into(),
                vec![CodexThreadSummary {
                    thread_id: "thr_1".into(),
                    session_id: String::new(),
                    name: "Old".into(),
                    preview: String::new(),
                    cwd: String::new(),
                    status: String::new(),
                    source: String::new(),
                    updated_at: 0,
                }],
            );
        }

        backend.update_cached_thread_name("thr_1", "New title");

        let cache = backend.thread_list_cache.lock().expect("cache");
        assert_eq!(cache["im_session_1"][0].name, "New title");
        assert_eq!(cache["im_session_2"][0].name, "New title");
    }

    #[test]
    fn merge_loaded_threads_appends_missing_loaded_threads() {
        let mut threads = vec![CodexThreadSummary {
            thread_id: "thr_recent".into(),
            session_id: "sess_1".into(),
            name: "recent".into(),
            preview: String::new(),
            cwd: "/repo".into(),
            status: "active".into(),
            source: "cli".into(),
            updated_at: 1,
        }];
        let loaded = vec![
            CodexThreadSummary {
                thread_id: "thr_recent".into(),
                session_id: String::new(),
                name: String::new(),
                preview: String::new(),
                cwd: String::new(),
                status: "loaded".into(),
                source: String::new(),
                updated_at: 0,
            },
            CodexThreadSummary {
                thread_id: "thr_remote_loaded".into(),
                session_id: String::new(),
                name: String::new(),
                preview: String::new(),
                cwd: String::new(),
                status: "loaded".into(),
                source: String::new(),
                updated_at: 0,
            },
        ];

        merge_loaded_threads(&mut threads, loaded, 10);

        assert_eq!(
            threads
                .iter()
                .map(|thread| thread.thread_id.as_str())
                .collect::<Vec<_>>(),
            vec!["thr_recent", "thr_remote_loaded"]
        );
    }

    #[test]
    fn pending_request_with_explicit_id_keeps_session_isolation() {
        let pending = Arc::new(PendingServerRequests::default());
        let (responder, _receiver) = oneshot::channel();
        let _cleanup = pending.insert(
            "req_1".into(),
            PendingServerRequest {
                im_session_id: "im_session_1".into(),
                method: "execCommandApproval".into(),
                params: Value::Null,
                responder,
            },
        );

        let err = match pending.take_for_session("im_session_2", Some("req_1")) {
            Ok(_) => panic!("other session must not resolve request"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("another IM session"));
        let (request_id, request) = pending
            .take_for_session("im_session_1", Some("req_1"))
            .expect("original session can still resolve request");
        assert_eq!(request_id, "req_1");
        assert_eq!(request.im_session_id, "im_session_1");
    }

    #[test]
    fn invalid_pending_response_keeps_request_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend =
            CodexAppServerBackend::from_config(&AgentConfig::default(), dir.path().into())
                .expect("backend");
        let (responder, _receiver) = oneshot::channel();
        let _cleanup = backend.pending_server_requests.insert(
            "req_1".into(),
            PendingServerRequest {
                im_session_id: "im_session_1".into(),
                method: "execCommandApproval".into(),
                params: json!({ "command": "cargo test" }),
                responder,
            },
        );

        let events = backend.cmd_pending_response(
            "im_session_1",
            Some("req_1".into()),
            PendingResponse::Answer("yes".into()),
        );

        assert!(events
            .iter()
            .any(|event| event.text.contains("does not accept this response")));
        let (request_id, request) = backend
            .pending_server_requests
            .take_for_session("im_session_1", Some("req_1"))
            .expect("invalid response must not drop pending request");
        assert_eq!(request_id, "req_1");
        assert_eq!(request.method, "execCommandApproval");
    }

    #[test]
    fn detects_missing_rollout_rpc_error() {
        let error = GatewayError::Other(
            r#"codex rpc error: {"code":-32600,"message":"no rollout found for thread id 019ecefa-b8b0-7213-a809-5efbdcf7471a"}"#
                .into(),
        );

        assert!(is_missing_rollout_error(&error));
    }

    #[test]
    fn stale_binding_replacement_cwd_prefers_binding_cwd() {
        let stale = ThreadBinding {
            thread_id: "thr_stale".into(),
            cwd: "/bound/repo".into(),
        };

        assert_eq!(
            replacement_cwd_for_stale_binding(&stale, "/default/repo").as_deref(),
            Some("/bound/repo")
        );
    }

    #[test]
    fn stale_binding_replacement_cwd_falls_back_to_default_cwd() {
        let stale = ThreadBinding {
            thread_id: "thr_stale".into(),
            cwd: String::new(),
        };

        assert_eq!(
            replacement_cwd_for_stale_binding(&stale, "/default/repo").as_deref(),
            Some("/default/repo")
        );
    }
}
