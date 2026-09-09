use crate::error::{GatewayError, Result};
use futures::Stream;
use futures::StreamExt;
use reqwest::Response;
use serde_json::Value;

pub fn sse_json_stream(response: Response) -> impl Stream<Item = Result<Value>> + Send {
    async_stream::stream! {
        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    yield Err(GatewayError::Http(e));
                    return;
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(idx) = buffer.find('\n') {
                let line = buffer[..idx].to_string();
                buffer.drain(..=idx);
                if let Some(v) = parse_sse_line(&line) {
                    yield v;
                }
            }
        }

        for line in buffer.lines() {
            if let Some(v) = parse_sse_line(line) {
                yield v;
            }
        }
    }
}

fn parse_sse_line(line: &str) -> Option<Result<Value>> {
    let line = line.trim();
    let json_str = line.strip_prefix("data:")?.trim();
    if json_str.is_empty() || json_str == "[DONE]" {
        return None;
    }
    Some(serde_json::from_str(json_str).map_err(GatewayError::Json))
}
