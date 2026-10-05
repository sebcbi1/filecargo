#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Helpers shared by the integration test files.

use sha2::{Digest, Sha256};

/// Deterministic, incompressible-ish bytes so resume bugs show up as hash mismatches.
pub fn sample_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---- scripted prompter -------------------------------------------------------------------

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use filecargo_remote_fs::{
    CredentialAnswer, CredentialPrompt, HostKeyPrompt, Prompter, TrustDecision,
};

type Script = Option<(Vec<String>, bool)>;

/// Answers prompts from a script and records what it was asked.
#[derive(Default)]
pub struct TestPrompter {
    pub host_key_decision: Mutex<Option<TrustDecision>>,
    /// Popped one per credential prompt; `None` entries cancel the prompt.
    pub credential_answers: Mutex<VecDeque<Script>>,
    pub host_key_prompts: Mutex<Vec<HostKeyPrompt>>,
    pub credential_prompts: Mutex<Vec<CredentialPrompt>>,
}

impl TestPrompter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn trust(self: &Arc<Self>, decision: TrustDecision) -> Arc<Self> {
        *self.host_key_decision.lock().unwrap() = Some(decision);
        self.clone()
    }

    pub fn answer(&self, values: &[&str], remember: bool) {
        let values = values.iter().map(|v| (*v).to_owned()).collect();
        self.credential_answers
            .lock()
            .unwrap()
            .push_back(Some((values, remember)));
    }

    pub fn host_key_prompt_count(&self) -> usize {
        self.host_key_prompts.lock().unwrap().len()
    }

    pub fn credential_prompt_count(&self) -> usize {
        self.credential_prompts.lock().unwrap().len()
    }
}

#[async_trait]
impl Prompter for TestPrompter {
    async fn credential(&self, request: CredentialPrompt) -> Option<CredentialAnswer> {
        self.credential_prompts.lock().unwrap().push(request);
        let (values, remember) = self.credential_answers.lock().unwrap().pop_front()??;
        Some(CredentialAnswer {
            values: values
                .into_iter()
                .map(secrecy::SecretString::from)
                .collect(),
            remember,
        })
    }

    async fn host_key(&self, request: HostKeyPrompt) -> TrustDecision {
        self.host_key_prompts.lock().unwrap().push(request);
        self.host_key_decision
            .lock()
            .unwrap()
            .unwrap_or(TrustDecision::Reject)
    }
}

// ---- docker sftp session -----------------------------------------------------------------

use filecargo_config::{Auth, MemoryStore, Paths, Protocol, Site};
use filecargo_remote_fs::{ConnectContext, Session, connect};

/// A connected session to the docker `sftp` server (`fcuser` / `fcpass`, port 2222).
pub async fn docker_sftp() -> (Session, Arc<TestPrompter>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let prompter = TestPrompter::new();
    prompter.trust(TrustDecision::TrustOnce);
    prompter.answer(&["fcpass"], false);
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().join("cfg"))),
        Arc::new(MemoryStore::new()),
        prompter.clone(),
    );
    ctx.user_known_hosts = None;
    let mut site = Site::new("docker-sftp", Protocol::Sftp, "127.0.0.1");
    site.port = Some(2222);
    site.user = "fcuser".to_owned();
    site.auth = Auth::Password { remember: false };
    let session = connect(&site, &ctx).await.unwrap();
    (session, prompter, dir)
}

// ---- docker ftp session ------------------------------------------------------------------

/// A connected session to one of the docker FTP servers (`ftpuser` / `ftppass`).
/// Ports: 2121 explicit/plain, 2123 plain without MLSD in FEAT.
pub async fn docker_ftp(port: u16) -> (Session, Arc<TestPrompter>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let prompter = TestPrompter::new();
    prompter.trust(TrustDecision::Reject);
    prompter.answer(&["ftppass"], false);
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().join("cfg"))),
        Arc::new(MemoryStore::new()),
        prompter.clone(),
    );
    ctx.user_known_hosts = None;
    let session = connect(&docker_ftp_site(port), &ctx).await.unwrap();
    (session, prompter, dir)
}

pub fn docker_ftp_site(port: u16) -> Site {
    let mut site = Site::new("docker-ftp", Protocol::Ftp, "127.0.0.1");
    site.port = Some(port);
    site.user = "ftpuser".to_owned();
    site.auth = Auth::Password { remember: false };
    site
}
