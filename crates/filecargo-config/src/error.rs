/// Errors returned by the config module.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot determine home directory: {0}")]
    NoHomeDir(String),
}
