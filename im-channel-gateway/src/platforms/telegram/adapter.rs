use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::Message;
use teloxide::update_listeners;
use tokio::sync::watch;

use crate::config::TelegramConfig;
use crate::context::GatewayContext;
use crate::error::{GatewayError, Result};
use crate::pipeline::{message_priority, native_from_json};
use crate::platforms::telegram::debounce::{apply_debounce, MediaDebounce};
use crate::platforms::telegram::inbound::{
    build_native_from_message, check_group_mention, fetch_bot_identity,
};
use crate::platforms::telegram::polling::{
    reset_reconnect_counters, ListenerErrorHandler, PollingReconnectState, ReconnectCounters,
    RECONNECT_FACTOR, RECONNECT_INITIAL_S, RECONNECT_MAX_S,
};
use crate::platforms::telegram::registry::TelegramAccountRegistry;
use crate::platforms::telegram::stream::process_telegram_message;
use crate::queue::{make_consumer, session_key};

pub struct TelegramAccountRunner {
    account_id: String,
    channel_id: String,
    cfg: TelegramConfig,
    media_dir: PathBuf,
    registry: Arc<TelegramAccountRegistry>,
    shutdown: Arc<watch::Sender<bool>>,
    shutdown_rx: watch::Receiver<bool>,
}

pub enum TelegramRunnerStart {
    Started,
    Merged(String),
}

