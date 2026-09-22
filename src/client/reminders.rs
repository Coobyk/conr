use std::fs;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::client::config::data_dir;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReminderState {
    pub registered_at: Option<DateTime<Utc>>,
    pub last_reminder_at: Option<DateTime<Utc>>,
    pub reminder_stage: u8, // 0=none yet, 1=day, 2=week, 3+=monthly
    /// Last active handle — used for auto-login on launch.
    pub handle: Option<String>,
    /// When true (default), try keychain/token file + login on startup.
    #[serde(default = "default_true")]
    pub auto_login: bool,
}

fn default_true() -> bool {
    true
}

impl ReminderState {
    pub fn load() -> Result<Self> {
        let path = data_dir()?.join("state.toml");
        if !path.exists() {
            return Ok(Self {
                auto_login: true,
                ..Self::default()
            });
        }
        let text = fs::read_to_string(path)?;
        Ok(toml::from_str(&text).unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        let path = data_dir()?.join("state.toml");
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Remember this handle as the active session (register unlock or login).
    pub fn set_active_handle(&mut self, handle: &str) -> Result<()> {
        self.handle = Some(handle.to_string());
        self.auto_login = true;
        self.save()
    }

    pub fn mark_registered(&mut self, handle: &str) -> Result<()> {
        self.registered_at = Some(Utc::now());
        self.handle = Some(handle.to_string());
        self.auto_login = true;
        self.reminder_stage = 0;
        self.last_reminder_at = None;
        self.save()
    }

    pub fn clear_session(&mut self) -> Result<()> {
        self.handle = None;
        self.auto_login = false;
        self.save()
    }

    /// Returns true if a reminder should be shown now.
    pub fn should_remind(&self) -> bool {
        let Some(reg) = self.registered_at else {
            return false;
        };
        let now = Utc::now();
        match self.reminder_stage {
            0 => now >= reg + Duration::days(1),
            1 => now >= reg + Duration::days(7),
            _ => {
                let last = self.last_reminder_at.unwrap_or(reg);
                now >= last + Duration::days(30)
            }
        }
    }

    pub fn acknowledge(&mut self) -> Result<()> {
        self.last_reminder_at = Some(Utc::now());
        self.reminder_stage = self.reminder_stage.saturating_add(1).max(1);
        if self.reminder_stage == 1 {
            // after first (day) bump
        }
        // stage: 0 -> 1 after day ack, 1 -> 2 after week, then monthly stays at 2+
        self.save()
    }
}
