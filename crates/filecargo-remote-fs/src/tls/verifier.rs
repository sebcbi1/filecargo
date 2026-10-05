//! Server-certificate policy for FTPS: the OS trust store plus per-site pins. A failure is
//! recorded (rustls verification is synchronous) so `connect` can ask the user and retry with
//! the certificate pinned.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error, SignatureScheme};
use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::CertificateProblem;

/// Everything the user is shown about a rejected certificate.
#[derive(Debug, Clone)]
pub(crate) struct CertFailure {
    pub problem: CertificateProblem,
    pub subject: String,
    pub issuer: String,
    pub not_after: String,
    /// Lower-case hex SHA-256 of the DER, the pin.
    pub sha256: String,
}

pub(crate) fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Derives the reason from the certificate itself: platform verifiers report most failures as
/// an opaque `Other` (macOS), so expired / self-signed are read off the certificate.
fn describe(der: &[u8], error: &Error) -> CertFailure {
    let sha256 = fingerprint(der);
    let Ok((_, cert)) = X509Certificate::from_der(der) else {
        return CertFailure {
            problem: CertificateProblem::Untrusted,
            subject: "(unreadable certificate)".to_owned(),
            issuer: String::new(),
            not_after: String::new(),
            sha256,
        };
    };
    let (subject, issuer) = (cert.subject().to_string(), cert.issuer().to_string());
    let expired = cert.validity().not_after.timestamp() < now_secs();
    let problem = match error {
        Error::InvalidCertificate(
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
        ) => CertificateProblem::NameMismatch,
        Error::InvalidCertificate(
            CertificateError::Expired | CertificateError::ExpiredContext { .. },
        ) => CertificateProblem::Expired,
        _ if expired => CertificateProblem::Expired,
        _ if subject == issuer => CertificateProblem::SelfSigned,
        _ => CertificateProblem::Untrusted,
    };
    CertFailure {
        problem,
        subject,
        issuer,
        not_after: cert.validity().not_after.to_string(),
        sha256,
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[derive(Debug)]
pub(crate) struct PinningVerifier {
    inner: rustls_platform_verifier::Verifier,
    /// Hex SHA-256 fingerprints accepted for this site, whatever the platform says.
    pinned: HashSet<String>,
    failure: Mutex<Option<CertFailure>>,
}

impl PinningVerifier {
    pub(crate) fn new(
        provider: Arc<rustls::crypto::CryptoProvider>,
        pinned: HashSet<String>,
    ) -> Result<Self, Error> {
        Ok(Self {
            inner: rustls_platform_verifier::Verifier::new(provider)?,
            pinned,
            failure: Mutex::new(None),
        })
    }

    /// The failure recorded by the last rejected handshake, if any.
    pub(crate) fn take_failure(&self) -> Option<CertFailure> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if self.pinned.contains(&fingerprint(end_entity)) {
            return Ok(ServerCertVerified::assertion());
        }
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .inspect_err(|error| {
                *self.failure.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(describe(end_entity, error));
            })
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use rustls::pki_types::ServerName;

    use super::*;

    const SELF_SIGNED: &str = include_str!("../../../../tests/docker/certs/server.crt");
    const EXPIRED: &str = include_str!("../../../../tests/docker/certs/expired.crt");
    const CA_ISSUED: &str = include_str!("../../../../tests/docker/certs/leaf.crt");

    fn der(pem: &str) -> Vec<u8> {
        let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .unwrap()
    }

    fn unknown_issuer() -> Error {
        Error::InvalidCertificate(CertificateError::UnknownIssuer)
    }

    #[test]
    fn a_self_signed_certificate_is_described_as_such() {
        let failure = describe(&der(SELF_SIGNED), &unknown_issuer());
        assert_eq!(failure.problem, CertificateProblem::SelfSigned);
        assert!(failure.subject.contains("localhost"));
        assert_eq!(failure.sha256.len(), 64);
        assert!(failure.not_after.contains("2126"), "{}", failure.not_after);
    }

    #[test]
    fn an_expired_certificate_is_expired_whatever_error_the_platform_reports() {
        // macOS reports most failures as `Other`; the date on the certificate decides
        let opaque = Error::InvalidCertificate(CertificateError::Other(rustls::OtherError(
            Arc::new(std::io::Error::other("opaque")),
        )));
        assert_eq!(
            describe(&der(EXPIRED), &opaque).problem,
            CertificateProblem::Expired
        );
        assert_eq!(
            describe(&der(EXPIRED), &unknown_issuer()).problem,
            CertificateProblem::Expired
        );
    }

    #[test]
    fn a_certificate_from_an_unknown_authority_is_untrusted() {
        assert_eq!(
            describe(&der(CA_ISSUED), &unknown_issuer()).problem,
            CertificateProblem::Untrusted
        );
    }

    #[test]
    fn a_name_mismatch_is_reported_as_such() {
        let error = Error::InvalidCertificate(CertificateError::NotValidForName);
        assert_eq!(
            describe(&der(CA_ISSUED), &error).problem,
            CertificateProblem::NameMismatch
        );
    }

    #[test]
    fn garbage_still_produces_a_prompt_instead_of_a_panic() {
        let failure = describe(b"not a certificate", &unknown_issuer());
        assert_eq!(failure.problem, CertificateProblem::Untrusted);
    }

    fn verifier(pinned: &[String]) -> PinningVerifier {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        PinningVerifier::new(provider, pinned.iter().cloned().collect()).unwrap()
    }

    fn verify(v: &PinningVerifier, pem: &str) -> Result<ServerCertVerified, Error> {
        let der = der(pem);
        v.verify_server_cert(
            &CertificateDer::from(der),
            &[],
            &ServerName::try_from("localhost").unwrap(),
            &[],
            UnixTime::now(),
        )
    }

    #[test]
    fn an_unpinned_self_signed_certificate_fails_and_records_why() {
        let v = verifier(&[]);
        assert!(verify(&v, SELF_SIGNED).is_err());
        let failure = v.take_failure().expect("the failure must be recorded");
        assert_eq!(failure.problem, CertificateProblem::SelfSigned);
        assert_eq!(failure.sha256, fingerprint(&der(SELF_SIGNED)));
        assert!(v.take_failure().is_none(), "taking the failure clears it");
    }

    #[test]
    fn a_pinned_certificate_is_accepted_and_only_that_one() {
        let v = verifier(&[fingerprint(&der(SELF_SIGNED))]);
        assert!(verify(&v, SELF_SIGNED).is_ok());
        assert!(v.take_failure().is_none());
        // a pin never vouches for a different certificate, even for the same name
        assert!(verify(&v, EXPIRED).is_err());
        assert_eq!(
            v.take_failure().unwrap().problem,
            CertificateProblem::Expired
        );
    }
}
