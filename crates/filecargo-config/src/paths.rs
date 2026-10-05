use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;

use crate::error::ConfigError;

const APP_DIR: &str = "filecargo";
const OVERRIDE_ENV: &str = "FILECARGO_CONFIG_DIR";

/// Locations of every file filecargo reads or writes.
///
/// Files owned by other modules (`known_hosts`, `queue.json`, ...) are resolved here so all
/// path decisions live in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl Paths {
    /// Resolves from `FILECARGO_CONFIG_DIR` when set, otherwise the platform directories.
    pub fn resolve() -> Result<Self, ConfigError> {
        match std::env::var_os(OVERRIDE_ENV).filter(|v| !v.is_empty()) {
            Some(dir) => Ok(Self::from_override(Some(PathBuf::from(dir)))),
            None => Self::platform(),
        }
    }

    /// Pure constructor used by tests: `Some(dir)` puts config and data under `dir`.
    pub fn from_override(dir: Option<PathBuf>) -> Self {
        match dir {
            Some(dir) => Self {
                config_dir: dir.clone(),
                data_dir: dir,
            },
            None => Self::platform().unwrap_or_else(|_| Self {
                config_dir: PathBuf::from(APP_DIR),
                data_dir: PathBuf::from(APP_DIR),
            }),
        }
    }

    fn platform() -> Result<Self, ConfigError> {
        let strategy =
            etcetera::choose_base_strategy().map_err(|e| ConfigError::NoHomeDir(e.to_string()))?;
        Ok(Self {
            config_dir: strategy.config_dir().join(APP_DIR),
            data_dir: strategy.data_dir().join(APP_DIR),
        })
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn servers(&self) -> PathBuf {
        self.config_dir.join("servers.toml")
    }

    pub fn settings(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    pub fn lock(&self) -> PathBuf {
        self.config_dir.join(".lock")
    }

    pub fn known_hosts(&self) -> PathBuf {
        self.data_dir.join("known_hosts")
    }

    pub fn trusted_certs(&self) -> PathBuf {
        self.data_dir.join("trusted_certs.toml")
    }

    pub fn queue(&self) -> PathBuf {
        self.data_dir.join("queue.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_puts_config_and_data_under_one_dir() {
        let p = Paths::from_override(Some(PathBuf::from("/tmp/fc")));
        assert_eq!(p.servers(), PathBuf::from("/tmp/fc/servers.toml"));
        assert_eq!(p.settings(), PathBuf::from("/tmp/fc/settings.toml"));
        assert_eq!(p.lock(), PathBuf::from("/tmp/fc/.lock"));
        assert_eq!(p.known_hosts(), PathBuf::from("/tmp/fc/known_hosts"));
        assert_eq!(
            p.trusted_certs(),
            PathBuf::from("/tmp/fc/trusted_certs.toml")
        );
        assert_eq!(p.queue(), PathBuf::from("/tmp/fc/queue.json"));
    }

    #[test]
    fn platform_paths_end_in_app_dir() {
        let p = Paths::platform().unwrap();
        assert!(p.config_dir().ends_with(APP_DIR));
        assert!(p.data_dir().ends_with(APP_DIR));
    }

    #[cfg(unix)]
    #[test]
    fn platform_paths_use_xdg_layout_on_unix() {
        let p = Paths::platform().unwrap();
        let s = p.config_dir().to_string_lossy();
        assert!(
            s.contains(".config") || std::env::var_os("XDG_CONFIG_HOME").is_some(),
            "{s}"
        );
    }
}
