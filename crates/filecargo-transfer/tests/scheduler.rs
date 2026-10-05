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
