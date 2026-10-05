use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use filecargo_config::Site;
use russh::client;
use russh::keys::{PublicKey, PublicKeyOrCertificate};
use tokio::net::TcpStream;

use super::host_keys;
use crate::{ConnectContext, ConnectError};

/// Handler error: either a russh failure or one of our own decisions (host key policy).
#[derive(Debug)]
pub(crate) enum SshError {
    Russh(russh::Error),
    Connect(ConnectError),
}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        Self::Russh(e)
    }
}

pub(crate) struct SshHandler {
    ctx: ConnectContext,
    host: String,
    port: u16,
    /// True while a human is being asked about the host key; pauses the handshake timeout.
    prompting: Arc<AtomicBool>,
}

impl client::Handler for SshHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key: &PublicKey = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            PublicKeyOrCertificate::Certificate(_) => {
                return Err(SshError::Connect(ConnectError::Network(
                    "SSH host certificates are not supported".to_owned(),
                )));
            }
        };
        self.prompting.store(true, Ordering::SeqCst);
        let verdict = host_keys::verify(&self.ctx, &self.host, self.port, key).await;
        self.prompting.store(false, Ordering::SeqCst);
        verdict.map(|()| true).map_err(SshError::Connect)
    }
}

/// A connected, host-key-verified, **not yet authenticated** SSH transport.
pub struct SshConnection {
    pub(crate) handle: client::Handle<SshHandler>,
}

impl SshConnection {
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }
}

impl std::fmt::Debug for SshConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConnection")
            .field("closed", &self.is_closed())
            .finish()
    }
}

fn client_config(ctx: &ConnectContext) -> client::Config {
    client::Config {
        keepalive_interval: Some(Duration::from_secs(u64::from(ctx.timeouts.keepalive_secs))),
        ..client::Config::default()
    }
}

async fn tcp_connect(host: &str, port: u16) -> Result<TcpStream, ConnectError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ConnectError::Resolve(format!("{host}: {e}")))?
        .collect();
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            ConnectError::Refused(format!("{host}:{port}"))
        }
        Some(e) => ConnectError::Network(format!("{host}:{port}: {e}")),
        None => ConnectError::Resolve(format!("{host}: no addresses")),
    })
}

/// Opens the TCP connection, runs the SSH handshake and verifies the host key (prompting when
/// it is unknown). Authentication is a separate step.
///
/// `timeout_secs` bounds the connect and the handshake, but not the time a human takes to
/// answer the host-key prompt.
pub async fn connect_ssh(site: &Site, ctx: &ConnectContext) -> Result<SshConnection, ConnectError> {
    let (host, port) = (site.host.clone(), site.effective_port());
    let limit = Duration::from_secs(u64::from(ctx.timeouts.timeout_secs));
    tracing::info!(target: "filecargo::protocol", site = %site.name, host, port, "connecting over SSH");

    let prompting = Arc::new(AtomicBool::new(false));
    let handshake = async {
        let stream = tcp_connect(&host, port).await?;
        let handler = SshHandler {
            ctx: ctx.clone(),
            host: host.clone(),
            port,
            prompting: prompting.clone(),
        };
        client::connect_stream(Arc::new(client_config(ctx)), stream, handler)
            .await
            .map_err(|e| match e {
                SshError::Connect(c) => c,
                SshError::Russh(r) => ConnectError::Network(r.to_string()),
            })
    };
    tokio::pin!(handshake);
    let handle = loop {
        tokio::select! {
            result = &mut handshake => break result?,
            () = tokio::time::sleep(limit) => {
                if !prompting.load(Ordering::SeqCst) {
                    return Err(ConnectError::Timeout);
                }
            }
        }
    };
    Ok(SshConnection { handle })
}
