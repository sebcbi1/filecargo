//! Prompts: every decision that needs a human goes through one FIFO, and the UI shows the
//! front of it.

use async_trait::async_trait;
use filecargo_remote_fs::{
    CertificatePrompt, CredentialAnswer, CredentialPrompt, HostKeyPrompt, Prompter, TrustDecision,
};
use tokio::sync::{mpsc, oneshot};

use crate::app::{Core, Msg};
use crate::state::{PromptAnswer, PromptId, PromptKind};

/// What a confirmed prompt does, for prompts the app raises itself (not for background tasks).
pub(crate) enum PromptAction {
    Delete {
        names: Vec<String>,
    },
    Conflict {
        transfer: filecargo_transfer::TransferId,
    },
    Quit,
}

/// A request from a background task: show `kind`, send the answer back on `reply`.
pub(crate) struct PromptRequest {
    pub kind: PromptKind,
    pub reply: oneshot::Sender<PromptAnswer>,
}

/// The `remote_fs::Prompter` of every session: queues a prompt in the actor and waits for the
/// matching `Command::Answer`. If the app shuts down first, the prompt counts as cancelled.
pub(crate) struct ActorPrompter {
    pub(crate) messages: mpsc::UnboundedSender<Msg>,
}

impl ActorPrompter {
    async fn ask(&self, kind: PromptKind) -> Option<PromptAnswer> {
        let (reply, answer) = oneshot::channel();
        self.messages
            .send(Msg::Prompt(PromptRequest { kind, reply }))
            .ok()?;
        answer.await.ok()
    }
}

#[async_trait]
impl Prompter for ActorPrompter {
    async fn credential(&self, request: CredentialPrompt) -> Option<CredentialAnswer> {
        match self.ask(PromptKind::Credential(request)).await {
            Some(PromptAnswer::Credential(answer)) => answer,
            _ => None,
        }
    }

    async fn host_key(&self, request: HostKeyPrompt) -> TrustDecision {
        match self.ask(PromptKind::HostKey(request)).await {
            Some(PromptAnswer::Trust(decision)) => decision,
            _ => TrustDecision::Reject,
        }
    }

    async fn certificate(&self, request: CertificatePrompt) -> TrustDecision {
        match self.ask(PromptKind::Certificate(request)).await {
            Some(PromptAnswer::Trust(decision)) => decision,
            _ => TrustDecision::Reject,
        }
    }
}

impl Core {
    /// A prompt from a background task: queued behind the ones showing.
    pub(crate) fn request_prompt(&mut self, request: PromptRequest) {
        self.note_prompt(&request.kind);
        let id = self.enqueue_prompt(request.kind);
        self.replies.insert(id, request.reply);
    }

    /// Only the prompt the UI is showing (the front of the queue) can be answered; anything
    /// else is stale or unknown and is ignored.
    pub(crate) fn answer_prompt(&mut self, id: PromptId, answer: PromptAnswer) {
        if self.prompts.front().map(|p| p.id) != Some(id) {
            tracing::debug!(target: "filecargo::app", ?id, "ignoring an answer to a prompt that is not showing");
            return;
        }
        self.prompts.pop_front();
        if let Some(action) = self.actions.remove(&id) {
            match (action, &answer) {
                (PromptAction::Delete { names }, PromptAnswer::Confirm(true)) => {
                    self.run_delete(names)
                }
                (PromptAction::Conflict { transfer }, _) => self.answer_conflict(transfer, &answer),
                (PromptAction::Quit, PromptAnswer::Confirm(true)) => self.quitting = true,
                (PromptAction::Quit, _) => {}
                (PromptAction::Delete { .. }, _) => {}
            }
        }
        if let Some(reply) = self.replies.remove(&id) {
            let _ = reply.send(answer);
        }
        self.sync_prompt();
    }
}
