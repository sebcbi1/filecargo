//! Decisions that need a human. `app-core` implements [`Prompter`]; this crate never touches a UI.

use std::path::PathBuf;

use async_trait::async_trait;
use filecargo_config::SecretString;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialPrompt {
    Password {
        site: String,
        user: String,
        /// The previous attempt was rejected.
        retry: bool,
    },
    Passphrase {
        site: String,
        key_path: PathBuf,
        retry: bool,
    },
    KeyboardInteractive {
        site: String,
        name: String,
        instructions: String,
        /// `(prompt, echo)` pairs.
        prompts: Vec<(String, bool)>,
    },
}

pub struct CredentialAnswer {
    /// One value per prompt (one for password / passphrase).
    pub values: Vec<SecretString>,
    pub remember: bool,
}

impl std::fmt::Debug for CredentialAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialAnswer")
            .field("values", &self.values.len())
            .field("remember", &self.remember)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    /// e.g. `ssh-ed25519`.
    pub algorithm: String,
    /// `SHA256:<base64>`, as OpenSSH prints it.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    TrustOnce,
    TrustAlways,
    Reject,
}

#[async_trait]
pub trait Prompter: Send + Sync {
    /// `None` means the user cancelled.
    async fn credential(&self, request: CredentialPrompt) -> Option<CredentialAnswer>;
    async fn host_key(&self, request: HostKeyPrompt) -> TrustDecision;
    /// An FTPS certificate the operating system does not accept. `TrustOnce` and `TrustAlways`
    /// pin exactly this certificate for this host and port.
    async fn certificate(&self, request: CertificatePrompt) -> TrustDecision;
}

/// Why the platform would not accept the server's certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateProblem {
    Expired,
    SelfSigned,
    /// The certificate is for another host name.
    NameMismatch,
    /// Signed by an authority the operating system does not trust.
    Untrusted,
}

impl std::fmt::Display for CertificateProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Expired => "the certificate has expired",
            Self::SelfSigned => "the certificate is self-signed",
            Self::NameMismatch => "the certificate is for a different host name",
            Self::Untrusted => "the certificate is not signed by a trusted authority",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificatePrompt {
    pub host: String,
    pub port: u16,
    pub problem: CertificateProblem,
    pub subject: String,
    pub issuer: String,
    /// Expiry as printed by the certificate parser (UTC).
    pub not_after: String,
    /// Lower-case hex SHA-256 of the DER certificate.
    pub sha256: String,
}
