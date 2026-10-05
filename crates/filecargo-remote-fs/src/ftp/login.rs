use std::net::SocketAddr;
use std::time::Duration;

use filecargo_config::{Auth, ExposeSecret, Protocol, SecretKey, Site};
use suppaftp::tokio::AsyncRustlsFtpStream;
use suppaftp::types::{FileType as TransferType, Mode};
use suppaftp::{FtpError, Status};

use super::fs::{Features, FtpFs};
use crate::credentials::{self, password_prompt};
use crate::{ConnectContext, ConnectError};

fn auth_failed(method: &str) -> ConnectError {
    ConnectError::AuthFailed {
        methods_tried: vec![method.to_owned()],
    }
}

async fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, ConnectError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ConnectError::Resolve(format!("{host}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(ConnectError::Resolve(format!("{host}: no addresses")));
    }
    Ok(addrs)
}

fn connect_error(error: &FtpError, host: &str, port: u16) -> ConnectError {
    match error {
        FtpError::ConnectionError(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            ConnectError::Refused(format!("{host}:{port}"))
        }
        FtpError::ConnectionError(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            ConnectError::Timeout
        }
        other => ConnectError::Network(other.to_string()),
    }
}

async fn tcp_ftp(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<AsyncRustlsFtpStream, ConnectError> {
    let mut last = None;
    for addr in resolve(host, port).await? {
        match AsyncRustlsFtpStream::connect_timeout(addr, timeout).await {
            Ok(ftp) => return Ok(ftp),
            Err(e) => last = Some(e),
        }
    }
    Err(last.map_or_else(
        || ConnectError::Network("no address worked".to_owned()),
        |e| connect_error(&e, host, port),
    ))
}

fn rejected(error: &FtpError) -> bool {
    matches!(
        error,
        FtpError::UnexpectedResponse(r)
            if matches!(r.status, Status::NotLoggedIn | Status::InvalidCredentials | Status::LoginNeedAccount)
    )
}

/// Connects, logs in and prepares the session (binary type, UTF-8, passive/active mode).
pub(crate) async fn open(site: &Site, ctx: &ConnectContext) -> Result<FtpFs, ConnectError> {
    let (host, port) = (site.host.clone(), site.effective_port());
    let timeout = Duration::from_secs(u64::from(ctx.timeouts.timeout_secs));
    if site.protocol == Protocol::Ftp {
        tracing::warn!(target: "filecargo::protocol", site = %site.name, "plain FTP: the password travels in clear text");
    }
    tracing::info!(target: "filecargo::protocol", site = %site.name, host, port, "connecting over FTP");
    let mut ftp = tcp_ftp(&host, port, timeout).await?;

    match &site.auth {
        Auth::Anonymous => {
            ftp.login("anonymous", "anonymous@").await.map_err(|e| {
                if rejected(&e) {
                    auth_failed("anonymous")
                } else {
                    connect_error(&e, &host, port)
                }
            })?;
        }
        Auth::Password { remember } => {
            let key = SecretKey::Password(site.id);
            let mut logged_in = false;
            for attempt in 0..2u8 {
                let obtained = credentials::obtain(ctx, key, *remember, attempt, |retry| {
                    password_prompt(site, retry)
                })
                .await?;
                match ftp
                    .login(site.user.as_str(), obtained.value.expose_secret())
                    .await
                {
                    Ok(()) => {
                        credentials::succeeded(ctx, key, *remember, &obtained);
                        logged_in = true;
                        break;
                    }
                    Err(e) if rejected(&e) => credentials::failed(ctx, &key),
                    Err(e) => return Err(connect_error(&e, &host, port)),
                }
            }
            if !logged_in {
                return Err(auth_failed("password"));
            }
        }
        Auth::KeyFile { .. } | Auth::Agent => return Err(auth_failed("key/agent (SFTP only)")),
    }
    tracing::info!(target: "filecargo::protocol", site = %site.name, "FTP login accepted");

    let fatal = |e: FtpError| connect_error(&e, &host, port);
    ftp.transfer_type(TransferType::Binary)
        .await
        .map_err(fatal)?;
    let features = ftp
        .feat()
        .await
        .map(|f| Features::from_feat(&f))
        .unwrap_or_default();
    if features.has("UTF8") {
        // not fatal: some servers answer 200 only to a bare `OPTS UTF8 ON`
        let _ = ftp.opts("UTF8", Some("ON")).await;
    }
    if features.has("EPSV") {
        ftp.set_mode(Mode::ExtendedPassive);
    } else {
        ftp.set_mode(Mode::Passive);
        ftp.set_passive_nat_workaround(true);
    }
    Ok(FtpFs::new(ftp, features, site.protocol, timeout))
}
