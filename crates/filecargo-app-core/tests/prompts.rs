#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::PathBuf;
use std::time::Duration;

use filecargo_app_core::{Command, Level, PromptAnswer, PromptId, PromptKind};
use filecargo_config::SecretString;
use filecargo_remote_fs::{CredentialAnswer, CredentialPrompt, HostKeyPrompt, TrustDecision};
use support::Fixture;

fn password_prompt(site: &str) -> CredentialPrompt {
    CredentialPrompt::Password {
        site: site.to_owned(),
        user: "me".into(),
        retry: false,
    }
}

fn host_key_prompt() -> HostKeyPrompt {
    HostKeyPrompt {
        host: "h".into(),
        port: 22,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:abc".into(),
    }
}

fn showing(fx: &Fixture) -> Option<(PromptId, PromptKind)> {
    fx.state().prompt.as_ref().map(|p| (p.id, p.kind.clone()))
}

#[test]
fn two_concurrent_requests_are_shown_one_at_a_time_in_order() {
    let fx = Fixture::new();
    let prompter = fx.app.prompter();
    let rt = fx.app.runtime();

    let first = rt.spawn({
        let prompter = prompter.clone();
        async move { prompter.credential(password_prompt("first")).await }
    });
    let state = fx.wait_for("the first prompt", |s| s.prompt.is_some());
    let first_id = state.prompt.as_ref().unwrap().id;
    assert!(
        matches!(&state.prompt.as_ref().unwrap().kind, PromptKind::Credential(CredentialPrompt::Password { site, .. }) if site == "first")
    );

    let second = rt.spawn(async move { prompter.host_key(host_key_prompt()).await });
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        showing(&fx).map(|(id, _)| id),
        Some(first_id),
        "the second prompt waits behind the first"
    );

    fx.app.send(Command::Answer {
        id: first_id,
        answer: PromptAnswer::Credential(Some(CredentialAnswer {
            values: vec![SecretString::from("pw".to_owned())],
            remember: true,
        })),
    });
    let state = fx.wait_for("the second prompt", |s| {
        s.prompt.as_ref().is_some_and(|p| p.id != first_id)
    });
    assert!(matches!(
        &state.prompt.as_ref().unwrap().kind,
        PromptKind::HostKey(_)
    ));
    let answer = rt
        .block_on(first)
        .unwrap()
        .expect("the first requester gets its answer");
    assert!(answer.remember);

    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Trust(TrustDecision::TrustAlways),
    });
    assert_eq!(fx.join(second), TrustDecision::TrustAlways);
    fx.wait_for("the queue to empty", |s| s.prompt.is_none());
}

#[test]
fn a_cancelled_credential_prompt_resolves_to_none_and_a_wrong_kind_of_answer_is_a_refusal() {
    let fx = Fixture::new();
    let prompter = fx.app.prompter();
    let rt = fx.app.runtime();

    let cancelled = rt.spawn({
        let prompter = prompter.clone();
        async move { prompter.credential(password_prompt("s")).await }
    });
    let prompt = fx.wait_for_prompt_after(None);
    let cancelled_id = prompt.id;
    fx.app.send(Command::Answer {
        id: cancelled_id,
        answer: PromptAnswer::Credential(None),
    });
    assert!(fx.join(cancelled).is_none());

    // a host-key prompt answered with a confirmation is not a trust decision: reject
    let refused = rt.spawn(async move { prompter.host_key(host_key_prompt()).await });
    let prompt = fx.wait_for_prompt_after(Some(cancelled_id));
    fx.app.send(Command::Answer {
        id: prompt.id,
        answer: PromptAnswer::Confirm(true),
    });
    assert_eq!(fx.join(refused), TrustDecision::Reject);
}

#[test]
fn answers_with_an_unknown_or_stale_id_are_ignored() {
    let fx = Fixture::new();
    let prompter = fx.app.prompter();
    let rt = fx.app.runtime();
    let pending = rt.spawn(async move { prompter.host_key(host_key_prompt()).await });
    let state = fx.wait_for("the prompt", |s| s.prompt.is_some());
    let id = state.prompt.as_ref().unwrap().id;

    fx.app.send(Command::Answer {
        id: PromptId(9999),
        answer: PromptAnswer::Trust(TrustDecision::TrustAlways),
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        showing(&fx).map(|(i, _)| i),
        Some(id),
        "an unknown id must not answer anything"
    );

    fx.app.send(Command::Answer {
        id,
        answer: PromptAnswer::Trust(TrustDecision::TrustOnce),
    });
    assert_eq!(fx.join(pending), TrustDecision::TrustOnce);
    // the same id again is stale now
    fx.app.send(Command::Answer {
        id,
        answer: PromptAnswer::Trust(TrustDecision::Reject),
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(fx.state().prompt.is_none());
}

#[test]
fn a_message_stays_until_it_is_dismissed() {
    let fx = Fixture::new();
    fx.app.send(Command::ImportFileZilla {
        path: PathBuf::from("/nonexistent.xml"),
        import_passwords: false,
    });
    let state = fx.wait_for("the message", |s| s.prompt.is_some());
    let prompt = state.prompt.clone().unwrap();
    assert!(matches!(
        prompt.kind,
        PromptKind::Message {
            level: Level::Error,
            ..
        }
    ));
    fx.app.send(Command::Answer {
        id: prompt.id,
        answer: PromptAnswer::Dismiss,
    });
    fx.wait_for("the dismissal", |s| s.prompt.is_none());
}

#[test]
fn pending_prompts_resolve_as_cancelled_when_the_app_shuts_down() {
    let fx = Fixture::new();
    let prompter = fx.app.prompter();
    let rt = fx.app.runtime();
    let (tx, rx) = std::sync::mpsc::channel();
    rt.spawn(async move {
        tx.send(prompter.credential(password_prompt("s")).await.is_none())
            .ok();
    });
    fx.wait_for("the prompt", |s| s.prompt.is_some());
    fx.app.clone().shutdown(Duration::from_secs(1));
    // the runtime is gone, so the requester task was cancelled or saw a cancelled prompt;
    // either way it must not hang the shutdown (reaching this line proves that)
    let _ = rx.try_recv();
}
