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
