#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(String),
    #[error("{0} is not supported by this server")]
    Unsupported(&'static str),
    /// vsftpd `require_ssl_reuse` / FileZilla Server: the data connection's TLS session must
    /// resume the control session, which the FTP library cannot do yet (suppaftp #93).
    #[error(
        "This server requires TLS session reuse on data connections, which filecargo does not \
         support yet. Use SFTP, or ask the admin to set `require_ssl_reuse=NO`."
    )]
    TlsSessionReuseRequired,
    #[error("disconnected: {0}")]
    Disconnected(String),
    #[error("operation timed out")]
    Timeout,
    #[error("unexpected server reply{}: {message}", code.map(|c| format!(" {c}")).unwrap_or_default())]
    Protocol { code: Option<u16>, message: String },
    /// The caller's sink or source failed, not the server.
    #[error("local i/o error: {0}")]
    LocalIo(String),
}

impl FsError {
    /// Whether retrying (usually on a fresh connection) can help.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Disconnected(_) | Self::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_connection_errors_are_retryable() {
        assert!(FsError::Disconnected("reset".into()).is_retryable());
        assert!(FsError::Timeout.is_retryable());
        assert!(!FsError::NotFound("/x".into()).is_retryable());
        assert!(!FsError::TlsSessionReuseRequired.is_retryable());
        assert!(!FsError::LocalIo("disk full".into()).is_retryable());
    }

    #[test]
    fn tls_reuse_error_has_the_actionable_message() {
        let msg = FsError::TlsSessionReuseRequired.to_string();
        assert!(msg.contains("TLS session reuse"));
        assert!(msg.contains("require_ssl_reuse=NO"));
    }

    #[test]
    fn protocol_error_shows_the_code_when_known() {
        let with = FsError::Protocol {
            code: Some(550),
            message: "no".into(),
        };
        let without = FsError::Protocol {
            code: None,
            message: "no".into(),
        };
        assert_eq!(with.to_string(), "unexpected server reply 550: no");
        assert_eq!(without.to_string(), "unexpected server reply: no");
    }
}
