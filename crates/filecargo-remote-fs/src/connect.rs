use std::path::PathBuf;
use std::sync::Arc;

use filecargo_config::{ConnectionSettings, Paths, SecretStore};

use crate::{FsError, Prompter, SessionTrust};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("cannot resolve host: {0}")]
    Resolve(String),
    #[error("connection refused: {0}")]
    Refused(String),
    #[error("connection timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("host key rejected")]
    HostKeyRejected,
    #[error(
        "the host key of {host} changed (new fingerprint {fingerprint}); the old key is in {}. \
         Remove that entry yourself if the change is expected.",
        known_in.display()
    )]
    HostKeyChanged {
        host: String,
        fingerprint: String,
        known_in: PathBuf,
    },
    #[error("certificate rejected")]
    CertificateRejected,
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("authentication failed (tried: {})", methods_tried.join(", "))]
    AuthFailed { methods_tried: Vec<String> },
    #[error("cannot use key file: {0}")]
    KeyFile(String),
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Fs(#[from] FsError),
}

/// Everything `connect` needs besides the site itself.
#[derive(Clone)]
pub struct ConnectContext {
    /// Where filecargo's own `known_hosts` and `trusted_certs.toml` live.
    pub paths: Paths,
    pub secrets: Arc<dyn SecretStore>,
    pub prompter: Arc<dyn Prompter>,
    /// Shared by every connection of the app.
    pub trust: Arc<SessionTrust>,
    pub timeouts: ConnectionSettings,
    /// The user's OpenSSH `known_hosts`, read-only and never written. `None` disables it.
    /// [`ConnectContext::new`] points it at `~/.ssh/known_hosts`; tests override it.
    pub user_known_hosts: Option<PathBuf>,
    /// ssh-agent socket (unix) or pipe (Windows) path. `None` means `SSH_AUTH_SOCK` / the
    /// platform default; tests point it at their own agent.
    pub agent_socket: Option<PathBuf>,
}

impl ConnectContext {
    pub fn new(paths: Paths, secrets: Arc<dyn SecretStore>, prompter: Arc<dyn Prompter>) -> Self {
        Self {
            paths,
            secrets,
            prompter,
            trust: Arc::new(SessionTrust::new()),
            timeouts: ConnectionSettings::default(),
            user_known_hosts: std::env::home_dir().map(|h| h.join(".ssh").join("known_hosts")),
            agent_socket: None,
        }
    }
}
