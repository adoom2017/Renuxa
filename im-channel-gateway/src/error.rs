use thiserror::Error;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("Agent API error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("SSE stream ended without a completed message")]
    NoCompletedMessage,
    #[error("channel error [{channel}]: {message}")]
    Channel { channel: String, message: String },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, GatewayError>;
