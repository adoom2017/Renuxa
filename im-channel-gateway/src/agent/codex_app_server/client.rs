use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::header::{HeaderValue, AUTHORIZATION},
        Message,
    },
};

use crate::agent::codex_app_server::events::TurnEventMapper;
use crate::config::{normalize_codex_sandbox, CodexAppServerConfig};
use crate::error::{GatewayError, Result};
use crate::types::AgentEvent;

const CLIENT_NAME: &str = "im_channel_gateway";
const CLIENT_VERSION: &str = "0.1.0";

pub struct CodexAppServerClient {
    cfg: CodexAppServerConfig,
    bearer_token: Option<String>,
}

pub type ServerRequestHandler =
    Arc<dyn Fn(ServerRequest) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ServerRequest {
    pub method: String,
    pub id: Value,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexThreadSummary {
    pub thread_id: String,
    pub session_id: String,
    pub name: String,
    pub preview: String,
    pub cwd: String,
    pub status: String,
    pub source: String,
    pub updated_at: i64,
}

impl CodexAppServerClient {
    pub fn new(cfg: CodexAppServerConfig) -> Result<Self> {
        let bearer_token = if cfg.token_file.is_empty() {
            None
        } else {
            let token = std::fs::read_to_string(&cfg.token_file)
                .map_err(|e| GatewayError::Other(format!("read token_file: {e}")))?
                .trim()
                .to_string();
            if token.is_empty() {
                None
            } else {
                Some(token)
            }
        };
        Ok(Self { cfg, bearer_token })
    }

    pub async fn thread_resume(&self, thread_id: &str) -> Result<()> {
        let params = json!({ "threadId": thread_id });
        self.call("thread/resume", params, Duration::from_secs(60))
            .await?;
        Ok(())
    }

    pub async fn thread_set_name(&self, thread_id: &str, name: &str) -> Result<()> {
        self.call(
            "thread/name/set",
            thread_set_name_params(thread_id, name),
            Duration::from_secs(30),
        )
        .await?;
        Ok(())
    }

    pub async fn thread_list(&self, limit: usize) -> Result<Vec<CodexThreadSummary>> {
        let limit = limit.clamp(1, 20);
        let params = json!({
            "limit": limit,
            "sortKey": "updated_at",
            "sortDirection": "desc",
            "archived": false,
            "sourceKinds": [
                "cli",
                "vscode",
                "exec",
                "appServer",
                "subAgent",
                "subAgentReview",
                "subAgentCompact",
                "subAgentThreadSpawn",
                "subAgentOther",
                "unknown"
            ],
        });
        let result = self
            .call("thread/list", params, Duration::from_secs(60))
            .await?;
        parse_thread_list_response(&result)
    }

    pub async fn thread_loaded_list(&self, limit: usize) -> Result<Vec<CodexThreadSummary>> {
        let limit = limit.clamp(1, 20);
        let result = self
            .call(
                "thread/loaded/list",
                json!({ "limit": limit }),
                Duration::from_secs(30),
            )
            .await?;
        parse_loaded_thread_list_response(&result)
    }

    /// Run `turn/start` and invoke `on_event` for each mapped notification until the turn completes.
    pub async fn turn_start_stream_with_handler<F>(
        &self,
        thread_id: &str,
        prompt: &str,
        max_secs: u64,
        on_event: &mut F,
        server_request_handler: Option<ServerRequestHandler>,
    ) -> Result<String>
    where
        F: FnMut(AgentEvent),
    {
        let deadline = Duration::from_secs(max_secs.max(1));
        let mut session = self.connect_with_handler(server_request_handler).await?;
        tracing::info!("codex ws: connected");
        session.initialize().await?;
        tracing::info!("codex ws: initialized");
        self.turn_start_on_session(&mut session, thread_id, prompt, deadline, true, on_event)
            .await
    }

    pub async fn thread_start_turn_stream_with_handler<F>(
        &self,
        cwd: &str,
        prompt: &str,
        max_secs: u64,
        on_event: &mut F,
        server_request_handler: Option<ServerRequestHandler>,
    ) -> Result<(String, String)>
    where
        F: FnMut(AgentEvent),
    {
        let deadline = Duration::from_secs(max_secs.max(1));
        let mut session = self.connect_with_handler(server_request_handler).await?;
        tracing::info!("codex ws: connected");
        session.initialize().await?;
        tracing::info!("codex ws: initialized");
        let result = session
            .request(
                "thread/start",
                thread_start_params(&self.cfg, cwd),
                Duration::from_secs(60),
            )
            .await?;
        let thread_id = extract_thread_id(&result)?;
        tracing::info!(thread_id = %thread_id, cwd, "codex ws: thread started");
        let text = self
            .turn_start_on_session(&mut session, &thread_id, prompt, deadline, false, on_event)
            .await?;
        Ok((thread_id, text))
    }

    async fn turn_start_on_session<F>(
        &self,
        session: &mut WsSession,
        thread_id: &str,
        prompt: &str,
        deadline: Duration,
        resume_thread: bool,
        on_event: &mut F,
    ) -> Result<String>
    where
        F: FnMut(AgentEvent),
    {
        let mut params = json!({
            "threadId": thread_id,
            "input": [{ "type": "text", "text": prompt }],
        });
        if !self.cfg.approval_policy.is_empty() {
            params["approvalPolicy"] = json!(self.cfg.approval_policy);
        }
        if resume_thread {
            // Subscription is per-connection: thread/resume on THIS socket subscribes it to the
            // thread's turn/item notifications (and loads history into the response, not as events).
            // Without it, turn/* and item/* notifications are never delivered on this connection.
            session
                .request(
                    "thread/resume",
                    json!({ "threadId": thread_id }),
                    Duration::from_secs(60),
                )
                .await?;
            tracing::info!(thread_id, "codex ws: thread resumed (subscribed)");
        } else {
            tracing::info!(
                thread_id,
                "codex ws: using newly-started thread on current connection"
            );
        }
        // turn/start returns the initial turn immediately; the actual output streams afterwards
        // as turn/started, item/*, turn/completed notifications on this same connection.
        let turn_id = session.send_request("turn/start", params).await?;
        tracing::info!(turn_id, "codex ws: turn/start sent, awaiting notifications");
        session
            .collect_turn_events(deadline, turn_id, on_event)
            .await
    }

    async fn call(&self, method: &str, params: Value, wait: Duration) -> Result<Value> {
        let mut session = self.connect().await?;
        session.initialize().await?;
        session.request(method, params, wait).await
    }

    async fn connect(&self) -> Result<WsSession> {
        self.connect_with_handler(None).await
    }

    async fn connect_with_handler(
        &self,
        server_request_handler: Option<ServerRequestHandler>,
    ) -> Result<WsSession> {
        let mut request = self
            .cfg
            .listen
            .as_str()
            .into_client_request()
            .map_err(|e| GatewayError::Other(format!("ws request: {e}")))?;
        if let Some(token) = &self.bearer_token {
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| GatewayError::Other(format!("auth header: {e}")))?;
            request.headers_mut().insert(AUTHORIZATION, value);
        }
        let (ws, _) = connect_async(request)
            .await
            .map_err(|e| GatewayError::Other(format!("connect {}: {e}", self.cfg.listen)))?;
        Ok(WsSession {
            ws,
            next_id: 1,
            inbound_pending: VecDeque::new(),
            server_request_handler,
        })
    }
}

