#![allow(dead_code)] // each test binary uses a different subset

use std::sync::Arc;

use filecargo_config::{ConfigError, ConfigStore, MemoryStore, Paths};

/// Opens a store backed by a throwaway in-memory secret store.
pub fn open(paths: Paths) -> Result<ConfigStore, ConfigError> {
    open_with(paths, Arc::new(MemoryStore::new()))
}

pub fn open_with(
    paths: Paths,
    secrets: Arc<dyn filecargo_config::SecretStore>,
) -> Result<ConfigStore, ConfigError> {
    ConfigStore::open(paths, secrets)
}
