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
}
