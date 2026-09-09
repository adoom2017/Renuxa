use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::{watch, Mutex, RwLock};

/// Shared state for `getupdates` cursor and serialized poll access.
pub(crate) struct WeChatPollState {
    pub cursor: Arc<RwLock<String>>,
    pub lock: Arc<Mutex<()>>,
    pub data_dir: PathBuf,
    pub account_id: String,
}

use crate::config::WeChatConfig;
use crate::context::GatewayContext;
use crate::error::{GatewayError, Result};
use crate::pipeline::{message_priority, native_from_json, NativeMessage, ProcessedReply};
use crate::platforms::wechat::client::{
    ILinkClient, DEFAULT_BASE_URL, WECHAT_SEND_INTERVAL, WECHAT_TEXT_CHUNK_CHARS,
    WECHAT_TYPING_STATUS_CLEANUP, WECHAT_TYPING_STATUS_TYPING,
};
use crate::platforms::wechat::registry::{
    load_cursor_for_account, save_cursor_for_account, token_file_path, WeChatAccountEntry,
    WeChatAccountRegistry,
};
use crate::platforms::wechat::stream::process_wechat_message;
use crate::queue::{make_consumer, session_key};
use crate::text::split_text_chars;
use crate::types::{ChannelPart, PartKind};

const DEDUP_MAX: usize = 2000;
const SENT_ECHO_TTL: Duration = Duration::from_secs(120);
const EMPTY_POLL_SLEEP: Duration = Duration::from_millis(250);
const TYPING_TICKET_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const TYPING_TICKET_REFRESH_BACKOFF: Duration = Duration::from_secs(60);
const TYPING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) type SentEchoStore = Arc<Mutex<HashMap<String, Instant>>>;
pub(crate) type ContextTokenStore = Arc<RwLock<HashMap<String, String>>>;
pub(crate) type TypingTicketStore = Arc<RwLock<HashMap<String, TypingTicketEntry>>>;
pub(crate) type TypingSessionStore = Arc<Mutex<HashMap<String, TypingSession>>>;

#[derive(Clone)]
pub(crate) struct TypingTicketEntry {
    ticket: String,
    expires_at: Instant,
    refresh_after: Instant,
}

pub(crate) struct TypingSession {
    user_id: String,
    ticket: String,
    keepalive: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub(crate) struct WeChatRuntimeStores {
    pub sent_echoes: SentEchoStore,
    pub context_tokens: ContextTokenStore,
    pub typing_tickets: TypingTicketStore,
    pub typing_sessions: TypingSessionStore,
}

pub struct WeChatAccountRunner {
    account_id: String,
    channel_id: String,
    cfg: WeChatConfig,
    entry: WeChatAccountEntry,
    data_dir: PathBuf,
    registry: Arc<WeChatAccountRegistry>,
    shutdown: Arc<watch::Sender<bool>>,
    shutdown_rx: watch::Receiver<bool>,
    dedup_ids: Arc<Mutex<VecDeque<String>>>,
    sent_echoes: SentEchoStore,
    context_tokens: ContextTokenStore,
    typing_tickets: TypingTicketStore,
    typing_sessions: TypingSessionStore,
    cursor: Arc<RwLock<String>>,
}

impl WeChatAccountRunner {
    pub fn new(
        account_id: String,
        channel_id: String,
        cfg: WeChatConfig,
        entry: WeChatAccountEntry,
        data_dir: PathBuf,
        registry: Arc<WeChatAccountRegistry>,
    ) -> Result<Self> {
        let (tx, rx) = watch::channel(false);
        Ok(Self {
            account_id,
            channel_id,
            cfg,
            entry,
            data_dir,
            registry,
            shutdown: Arc::new(tx),
            shutdown_rx: rx,
            dedup_ids: Arc::new(Mutex::new(VecDeque::new())),
            sent_echoes: Arc::new(Mutex::new(HashMap::new())),
            context_tokens: Arc::new(RwLock::new(HashMap::new())),
            typing_tickets: Arc::new(RwLock::new(HashMap::new())),
            typing_sessions: Arc::new(Mutex::new(HashMap::new())),
            cursor: Arc::new(RwLock::new(String::new())),
        })
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
    }

    async fn remember_context_token(store: &ContextTokenStore, user_id: &str, token: &str) {
        let token = token.trim();
        if user_id.is_empty() || token.is_empty() {
            return;
        }
        store
            .write()
            .await
            .insert(user_id.to_string(), token.to_string());
    }

    async fn resolve_context_token(
        store: &ContextTokenStore,
        to_user: &str,
        fallback: &str,
    ) -> String {
        let cached = store.read().await.get(to_user).cloned();
        cached.unwrap_or_else(|| fallback.to_string())
    }

    async fn apply_getupdates_response(poll: &WeChatPollState, data: &Value) {
        if let Some(buf) = Self::getupdates_cursor(data) {
            *poll.cursor.write().await = buf.to_string();
            let _ = save_cursor_for_account(&poll.data_dir, &poll.account_id, buf);
        }
    }

    fn getupdates_cursor(data: &Value) -> Option<&str> {
        Self::string_field_ref(
            data,
            &[
                "get_updates_buf",
                "getUpdatesBuf",
                "next_get_updates_buf",
                "nextGetUpdatesBuf",
            ],
        )
    }

    #[cfg(test)]
    fn token_suffix(token: &str) -> String {
        let t = token.trim();
        let chars = t.chars().count();
        if chars <= 8 {
            t.to_string()
        } else {
            format!("...{}", t.chars().skip(chars - 8).collect::<String>())
        }
    }

    fn typing_ticket_from_config(data: &Value) -> Option<&str> {
        data.get("typing_ticket")
            .or_else(|| data.get("data").and_then(|v| v.get("typing_ticket")))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }

    async fn cached_typing_ticket(store: &TypingTicketStore, user_id: &str) -> Option<String> {
        let now = Instant::now();
        let ticket = {
            let tickets = store.read().await;
            let entry = tickets.get(user_id)?;
            if entry.ticket.is_empty() || entry.expires_at <= now {
                return None;
            }
            Some(entry.ticket.clone())
        };
        ticket
    }

    async fn remember_typing_ticket(store: &TypingTicketStore, user_id: &str, ticket: &str) {
        if user_id.is_empty() || ticket.trim().is_empty() {
            return;
        }
        let now = Instant::now();
        store.write().await.insert(
            user_id.to_string(),
            TypingTicketEntry {
                ticket: ticket.trim().to_string(),
                expires_at: now + TYPING_TICKET_TTL,
                refresh_after: now,
            },
        );
    }

    async fn remember_typing_ticket_failure(store: &TypingTicketStore, user_id: &str) {
        if user_id.is_empty() {
            return;
        }
        let now = Instant::now();
        store.write().await.insert(
            user_id.to_string(),
            TypingTicketEntry {
                ticket: String::new(),
                expires_at: now,
                refresh_after: now + TYPING_TICKET_REFRESH_BACKOFF,
            },
        );
    }

