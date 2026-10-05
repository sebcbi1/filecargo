#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! FTP control-connection behavior (listing, metadata ops, login) against docker ProFTPD.

mod support;

use std::sync::Arc;

use filecargo_config::{MemoryStore, Paths};
use filecargo_remote_fs::{
    ConnectContext, ConnectError, CredentialPrompt, EntryKind, FsError, RemotePath, connect,
};
use support::{TestPrompter, docker_ftp, docker_ftp_site};

fn p(base: &RemotePath, name: &str) -> RemotePath {
    name.split('/')
        .fold(base.clone(), |acc, part| acc.join(part).unwrap())
}

async fn workdir(
    port: u16,
    name: &str,
) -> (filecargo_remote_fs::Session, RemotePath, tempfile::TempDir) {
    let (session, _, dir) = docker_ftp(port).await;
    let base = session
        .info
        .home
        .join(&format!("fc-{name}-{port}-{}", std::process::id()))
        .unwrap();
    session.fs.mkdir(&base).await.unwrap();
    (session, base, dir)
}

#[tokio::test]
async fn ftp_session_reports_protocol_and_a_home_directory() {
    let (session, _, _dir) = docker_ftp(2121).await;
    assert_eq!(session.info.protocol, filecargo_config::Protocol::Ftp);
    assert!(session.shell.is_none(), "FTP sessions have no shell");
    assert!(
        session
            .fs
            .stat(&session.info.home)
            .await
            .unwrap()
            .unwrap()
            .is_dir()
    );
    session.fs.close().await;
}

#[tokio::test]
async fn ftp_directory_operations_work_with_mlsd_and_with_the_list_fallback() {
    for port in [2121, 2123] {
        let (session, base, _dir) = workdir(port, "ops").await;
        let fs = &session.fs;

        fs.mkdir(&p(&base, "alpha")).await.unwrap();
        fs.mkdir(&p(&base, "alpha/inner")).await.unwrap();
        assert!(
            matches!(
                fs.mkdir(&p(&base, "alpha")).await,
                Err(FsError::AlreadyExists(_))
            ),
            "port {port}"
        );

        let names: Vec<_> = fs
            .list(&base)
            .await
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.kind))
            .collect();
        assert_eq!(names, [("alpha".to_owned(), EntryKind::Dir)], "port {port}");

        let stat = fs.stat(&p(&base, "alpha")).await.unwrap().unwrap();
        assert!(stat.is_dir(), "port {port}");
        assert_eq!(
            fs.stat(&p(&base, "nope")).await.unwrap(),
            None,
            "port {port}"
        );

        assert!(
            matches!(
                fs.remove_dir(&p(&base, "alpha")).await,
                Err(FsError::DirectoryNotEmpty(_))
            ),
            "port {port}"
        );

        fs.rename(&p(&base, "alpha/inner"), &p(&base, "beta"))
            .await
            .unwrap();
        assert!(fs.stat(&p(&base, "beta")).await.unwrap().is_some());
        assert_eq!(fs.stat(&p(&base, "alpha/inner")).await.unwrap(), None);

        fs.chmod(&p(&base, "beta"), 0o750).await.unwrap();
        let mode = fs
            .stat(&p(&base, "beta"))
            .await
            .unwrap()
            .unwrap()
            .permissions;
        assert_eq!(mode.map(|m| m & 0o777), Some(0o750), "port {port}");

        fs.remove_dir(&p(&base, "beta")).await.unwrap();
        fs.remove_all(&base).await.unwrap();
        assert!(
            matches!(fs.list(&base).await, Err(FsError::NotFound(_))),
            "port {port}"
        );
        fs.close().await;
    }
}

#[tokio::test]
async fn ftp_unicode_directory_names_round_trip() {
    let (session, base, _dir) = workdir(2121, "uni").await;
    let name = "dossier été ☃ (1)";
    session.fs.mkdir(&p(&base, name)).await.unwrap();
    let names: Vec<_> = session
        .fs
        .list(&base)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, [name]);
    session.fs.remove_all(&base).await.unwrap();
}

#[tokio::test]
async fn ftp_concurrent_calls_on_one_connection_are_serialized() {
    let (session, base, _dir) = workdir(2121, "conc").await;
    let fs = session.fs.clone();
    let tasks: Vec<_> = (0..12)
        .map(|i| {
            let (fs, base) = (fs.clone(), base.clone());
            tokio::spawn(async move {
                let dir = p(&base, &format!("d{i}"));
                fs.mkdir(&dir).await.unwrap();
                assert!(fs.stat(&dir).await.unwrap().unwrap().is_dir());
                fs.list(&base).await.unwrap().len()
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(fs.list(&base).await.unwrap().len(), 12);
    fs.remove_all(&base).await.unwrap();
}

#[tokio::test]
async fn ftp_line_breaks_in_paths_are_refused_before_reaching_the_server() {
    let (session, base, _dir) = workdir(2121, "inj").await;
    let evil = base.join("a\r\nDELE x").unwrap();
    for result in [
        session.fs.mkdir(&evil).await,
        session.fs.remove_file(&evil).await,
    ] {
        assert!(
            matches!(result, Err(FsError::Protocol { .. })),
            "{result:?}"
        );
    }
    session.fs.remove_all(&base).await.unwrap();
}

async fn login_with(
    passwords: &[&str],
    remember_stale: Option<&str>,
) -> (Result<(), ConnectError>, Arc<TestPrompter>) {
    let dir = tempfile::tempdir().unwrap();
    let prompter = TestPrompter::new();
    for password in passwords {
        prompter.answer(&[password], false);
    }
    let secrets = Arc::new(MemoryStore::new());
    let mut site = docker_ftp_site(2121);
    if let Some(stale) = remember_stale {
        site.auth = filecargo_config::Auth::Password { remember: true };
        use filecargo_config::{SecretKey, SecretStore, SecretString};
        secrets
            .set(
                &SecretKey::Password(site.id),
                &SecretString::from(stale.to_owned()),
            )
            .unwrap();
    }
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().to_path_buf())),
        secrets,
        prompter.clone(),
    );
    ctx.user_known_hosts = None;
    let result = connect(&site, &ctx).await.map(drop);
    (result, prompter)
}

#[tokio::test]
async fn ftp_rejected_stored_password_prompts_once_with_retry() {
    let (result, prompter) = login_with(&["ftppass"], Some("stale")).await;
    result.unwrap();
    let prompts = prompter.credential_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(matches!(
        &prompts[0],
        CredentialPrompt::Password { retry: true, .. }
    ));
}

#[tokio::test]
async fn ftp_two_wrong_passwords_are_auth_failed() {
    let (result, prompter) = login_with(&["nope", "still-nope"], None).await;
    assert!(
        matches!(result, Err(ConnectError::AuthFailed { .. })),
        "{result:?}"
    );
    assert_eq!(prompter.credential_prompt_count(), 2);
}

#[tokio::test]
async fn ftp_connection_refused_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().to_path_buf())),
        Arc::new(MemoryStore::new()),
        TestPrompter::new(),
    );
    ctx.user_known_hosts = None;
    let result = connect(&docker_ftp_site(1), &ctx).await;
    assert!(
        matches!(result, Err(ConnectError::Refused(_))),
        "{:?}",
        result.err()
    );
}
