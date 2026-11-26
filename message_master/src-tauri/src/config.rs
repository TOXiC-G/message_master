use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub drive_folder_input: String,
    #[serde(default)]
    pub drive_folder_name: String,
    #[serde(default)]
    pub local_output_dir: String,
    #[serde(default = "default_save_mode")]
    pub save_mode: String,
    #[serde(default = "default_save_mode")]
    pub drive_save_mode: String,
    #[serde(default)]
    pub upload_directly: bool,
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub refresh_token: Option<String>,
    pub max_size_mb: f64,
    pub target_lufs: f64,
    pub make_public_link: bool,
}

fn default_save_mode() -> String {
    "direct".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            drive_folder_input: String::new(),
            drive_folder_name: String::new(),
            local_output_dir: String::new(),
            save_mode: default_save_mode(),
            drive_save_mode: default_save_mode(),
            upload_directly: false,
            oauth_client_id: String::new(),
            oauth_client_secret: String::new(),
            refresh_token: None,
            max_size_mb: 30.0,
            target_lufs: -16.0,
            make_public_link: true,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("MessageMaster").join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfig>(&raw) {
                return cfg;
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
        }
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, raw).map_err(|e| format!("Failed to save config: {e}"))?;
        Ok(())
    }

    /// Extracts a Google Drive folder ID from either a raw ID or a Drive folder URL.
    pub fn folder_id(&self) -> Option<String> {
        let trimmed = self.drive_folder_input.trim();
        if trimmed.is_empty() || trimmed == "root" {
            return None;
        }
        if let Some(pos) = trimmed.find("/folders/") {
            let rest = &trimmed[pos + 9..];
            let id = rest.split(['?', '/', '#']).next().unwrap_or("").trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        Some(trimmed.to_string())
    }
}