    async fn resolve_typing_ticket(
        client: &ILinkClient,
        store: &TypingTicketStore,
        user_id: &str,
        context_token: &str,
    ) -> Option<String> {
        if let Some(ticket) = Self::cached_typing_ticket(store, user_id).await {
            return Some(ticket);
        }
        let now = Instant::now();
        if let Some(refresh_after) = store
            .read()
            .await
            .get(user_id)
            .map(|entry| entry.refresh_after)
        {
            if refresh_after > now {
                tracing::debug!(
                    user_id = %user_id,
                    wait_ms = refresh_after.duration_since(now).as_millis(),
                    "wechat: skip typing_ticket refresh during backoff"
                );
                return None;
            }
        }
        let data = match client.getconfig(user_id, context_token).await {
            Ok(data) => data,
            Err(e) => {
                Self::remember_typing_ticket_failure(store, user_id).await;
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "wechat: getconfig failed; continue without typing"
                );
                return None;
            }
        };
        let Some(ticket) = Self::typing_ticket_from_config(&data) else {
            Self::remember_typing_ticket_failure(store, user_id).await;
            tracing::warn!(
                user_id = %user_id,
                "wechat: getconfig response missing typing_ticket"
            );
            return None;
        };
        let ticket = ticket.to_string();
        Self::remember_typing_ticket(store, user_id, &ticket).await;
        Some(ticket)
    }

    pub(crate) async fn send_typing_start(
        client: &ILinkClient,
        tickets: &TypingTicketStore,
        user_id: &str,
        context_token: &str,
    ) -> Option<String> {
        if user_id.is_empty() || context_token.is_empty() {
            tracing::warn!(
                user_id = %user_id,
                has_context_token = !context_token.is_empty(),
                "wechat: skip typing start; missing routing fields"
            );
            return None;
        }
        let ticket = Self::resolve_typing_ticket(client, tickets, user_id, context_token).await?;
        if Self::send_typing_status(
            client,
            tickets,
            user_id,
            &ticket,
            WECHAT_TYPING_STATUS_TYPING,
            "started",
        )
        .await
        {
            Some(ticket)
        } else {
            None
        }
    }

    pub(crate) async fn send_typing_cleanup(
        client: &ILinkClient,
        tickets: &TypingTicketStore,
        user_id: &str,
        typing_ticket: Option<&str>,
    ) {
        let Some(ticket) = typing_ticket.map(str::trim).filter(|t| !t.is_empty()) else {
            return;
        };
        if user_id.is_empty() {
            return;
        }
        let _ = Self::send_typing_status(
            client,
            tickets,
            user_id,
            ticket,
            WECHAT_TYPING_STATUS_CLEANUP,
            "cleaned up",
        )
        .await;
    }

    async fn send_typing_status(
        client: &ILinkClient,
        tickets: &TypingTicketStore,
        user_id: &str,
        ticket: &str,
        status: i64,
        action: &str,
    ) -> bool {
        match client.sendtyping(user_id, ticket, status).await {
            Ok(resp) => {
                tracing::debug!(
                    user_id = %user_id,
                    status,
                    response = %resp,
                    "wechat: typing status {action}"
                );
                true
            }
            Err(e) => {
                Self::remember_typing_ticket_failure(tickets, user_id).await;
                tracing::warn!(
                    user_id = %user_id,
                    status,
                    error = %e,
                    "wechat: sendtyping {action} failed"
                );
                false
            }
        }
    }

    fn typing_session_key(user_id: &str, context_token: &str) -> String {
        format!(
            "{user_id}:{}",
            hex::encode(md5::compute(context_token.as_bytes()).0)
        )
    }