struct WsSession {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_id: u64,
    /// Notifications (and unrelated RPC replies) received while waiting for a specific id.
    inbound_pending: VecDeque<Value>,
    server_request_handler: Option<ServerRequestHandler>,
}

impl WsSession {
    async fn initialize(&mut self) -> Result<()> {
        let result = self
            .request("initialize", initialize_params(), Duration::from_secs(30))
            .await?;
        tracing::debug!(?result, "codex app-server initialize result");
        self.send_notification("initialized", json!({})).await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value, wait: Duration) -> Result<Value> {
        let id = self.send_request(method, params).await?;
        self.wait_for_id(id, wait).await
    }

    /// Send a JSON-RPC request and return its id without waiting for the response.
    async fn send_request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_json(json!({
            "method": method,
            "id": id,
            "params": params,
        }))
        .await?;
        Ok(id)
    }

    async fn send_notification(&mut self, method: &str, params: Value) -> Result<()> {
        self.send_json(json!({ "method": method, "params": params }))
            .await
    }

    async fn send_json(&mut self, value: Value) -> Result<()> {
        let raw = serde_json::to_string(&value)
            .map_err(|e| GatewayError::Other(format!("json encode: {e}")))?;
        self.ws
            .send(Message::Text(raw.into()))
            .await
            .map_err(|e| GatewayError::Other(format!("ws send: {e}")))?;
        Ok(())
    }

    async fn wait_for_id(&mut self, id: u64, wait: Duration) -> Result<Value> {
        let result = timeout(wait, async {
            loop {
                let msg = self.recv_json().await?;
                if is_jsonrpc_response_for(&msg, id) {
                    if let Some(err) = msg.get("error") {
                        return Err(GatewayError::Other(format!("codex rpc error: {err}")));
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
                if self.try_answer_server_request(&msg).await? {
                    continue;
                }
                if is_jsonrpc_notification(&msg) {
                    tracing::debug!(
                        method = ?msg.get("method").and_then(|m| m.as_str()),
                        "codex ws: ignored notification while waiting for rpc id={id}"
                    );
                    continue;
                }
                tracing::info!(
                    frame_id = ?msg.get("id"),
                    method = ?msg.get("method").and_then(|m| m.as_str()),
                    "codex ws: ignored frame while waiting for rpc id={id}"
                );
            }
        })
        .await
        .map_err(|_| GatewayError::Other(format!("codex rpc timeout waiting for id={id}")))?;
        result
    }

    /// Read frames until the turn completes, mapping notifications to events in real time.
    /// `turn_id` is the JSON-RPC id of the `turn/start` request; its response (delivered
    /// only when the turn finishes) is treated as a terminal signal.
    async fn collect_turn_events<F>(
        &mut self,
        wait: Duration,
        turn_id: u64,
        on_event: &mut F,
    ) -> Result<String>
    where
        F: FnMut(AgentEvent),
    {
        let mut mapper = TurnEventMapper::new();
        let mut turn_response: Option<Value> = None;
        let result = timeout(wait, async {
            loop {
                let msg = self.recv_next().await?;
                // turn/start ack: returns immediately with the initial turn. NOT terminal —
                // keep reading notifications until turn/completed. Capture it as fallback text.
                if is_jsonrpc_response_for(&msg, turn_id) {
                    if let Some(err) = msg.get("error") {
                        return Err(GatewayError::Other(format!(
                            "codex turn/start error: {err}"
                        )));
                    }
                    turn_response = msg.get("result").cloned();
                    continue;
                }
                if self.try_answer_server_request(&msg).await? {
                    continue;
                }
                for ev in mapper.push(&msg) {
                    on_event(ev);
                }
                if mapper.is_turn_done() {
                    break;
                }
            }
            Ok::<(), GatewayError>(())
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                tracing::warn!("codex turn collection timed out; returning partial text");
            }
        }

        let mut text = mapper.accumulated.trim().to_string();
        let streamed = mapper.emitted_any();
        if text.is_empty() {
            if let Some(resp) = &turn_response {
                text = extract_turn_agent_text(resp);
            }
        }
        if text.is_empty() {
            return Err(GatewayError::Other(
                "codex turn completed without agent text".into(),
            ));
        }
        // No streaming notifications arrived (turn/start response was the only payload):
        // synthesize the events the pipeline needs to build a reply.
        if !streamed {
            on_event(AgentEvent::assistant_text(text.clone()));
            on_event(AgentEvent::response_completed());
        }
        Ok(text)
    }

    async fn recv_next(&mut self) -> Result<Value> {
        if let Some(msg) = self.inbound_pending.pop_front() {
            return Ok(msg);
        }
        self.recv_json().await
    }

    async fn recv_json(&mut self) -> Result<Value> {
        loop {
            let frame = self
                .ws
                .next()
                .await
                .ok_or_else(|| GatewayError::Other("ws closed".into()))?
                .map_err(|e| GatewayError::Other(format!("ws recv: {e}")))?;
            match frame {
                Message::Text(t) => {
                    return serde_json::from_str(t.as_ref())
                        .map_err(|e| GatewayError::Other(format!("json parse: {e}")));
                }
                Message::Ping(p) => {
                    self.ws
                        .send(Message::Pong(p))
                        .await
                        .map_err(|e| GatewayError::Other(format!("ws pong: {e}")))?;
                }
                Message::Close(_) => {
                    return Err(GatewayError::Other("ws connection closed".into()));
                }
                _ => {}
            }
        }
    }

    /// Answer server-initiated requests. Turn streams may install a handler that asks the IM
    /// user; utility RPC calls keep using the conservative default auto-answer path.
    async fn try_answer_server_request(&mut self, msg: &Value) -> Result<bool> {
        let Some(request) = server_request_from_msg(msg) else {
            return Ok(false);
        };
        let result = if let Some(handler) = self.server_request_handler.clone() {
            handler(request.clone()).await?
        } else if let Some(result) = server_request_accept_result(&request.method) {
            result
        } else {
            tracing::warn!(
                method = %request.method,
                "codex app-server: unhandled server request; turn may stall"
            );
            return Ok(false);
        };

        tracing::debug!(method = %request.method, "codex app-server: answered server request");
        self.send_json(json!({ "id": request.id, "result": result }))
            .await?;
        Ok(true)
    }
}

fn server_request_from_msg(msg: &Value) -> Option<ServerRequest> {
    let method = msg.get("method")?.as_str()?;
    let id = msg.get("id")?.clone();
    let params = msg.get("params")?.clone();
    if msg.get("result").is_some() || msg.get("error").is_some() {
        return None;
    }
    if !is_known_server_request(method) {
        return None;
    }
    Some(ServerRequest {
        method: method.to_string(),
        id,
        params,
    })
}

fn is_known_server_request(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "execCommandApproval"
            | "item/fileChange/requestApproval"
            | "applyPatchApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
    )
}

