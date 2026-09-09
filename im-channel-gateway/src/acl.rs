use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Default, Serialize, Deserialize)]
struct ChannelAcl {
    #[serde(default)]
    whitelist: HashMap<String, String>,
    #[serde(default)]
    blacklist: HashMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AclFile {
    #[serde(default)]
    channels: HashMap<String, ChannelAcl>,
}

pub struct AccessControl {
    path: std::path::PathBuf,
    data: RwLock<AclFile>,
}

impl AccessControl {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("access_control.json");
        let data = if path.is_file() {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            AclFile::default()
        };
        Ok(Self {
            path,
            data: RwLock::new(data),
        })
    }

    /// Whitelist gate when ACL enabled on channel config (matches Python gate).
    pub fn check_blocked_with_flags(
        &self,
        channel: &str,
        sender_id: &str,
        dm_acl: bool,
        group_acl: bool,
        is_group: bool,
    ) -> bool {
        if is_group && !group_acl {
            return false;
        }
        if !is_group && !dm_acl {
            return false;
        }
        let guard = self.data.read().unwrap();
        let Some(ch) = guard.channels.get(channel).or_else(|| {
            channel
                .split_once(':')
                .and_then(|(base, _)| guard.channels.get(base))
        }) else {
            return false;
        };
        if ch.blacklist.contains_key(sender_id) {
            return true;
        }
        !ch.whitelist.contains_key(sender_id)
    }

    pub fn deny_message(&self, language: &str, sender_id: &str) -> String {
        if language.starts_with("zh") {
            format!("您目前没有访问此智能体的权限，需要审批。\nID: {sender_id}")
        } else {
            format!("You do not have access to this agent. Approval required.\nID: {sender_id}")
        }
    }

    pub fn save(&self) -> Result<()> {
        let guard = self.data.read().unwrap();
        let raw = serde_json::to_string_pretty(&*guard)?;
        std::fs::write(&self.path, raw)?;
        Ok(())
    }
}
