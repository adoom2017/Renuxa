use crate::{AppState, error::ApiError};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{extract::FromRequestParts, http::request::Parts};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: Uuid,
    exp: usize,
    iat: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct CurrentUser(pub Uuid);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        let claims = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| ApiError::Unauthorized)?
        .claims;
        Ok(Self(claims.sub))
    }
}

pub fn issue_token(user_id: Uuid, state: &AppState) -> Result<String, ApiError> {
    let now = Utc::now();
    let claims = Claims {
        sub: user_id,
        iat: now.timestamp() as usize,
        exp: (now + Duration::minutes(30)).timestamp() as usize,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|_| ApiError::Internal)
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}