pub fn server_request_accept_result(method: &str) -> Option<Value> {
    match method {
        "item/commandExecution/requestApproval" | "execCommandApproval" => {
            Some(json!({ "decision": "acceptForSession" }))
        }
        "item/fileChange/requestApproval" | "applyPatchApproval" => {
            Some(json!({ "decision": "acceptForSession" }))
        }
        "item/permissions/requestApproval" => Some(json!({ "decision": "accept" })),
        "item/tool/requestUserInput" => Some(json!({ "answers": {} })),
        _ => None,
    }
}

pub fn server_request_deny_result(method: &str) -> Option<Value> {
    match method {
        "item/commandExecution/requestApproval"
        | "execCommandApproval"
        | "item/fileChange/requestApproval"
        | "applyPatchApproval"
        | "item/permissions/requestApproval" => Some(json!({ "decision": "reject" })),
        "item/tool/requestUserInput" => Some(json!({ "answers": {} })),
        _ => None,
    }
}

pub fn server_request_answer_result(method: &str, params: &Value, text: &str) -> Option<Value> {
    if method != "item/tool/requestUserInput" {
        return None;
    }
    Some(json!({ "answers": build_user_input_answers(params, text) }))
}

pub fn server_request_question_ids(params: &Value) -> Vec<String> {
    params
        .get("questions")
        .or_else(|| params.pointer("/input/questions"))
        .or_else(|| params.pointer("/request/questions"))
        .and_then(|v| v.as_array())
        .map(|questions| {
            questions
                .iter()
                .filter_map(|question| {
                    question
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn build_user_input_answers(params: &Value, text: &str) -> Value {
    let ids = server_request_question_ids(params);
    if ids.len() == 1 {
        let mut answers = BTreeMap::new();
        answers.insert(ids[0].clone(), text.trim().to_string());
        return serde_json::to_value(answers)
            .unwrap_or_else(|_| json!({ "response": text.trim() }));
    }
    let pairs = parse_answer_pairs(text);
    if !pairs.is_empty() {
        return serde_json::to_value(pairs).unwrap_or_else(|_| json!({ "response": text.trim() }));
    }
    json!({ "response": text.trim() })
}

fn parse_answer_pairs(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for part in text.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        out.insert(key.to_string(), value.trim().to_string());
    }
    out
}

fn is_jsonrpc_notification(msg: &Value) -> bool {
    msg.get("method").is_some() && msg.get("id").is_none()
}

fn is_jsonrpc_response_for(msg: &Value, expected_id: u64) -> bool {
    let Some(id_val) = msg.get("id") else {
        return false;
    };
    if msg.get("result").is_none() && msg.get("error").is_none() {
        return false;
    }
    json_id_matches(id_val, expected_id)
}

fn json_id_matches(id_val: &Value, expected_id: u64) -> bool {
    match id_val {
        Value::Number(n) => n.as_u64() == Some(expected_id),
        Value::String(s) => s.parse::<u64>().ok() == Some(expected_id),
        _ => false,
    }
}

fn initialize_params() -> Value {
    json!({
        "clientInfo": {
            "name": CLIENT_NAME,
            "title": "IM Channel Gateway",
            "version": CLIENT_VERSION,
        },
        "capabilities": {
            "optOutNotificationMethods": [
                "remoteControl/status/changed",
                "thread/tokenUsage/updated",
                "hook/started",
                "hook/completed",
                "thread/status/changed",
            ],
        },
    })
}

fn thread_start_params(cfg: &CodexAppServerConfig, cwd: &str) -> Value {
    let mut params = json!({
        "model": cfg.model,
        "approvalPolicy": cfg.approval_policy,
        "sandbox": normalize_codex_sandbox(&cfg.sandbox),
        "serviceName": CLIENT_NAME,
    });
    if !cwd.is_empty() {
        params["cwd"] = json!(cwd);
    }
    params
}

fn thread_set_name_params(thread_id: &str, name: &str) -> Value {
    json!({
        "threadId": thread_id,
        "name": name,
    })
}

fn extract_thread_id(result: &Value) -> Result<String> {
    result
        .pointer("/thread/id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| GatewayError::Other("thread/start missing thread.id".into()))
}

fn parse_thread_list_response(result: &Value) -> Result<Vec<CodexThreadSummary>> {
    let data = result
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GatewayError::Other("thread/list missing data array".into()))?;
    Ok(data.iter().filter_map(thread_summary_from_value).collect())
}

fn parse_loaded_thread_list_response(result: &Value) -> Result<Vec<CodexThreadSummary>> {
    let data = result
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GatewayError::Other("thread/loaded/list missing data array".into()))?;
    Ok(data
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|id| !id.trim().is_empty())
        .map(|id| CodexThreadSummary {
            thread_id: id.trim().to_string(),
            session_id: String::new(),
            name: String::new(),
            preview: String::new(),
            cwd: String::new(),
            status: "loaded".to_string(),
            source: String::new(),
            updated_at: 0,
        })
        .collect())
}

fn thread_summary_from_value(value: &Value) -> Option<CodexThreadSummary> {
    let thread_id = value.get("id")?.as_str()?.trim();
    if thread_id.is_empty() {
        return None;
    }
    Some(CodexThreadSummary {
        thread_id: thread_id.to_string(),
        session_id: value
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        name: value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        preview: value
            .get("preview")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        cwd: value
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status: value
            .pointer("/status/type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        source: value
            .pointer("/source/kind")
            .or_else(|| value.pointer("/threadSource/kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        updated_at: value.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0),
    })
}

/// Pull the last `agentMessage` text out of a `turn/start` response's `turn.items`.
fn extract_turn_agent_text(result: &Value) -> String {
    let Some(items) = result.pointer("/turn/items").and_then(|v| v.as_array()) else {
        return String::new();
    };
    items
        .iter()
        .rfind(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("agentMessage")
                && item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .is_some_and(|t| !t.is_empty())
        })
        .and_then(|item| item.get("text").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_numeric_and_string_rpc_ids() {
        assert!(is_jsonrpc_response_for(&json!({"id": 2, "result": {}}), 2));
        assert!(is_jsonrpc_response_for(
            &json!({"id": "2", "result": {}}),
            2
        ));
        assert!(!is_jsonrpc_response_for(
            &json!({"method": "turn/completed", "params": {}}),
            2
        ));
    }

    #[test]
    fn initialize_opts_out_of_noisy_status_notifications() {
        let params = initialize_params();
        let methods = params
            .pointer("/capabilities/optOutNotificationMethods")
            .and_then(|v| v.as_array())
            .expect("optOutNotificationMethods");

        for expected in [
            "remoteControl/status/changed",
            "thread/tokenUsage/updated",
            "hook/started",
            "hook/completed",
            "thread/status/changed",
        ] {
            assert!(methods.iter().any(|m| m.as_str() == Some(expected)));
        }
    }

    #[test]
    fn thread_start_params_normalize_legacy_sandbox_spelling() {
        let cfg = CodexAppServerConfig {
            sandbox: "workspaceWrite".to_string(),
            ..Default::default()
        };

        let params = thread_start_params(&cfg, "/tmp/project");

        assert_eq!(
            params.get("sandbox").and_then(|v| v.as_str()),
            Some("workspace-write")
        );
        assert_eq!(
            params.get("cwd").and_then(|v| v.as_str()),
            Some("/tmp/project")
        );
    }

    #[test]
    fn thread_set_name_params_use_protocol_field_names() {
        let params = thread_set_name_params("thr_1", "rust-agent wechat");

        assert_eq!(
            params.get("threadId").and_then(|v| v.as_str()),
            Some("thr_1")
        );
        assert_eq!(
            params.get("name").and_then(|v| v.as_str()),
            Some("rust-agent wechat")
        );
    }

    #[test]
    fn parses_thread_list_response() {
        let threads = parse_thread_list_response(&json!({
            "data": [{
                "id": "thr_1",
                "sessionId": "sess_1",
                "name": "Fix bug",
                "preview": "hello",
                "cwd": "/work/repo",
                "updatedAt": 123,
                "status": { "type": "idle" },
                "source": { "kind": "cli" }
            }],
            "nextCursor": null,
            "backwardsCursor": null
        }))
        .expect("thread list");

        assert_eq!(
            threads,
            vec![CodexThreadSummary {
                thread_id: "thr_1".into(),
                session_id: "sess_1".into(),
                name: "Fix bug".into(),
                preview: "hello".into(),
                cwd: "/work/repo".into(),
                status: "idle".into(),
                source: "cli".into(),
                updated_at: 123,
            }]
        );
    }

    #[test]
    fn parses_loaded_thread_list_response() {
        let threads = parse_loaded_thread_list_response(&json!({
            "data": ["thr_1", "thr_2"],
            "nextCursor": null
        }))
        .expect("loaded thread list");

        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].thread_id, "thr_1");
        assert_eq!(threads[0].status, "loaded");
    }

    #[test]
    fn builds_server_request_results() {
        assert_eq!(
            server_request_accept_result("execCommandApproval"),
            Some(json!({ "decision": "acceptForSession" }))
        );
        assert_eq!(
            server_request_deny_result("applyPatchApproval"),
            Some(json!({ "decision": "reject" }))
        );
        assert_eq!(
            server_request_answer_result(
                "item/tool/requestUserInput",
                &json!({ "questions": [{ "id": "choice" }] }),
                "A"
            ),
            Some(json!({ "answers": { "choice": "A" } }))
        );
        assert_eq!(
            server_request_answer_result(
                "item/tool/requestUserInput",
                &json!({ "questions": [{ "id": "first" }, { "id": "second" }] }),
                "first=yes; second=no"
            ),
            Some(json!({ "answers": { "first": "yes", "second": "no" } }))
        );
    }
}
