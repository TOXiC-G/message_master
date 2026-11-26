use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEntry {
    pub title: String,
    pub last_part: u32,
    pub preacher: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageCatalog {
    pub messages: Vec<MessageEntry>,
    pub preachers: Vec<String>,
    pub languages: Vec<String>,
}

impl Default for MessageCatalog {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            preachers: Vec::new(),
            languages: vec!["English".to_string()],
        }
    }
}

impl MessageCatalog {
    fn path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("MessageMaster").join("catalog.json")
    }

    pub fn load() -> Self {
        let p = Self::path();
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Ok(cat) = serde_json::from_str::<MessageCatalog>(&raw) {
                return cat;
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&p, raw).map_err(|e| e.to_string())
    }

    pub fn record_usage(&mut self, title: &str, part: u32, preacher: &str, language: &str) {
        let t = title.trim();
        let pr = preacher.trim();
        let lang = language.trim();

        if !t.is_empty() {
            if let Some(existing) = self
                .messages
                .iter_mut()
                .find(|m| m.title.eq_ignore_ascii_case(t))
            {
                existing.title = t.to_string();
                if part > 0 {
                    existing.last_part = part;
                }
                if !pr.is_empty() {
                    existing.preacher = pr.to_string();
                }
                if !lang.is_empty() {
                    existing.language = lang.to_string();
                }
            } else {
                self.messages.insert(
                    0,
                    MessageEntry {
                        title: t.to_string(),
                        last_part: part,
                        preacher: pr.to_string(),
                        language: lang.to_string(),
                    },
                );
            }
        }

        if !pr.is_empty() && !self.preachers.iter().any(|x| x.eq_ignore_ascii_case(pr)) {
            self.preachers.push(pr.to_string());
        }
        if !lang.is_empty() && !self.languages.iter().any(|x| x.eq_ignore_ascii_case(lang)) {
            self.languages.push(lang.to_string());
        }
        let _ = self.save();
    }
}