    fn reply_target_from_meta(meta: &HashMap<String, Value>, fallback_user_id: &str) -> String {
        meta.get("reply_to_user_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                meta.get("from_user_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or(fallback_user_id)
            .to_string()
    }

    fn native_reply_target(native: &NativeMessage) -> String {
        Self::reply_target_from_meta(&native.meta, &native.sender_id)
    }

    fn spawn_typing_keepalive(
        client: Arc<ILinkClient>,
        tickets: TypingTicketStore,
        user_id: String,
        ticket: String,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(TYPING_KEEPALIVE_INTERVAL).await;
                if !Self::send_typing_status(
                    &client,
                    &tickets,
                    &user_id,
                    &ticket,
                    WECHAT_TYPING_STATUS_TYPING,
                    "keepalive sent",
                )
                .await
                {
                    break;
                }
            }
        })
    }

    pub(crate) async fn start_typing_session(
        client: Arc<ILinkClient>,
        tickets: &TypingTicketStore,
        sessions: &TypingSessionStore,
        user_id: &str,
        context_token: &str,
    ) -> Option<(String, String)> {
        let ticket = Self::send_typing_start(&client, tickets, user_id, context_token).await?;
        let session_key = Self::typing_session_key(user_id, context_token);
        let keepalive = Self::spawn_typing_keepalive(
            client,
            tickets.clone(),
            user_id.to_string(),
            ticket.clone(),
        );
        if let Some(previous) = sessions.lock().await.insert(
            session_key.clone(),
            TypingSession {
                user_id: user_id.to_string(),
                ticket: ticket.clone(),
                keepalive,
            },
        ) {
            previous.keepalive.abort();
        }
        Some((session_key, ticket))
    }

    pub(crate) async fn finish_typing_session(
        client: &ILinkClient,
        tickets: &TypingTicketStore,
        sessions: &TypingSessionStore,
        session_key: Option<&str>,
        fallback_user_id: &str,
        fallback_ticket: Option<&str>,
    ) {
        if let Some(session_key) = session_key {
            if let Some(session) = sessions.lock().await.remove(session_key) {
                session.keepalive.abort();
                Self::send_typing_cleanup(client, tickets, &session.user_id, Some(&session.ticket))
                    .await;
                return;
            }
        }
        Self::send_typing_cleanup(client, tickets, fallback_user_id, fallback_ticket).await;
    }

    fn base_url(&self) -> &str {
        if !self.entry.base_url.is_empty() {
            &self.entry.base_url
        } else if self.cfg.base_url.is_empty() {
            DEFAULT_BASE_URL
        } else {
            &self.cfg.base_url
        }
    }

    async fn resolve_token(&self) -> Result<String> {
        let token = self.registry.load_token(&self.account_id)?;
        if !token.is_empty() {
            return Ok(token);
        }
        Err(GatewayError::Config(format!(
            "wechat account {} missing bot_token; run `im-channel-gateway login wechat`",
            self.account_id
        )))
    }

    fn media_part(kind: PartKind, label: &str, item: &Value) -> ChannelPart {
        let mut extra = HashMap::new();
        extra.insert("wechat_item".to_string(), item.clone());
        extra.insert("wechat_item_label".to_string(), json!(label));
        ChannelPart {
            kind,
            text: format!("[WeChat {label} message]"),
            data: Some(item.clone()),
            extra,
            ..Default::default()
        }
    }

    fn parse_item(item: &Value) -> Option<ChannelPart> {
        let item_type = item.get("type").and_then(|v| v.as_i64()).unwrap_or(0);
        match item_type {
            1 => {
                let text = item
                    .get("text_item")
                    .and_then(|t| t.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    None
                } else {
                    Some(ChannelPart::text_part(text))
                }
            }
            2 => Some(Self::media_part(PartKind::Image, "image", item)),
            3 => {
                let mut part = Self::media_part(PartKind::Audio, "voice", item);
                if let Some(text) = item
                    .get("voice_item")
                    .and_then(|v| v.get("text"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    part.text = format!("[WeChat voice message]\n{text}");
                }
                Some(part)
            }
            4 => {
                let mut part = Self::media_part(PartKind::File, "file", item);
                if let Some(name) = item
                    .get("file_item")
                    .and_then(|v| v.get("file_name"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    part.text = format!("[WeChat file message: {name}]");
                }
                Some(part)
            }
            5 => Some(Self::media_part(PartKind::Video, "video", item)),
            _ => None,
        }
    }

    fn parse_inbound(msg: &Value, account_id: &str, channel_id: &str) -> Option<NativeMessage> {
        let msg_type = msg
            .get("message_type")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if msg_type != 1 {
            return None;
        }
        let from_user_id = msg
            .get("from_user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if from_user_id.is_empty() {
            return None;
        }
        let group_id = msg.get("group_id").and_then(|v| v.as_str()).unwrap_or("");
        let is_group = !group_id.is_empty();
        let session_id = if is_group {
            format!("wechat:{account_id}:group:{group_id}")
        } else {
            format!("wechat:{account_id}:dm:{from_user_id}")
        };
        let reply_to_user_id = if is_group {
            group_id.to_string()
        } else {
            from_user_id.clone()
        };
        let context_token = msg
            .get("context_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let to_user_id = Self::string_field(
            msg,
            &[
                "to_user_id",
                "bot_user_id",
                "botUserId",
                "receiver_user_id",
                "receiverUserId",
            ],
        )
        .unwrap_or_default();

        let mut content_parts = Vec::new();
        if let Some(items) = msg.get("item_list").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(part) = Self::parse_item(item) {
                    content_parts.push(part);
                }
            }
        }
        if content_parts.is_empty() {
            return None;
        }

        let mut meta = HashMap::new();
        meta.insert("message_id".into(), json!(stable_message_id(msg)));
        meta.insert("account_id".into(), json!(account_id));
        meta.insert("from_user_id".into(), json!(from_user_id));
        meta.insert("reply_to_user_id".into(), json!(reply_to_user_id));
        meta.insert("to_user_id".into(), json!(to_user_id));
        meta.insert("context_token".into(), json!(context_token));
        meta.insert("is_group".into(), json!(is_group));
        if is_group {
            meta.insert("group_id".into(), json!(group_id));
        }

        Some(NativeMessage {
            channel_id: channel_id.to_string(),
            sender_id: from_user_id,
            session_id: Some(session_id),
            content_parts,
            meta,
        })
    }

    fn sent_echo_key(user_id: &str, text: &str) -> String {
        format!(
            "{user_id}:{}",
            hex::encode(md5::compute(text.trim().as_bytes()).0)
        )
    }

    fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
        keys.iter()
            .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    }

    fn string_field_ref<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
        Self::object_variants(value).find_map(|obj| {
            keys.iter()
                .find_map(|key| obj.get(*key).and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|v| !v.is_empty())
        })
    }

    fn object_variants(value: &Value) -> impl Iterator<Item = &serde_json::Map<String, Value>> {
        value
            .as_object()
            .into_iter()
            .chain(value.get("data").and_then(|v| v.as_object()))
    }

    fn getupdates_messages(data: &Value) -> Vec<Value> {
        const ARRAY_KEYS: &[&str] = &["msgs", "msg_list", "message_list", "messages", "updates"];
        for obj in Self::object_variants(data) {
            for key in ARRAY_KEYS {
                if let Some(items) = obj.get(*key).and_then(|v| v.as_array()) {
                    return items.clone();
                }
            }
            if let Some(msg) = obj.get("msg").filter(|v| v.is_object()) {
                return vec![msg.clone()];
            }
        }
        Vec::new()
    }

    pub(crate) async fn remember_sent_echo(store: &SentEchoStore, user_id: &str, text: &str) {
        let text = text.trim();
        if user_id.is_empty() || text.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut echoes = store.lock().await;
        echoes.retain(|_, t| now.duration_since(*t) < SENT_ECHO_TTL);
        echoes.insert(Self::sent_echo_key(user_id, text), now);
    }

    async fn is_sent_echo(store: &SentEchoStore, native: &NativeMessage, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let now = Instant::now();
        let mut echoes = store.lock().await;
        echoes.retain(|_, t| now.duration_since(*t) < SENT_ECHO_TTL);
        let target = Self::native_reply_target(native);
        echoes.contains_key(&Self::sent_echo_key(&target, text))
    }

    /// Outbound send path: use the latest token cached from an inbound user message.
    pub(crate) async fn send_reply(
        client: &ILinkClient,
        reply: &ProcessedReply,
        prefix: &str,
        sent_echoes: Option<&SentEchoStore>,
        context_tokens: Option<&ContextTokenStore>,
    ) -> Result<()> {
        let to_user = reply
            .meta
            .get("reply_to_user_id")
            .or_else(|| reply.meta.get("from_user_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::Other("wechat: missing from_user_id".into()))?;
        let fallback_token = reply
            .meta
            .get("context_token")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ctx_token = match context_tokens {
            Some(store) => Self::resolve_context_token(store, to_user, fallback_token).await,
            None => fallback_token.to_string(),
        };
        if ctx_token.is_empty() {
            return Err(GatewayError::Channel {
                channel: "wechat".into(),
                message: "missing context_token for outbound reply".into(),
            });
        }
        let mut body = reply.text.clone();
        if !prefix.is_empty() {
            body = format!("{prefix}  {body}");
        }
        let chunks = split_text_chars(&body, WECHAT_TEXT_CHUNK_CHARS);
        let chunk_total = chunks.len();
        for (idx, chunk) in chunks.iter().enumerate() {
            if idx > 0 {
                tokio::time::sleep(WECHAT_SEND_INTERVAL).await;
            }
            match client.send_text(to_user, chunk, &ctx_token).await {
                Ok(resp) => {
                    tracing::info!(
                        to_user = %to_user,
                        chunk_no = idx + 1,
                        chunk_total,
                        chunk_len = chunk.len(),
                        response = %resp,
                        "wechat: sent text chunk"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        to_user = %to_user,
                        chunk_no = idx + 1,
                        chunk_total,
                        chunk_len = chunk.len(),
                        error = %e,
                        "wechat: send_text failed"
                    );
                    return Err(e);
                }
            }
            if let Some(store) = sent_echoes {
                Self::remember_sent_echo(store, to_user, chunk).await;
            }
        }
        tracing::info!(
            to_user = %to_user,
            body_len = body.len(),
            chunk_total,
            "wechat: sent reply"
        );
        Ok(())
    }

    pub async fn start(&self, ctx: GatewayContext) -> Result<()> {
        let token = self.resolve_token().await?;
        let token_file = token_file_path(&self.data_dir, &self.account_id);
        let client = Arc::new(ILinkClient::new_with_token_file(
            token,
            self.base_url(),
            token_file,
        ));
        let cfg = self.cfg.clone();
        let account_id = self.account_id.clone();
        let channel_id = self.channel_id.clone();
        let registry = self.registry.clone();
        let data_dir = self.data_dir.clone();
        let cursor_store = self.cursor.clone();
        {
            let mut c = cursor_store.write().await;
            *c = load_cursor_for_account(&data_dir, &account_id);
        }
        let mut shutdown_rx = self.shutdown_rx.clone();
        let dedup_ids = self.dedup_ids.clone();
        let sent_echoes = self.sent_echoes.clone();
        let context_tokens = self.context_tokens.clone();
        let typing_tickets = self.typing_tickets.clone();
        let typing_sessions = self.typing_sessions.clone();
        let poll_state = Arc::new(WeChatPollState {
            cursor: cursor_store.clone(),
            lock: Arc::new(Mutex::new(())),
            data_dir: data_dir.clone(),
            account_id: account_id.clone(),
        });

        tokio::spawn(async move {
            let mut failures: u32 = 0;
            'poll: loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                let data = {
                    let _guard = poll_state.lock.lock().await;
                    let cursor = poll_state.cursor.read().await.clone();
                    tokio::select! {
                        biased;
                        _ = shutdown_rx.changed() => break 'poll,
                        result = client.getupdates(&cursor) => result,
                    }
                };
                match data {
                    Ok(data) => {
                        failures = 0;
                        let msgs = Self::getupdates_messages(&data);
                        if !msgs.is_empty() {
                            tracing::debug!(
                                account_id = %account_id,
                                count = msgs.len(),
                                "wechat: received messages from poll"
                            );
                        } else {
                            tokio::time::sleep(EMPTY_POLL_SLEEP).await;
                        }
                        for msg in msgs {
                            let Some(mut native) =
                                Self::parse_inbound(&msg, &account_id, &channel_id)
                            else {
                                tracing::debug!(
                                    account_id = %account_id,
                                    msg_type = msg.get("message_type").and_then(|v| v.as_i64()),
                                    msg_state = msg.get("message_state").and_then(|v| v.as_i64()),
                                    "wechat: skipped non-text inbound message"
                                );
                                continue;
                            };
                            if let Some(bot_user_id) = native
                                .meta
                                .get("to_user_id")
                                .and_then(|v| v.as_str())
                                .filter(|t| !t.is_empty())
                            {
                                if let Ok(Some(merged_id)) =
                                    registry.upsert_bot_user_id(&account_id, bot_user_id)
                                {
                                    tracing::info!(
                                        account_id = %account_id,
                                        merged_into = %merged_id,
                                        bot_user_id = %bot_user_id,
                                        "wechat: merged duplicate bot account"
                                    );
                                    break 'poll;
                                }
                            }
                            if let Some(token) = native
                                .meta
                                .get("context_token")
                                .and_then(|v| v.as_str())
                                .filter(|t| !t.is_empty())
                            {
                                let reply_target = Self::native_reply_target(&native);
                                Self::remember_context_token(&context_tokens, &reply_target, token)
                                    .await;
                                tracing::debug!(
                                    account_id = %account_id,
                                    user_id = %native.sender_id,
                                    reply_to_user_id = %reply_target,
                                    "wechat: context_token seeded from user message"
                                );
                            }
                            if cfg.inline_images {
                                if !native
                                    .meta
                                    .get("is_group")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(true)
                                    && inline_images(&client, &mut native).await.is_err()
                                {
                                    let target = Self::native_reply_target(&native);
                                    let token = native
                                        .meta
                                        .get("context_token")
                                        .and_then(Value::as_str)
                                        .unwrap_or("");
                                    let _ = client.send_text(&target, "图片处理失败，请重新发送 JPEG、PNG 或 WebP；每条最多 3 张、每张 8 MB、1600 万像素。", token).await;
                                    continue;
                                }
                                // Persist the cursor only after processing and reply delivery.
                                loop {
                                    let stores = WeChatRuntimeStores {
                                        sent_echoes: sent_echoes.clone(),
                                        context_tokens: context_tokens.clone(),
                                        typing_tickets: typing_tickets.clone(),
                                        typing_sessions: typing_sessions.clone(),
                                    };
                                    if process_wechat_message(
                                        &ctx,
                                        client.clone(),
                                        native.clone(),
                                        &cfg.base.bot_prefix,
                                        false,
                                        stores,
                                    )
                                    .await
                                    .is_ok()
                                    {
                                        break;
                                    }
                                    tracing::warn!(
                                        "wechat delivery failed; retrying stable message ID"
                                    );
                                    if *shutdown_rx.borrow() {
                                        break 'poll;
                                    }
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                }
                                continue;
                            }
                            let query = native
                                .content_parts
                                .first()
                                .map(|p| p.text.as_str())
                                .unwrap_or("");
                            if Self::is_sent_echo(&sent_echoes, &native, query).await {
                                tracing::debug!(
                                    account_id = %account_id,
                                    user_id = %native.sender_id,
                                    query_len = query.len(),
                                    "wechat: skipped sent-message echo"
                                );
                                continue;
                            }
                            let dedup_key = native
                                .meta
                                .get("message_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !dedup_key.is_empty() {
                                let mut dq = dedup_ids.lock().await;
                                if dq.iter().any(|k| k == &dedup_key) {
                                    tracing::debug!(
                                        account_id = %account_id,
                                        dedup_key = %dedup_key,
                                        "wechat: skipped duplicate (context_token)"
                                    );
                                    continue;
                                }
                                dq.push_back(dedup_key.clone());
                                while dq.len() > DEDUP_MAX {
                                    dq.pop_front();
                                }
                            }
                            let query = native
                                .content_parts
                                .first()
                                .map(|p| p.text.as_str())
                                .unwrap_or("");
                            let typing_context_token = native
                                .meta
                                .get("context_token")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Some((typing_session_key, ticket)) = Self::start_typing_session(
                                client.clone(),
                                &typing_tickets,
                                &typing_sessions,
                                &Self::native_reply_target(&native),
                                &typing_context_token,
                            )
                            .await
                            {
                                native.meta.insert("typing_ticket".into(), json!(ticket));
                                native
                                    .meta
                                    .insert("typing_session_key".into(), json!(typing_session_key));
                            }
                            let session_id = native
                                .session_id
                                .clone()
                                .unwrap_or_else(|| session_key(&channel_id, &native.sender_id));
                            let priority = message_priority(&native);
                            tracing::info!(
                                account_id = %account_id,
                                user_id = %native.sender_id,
                                session_id = %session_id,
                                query_len = query.len(),
                                query_preview = %preview_text(query, 120),
                                priority,
                                "wechat: enqueue inbound message"
                            );
                            let payload = serde_json::json!({
                                "channel_id": channel_id,
                                "sender_id": native.sender_id,
                                "session_id": session_id,
                                "content_parts": native.content_parts.iter().map(|p| {
                                    let kind = match p.kind {
                                        PartKind::Image => "image",
                                        PartKind::Video => "video",
                                        PartKind::File => "file",
                                        PartKind::Audio => "audio",
                                        _ => "text",
                                    };
                                    let mut obj = serde_json::json!({
                                        "type": kind,
                                        "text": p.text,
                                    });
                                    if !p.url.is_empty() {
                                        match p.kind {
                                            PartKind::Image => obj["image_url"] = serde_json::json!(p.url),
                                            PartKind::Video => obj["video_url"] = serde_json::json!(p.url),
                                            PartKind::File => obj["file_url"] = serde_json::json!(p.url),
                                            PartKind::Audio => obj["audio_url"] = serde_json::json!(p.url),
                                            _ => {}
                                        }
                                    }
                                    if let Some(data) = &p.data {
                                        obj["data"] = data.clone();
                                    }
                                    if !p.extra.is_empty() {
                                        obj["extra"] = serde_json::json!(p.extra);
                                    }
                                    obj
                                }).collect::<Vec<_>>(),
                                "meta": native.meta,
                            });

                            let client2 = client.clone();
                            let stores = WeChatRuntimeStores {
                                sent_echoes: sent_echoes.clone(),
                                context_tokens: context_tokens.clone(),
                                typing_tickets: typing_tickets.clone(),
                                typing_sessions: typing_sessions.clone(),
                            };
                            let prefix = cfg.base.bot_prefix.clone();
                            let ctx_enqueue = ctx.clone();
                            let ctx_worker = ctx.clone();
                            let dequeue_channel_id = channel_id.clone();
                            let consumer = make_consumer(move |_key, mut rx| {
                                let client = client2.clone();
                                let prefix = prefix.clone();
                                let ctx = ctx_worker.clone();
                                let stores = stores.clone();
                                let dequeue_channel_id = dequeue_channel_id.clone();
                                Box::pin(async move {
                                    while let Some(item) = rx.recv().await {
                                        match native_from_json(&dequeue_channel_id, item) {
                                            Ok(native) => {
                                                let query = native
                                                    .content_parts
                                                    .first()
                                                    .map(|p| p.text.as_str())
                                                    .unwrap_or("");
                                                tracing::info!(
                                                    user_id = %native.sender_id,
                                                    query_preview = %preview_text(query, 120),
                                                    "wechat: processing dequeued message"
                                                );
                                                if let Err(e) = process_wechat_message(
                                                    &ctx,
                                                    client.clone(),
                                                    native,
                                                    &prefix,
                                                    cfg.streaming_enabled,
                                                    stores.clone(),
                                                )
                                                .await
                                                {
                                                    tracing::error!("wechat process: {e}");
                                                }
                                            }
                                            Err(e) => tracing::error!("wechat parse: {e}"),
                                        }
                                    }
                                })
                            });

                            ctx_enqueue
                                .queue
                                .enqueue(&channel_id, &session_id, priority, payload, consumer)
                                .await;
                        }
                        Self::apply_getupdates_response(&poll_state, &data).await;
                    }
                    Err(e) => {
                        failures += 1;
                        let backoff = Duration::from_secs(
                            (5u64 * 2u64.pow(failures.saturating_sub(1))).min(120),
                        );
                        tracing::error!(
                            account_id = %account_id,
                            "wechat poll error ({failures}): {e}; retry in {}s",
                            backoff.as_secs()
                        );
                        tokio::select! {
                            biased;
                            _ = shutdown_rx.changed() => break 'poll,
                            _ = tokio::time::sleep(backoff) => {},
                        }
                    }
                }
            }
        });

        tracing::info!(account_id = %self.account_id, "wechat account runner started");
        Ok(())
    }
}

