//! TLS for FTPS: certificate policy (OS trust store + pins), the user prompt, and the
//! connector handed to `suppaftp`.

mod connector;
mod pins;
mod verifier;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rustls::ClientConfig;

pub(crate) use connector::{RecordingConnector, TlsInfo};
pub(crate) use verifier::CertFailure;
use verifier::PinningVerifier;

use crate::{CertificatePrompt, ConnectContext, ConnectError, FsError, TrustDecision};

/// TLS state for one site: **one** `ClientConfig` shared by the control connection and every
/// data connection, so its session cache gives data connections a chance to resume the
/// control connection's session.
pub(crate) struct SiteTls {
    connector: RecordingConnector,
    verifier: Arc<PinningVerifier>,
    pub(crate) info: Arc<Mutex<TlsInfo>>,
}

impl SiteTls {
    pub(crate) fn new(pinned: HashSet<String>, resumption: bool) -> Result<Self, ConnectError> {
        let tls = |e: rustls::Error| ConnectError::Tls(e.to_string());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PinningVerifier::new(provider.clone(), pinned).map_err(tls)?);
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(tls)?
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone())
            .with_no_client_auth();
        if !resumption {
            config.resumption = rustls::client::Resumption::disabled();
        }
        let info = Arc::new(Mutex::new(TlsInfo::default()));
        Ok(Self {
            connector: RecordingConnector {
                inner: tokio_rustls::TlsConnector::from(Arc::new(config)),
                info: info.clone(),
            },
            verifier,
            info,
        })
    }

    pub(crate) fn connector(&self) -> RecordingConnector {
        self.connector.clone()
    }

    /// The certificate failure recorded by the last rejected handshake.
    pub(crate) fn take_failure(&self) -> Option<CertFailure> {
        self.verifier.take_failure()
    }
}

fn pin_error(e: pins::PinError) -> ConnectError {
    ConnectError::Fs(FsError::LocalIo(e.to_string()))
}

/// Fingerprints accepted for `host:port`: permanent pins plus *Trust once* decisions.
pub(crate) fn pinned_for(
    ctx: &ConnectContext,
    host: &str,
    port: u16,
) -> Result<HashSet<String>, ConnectError> {
    let mut set: HashSet<String> = pins::pinned(&ctx.paths.trusted_certs(), host, port)
        .map_err(pin_error)?
        .into_iter()
        .collect();
    set.extend(ctx.trust.trusted_certs(host, port));
    Ok(set)
}

/// Asks the user about a rejected certificate and records the answer.
pub(crate) async fn ask(
    ctx: &ConnectContext,
    host: &str,
    port: u16,
    failure: &CertFailure,
) -> Result<(), ConnectError> {
    let decision = ctx
        .prompter
        .certificate(CertificatePrompt {
            host: host.to_owned(),
            port,
            problem: failure.problem,
            subject: failure.subject.clone(),
            issuer: failure.issuer.clone(),
            not_after: failure.not_after.clone(),
            sha256: failure.sha256.clone(),
        })
        .await;
    match decision {
        TrustDecision::Reject => Err(ConnectError::CertificateRejected),
        TrustDecision::TrustOnce => {
            ctx.trust.trust_cert(host, port, &failure.sha256);
            Ok(())
        }
        TrustDecision::TrustAlways => {
            pins::add(&ctx.paths.trusted_certs(), host, port, &failure.sha256)
                .map_err(pin_error)?;
            tracing::info!(target: "filecargo::protocol", host, port, sha256 = %failure.sha256, "FTPS certificate pinned");
            Ok(())
        }
    }
}
