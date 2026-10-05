use std::path::PathBuf;
use std::sync::Arc;

use filecargo_config::{ConnectionSettings, Paths, Protocol, SecretStore, Site};

use crate::{FsError, Prompter, RemoteFs, RemotePath, SessionTrust, ShellOpener};

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
    /// Whether FTPS data connections may resume the control connection's TLS session
    /// (default `true`). Servers that require reuse (ProFTPD, vsftpd's default, FileZilla
    /// Server) only work with it; turning it off makes them fail with
    /// [`FsError::TlsSessionReuseRequired`], which is how that path is tested.
    pub tls_session_resumption: bool,
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
            tls_session_resumption: true,
        }
    }
}

/// What the UI shows about a connected session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub protocol: Protocol,
    /// Server greeting, when the protocol has one.
    pub banner: Option<String>,
    /// TLS version and cipher for FTPS sessions.
    pub tls: Option<String>,
    pub home: RemotePath,
}

/// A connected server.
pub struct Session {
    pub fs: Arc<dyn RemoteFs>,
    pub info: SessionInfo,
    /// `Some` for SFTP only.
    pub shell: Option<ShellOpener>,
}

/// Connects to `site`, prompting through `ctx.prompter` when a decision is needed.
pub async fn connect(site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError> {
    match site.protocol {
        Protocol::Sftp => {
            let sftp = crate::sftp::open(site, ctx).await?;
            let home = sftp.home().await?;
            Ok(Session {
                info: SessionInfo {
                    protocol: Protocol::Sftp,
                    banner: None,
                    tls: None,
                    home,
                },
                shell: Some(sftp.shell_opener()),
                fs: Arc::new(sftp),
            })
        }
        Protocol::Ftp | Protocol::FtpsExplicit | Protocol::FtpsImplicit => {
            let ftp = crate::ftp::open(site, ctx).await?;
            let home = ftp.home().await?;
            Ok(Session {
                info: SessionInfo {
                    protocol: site.protocol,
                    banner: None,
                    tls: ftp.tls_summary(),
                    home,
                },
                shell: None,
                fs: Arc::new(ftp),
            })
        }
    }
}
