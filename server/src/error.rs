use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("未授权")]
    Unauthorized,
    #[error("请求内容无效: {0}")]
    Validation(String),
    #[error("记录不存在")]
    NotFound,
    #[error("数据库操作失败")]
    Database(#[from] sqlx::Error),
    #[error("外部服务暂不可用")]
    Upstream,
    #[error("内部服务错误")]
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Upstream => StatusCode::BAD_GATEWAY,
            Self::Database(_) | Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(json!({ "error": { "code": status.as_u16(), "message": self.to_string() } })),
        )
            .into_response()
    }
}
