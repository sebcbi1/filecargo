#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Per-site pause, clear and clear-failed: other sites are never affected.

mod support;

use std::time::Duration;

use filecargo_config::SiteId;
use filecargo_transfer::{Direction, ItemState, NewTransfer};
use support::{Harness, remote, sample_bytes};

fn upload(site: SiteId, h: &Harness, name: &str) -> NewTransfer {
    NewTransfer {
        site,
        direction: Direction::Upload,
        local: h.local.path().join(name),
        remote: remote(&format!("/{name}")),
        is_dir: false,
        size: None,
        conflict: None,
    }
}

/// A second site with its own server directory.
fn second_site(h: &Harness) -> (SiteId, tempfile::TempDir) {
    let server = tempfile::tempdir().unwrap();
    (h.connector.site(server.path()), server)
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..400 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn a_paused_site_starts_nothing_while_other_sites_keep_going() {
    let h = Harness::new(2).await;
    let (b, b_server) = second_site(&h);
    for name in ["a.txt", "b.txt"] {
        std::fs::write(h.local.path().join(name), name).unwrap();
    }
    h.queue.set_site_paused(h.site, true);
    h.queue
        .enqueue(vec![upload(h.site, &h, "a.txt"), upload(b, &h, "b.txt")]);

    wait_until("B's file", || b_server.path().join("b.txt").exists()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!h.server.path().join("a.txt").exists(), "A is paused");
    let snapshot = h.queue.snapshot();
    assert_eq!(
        snapshot.paused_sites.iter().copied().collect::<Vec<_>>(),
        [h.site]
    );
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.pending[0].item.site, h.site);

    h.queue.set_site_paused(h.site, false);
    wait_until("A's file", || h.server.path().join("a.txt").exists()).await;
    assert!(h.queue.snapshot().paused_sites.is_empty());
}

#[tokio::test]
async fn pausing_leaves_a_running_item_to_finish() {
    let h = Harness::new(1).await;
    h.connector
        .set_throttle(8 * 1024, Duration::from_millis(50));
    std::fs::write(h.local.path().join("big.bin"), sample_bytes(1, 64 * 1024)).unwrap();
    std::fs::write(h.local.path().join("next.txt"), "next").unwrap();
    h.queue.enqueue(vec![
        upload(h.site, &h, "big.bin"),
        upload(h.site, &h, "next.txt"),
    ]);
    wait_until("the big upload to start", || {
        h.server.path().join("big.bin").exists()
    })
    .await;
    h.queue.set_site_paused(h.site, true);
    wait_until("the running item to finish", || {
        h.server
            .path()
            .join("big.bin")
            .metadata()
            .is_ok_and(|m| m.len() == 64 * 1024)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !h.server.path().join("next.txt").exists(),
        "nothing new starts"
    );
}

#[tokio::test]
async fn clear_cancels_the_sites_active_items_keeps_the_partial_file_and_spares_other_sites() {
    let h = Harness::new(1).await;
    let (b, _b_server) = second_site(&h);
    h.connector
        .set_throttle(4 * 1024, Duration::from_millis(100));
    let total = 256 * 1024;
    std::fs::write(h.local.path().join("big.bin"), sample_bytes(7, total)).unwrap();
    for name in ["p1.txt", "p2.txt", "other.txt"] {
        std::fs::write(h.local.path().join(name), name).unwrap();
    }
    // A: one running (slow) + one pending + one held; B: one held
    h.queue.enqueue(vec![
        upload(h.site, &h, "big.bin"),
        upload(h.site, &h, "p1.txt"),
    ]);
    h.queue.enqueue_held(vec![
        upload(h.site, &h, "p2.txt"),
        upload(b, &h, "other.txt"),
    ]);
    wait_until("a partial upload", || {
        h.server
            .path()
            .join("big.bin")
            .metadata()
            .is_ok_and(|m| m.len() > 0)
    })
    .await;

    h.queue.clear(h.site);
    wait_until("A's queue to empty", || {
        h.queue
            .snapshot()
            .pending
            .iter()
            .all(|v| v.item.site != h.site)
    })
    .await;
    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.pending.len(), 1, "{snapshot:#?}");
    assert_eq!(snapshot.pending[0].item.site, b);
    assert!(
        matches!(snapshot.pending[0].item.state, ItemState::Held),
        "B is untouched"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    let partial = h.server.path().join("big.bin").metadata().unwrap().len();
    assert!(
        partial > 0 && partial < total as u64,
        "the partial file stays: {partial}"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        h.server.path().join("big.bin").metadata().unwrap().len(),
        partial,
        "the cancelled upload stopped writing"
    );
    assert!(!h.server.path().join("p1.txt").exists());
}

#[tokio::test]
async fn clear_failed_and_clear_completed_only_touch_their_site() {
    let mut h = Harness::new(2).await;
    let (b, _b_server) = second_site(&h);
    std::fs::write(h.local.path().join("ok.txt"), "ok").unwrap();
    // missing sources fail; ok.txt completes
    h.queue.enqueue(vec![
        upload(h.site, &h, "missing-a.txt"),
        upload(b, &h, "missing-b.txt"),
        upload(h.site, &h, "ok.txt"),
        upload(b, &h, "ok.txt"),
    ]);
    h.idle().await;
    let snapshot = h.queue.snapshot();
    assert_eq!((snapshot.failed.len(), snapshot.completed.len()), (2, 2));

    h.queue.clear_failed(h.site);
    wait_until("A's failures to go", || {
        h.queue.snapshot().failed.len() == 1
    })
    .await;
    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.failed[0].item.site, b);
    assert_eq!(
        snapshot.completed.len(),
        2,
        "completed items are left alone"
    );

    h.queue.clear_completed_site(b);
    wait_until("B's completed to go", || {
        h.queue.snapshot().completed.len() == 1
    })
    .await;
    assert_eq!(h.queue.snapshot().completed[0].item.site, h.site);
    assert_eq!(h.queue.snapshot().failed.len(), 1);
}

#[tokio::test]
async fn start_held_also_resumes_a_paused_site() {
    let h = Harness::new(1).await;
    std::fs::write(h.local.path().join("a.txt"), "a").unwrap();
    h.queue.enqueue_held(vec![upload(h.site, &h, "a.txt")]);
    h.queue.set_site_paused(h.site, true);
    h.queue.start_held(h.site);
    wait_until("the upload", || h.server.path().join("a.txt").exists()).await;
    assert!(h.queue.snapshot().paused_sites.is_empty());
}