fn preview_text(_s: &str, _max_chars: usize) -> String {
    "[redacted]".into()
}

fn stable_message_id(message: &Value) -> String {
    for key in ["message_id", "msg_id", "msgid", "client_id"] {
        if let Some(value) = message.get(key) {
            if let Some(s) = value.as_str().filter(|s| !s.is_empty()) {
                return s.into();
            }
            if value.is_number() {
                return value.to_string();
            }
        }
    }
    use sha2::{Digest, Sha256};
    format!(
        "sha256:{:x}",
        Sha256::digest(message.to_string().as_bytes())
    )
}

async fn inline_images(client: &ILinkClient, native: &mut NativeMessage) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let invalid = || GatewayError::Other("invalid or oversized image".into());
    if native
        .content_parts
        .iter()
        .filter(|p| p.kind == PartKind::Image)
        .count()
        > 3
    {
        return Err(invalid());
    }
    for part in &mut native.content_parts {
        if part.kind != PartKind::Image {
            continue;
        }
        let item = part
            .data
            .as_ref()
            .and_then(|v| v.get("image_item"))
            .ok_or_else(invalid)?;
        let media = item.get("media").unwrap_or(item);
        let key = media
            .get("aes_key")
            .or_else(|| item.get("aeskey"))
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let query = media
            .get("encrypt_query_param")
            .and_then(Value::as_str)
            .unwrap_or("");
        let url = media.get("url").and_then(Value::as_str).unwrap_or("");
        let bytes = client.download_media(url, key, query).await?;
        let format = image::guess_format(&bytes).map_err(|_| invalid())?;
        let mime = match format {
            image::ImageFormat::Jpeg => "image/jpeg",
            image::ImageFormat::Png => "image/png",
            image::ImageFormat::WebP => "image/webp",
            _ => return Err(invalid()),
        };
        let (w, h) = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
            .into_dimensions()
            .map_err(|_| invalid())?;
        if w > 8192 || h > 8192 || u64::from(w) * u64::from(h) > 16_000_000 {
            return Err(invalid());
        }
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(80 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().map_err(|_| invalid())?;
        part.url = format!("data:{mime};base64,{}", STANDARD.encode(bytes));
        part.text.clear();
        part.data = None;
        part.extra.clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_text_and_images_keep_distinct_platform_ids() {
        let mut first = serde_json::json!({"message_id":1,"text":"确认","context_token":"same"});
        let second = serde_json::json!({"message_id":2,"text":"确认","context_token":"same"});
        assert_ne!(stable_message_id(&first), stable_message_id(&second));
        first = serde_json::json!({"image_item":{"media":"first"},"create_time":1});
        let mut next = first.clone();
        next["create_time"] = serde_json::json!(2);
        assert_eq!(stable_message_id(&first), stable_message_id(&first.clone()));
        assert_ne!(stable_message_id(&first), stable_message_id(&next));
    }
    use crate::platforms::wechat::registry::WeChatAccountRegistry;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TEST_ACCOUNT: &str = "acc_test";
    const TEST_CHANNEL: &str = "wechat:acc_test";

    fn parse(msg: &Value) -> NativeMessage {
        WeChatAccountRunner::parse_inbound(msg, TEST_ACCOUNT, TEST_CHANNEL).expect("native message")
    }

    fn test_runner(dir: &tempfile::TempDir, token: &str) -> WeChatAccountRunner {
        let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let account_id = registry.register_from_login(token, "").unwrap();
        let entry = registry.get(&account_id).unwrap().unwrap();
        WeChatAccountRunner::new(
            account_id.clone(),
            format!("wechat:{account_id}"),
            WeChatConfig::default(),
            entry,
            dir.path().to_path_buf(),
            registry,
        )
        .unwrap()
    }

    fn text_msg(message_state: i64, text: &str) -> Value {
        json!({
            "message_type": 1,
            "message_state": message_state,
            "from_user_id": "wx_user",
            "to_user_id": "wx_bot",
            "context_token": "ctx_1",
            "item_list": [
                { "type": 1, "text_item": { "text": text } }
            ]
        })
    }

    #[test]
    fn parse_inbound_accepts_user_text() {
        let native = parse(&text_msg(1, "hello"));

        assert_eq!(native.channel_id, TEST_CHANNEL);
        assert_eq!(native.sender_id, "wx_user");
        assert_eq!(
            native.session_id.as_deref(),
            Some("wechat:acc_test:dm:wx_user")
        );
        assert_eq!(native.content_parts[0].text, "hello");
        assert_eq!(
            native.meta.get("reply_to_user_id").and_then(|v| v.as_str()),
            Some("wx_user")
        );
    }

    #[test]
    fn parse_inbound_group_uses_group_reply_target() {
        let mut msg = text_msg(1, "hello group");
        msg["group_id"] = json!("group_1");
        let native = parse(&msg);

        assert_eq!(
            native.session_id.as_deref(),
            Some("wechat:acc_test:group:group_1")
        );
        assert_eq!(native.sender_id, "wx_user");
        assert_eq!(
            native.meta.get("reply_to_user_id").and_then(|v| v.as_str()),
            Some("group_1")
        );
        assert_eq!(
            native.meta.get("from_user_id").and_then(|v| v.as_str()),
            Some("wx_user")
        );
    }

    #[test]
    fn parse_inbound_accepts_state_two_user_text() {
        let native = parse(&text_msg(2, "hello from wechat"));

        assert_eq!(native.sender_id, "wx_user");
        assert_eq!(native.content_parts[0].text, "hello from wechat");
    }

    #[test]
    fn parse_inbound_normalizes_null_to_user_id() {
        let mut msg = text_msg(1, "hello");
        msg["to_user_id"] = Value::Null;
        let native = parse(&msg);

        assert_eq!(
            native.meta.get("to_user_id").and_then(|v| v.as_str()),
            Some("")
        );
    }

    #[test]
    fn parse_inbound_uses_bot_user_id_when_to_user_id_is_null() {
        let mut msg = text_msg(1, "hello");
        msg["to_user_id"] = Value::Null;
        msg["bot_user_id"] = json!(" wx_bot ");
        let native = parse(&msg);

        assert_eq!(
            native.meta.get("to_user_id").and_then(|v| v.as_str()),
            Some("wx_bot")
        );
    }

    #[test]
    fn parse_inbound_accepts_image_item() {
        let msg = json!({
            "message_type": 1,
            "from_user_id": "wx_user",
            "to_user_id": "wx_bot",
            "context_token": "ctx_media",
            "item_list": [{
                "type": 2,
                "image_item": {
                    "media": {
                        "encrypt_query_param": "enc",
                        "aes_key": "key",
                        "encrypt_type": 1
                    },
                    "mid_size": 128
                }
            }]
        });

        let native = parse(&msg);

        assert_eq!(native.content_parts[0].kind, PartKind::Image);
        assert_eq!(native.content_parts[0].text, "[WeChat image message]");
        assert!(native.content_parts[0].data.is_some());
    }

    #[test]
    fn parse_inbound_accepts_voice_text_item() {
        let msg = json!({
            "message_type": 1,
            "from_user_id": "wx_user",
            "to_user_id": "wx_bot",
            "context_token": "ctx_voice",
            "item_list": [{
                "type": 3,
                "voice_item": {
                    "text": "下午三点到",
                    "playtime": 1200
                }
            }]
        });

        let native = parse(&msg);

        assert_eq!(native.content_parts[0].kind, PartKind::Audio);
        assert!(native.content_parts[0].text.contains("下午三点到"));
    }

    #[test]
    fn parse_inbound_accepts_file_item() {
        let msg = json!({
            "message_type": 1,
            "from_user_id": "wx_user",
            "to_user_id": "wx_bot",
            "context_token": "ctx_file",
            "item_list": [{
                "type": 4,
                "file_item": {
                    "file_name": "report.pdf",
                    "len": "42"
                }
            }]
        });

        let native = parse(&msg);

        assert_eq!(native.content_parts[0].kind, PartKind::File);
        assert!(native.content_parts[0].text.contains("report.pdf"));
    }

    #[test]
    fn parse_inbound_accepts_video_item() {
        let msg = json!({
            "message_type": 1,
            "from_user_id": "wx_user",
            "to_user_id": "wx_bot",
            "context_token": "ctx_video",
            "item_list": [{
                "type": 5,
                "video_item": {
                    "video_size": 2048,
                    "play_length": 3000
                }
            }]
        });

        let native = parse(&msg);

        assert_eq!(native.content_parts[0].kind, PartKind::Video);
        assert_eq!(native.content_parts[0].text, "[WeChat video message]");
    }

    #[test]
    fn parse_inbound_rejects_empty_sender_send_echo() {
        let mut msg = text_msg(2, "generated reply");
        msg["from_user_id"] = json!("");

        assert!(WeChatAccountRunner::parse_inbound(&msg, TEST_ACCOUNT, TEST_CHANNEL).is_none());
    }

    #[tokio::test]
    async fn sent_echo_store_matches_recent_sent_text_within_ttl() {
        let store = Arc::new(Mutex::new(HashMap::new()));
        WeChatAccountRunner::remember_sent_echo(&store, "wx_user", "generated reply").await;
        let native = parse(&text_msg(2, "generated reply"));

        assert!(WeChatAccountRunner::is_sent_echo(&store, &native, "generated reply").await);
        assert!(WeChatAccountRunner::is_sent_echo(&store, &native, "generated reply").await);
    }

    #[tokio::test]
    async fn resolve_token_loads_from_registry() {
        let dir = tempfile::tempdir().unwrap();
        let runner = test_runner(&dir, "file_token");
        let token = runner.resolve_token().await.unwrap();
        assert_eq!(token, "file_token");
    }

    #[tokio::test]
    async fn user_message_context_token_replaces_cached_token() {
        let store = Arc::new(RwLock::new(HashMap::new()));
        WeChatAccountRunner::remember_context_token(&store, "wx_user", "ctx_old").await;
        WeChatAccountRunner::remember_context_token(&store, "wx_user", "ctx_new").await;

        assert_eq!(
            WeChatAccountRunner::resolve_context_token(&store, "wx_user", "fallback").await,
            "ctx_new"
        );
    }

    #[test]
    fn token_suffix_handles_unicode_safely() {
        assert_eq!(
            WeChatAccountRunner::token_suffix("abcdefghi"),
            "...bcdefghi"
        );
        assert_eq!(
            WeChatAccountRunner::token_suffix("你好世界上下左右中"),
            "...好世界上下左右中"
        );
    }

    #[derive(Default, Clone)]
    struct RecordedRequest {
        path: String,
        body: String,
    }

    async fn recording_post_server(
        responses: Vec<String>,
    ) -> (String, Arc<std::sync::Mutex<Vec<RecordedRequest>>>) {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded2 = recorded.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        tokio::spawn(async move {
            for body in responses {
                let (mut stream, _) = listener.accept().await.expect("accept request");
                let mut buf = vec![0_u8; 16 * 1024];
                let n = stream.read(&mut buf).await.expect("read request");
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                let json_body = req
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body.to_string())
                    .unwrap_or_default();
                recorded2.lock().unwrap().push(RecordedRequest {
                    path,
                    body: json_body,
                });
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
        });
        (format!("http://{addr}"), recorded)
    }

    fn test_reply(text: &str, context_token: &str) -> ProcessedReply {
        let mut meta = HashMap::new();
        meta.insert("from_user_id".into(), json!("wx_user"));
        meta.insert("context_token".into(), json!(context_token));
        ProcessedReply {
            text: text.into(),
            parts: vec![],
            meta,
            session_id: "wechat:wx_user".into(),
            user_id: "wx_user".into(),
        }
    }

    fn context_token_from_send_body(body: &str) -> String {
        let value: Value = serde_json::from_str(body).expect("sendmessage json body");
        value["msg"]["context_token"]
            .as_str()
            .expect("context_token in sendmessage body")
            .to_string()
    }

    fn to_user_from_send_body(body: &str) -> String {
        let value: Value = serde_json::from_str(body).expect("sendmessage json body");
        value["msg"]["to_user_id"]
            .as_str()
            .expect("to_user_id in sendmessage body")
            .to_string()
    }

    #[tokio::test]
    async fn send_typing_start_and_cleanup_use_getconfig_ticket() {
        let (base_url, recorded) = recording_post_server(vec![
            json!({"ret": 0, "typing_ticket": "ticket_1"}).to_string(),
            json!({"ret": 0}).to_string(),
            json!({"ret": 0}).to_string(),
        ])
        .await;
        let client = ILinkClient::new("token", base_url);
        let tickets = Arc::new(RwLock::new(HashMap::new()));

        let ticket =
            WeChatAccountRunner::send_typing_start(&client, &tickets, "wx_user", "ctx_1").await;
        WeChatAccountRunner::send_typing_cleanup(&client, &tickets, "wx_user", ticket.as_deref())
            .await;

        assert_eq!(ticket.as_deref(), Some("ticket_1"));
        let reqs = recorded.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert!(reqs[0].path.contains("getconfig"));
        assert!(reqs[1].path.contains("sendtyping"));
        assert!(reqs[2].path.contains("sendtyping"));

        let getconfig: Value = serde_json::from_str(&reqs[0].body).expect("getconfig body");
        assert_eq!(getconfig["ilink_user_id"], "wx_user");
        assert_eq!(getconfig["context_token"], "ctx_1");

        let typing_start: Value = serde_json::from_str(&reqs[1].body).expect("typing start body");
        assert_eq!(typing_start["typing_ticket"], "ticket_1");
        assert_eq!(typing_start["status"], WECHAT_TYPING_STATUS_TYPING);

        let typing_cleanup: Value =
            serde_json::from_str(&reqs[2].body).expect("typing cleanup body");
        assert_eq!(typing_cleanup["typing_ticket"], "ticket_1");
        assert_eq!(typing_cleanup["status"], WECHAT_TYPING_STATUS_CLEANUP);
    }

    #[tokio::test]
    async fn send_typing_start_reuses_cached_ticket() {
        let (base_url, recorded) = recording_post_server(vec![
            json!({"ret": 0, "typing_ticket": "ticket_1"}).to_string(),
            json!({"ret": 0}).to_string(),
            json!({"ret": 0}).to_string(),
        ])
        .await;
        let client = ILinkClient::new("token", base_url);
        let tickets = Arc::new(RwLock::new(HashMap::new()));

        let first =
            WeChatAccountRunner::send_typing_start(&client, &tickets, "wx_user", "ctx_1").await;
        let second =
            WeChatAccountRunner::send_typing_start(&client, &tickets, "wx_user", "ctx_2").await;

        assert_eq!(first.as_deref(), Some("ticket_1"));
        assert_eq!(second.as_deref(), Some("ticket_1"));
        let reqs = recorded.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert!(reqs[0].path.contains("getconfig"));
        assert!(reqs[1].path.contains("sendtyping"));
        assert!(reqs[2].path.contains("sendtyping"));
    }

    #[tokio::test]
    async fn start_typing_session_registers_keepalive_until_finish() {
        let (base_url, recorded) = recording_post_server(vec![
            json!({"ret": 0, "typing_ticket": "ticket_1"}).to_string(),
            json!({"ret": 0}).to_string(),
            json!({"ret": 0}).to_string(),
        ])
        .await;
        let client = Arc::new(ILinkClient::new("token", base_url));
        let tickets = Arc::new(RwLock::new(HashMap::new()));
        let sessions = Arc::new(Mutex::new(HashMap::new()));

        let (session_key, ticket) = WeChatAccountRunner::start_typing_session(
            client.clone(),
            &tickets,
            &sessions,
            "wx_user",
            "ctx_1",
        )
        .await
        .expect("typing session");

        assert_eq!(ticket, "ticket_1");
        assert!(sessions.lock().await.contains_key(&session_key));
        WeChatAccountRunner::finish_typing_session(
            &client,
            &tickets,
            &sessions,
            Some(&session_key),
            "wx_user",
            Some(&ticket),
        )
        .await;

        assert!(!sessions.lock().await.contains_key(&session_key));
        let reqs = recorded.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert!(reqs[0].path.contains("getconfig"));
        assert!(reqs[1].path.contains("sendtyping"));
        assert!(reqs[2].path.contains("sendtyping"));
    }

    #[tokio::test]
    async fn sendtyping_cleanup_failure_invalidates_cached_ticket() {
        let (base_url, _recorded) =
            recording_post_server(vec![json!({"ret": -2, "errmsg": "bad params"}).to_string()])
                .await;
        let client = ILinkClient::new("token", base_url);
        let tickets = Arc::new(RwLock::new(HashMap::new()));
        WeChatAccountRunner::remember_typing_ticket(&tickets, "wx_user", "ticket_1").await;

        WeChatAccountRunner::send_typing_cleanup(&client, &tickets, "wx_user", Some("ticket_1"))
            .await;

        assert!(
            WeChatAccountRunner::cached_typing_ticket(&tickets, "wx_user")
                .await
                .is_none()
        );
        let entry = tickets.read().await.get("wx_user").cloned().unwrap();
        assert!(entry.ticket.is_empty());
        assert!(entry.refresh_after > Instant::now());
    }

    #[tokio::test]
    async fn send_reply_never_calls_getupdates() {
        let (base_url, recorded) = recording_post_server(vec![json!({}).to_string()]).await;
        let client = ILinkClient::new("token", base_url);
        let store = Arc::new(RwLock::new(HashMap::new()));
        WeChatAccountRunner::remember_context_token(&store, "wx_user", "token_a").await;
        let store2 = store.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            WeChatAccountRunner::remember_context_token(&store2, "wx_user", "token_b").await;
        });

        let start = Instant::now();
        WeChatAccountRunner::send_reply(
            &client,
            &test_reply("segment A", "token_a"),
            "",
            None,
            Some(&store),
        )
        .await
        .expect("send_reply");

        assert!(
            start.elapsed() < Duration::from_millis(500),
            "single-chunk non-stream send should not wait for background token refresh"
        );
        let reqs = recorded.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].path.contains("sendmessage"));
        assert!(!reqs[0].path.contains("getupdates"));
    }

    #[tokio::test]
    async fn send_reply_does_not_retry_or_replace_token_on_send_error() {
        let (base_url, recorded) = recording_post_server(vec![
            json!({"ret": -2, "errmsg": "bad params"}).to_string(),
            json!({}).to_string(),
        ])
        .await;
        let client = ILinkClient::new("token", base_url);
        let store = Arc::new(RwLock::new(HashMap::new()));
        WeChatAccountRunner::remember_context_token(&store, "wx_user", "token_cached").await;

        let err = WeChatAccountRunner::send_reply(
            &client,
            &test_reply("segment A", "token_fallback"),
            "",
            None,
            Some(&store),
        )
        .await
        .expect_err("send_reply should propagate first send error");

        assert!(err.to_string().contains("-2"));
        assert_eq!(
            WeChatAccountRunner::resolve_context_token(&store, "wx_user", "fallback").await,
            "token_cached"
        );
        let reqs = recorded.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(context_token_from_send_body(&reqs[0].body), "token_cached");
    }

    #[tokio::test]
    async fn send_reply_uses_reply_target_for_group() {
        let (base_url, recorded) = recording_post_server(vec![json!({}).to_string()]).await;
        let client = ILinkClient::new("token", base_url);
        let store = Arc::new(RwLock::new(HashMap::new()));
        WeChatAccountRunner::remember_context_token(&store, "group_1", "group_token").await;
        let mut reply = test_reply("group response", "fallback_token");
        reply
            .meta
            .insert("reply_to_user_id".into(), json!("group_1"));

        WeChatAccountRunner::send_reply(&client, &reply, "", None, Some(&store))
            .await
            .expect("send_reply");

        let reqs = recorded.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(to_user_from_send_body(&reqs[0].body), "group_1");
        assert_eq!(context_token_from_send_body(&reqs[0].body), "group_token");
    }

    #[tokio::test]
    async fn consecutive_send_reply_uses_token_refreshed_by_user_message() {
        let (base_url, recorded) =
            recording_post_server(vec![json!({}).to_string(), json!({}).to_string()]).await;
        let client = ILinkClient::new("token", base_url);
        let store = Arc::new(RwLock::new(HashMap::new()));
        WeChatAccountRunner::remember_context_token(&store, "wx_user", "token_a").await;

        WeChatAccountRunner::send_reply(
            &client,
            &test_reply("segment A", "token_a"),
            "",
            None,
            Some(&store),
        )
        .await
        .expect("send A");

        WeChatAccountRunner::remember_context_token(&store, "wx_user", "token_b").await;
        WeChatAccountRunner::send_reply(
            &client,
            &test_reply("segment B", "token_a"),
            "",
            None,
            Some(&store),
        )
        .await
        .expect("send B");

        let reqs = recorded.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(context_token_from_send_body(&reqs[0].body), "token_a");
        assert_eq!(context_token_from_send_body(&reqs[1].body), "token_b");
        assert!(reqs.iter().all(|r| !r.path.contains("getupdates")));
    }

    #[tokio::test]
    async fn apply_getupdates_batch_leaves_user_messages_enqueueable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let poll = WeChatPollState {
            cursor: Arc::new(RwLock::new(String::new())),
            lock: Arc::new(Mutex::new(())),
            data_dir: dir.path().to_path_buf(),
            account_id: TEST_ACCOUNT.to_string(),
        };
        let data = json!({
            "get_updates_buf": "cursor_next",
            "msgs": [
                {
                    "message_type": 2,
                    "to_user_id": "wx_user",
                    "context_token": "token_b",
                    "item_list": [{ "type": 1, "text_item": { "text": "bot echo" } }]
                },
                {
                    "message_type": 1,
                    "message_state": 2,
                    "from_user_id": "wx_user",
                    "to_user_id": "wx_bot",
                    "context_token": "token_user",
                    "item_list": [{ "type": 1, "text_item": { "text": "user follow-up" } }]
                }
            ]
        });

        WeChatAccountRunner::apply_getupdates_response(&poll, &data).await;

        assert_eq!(poll.cursor.read().await.as_str(), "cursor_next");
        let user_msg = data["msgs"][1].clone();
        let native = parse(&user_msg);
        assert_eq!(native.content_parts[0].text, "user follow-up");
        assert_eq!(
            native.meta.get("context_token").and_then(|v| v.as_str()),
            Some("token_user")
        );
    }

    #[tokio::test]
    async fn getupdates_accepts_nested_message_list_aliases() {
        let dir = tempfile::tempdir().expect("tempdir");
        let poll = WeChatPollState {
            cursor: Arc::new(RwLock::new(String::new())),
            lock: Arc::new(Mutex::new(())),
            data_dir: dir.path().to_path_buf(),
            account_id: TEST_ACCOUNT.to_string(),
        };
        let data = json!({
            "data": {
                "next_get_updates_buf": "cursor_next",
                "msg_list": [{
                    "message_type": 1,
                    "message_state": 2,
                    "from_user_id": "wx_user",
                    "bot_user_id": "wx_bot",
                    "context_token": "token_user",
                    "item_list": [{ "type": 1, "text_item": { "text": "nested user msg" } }]
                }]
            }
        });

        WeChatAccountRunner::apply_getupdates_response(&poll, &data).await;
        let msgs = WeChatAccountRunner::getupdates_messages(&data);

        assert_eq!(poll.cursor.read().await.as_str(), "cursor_next");
        assert_eq!(msgs.len(), 1);
        let native = parse(&msgs[0]);
        assert_eq!(native.content_parts[0].text, "nested user msg");
        assert_eq!(
            native.meta.get("to_user_id").and_then(|v| v.as_str()),
            Some("wx_bot")
        );
    }
}
