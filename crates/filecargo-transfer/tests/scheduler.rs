#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use filecargo_transfer::{Direction, ItemState, NewTransfer, Outcome};
use support::{Harness, remote, sample_bytes, sha256_hex};

fn upload(h: &Harness, name: &str) -> NewTransfer {
    NewTransfer {
        site: h.site,
        direction: Direction::Upload,
        local: h.local.path().join(name),
        remote: remote(&format!("/{name}")),
        is_dir: false,
        size: None,
        conflict: None,
    }
}

fn download(h: &Harness, name: &str) -> NewTransfer {
    NewTransfer {
        site: h.site,
        direction: Direction::Download,
        local: h.local.path().join("down").join(name),
        remote: remote(&format!("/{name}")),
        is_dir: false,
        size: None,
        conflict: None,
    }
}

#[tokio::test]
async fn twenty_uploads_with_three_slots_never_exceed_three_and_arrive_intact() {
    let mut h = Harness::new(3).await;
    h.connector.set_delay(Duration::from_millis(30));
    let names: Vec<String> = (0..20).map(|i| format!("file-{i:02}.bin")).collect();
    for (i, name) in names.iter().enumerate() {
        std::fs::write(
            h.local.path().join(name),
            sample_bytes(i as u64, 1000 + i * 977),
        )
        .unwrap();
    }
    let ids = h
        .queue
        .enqueue(names.iter().map(|n| upload(&h, n)).collect());
    assert_eq!(ids.len(), 20);
    h.idle().await;

    assert!(
        h.connector.stats.peak() <= 3,
        "peak was {}",
        h.connector.stats.peak()
    );
    assert_eq!(
        h.connector.stats.peak(),
        3,
        "three slots should actually run in parallel"
    );
    let snapshot = h.queue.snapshot();
    assert_eq!(
        (
            snapshot.pending.len(),
            snapshot.completed.len(),
            snapshot.failed.len()
        ),
        (0, 20, 0)
    );
    for view in &snapshot.completed {
        assert!(matches!(
            view.item.state,
            ItemState::Completed {
                outcome: Outcome::Transferred,
                ..
            }
        ));
    }
    for name in &names {
        let sent = std::fs::read(h.local.path().join(name)).unwrap();
        let arrived = std::fs::read(h.server.path().join(name)).unwrap();
        assert_eq!(sha256_hex(&arrived), sha256_hex(&sent), "{name}");
    }
}

#[tokio::test]
async fn downloads_land_in_new_directories_with_identical_bytes() {
    let mut h = Harness::new(2).await;
    for i in 0..6u64 {
        std::fs::write(
            h.server.path().join(format!("d{i}.bin")),
            sample_bytes(i, 5000 + i as usize),
        )
        .unwrap();
    }
    h.queue
        .enqueue((0..6).map(|i| download(&h, &format!("d{i}.bin"))).collect());
    h.idle().await;
    assert_eq!(h.queue.snapshot().completed.len(), 6);
    for i in 0..6u64 {
        let got = std::fs::read(h.local.path().join("down").join(format!("d{i}.bin"))).unwrap();
        assert_eq!(
            sha256_hex(&got),
            sha256_hex(&sample_bytes(i, 5000 + i as usize))
        );
    }
}

#[tokio::test]
async fn connections_are_reused_for_consecutive_items_of_one_site() {
    let mut h = Harness::new(1).await;
    for i in 0..5 {
        std::fs::write(h.local.path().join(format!("f{i}")), b"x").unwrap();
    }
    h.queue
        .enqueue((0..5).map(|i| upload(&h, &format!("f{i}"))).collect());
    h.idle().await;
    assert_eq!(
        h.connector.stats.connects(),
        1,
        "one slot, one site: one connection"
    );
}

#[tokio::test]
async fn pausing_starts_nothing_new_and_resuming_continues() {
    let mut h = Harness::new(2).await;
    std::fs::write(h.local.path().join("a"), b"aaa").unwrap();
    h.queue.set_processing(false);
    h.queue.enqueue(vec![upload(&h, "a")]);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        h.connector.stats.connects(),
        0,
        "nothing may start while paused"
    );
    let snapshot = h.queue.snapshot();
    assert_eq!((snapshot.pending.len(), snapshot.completed.len()), (1, 0));
    assert!(!snapshot.processing);

    h.queue.set_processing(true);
    h.idle().await;
    assert_eq!(h.queue.snapshot().completed.len(), 1);
    assert_eq!(std::fs::read(h.server.path().join("a")).unwrap(), b"aaa");
}

