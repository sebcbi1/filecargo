#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! AC8: the 1,000-file round trip against the real docker SFTP and FTP servers, through the
//! real `remote_fs::connect`, with `max_concurrent = 4`.

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use filecargo_config::{Auth, MemoryStore, Paths, Protocol, SiteId};
use filecargo_remote_fs::{
    CertificatePrompt, ConnectContext, CredentialAnswer, CredentialPrompt, HostKeyPrompt, Prompter,
    RemoteFs, RemotePath, TrustDecision, connect,
};
use filecargo_transfer::{
    Connector, Direction, NewTransfer, Queue, QueueEvent, QueueLimits, TransferError,
};
use support::{build_tree, tree_hashes};

struct Auto(&'static str);

#[async_trait]
impl Prompter for Auto {
    async fn credential(&self, _: CredentialPrompt) -> Option<CredentialAnswer> {
        Some(CredentialAnswer {
            values: vec![secrecy::SecretString::from(self.0.to_owned())],
            remember: false,
        })
    }
    async fn host_key(&self, _: HostKeyPrompt) -> TrustDecision {
        TrustDecision::TrustOnce
    }
    async fn certificate(&self, _: CertificatePrompt) -> TrustDecision {
        TrustDecision::TrustOnce
    }
}

struct RealConnector {
    sites: HashMap<SiteId, filecargo_config::Site>,
    ctx: ConnectContext,
}

#[async_trait]
impl Connector for RealConnector {
    async fn connect(&self, site: SiteId) -> Result<Arc<dyn RemoteFs>, TransferError> {
        let site = self.sites.get(&site).ok_or(TransferError::SiteDeleted)?;
        connect(site, &self.ctx)
            .await
            .map(|session| session.fs)
            .map_err(|e| TransferError::Connect(e.to_string()))
    }
}

async fn wait_idle(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<QueueEvent>,
    label: &str,
) -> (usize, usize) {
    let mut finished = (0usize, 0usize);
    let wait = async {
        while let Some(event) = events.recv().await {
            match event {
                QueueEvent::Idle => return,
                QueueEvent::ItemFinished { ok: true, .. } => finished.0 += 1,
                QueueEvent::ItemFinished { ok: false, .. } => finished.1 += 1,
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(600), wait)
        .await
        .unwrap_or_else(|_| panic!("{label}: not idle within 10 minutes"));
    finished
}

async fn round_trip(protocol: Protocol, port: u16, user: &str, password: &'static str) {
    let config = tempfile::tempdir().unwrap();
    let mut site = filecargo_config::Site::new("docker", protocol, "127.0.0.1");
    site.port = Some(port);
    site.user = user.to_owned();
    site.auth = Auth::Password { remember: false };
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(config.path().to_path_buf())),
        Arc::new(MemoryStore::new()),
        Arc::new(Auto(password)),
    );
    ctx.user_known_hosts = None;
    let site_id = site.id;
    let connector = Arc::new(RealConnector {
        sites: HashMap::from([(site_id, site.clone())]),
        ctx,
    });

    // an area of the server that is ours alone
    let probe = connect(&site, &connector.ctx).await.unwrap();
    let base = probe
        .info
        .home
        .join(&format!("fc-xfer-{port}-{}", std::process::id()))
        .unwrap();
    probe.fs.mkdir(&base).await.unwrap();

    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("src");
    build_tree(&src, 20);
    let (queue, mut events) = Queue::start(
        connector.clone(),
        config.path().join("queue.json"),
        QueueLimits {
            max_concurrent: 4,
            ..QueueLimits::default()
        },
    )
    .await
    .unwrap();

    let run = |transfer: NewTransfer| {
        queue.enqueue(vec![transfer]);
    };
    let remote_dir = base.join("tree").unwrap();
    run(NewTransfer {
        site: site_id,
        direction: Direction::Upload,
        local: src.clone(),
        remote: remote_dir.clone(),
        is_dir: true,
        size: None,
        conflict: None,
    });
    let up = wait_idle(&mut events, "upload").await;
    assert_eq!(up, (1051, 0), "{protocol:?} upload: (ok, failed)");

    let back = local.path().join("back");
    run(NewTransfer {
        site: site_id,
        direction: Direction::Download,
        local: back.clone(),
        remote: remote_dir,
        is_dir: true,
        size: None,
        conflict: None,
    });
    let down = wait_idle(&mut events, "download").await;
    assert_eq!(down, (1051, 0), "{protocol:?} download: (ok, failed)");

    let (sent_files, sent_dirs) = tree_hashes(&src);
    assert_eq!(sent_files.len(), 1000);
    assert_eq!(
        tree_hashes(&back),
        (sent_files, sent_dirs),
        "{protocol:?}: the round trip must be byte-identical"
    );

    queue.shutdown().await;
    let cleanup: RemotePath = base;
    probe.fs.remove_all(&cleanup).await.unwrap();
    probe.fs.close().await;
}

#[tokio::test]
async fn sftp_thousand_file_round_trip_with_four_workers() {
    round_trip(Protocol::Sftp, 2222, "fcuser", "fcpass").await;
}

#[tokio::test]
async fn ftp_thousand_file_round_trip_with_four_workers() {
    round_trip(Protocol::Ftp, 2121, "ftpuser", "ftppass").await;
}

/// v1.1: held items survive closing the queue, come back held, and transfer once started.
#[tokio::test]
async fn sftp_held_items_survive_a_restart_and_transfer_when_started() {
    let config = tempfile::tempdir().unwrap();
    let mut site = filecargo_config::Site::new("docker", Protocol::Sftp, "127.0.0.1");
    site.port = Some(2222);
    site.user = "fcuser".to_owned();
    site.auth = Auth::Password { remember: false };
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(config.path().to_path_buf())),
        Arc::new(MemoryStore::new()),
        Arc::new(Auto("fcpass")),
    );
    ctx.user_known_hosts = None;
    let site_id = site.id;
    let connector = Arc::new(RealConnector {
        sites: HashMap::from([(site_id, site.clone())]),
        ctx,
    });
    let probe = connect(&site, &connector.ctx).await.unwrap();
    let base = probe
        .info
        .home
        .join(&format!("fc-held-{}", std::process::id()))
        .unwrap();
    probe.fs.mkdir(&base).await.unwrap();

    let local = tempfile::tempdir().unwrap();
    let names = ["one.txt", "two.txt", "three.txt"];
    for name in names {
        std::fs::write(local.path().join(name), name).unwrap();
    }
    let store = config.path().join("queue.json");
    let start = || {
        Queue::start(
            connector.clone(),
            store.clone(),
            QueueLimits {
                max_concurrent: 2,
                ..QueueLimits::default()
            },
        )
    };

    // stage three uploads, then close the queue
    let (queue, _events) = start().await.unwrap();
    let ids = queue.enqueue_held(
        names
            .iter()
            .map(|name| NewTransfer {
                site: site_id,
                direction: Direction::Upload,
                local: local.path().join(name),
                remote: base.join(name).unwrap(),
                is_dir: false,
                size: None,
                conflict: None,
            })
            .collect(),
    );
    assert_eq!(ids.len(), 3);
    for _ in 0..200 {
        if queue.snapshot().pending.len() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(queue.snapshot().pending.len(), 3);
    queue.shutdown().await;

    // reopened from disk: still three held items, and nothing has been sent
    let (queue, mut events) = start().await.unwrap();
    let snapshot = queue.snapshot();
    assert_eq!(snapshot.pending.len(), 3);
    assert!(
        snapshot
            .pending
            .iter()
            .all(|v| matches!(v.item.state, filecargo_transfer::ItemState::Held))
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        probe
            .fs
            .stat(&base.join("one.txt").unwrap())
            .await
            .unwrap()
            .is_none()
    );

    queue.start_held(site_id);
    let done = wait_idle(&mut events, "held items").await;
    assert_eq!(done, (3, 0));
    for name in names {
        assert!(
            probe
                .fs
                .stat(&base.join(name).unwrap())
                .await
                .unwrap()
                .is_some(),
            "{name} arrived"
        );
    }

    queue.shutdown().await;
    probe.fs.remove_all(&base).await.unwrap();
    probe.fs.close().await;
}
