mod convert;
mod sse;

pub use convert::{agent_event_from_runtime_json, build_process_request_body};

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use std::time::Duration;

use crate::agent::http_sse::sse::sse_json_stream;
use crate::config::AgentConfig;
use crate::error::{GatewayError, Result};
use crate::gateway::{AgentBackend, AgentEventStream};
use crate::types::ChannelRequest;

#[derive(Clone)]
pub struct HttpSseBackend {
    base_url: String,
    path: String,
    agent_id: String,
    bearer_token: Option<String>,
    client: reqwest::Client,
}

impl HttpSseBackend {
    pub fn from_config(cfg: &AgentConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1800))
            .build()?;
        let bearer = if cfg.bearer_token.is_empty() {
            None
        } else {
            Some(cfg.bearer_token.clone())
        };
        Ok(Self {
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            path: cfg.path.clone(),
            agent_id: cfg.agent_id.clone(),
            bearer_token: bearer,
            client,
        })
    }

    fn process_url(&self) -> String {
        let path = if self.path.starts_with('/') {
            self.path.clone()
        } else {
            format!("/{}", self.path)
        };
        format!("{}{}", self.base_url, path)
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "X-Agent-Id",
            HeaderValue::from_str(&self.agent_id)
                .map_err(|e| GatewayError::Other(e.to_string()))?,
        );
        if let Some(token) = &self.bearer_token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| GatewayError::Other(e.to_string()))?,
            );
        }
        Ok(headers)
    }
}

#[async_trait]
impl AgentBackend for HttpSseBackend {
    async fn stream(&self, request: ChannelRequest) -> AgentEventStream {
        let this = self.clone();
        Box::pin(async_stream::stream! {
            let body = build_process_request_body(&request);
            let response = match this
                .client
                .post(this.process_url())
                .headers(match this.headers() {
                    Ok(h) => h,
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                })
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield Err(GatewayError::Http(e));
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status().as_u16();
                if this.path == "/api/integrations/wechat/process" && matches!(status, 413 | 422) {
                    yield Ok(crate::types::AgentEvent::assistant_text("消息无法处理。仅支持文字及 JPEG、PNG、WebP 图片；每条最多 3 张、单图最多 8 MB。请检查后重新发送。"));
                    yield Ok(crate::types::AgentEvent::response_completed());
                    return;
                }
                let body = response.text().await.unwrap_or_default();
                yield Err(GatewayError::Api { status, body });
                return;
            }

            let mut json_stream = Box::pin(sse_json_stream(response));
            while let Some(item) = json_stream.next().await {
                match item {
                    Ok(raw) => {
                        if let Some(ev) = agent_event_from_runtime_json(&raw) {
                            yield Ok(ev);
                        }
                    }
                    Err(e) => yield Err(e),
                }
            }
        })
    }
}
