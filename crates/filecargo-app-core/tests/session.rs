#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_app_core::{
    AppState, Command, ConnectStep, Level, PaneId, PromptAnswer, PromptKind, SessionState,
};
use filecargo_config::{SecretKey, SecretStore, SecretString};
use filecargo_remote_fs::{ConnectError, CredentialAnswer};
use support::{Fixture, TestFactory, server_tree, site_for};

fn remote_names(state: &AppState) -> Vec<String> {
    state
        .remote
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect()
}

fn connected_to(state: &AppState) -> Option<filecargo_config::SiteId> {
    match &state.session {
        SessionState::Connected { site, .. } => Some(*site),
        _ => None,
    }
}

#[test]
fn connecting_goes_through_connecting_to_connected_and_lists_the_sites_remote_dir() {
    let server = server_tree();
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("mine.txt"), "x").unwrap();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), None);
    *factory.delay.lock().unwrap() = Duration::from_millis(200);
    let fx = Fixture::with_factory(factory.clone());
    let mut site = site_for("a", "host-a");
    site.remote_dir = Some("/projects".to_owned());
    site.local_dir = Some(local.path().to_path_buf());
    let id = fx.add_site(site);

    fx.app.send(Command::Connect(id));
    let connecting = fx.wait_for("connecting", |s| {
        matches!(s.session, SessionState::Connecting { .. })
    });
    assert!(
        matches!(connecting.session, SessionState::Connecting { site, step: ConnectStep::Connecting } if site == id)
    );
    assert!(connecting.remote.is_none());

    let state = fx.wait_for("connected", |s| {
        connected_to(s) == Some(id) && s.remote.is_some()
    });
    assert_eq!(state.remote.as_ref().unwrap().path.as_str(), "/projects");
    assert_eq!(remote_names(&state), ["sub", "a.txt", "b.txt"]);
    let state = fx.wait_for("the local pane to follow the site", |s| {
        s.local.path.ends_with(local.path().file_name().unwrap())
    });
    assert_eq!(
        state
            .local
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["mine.txt"]
    );
}

#[test]
fn a_missing_remote_dir_falls_back_to_the_servers_home() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory);
    let mut site = site_for("a", "host-a");
    site.remote_dir = Some("/nope/not/here".to_owned());
    let id = fx.add_site(site);
    fx.app.send(Command::Connect(id));
    let state = fx.wait_for("connected", |s| s.remote.is_some());
    assert_eq!(state.remote.as_ref().unwrap().path.as_str(), "/");
    assert_eq!(remote_names(&state), ["projects", "readme.md"]);
}

#[test]
fn connecting_to_a_second_site_closes_the_first() {
    let (one, two) = (server_tree(), server_tree());
    std::fs::write(two.path().join("only-on-two.txt"), "2").unwrap();
    let factory = TestFactory::new();
    factory.serve("host-a", one.path().to_path_buf(), None);
    factory.serve("host-b", two.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory.clone());
    let (a, b) = (
        fx.add_site(site_for("a", "host-a")),
        fx.add_site(site_for("b", "host-b")),
    );

    fx.app.send(Command::Connect(a));
    fx.wait_for("a", |s| connected_to(s) == Some(a) && s.remote.is_some());
    fx.app.send(Command::Connect(b));
    let state = fx.wait_for("b", |s| connected_to(s) == Some(b) && s.remote.is_some());
    assert!(remote_names(&state).contains(&"only-on-two.txt".to_owned()));
    assert_eq!(factory.connects(), 2);
    assert_eq!(factory.closes(), 1, "the first session must be closed");

    fx.app.send(Command::Disconnect);
    let state = fx.wait_for("disconnected", |s| s.session == SessionState::Disconnected);
    assert!(state.remote.is_none());
    for _ in 0..50 {
        if factory.closes() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(factory.closes(), 2);
}

#[test]
fn a_password_prompt_appears_while_authenticating_and_answering_continues_the_connect() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), Some("hunter2"));
    let fx = Fixture::with_factory(factory);
    let id = fx.add_site(site_for("a", "host-a"));

    fx.app.send(Command::Connect(id));
    let state = fx.wait_for("the credential prompt", |s| s.prompt.is_some());
    assert!(matches!(
        state.prompt.as_ref().unwrap().kind,
        PromptKind::Credential(_)
    ));
    assert!(
        matches!(
            state.session,
            SessionState::Connecting {
                step: ConnectStep::Authenticating,
                ..
            }
        ),
        "{:?}",
        state.session
    );

    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Credential(Some(CredentialAnswer {
            values: vec![SecretString::from("hunter2".to_owned())],
            remember: false,
        })),
    });
    fx.wait_for("connected", |s| {
        connected_to(s) == Some(id) && s.remote.is_some()
    });
}

