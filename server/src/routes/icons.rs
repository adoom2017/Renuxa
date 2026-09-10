use crate::{AppState, error::ApiError, models::IconCandidate};
use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::header,
    response::Response,
    routing::get,
};
use serde::Deserialize;
use serde_json::Value;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/icons/search", get(search_icons))
        .route("/icons/image", get(icon_image))
}

#[derive(Deserialize)]
struct IconQuery {
    q: String,
    country: Option<String>,
}

#[derive(Deserialize)]
struct IconImageQuery {
    url: String,
}

async fn icon_image(
    State(state): State<AppState>,
    Query(query): Query<IconImageQuery>,
) -> Result<Response, ApiError> {
    let url =
        reqwest::Url::parse(&query.url).map_err(|_| ApiError::Validation("图标地址无效".into()))?;
    if !is_trusted_icon_url(&url) {
        return Err(ApiError::Validation("仅支持 App Store 图标地址".into()));
    }

    let upstream = state
        .http
        .get(url)
        .send()
        .await
        .map_err(|_| ApiError::Upstream)?;
    if !upstream.status().is_success() {
        return Err(ApiError::Upstream);
    }
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .ok_or(ApiError::Upstream)?;
    if !content_type
        .to_str()
        .is_ok_and(|value| value.starts_with("image/"))
    {
        return Err(ApiError::Upstream);
    }
    let bytes = upstream.bytes().await.map_err(|_| ApiError::Upstream)?;
    if bytes.len() > 5 * 1024 * 1024 {
        return Err(ApiError::Upstream);
    }

    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=604800, immutable")
        .body(Body::from(bytes))
        .map_err(|_| ApiError::Internal)
}

fn is_trusted_icon_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host == "mzstatic.com" || host.ends_with(".mzstatic.com"))
}

async fn search_icons(
    State(state): State<AppState>,
    Query(query): Query<IconQuery>,
) -> Result<Json<Vec<IconCandidate>>, ApiError> {
    if query.q.trim().is_empty() {
        return Ok(Json(vec![]));
    }
    let response: Value = state
        .http
        .get("https://itunes.apple.com/search")
        .query(&[
            ("term", query.q.as_str()),
            ("entity", "software"),
            ("limit", "8"),
            ("country", query.country.as_deref().unwrap_or("cn")),
        ])
        .send()
        .await
        .map_err(|_| ApiError::Upstream)?
        .json()
        .await
        .map_err(|_| ApiError::Upstream)?;
    let results = response["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(IconCandidate {
                name: v["trackName"].as_str()?.into(),
                developer: v["sellerName"].as_str().unwrap_or_default().into(),
                icon_url: v["artworkUrl512"]
                    .as_str()
                    .or_else(|| v["artworkUrl100"].as_str())?
                    .into(),
                store_url: v["trackViewUrl"].as_str().unwrap_or_default().into(),
                bundle_id: v["bundleId"].as_str().unwrap_or_default().into(),
            })
        })
        .collect();
    Ok(Json(results))
}

#[cfg(test)]
mod tests {
    use super::is_trusted_icon_url;

    #[test]
    fn only_allows_https_mzstatic_hosts() {
        assert!(is_trusted_icon_url(
            &reqwest::Url::parse("https://is1-ssl.mzstatic.com/image/thumb/icon.png").unwrap()
        ));
        assert!(!is_trusted_icon_url(
            &reqwest::Url::parse("http://is1-ssl.mzstatic.com/image/thumb/icon.png").unwrap()
        ));
        assert!(!is_trusted_icon_url(
            &reqwest::Url::parse("https://mzstatic.com.example.org/icon.png").unwrap()
        ));
    }
}