impl TelegramAccountRunner {
    pub fn new(
        account_id: String,
        channel_id: String,
        cfg: TelegramConfig,
        media_dir: PathBuf,
        registry: Arc<TelegramAccountRegistry>,
    ) -> Result<Self> {
        let (tx, rx) = watch::channel(false);
        Ok(Self {
            account_id,
            channel_id,
            cfg,
            media_dir,
            registry,
            shutdown: Arc::new(tx),
            shutdown_rx: rx,
        })
    }

    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
    }

    fn build_bot(cfg: &TelegramConfig, token: &str) -> Result<Bot> {
        if token.trim().is_empty() {
            return Err(GatewayError::Config(
                "telegram account bot_token is required".into(),
            ));
        }
        if !cfg.http_proxy.is_empty() {
            tracing::info!(
                "telegram http_proxy: setting HTTPS_PROXY={}",
                cfg.http_proxy
            );
            std::env::set_var("HTTPS_PROXY", &cfg.http_proxy);
            if !cfg.http_proxy_auth.is_empty() {
                tracing::warn!("http_proxy_auth: embed credentials in proxy URL if required");
            }
        }
        Ok(Bot::new(token.trim().to_string()))
    }

    pub async fn start(&self, ctx: GatewayContext) -> Result<TelegramRunnerStart> {
        let token = self.registry.load_token(&self.account_id)?;
        let bot = Self::build_bot(&self.cfg, &token)?;
        let identity = fetch_bot_identity(&bot)
            .await
            .map_err(|e| GatewayError::Channel {
                channel: "telegram".into(),
                message: e.to_string(),
            })?;
        if let Some(existing_id) = self.registry.upsert_bot_identity(
            &self.account_id,
            &identity.id.to_string(),
            &identity.username,
        )? {
            tracing::info!(
                account_id = %self.account_id,
                merged_into = %existing_id,
                bot_user_id = %identity.id,
                username = %identity.username,
                "telegram: merged duplicate bot account"
            );
            return Ok(TelegramRunnerStart::Merged(existing_id));
        }
        let identity = Arc::new(identity);

        let cfg = self.cfg.clone();
        let media_dir = self.media_dir.clone();
        let mut shutdown_rx = self.shutdown_rx.clone();
        let account_id = self.account_id.clone();
        let channel_id = self.channel_id.clone();

        tokio::spawn(async move {
            let debounce = Arc::new(MediaDebounce::default());
            let mut reconnect_counters = ReconnectCounters::default();
            let mut fallback_delay = RECONNECT_INITIAL_S;

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }

                let reconnect_state = PollingReconnectState::new(reconnect_counters.clone());
                let listener_eh = Arc::new(ListenerErrorHandler::new(reconnect_state.clone()));

                let handler = Update::filter_message().endpoint({
                    let ctx = ctx.clone();
                    let cfg = cfg.clone();
                    let media_dir = media_dir.clone();
                    let identity = identity.clone();
                    let debounce = debounce.clone();
                    let account_id = account_id.clone();
                    let channel_id = channel_id.clone();
                    move |bot: Bot, msg: Message| {
                        let ctx = ctx.clone();
                        let cfg = cfg.clone();
                        let media_dir = media_dir.clone();
                        let identity = identity.clone();
                        let debounce = debounce.clone();
                        let account_id = account_id.clone();
                        let channel_id = channel_id.clone();
                        async move {
                            let mut native = match build_native_from_message(
                                &bot,
                                &msg,
                                &media_dir,
                                &identity,
                                &account_id,
                                &channel_id,
                            )
                            .await
                            {
                                Some(n) => n,
                                None => return Ok::<(), teloxide::RequestError>(()),
                            };

                            let is_group = native
                                .meta
                                .get("is_group")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if !check_group_mention(cfg.require_mention, is_group, &native.meta) {
                                return Ok(());
                            }

                            let session_id = native
                                .session_id
                                .clone()
                                .unwrap_or_else(|| session_key(&channel_id, &native.sender_id));

                            let merged =
                                match apply_debounce(&debounce, &session_id, &native.content_parts)
                                {
                                    Some(parts) => parts,
                                    None => return Ok(()),
                                };
                            native.content_parts = merged;

                            let priority = message_priority(&native);
                            let payload = serde_json::json!({
                                "channel_id": channel_id.clone(),
                                "sender_id": native.sender_id,
                                "session_id": session_id,
                                "content_parts": native.content_parts.iter().map(|p| {
                                    let kind = match p.kind {
                                        crate::types::PartKind::Image => "image",
                                        crate::types::PartKind::Video => "video",
                                        crate::types::PartKind::File => "file",
                                        crate::types::PartKind::Audio => "audio",
                                        _ => "text",
                                    };
                                    let mut obj = serde_json::json!({"type": kind, "text": p.text});
                                    if !p.url.is_empty() {
                                        match p.kind {
                                            crate::types::PartKind::Image => {
                                                obj["image_url"] = serde_json::json!(p.url);
                                            }
                                            crate::types::PartKind::Video => {
                                                obj["video_url"] = serde_json::json!(p.url);
                                            }
                                            crate::types::PartKind::File => {
                                                obj["file_url"] = serde_json::json!(p.url);
                                            }
                                            crate::types::PartKind::Audio => {
                                                obj["audio_url"] = serde_json::json!(p.url);
                                            }
                                            _ => {}
                                        }
                                    }
                                    obj
                                }).collect::<Vec<_>>(),
                                "meta": native.meta,
                            });

                            let bot2 = bot.clone();
                            let prefix = cfg.base.bot_prefix.clone();
                            let streaming = cfg.streaming_enabled;
                            let filter_tool = cfg.base.filter_tool_messages;
                            let filter_thinking = cfg.base.filter_thinking;
                            let show_typing = cfg.show_typing;
                            let chat_id = msg.chat.id;
                            let dequeue_channel_id = channel_id.clone();

                            let ctx_enqueue = ctx.clone();
                            let ctx_worker = ctx.clone();
                            let consumer = make_consumer(move |_key, mut rx| {
                                let bot = bot2.clone();
                                let prefix = prefix.clone();
                                let ctx = ctx_worker.clone();
                                let dequeue_channel_id = dequeue_channel_id.clone();
                                Box::pin(async move {
                                    while let Some(item) = rx.recv().await {
                                        match native_from_json(&dequeue_channel_id, item) {
                                            Ok(native) => {
                                                if show_typing {
                                                    let _ = bot
                                                        .send_chat_action(
                                                            chat_id,
                                                            teloxide::types::ChatAction::Typing,
                                                        )
                                                        .await;
                                                }
                                                if let Err(e) = process_telegram_message(
                                                    &ctx,
                                                    bot.clone(),
                                                    native,
                                                    &prefix,
                                                    streaming,
                                                    filter_tool,
                                                    filter_thinking,
                                                )
                                                .await
                                                {
                                                    tracing::error!("telegram process: {e}");
                                                }
                                            }
                                            Err(e) => tracing::error!("telegram parse: {e}"),
                                        }
                                    }
                                })
                            });

                            ctx_enqueue
                                .queue
                                .enqueue(&channel_id, &session_id, priority, payload, consumer)
                                .await;

                            if show_typing {
                                let _ = bot
                                    .send_chat_action(chat_id, teloxide::types::ChatAction::Typing)
                                    .await;
                            }
                            Ok(())
                        }
                    }
                });

                let mut dispatcher = Dispatcher::builder(bot.clone(), handler).build();
                let listener = update_listeners::polling_default(bot.clone()).await;

                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            let _ = dispatcher.shutdown_token().shutdown();
                            break;
                        }
                    }
                    () = dispatcher.dispatch_with_listener(listener, listener_eh) => {}
                }

                if *shutdown_rx.borrow() {
                    break;
                }

                reconnect_counters = reconnect_state.snapshot_counters();
                let had_listener_error = reconnect_state.last_error.lock().unwrap().is_some();
                let delay = reconnect_state.next_delay(fallback_delay);
                tracing::info!("telegram: reconnecting in {delay:.1}s");
                tokio::time::sleep(Duration::from_secs_f64(delay)).await;

                if !had_listener_error {
                    reset_reconnect_counters(&mut reconnect_counters);
                    fallback_delay = RECONNECT_INITIAL_S;
                } else {
                    fallback_delay = (fallback_delay * RECONNECT_FACTOR).min(RECONNECT_MAX_S);
                }
            }
        });

        tracing::info!(
            account_id = %self.account_id,
            channel_id = %self.channel_id,
            streaming = self.cfg.streaming_enabled,
            "telegram account runner started (html, reconnect, debounce)"
        );
        Ok(TelegramRunnerStart::Started)
    }
}