#[test]
fn cancelling_the_password_prompt_fails_the_connect_without_writing_the_keychain() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), Some("pw"));
    let keychain = std::sync::Arc::new(filecargo_config::MemoryStore::new());
    let config = tempfile::tempdir().unwrap();
    let app = filecargo_app_core::App::start(filecargo_app_core::StartOptions {
        paths: Some(filecargo_config::Paths::from_override(Some(
            config.path().to_path_buf(),
        ))),
        secrets: Some(keychain.clone()),
        connector: Some(factory),
    })
    .unwrap();
    let fx = Fixture { app, config };
    let site = site_for("a", "host-a");
    let id = fx.add_site(site);

    fx.app.send(Command::Connect(id));
    let state = fx.wait_for("the credential prompt", |s| s.prompt.is_some());
    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Credential(None),
    });
    let state = fx.wait_for("failed", |s| {
        matches!(s.session, SessionState::Failed { .. })
    });
    assert!(state.remote.is_none());
    assert!(
        state.prompt.is_none(),
        "a cancelled prompt is not an error to show"
    );
    assert!(
        keychain.get(&SecretKey::Password(id)).unwrap().is_none(),
        "nothing may be stored"
    );
}

#[test]
fn a_remembered_password_reaches_the_keychain_only_when_the_user_says_so() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), Some("pw"));
    let keychain = std::sync::Arc::new(filecargo_config::MemoryStore::new());
    let config = tempfile::tempdir().unwrap();
    let app = filecargo_app_core::App::start(filecargo_app_core::StartOptions {
        paths: Some(filecargo_config::Paths::from_override(Some(
            config.path().to_path_buf(),
        ))),
        secrets: Some(keychain.clone()),
        connector: Some(factory),
    })
    .unwrap();
    let fx = Fixture { app, config };
    let id = fx.add_site(site_for("a", "host-a"));
    fx.app.send(Command::Connect(id));
    let state = fx.wait_for("the prompt", |s| s.prompt.is_some());
    fx.app.send(Command::Answer {
        id: state.prompt.as_ref().unwrap().id,
        answer: PromptAnswer::Credential(Some(CredentialAnswer {
            values: vec![SecretString::from("pw".to_owned())],
            remember: true,
        })),
    });
    fx.wait_for("connected", |s| s.remote.is_some());
    assert!(keychain.get(&SecretKey::Password(id)).unwrap().is_some());
}

