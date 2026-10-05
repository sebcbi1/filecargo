//! The `suppaftp` TLS connector: tokio-rustls plus a record of what was negotiated and whether
//! data connections managed to resume the control connection's TLS session.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rustls::HandshakeKind;
use rustls::pki_types::ServerName;
use suppaftp::tokio::{AsyncRustlsStream, AsyncTlsConnector};
use suppaftp::{FtpError, FtpResult};
use tokio::net::TcpStream;

/// What the handshakes of one site's connections looked like.
#[derive(Debug, Default, Clone)]
pub(crate) struct TlsInfo {
    /// e.g. `TLS 1.3, TLS13_AES_256_GCM_SHA384`, from the control connection.
    pub control: Option<String>,
    pub data_full: u32,
    pub data_resumed: u32,
}

#[derive(Clone)]
pub(crate) struct RecordingConnector {
    pub(crate) inner: tokio_rustls::TlsConnector,
    pub(crate) info: Arc<Mutex<TlsInfo>>,
}

impl std::fmt::Debug for RecordingConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecordingConnector")
    }
}

#[async_trait]
impl AsyncTlsConnector for RecordingConnector {
    type Stream = AsyncRustlsStream;

    async fn connect(&self, domain: &str, stream: TcpStream) -> FtpResult<Self::Stream> {
        let name = ServerName::try_from(domain.to_owned())
            .map_err(|e| FtpError::SecureError(e.to_string()))?;
        let tls = self
            .inner
            .connect(name, stream)
            .await
            .map_err(|e| FtpError::SecureError(e.to_string()))?;
        {
            let (_, connection) = tls.get_ref();
            let resumed = matches!(connection.handshake_kind(), Some(HandshakeKind::Resumed));
            let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
            if info.control.is_none() {
                let version = connection
                    .protocol_version()
                    .map_or_else(|| "TLS".to_owned(), |v| format!("{v:?}"));
                let suite = connection
                    .negotiated_cipher_suite()
                    .map_or_else(String::new, |s| format!(", {:?}", s.suite()));
                info.control = Some(format!("{version}{suite}"));
            } else if resumed {
                info.data_resumed += 1;
            } else {
                info.data_full += 1;
            }
        }
        Ok(AsyncRustlsStream::from(tls))
    }
}
