use serde::{Deserialize, Serialize};

use crate::error::ValidationError;

/// What to do when a transfer target already exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictRule {
    #[default]
    Ask,
    Overwrite,
    OverwriteIfNewer,
    Resume,
    Skip,
    Rename,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransferSettings {
    /// 1..=10
    pub max_concurrent: u8,
    pub default_conflict: ConflictRule,
}

impl Default for TransferSettings {
    fn default() -> Self {
        Self {
            max_concurrent: 2,
            default_conflict: ConflictRule::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionSettings {
    pub timeout_secs: u32,
    pub keepalive_secs: u32,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            timeout_secs: 20,
            keepalive_secs: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub show_hidden: bool,
    pub confirm_delete: bool,
    pub local_start_dir: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            show_hidden: false,
            confirm_delete: true,
            local_start_dir: "~".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogSettings {
    pub level: LogLevel,
}

/// App settings. Missing keys fall back to defaults; unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub transfers: TransferSettings,
    pub connection: ConnectionSettings,
    pub ui: UiSettings,
    pub log: LogSettings,
}

impl Settings {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        let bad = |field: &'static str, rule: &'static str| {
            Err(ValidationError::InvalidSetting { field, rule })
        };
        if !(1..=10).contains(&self.transfers.max_concurrent) {
            return bad("transfers.max_concurrent", "must be between 1 and 10");
        }
        if self.connection.timeout_secs == 0 {
            return bad("connection.timeout_secs", "must be greater than 0");
        }
        if self.connection.keepalive_secs == 0 {
            return bad("connection.keepalive_secs", "must be greater than 0");
        }
        Ok(())
    }
}