#[tokio::test]
async fn a_missing_source_fails_the_item_with_a_reason_and_keeps_the_queue_going() {
    let mut h = Harness::new(2).await;
    std::fs::write(h.local.path().join("good"), b"ok").unwrap();
    h.queue
        .enqueue(vec![upload(&h, "missing"), upload(&h, "good")]);
    h.idle().await;
    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.completed.len(), 1);
    assert_eq!(snapshot.failed.len(), 1);
    match &snapshot.failed[0].item.state {
        ItemState::Failed {
            reason, retryable, ..
        } => {
            assert!(reason.contains("missing"), "{reason}");
            assert!(!retryable);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_site_fails_with_site_was_deleted() {
    let mut h = Harness::new(1).await;
    let mut transfer = upload(&h, "x");
    transfer.site = filecargo_config::SiteId::new();
    std::fs::write(h.local.path().join("x"), b"x").unwrap();
    h.queue.enqueue(vec![transfer]);
    h.idle().await;
    match &h.queue.snapshot().failed[0].item.state {
        ItemState::Failed { reason, .. } => {
            assert!(reason.contains("site was deleted"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
}

fn held(h: &Harness) -> Vec<filecargo_transfer::QueueItemView> {
    h.queue
        .snapshot()
        .pending
        .iter()
        .filter(|v| matches!(v.item.state, ItemState::Held))
        .cloned()
        .collect()
}

#[tokio::test]
async fn held_items_wait_forever_even_with_free_workers() {
    let h = Harness::new(3).await;
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(h.local.path().join(name), name).unwrap();
    }
    let ids = h
        .queue
        .enqueue_held(["a.txt", "b.txt", "c.txt"].map(|n| upload(&h, n)).to_vec());
    assert_eq!(ids.len(), 3);
    tokio::time::sleep(Duration::from_millis(600)).await;

    let snapshot = h.queue.snapshot();
    assert_eq!(
        snapshot.pending.len(),
        3,
        "held items are listed as pending"
    );
    assert!(
        snapshot
            .pending
            .iter()
            .all(|v| matches!(v.item.state, ItemState::Held)),
        "{snapshot:#?}"
    );
    assert!(snapshot.completed.is_empty() && snapshot.failed.is_empty());
    assert_eq!(h.connector.stats.connects(), 0, "nothing connected");
    assert!(std::fs::read_dir(h.server.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn start_held_releases_only_that_sites_items() {
    let mut h = Harness::new(2).await;
    let other_server = tempfile::tempdir().unwrap();
    let other = h.connector.site(other_server.path());
    for name in ["a.txt", "b.txt"] {
        std::fs::write(h.local.path().join(name), name).unwrap();
    }
    let mut theirs = upload(&h, "b.txt");
    theirs.site = other;
    h.queue.enqueue_held(vec![upload(&h, "a.txt"), theirs]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(held(&h).len(), 2);

    h.queue.start_held(h.site);
    h.idle().await;

    assert_eq!(
        std::fs::read(h.server.path().join("a.txt")).unwrap(),
        b"a.txt"
    );
    let remaining = held(&h);
    assert_eq!(remaining.len(), 1, "the other site's item is still held");
    assert_eq!(remaining[0].item.site, other);
    assert!(!other_server.path().join("b.txt").exists());
    assert_eq!(h.queue.snapshot().completed.len(), 1);
}

#[tokio::test]
async fn a_held_directory_is_one_item_until_started_and_then_transfers_its_files() {
    let mut h = Harness::new(2).await;
    let dir = h.local.path().join("album");
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("one.jpg"), "1").unwrap();
    std::fs::write(dir.join("inner/two.jpg"), "22").unwrap();
    h.queue.enqueue_held(vec![NewTransfer {
        site: h.site,
        direction: Direction::Upload,
        local: dir.clone(),
        remote: remote("/album"),
        is_dir: true,
        size: None,
        conflict: None,
    }]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(held(&h).len(), 1);
    assert!(!h.server.path().join("album").exists());

    h.queue.start_held(h.site);
    h.idle().await;
    assert_eq!(
        std::fs::read(h.server.path().join("album/inner/two.jpg")).unwrap(),
        b"22"
    );
    assert_eq!(
        std::fs::read(h.server.path().join("album/one.jpg")).unwrap(),
        b"1"
    );
    assert!(held(&h).is_empty());
}

#[tokio::test]
async fn normal_items_run_while_other_items_stay_held() {
    let mut h = Harness::new(2).await;
    for name in ["a.txt", "b.txt"] {
        std::fs::write(h.local.path().join(name), name).unwrap();
    }
    h.queue.enqueue_held(vec![upload(&h, "a.txt")]);
    h.queue.enqueue(vec![upload(&h, "b.txt")]);
    h.idle().await;
    assert!(h.server.path().join("b.txt").exists());
    assert!(!h.server.path().join("a.txt").exists());
    assert_eq!(held(&h).len(), 1, "idle fires although a held item remains");
}
