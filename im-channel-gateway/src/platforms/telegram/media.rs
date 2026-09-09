use std::path::{Path, PathBuf};

use teloxide::net::Download;
use teloxide::prelude::*;
use uuid::Uuid;

use crate::error::{GatewayError, Result};

pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Download a Telegram file to `media_dir`; return `file://` URI for agent input.
pub async fn download_telegram_file(
    bot: &Bot,
    file_id: &str,
    media_dir: &Path,
    filename_hint: &str,
) -> Result<Option<String>> {
    let tg_file = bot
        .get_file(file_id)
        .await
        .map_err(|e| GatewayError::Channel {
            channel: "telegram".into(),
            message: format!("get_file: {e}"),
        })?;
    let file_path = tg_file.path.trim();
    if file_path.is_empty() {
        return Ok(None);
    }

    std::fs::create_dir_all(media_dir)?;
    let mut suffix = Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{s}"))
        .unwrap_or_default();
    if suffix.is_empty() && !filename_hint.is_empty() {
        suffix = Path::new(filename_hint)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{s}"))
            .unwrap_or_else(|| ".bin".to_string());
    }
    if suffix.is_empty() {
        suffix = ".bin".to_string();
    }
    let local_name = format!("{}{}", &Uuid::new_v4().simple().to_string()[..12], suffix);
    let local_path = media_dir.join(local_name);

    let mut dest = tokio::fs::File::create(&local_path).await?;
    bot.download_file(file_path, &mut dest)
        .await
        .map_err(|e| GatewayError::Channel {
            channel: "telegram".into(),
            message: format!("download_file: {e}"),
        })?;

    let uri = local_path.canonicalize().unwrap_or(local_path);
    Ok(Some(format!("file://{}", uri.display())))
}

pub fn local_path_from_url(url: &str) -> Option<PathBuf> {
    let raw = url.strip_prefix("file://")?;
    Some(PathBuf::from(raw))
}

pub fn check_file_size(path: &Path) -> Result<u64> {
    let meta = std::fs::metadata(path).map_err(GatewayError::Io)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(GatewayError::Channel {
            channel: "telegram".into(),
            message: format!(
                "file too large ({} MB, limit 50 MB)",
                meta.len() as f64 / (1024.0 * 1024.0)
            ),
        });
    }
    Ok(meta.len())
}