#[test]
fn auth_and_tls_failures_show_a_message_but_network_failures_only_fail() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory.clone());
    let id = fx.add_site(site_for("a", "host-a"));

    *factory.fail_next.lock().unwrap() = Some(ConnectError::Refused("host-a:22".into()));
    fx.app.send(Command::Connect(id));
    let state = fx.wait_for("failed", |s| {
        matches!(s.session, SessionState::Failed { .. })
    });
    assert!(
        state.prompt.is_none(),
        "a refused connection needs no popup: {:?}",
        state.prompt
    );
    assert!(
        matches!(&state.session, SessionState::Failed { error, .. } if error.contains("refused"))
    );

    *factory.fail_next.lock().unwrap() = Some(ConnectError::AuthFailed {
        methods_tried: vec!["password".into()],
    });
    fx.app.send(Command::Connect(id));
    let state = fx.wait_for("the message", |s| s.prompt.is_some());
    match &state.prompt.as_ref().unwrap().kind {
        PromptKind::Message { level, title, body } => {
            assert_eq!(
                (*level, title.as_str()),
                (Level::Error, "Connection failed")
            );
            assert!(body.contains("authentication failed"), "{body}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_lost_connection_fails_the_session_and_the_next_pane_command_reconnects_once() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory.clone());
    let id = fx.add_site(site_for("a", "host-a"));
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());

    factory.kill_latest(); // the network drops
    fx.app.send(Command::Refresh(PaneId::Remote));
    let state = fx.wait_for("failed", |s| {
        matches!(s.session, SessionState::Failed { .. })
    });
    assert!(
        matches!(&state.session, SessionState::Failed { error, .. } if error.contains("connection reset")),
        "{:?}",
        state.session
    );

    // the next remote command reconnects and then runs
    fx.app.send(Command::Navigate {
        pane: PaneId::Remote,
        path: "/projects".into(),
    });
    let state = fx.wait_for("reconnected", |s| {
        connected_to(s) == Some(id)
            && s.remote
                .as_ref()
                .is_some_and(|p| p.path.as_str() == "/projects")
    });
    assert_eq!(remote_names(&state), ["sub", "a.txt", "b.txt"]);
    assert_eq!(factory.connects(), 2);
}

#[test]
fn a_failed_reconnect_is_not_retried_automatically() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory.clone());
    let id = fx.add_site(site_for("a", "host-a"));
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());

    factory.kill_latest();
    fx.app.send(Command::Refresh(PaneId::Remote));
    fx.wait_for("failed", |s| {
        matches!(s.session, SessionState::Failed { .. })
    });

    *factory.fail_next.lock().unwrap() = Some(ConnectError::Timeout);
    fx.app.send(Command::Refresh(PaneId::Remote)); // triggers the one automatic reconnect, which fails
    fx.wait_for(
        "failed again",
        |s| matches!(&s.session, SessionState::Failed { error, .. } if error.contains("timed out")),
    );
    let after_failure = factory.connects();

    fx.app.send(Command::Refresh(PaneId::Remote)); // no second automatic attempt
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert!(
        state
            .notices
            .iter()
            .any(|n| n.text.contains("Not connected")),
        "{:?}",
        state.notices
    );
    assert_eq!(factory.connects(), after_failure);
}

#[test]
fn remote_navigation_up_sort_and_errors() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host-a", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory);
    let id = fx.add_site(site_for("a", "host-a"));
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());

    fx.app.send(Command::Navigate {
        pane: PaneId::Remote,
        path: "projects/sub".into(),
    });
    let state = fx.wait_for("sub", |s| {
        s.remote
            .as_ref()
            .is_some_and(|p| p.path.as_str() == "/projects/sub" && !p.loading)
    });
    assert_eq!(remote_names(&state), ["c.txt"]);

    fx.app.send(Command::Up(PaneId::Remote));
    fx.wait_for("up", |s| {
        s.remote
            .as_ref()
            .is_some_and(|p| p.path.as_str() == "/projects" && !p.loading)
    });

    fx.app.send(Command::Navigate {
        pane: PaneId::Remote,
        path: "../../..".into(),
    }); // above the root
    let state = fx.wait_for("an error", |s| {
        s.remote.as_ref().is_some_and(|p| p.error.is_some())
    });
    assert_eq!(
        state.remote.as_ref().unwrap().path.as_str(),
        "/projects",
        "the pane stays where it was"
    );

    fx.app.send(Command::Navigate {
        pane: PaneId::Remote,
        path: "/missing".into(),
    });
    let state = fx.wait_for("not found", |s| {
        s.remote
            .as_ref()
            .is_some_and(|p| p.error.as_deref().is_some_and(|e| e.contains("not found")))
    });
    assert_eq!(
        remote_names(&state),
        ["sub", "a.txt", "b.txt"],
        "the old entries stay"
    );

    fx.app.send(Command::SetSort {
        pane: PaneId::Remote,
        sort: filecargo_app_core::Sort {
            key: filecargo_app_core::SortKey::Name,
            ascending: false,
        },
    });
    let state = fx.wait_for("sorted", |s| {
        s.remote.as_ref().is_some_and(|p| !p.sort.ascending)
    });
    assert_eq!(remote_names(&state), ["sub", "b.txt", "a.txt"]);
}

#[test]
fn remote_commands_without_a_session_only_raise_a_notice() {
    let fx = Fixture::new();
    fx.app.send(Command::Refresh(PaneId::Remote));
    let state = fx.wait_for("a notice", |s| !s.notices.is_empty());
    assert!(state.notices[0].text.contains("Not connected"));
    assert!(state.remote.is_none());
}
