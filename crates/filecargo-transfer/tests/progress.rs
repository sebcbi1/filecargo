#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::{Duration, Instant};

use filecargo_transfer::{Direction, NewTransfer};
use support::{Harness, mtime, remote, sample_bytes, set_mtime};

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

#[tokio::test]
async fn changed_events_stay_under_ten_per_second_for_a_thousand_small_files() {
    let mut h = Harness::new(4).await;
    for i in 0..1000 {
        std::fs::write(h.local.path().join(format!("f{i:04}")), b"x").unwrap();
    }
    let started = Instant::now();
    h.queue
        .enqueue((0..1000).map(|i| upload(&h, &format!("f{i:04}"))).collect());
    h.idle().await;
    let elapsed = started.elapsed().as_secs_f64();

    assert_eq!(h.finished_ok, 1000);
    assert!(
        !h.changed_at.is_empty(),
        "the UI must hear about the changes"
    );
    let allowed = elapsed * 10.0 + 2.0;
    assert!(
        (h.changed_at.len() as f64) <= allowed,
        "{} Changed events in {elapsed:.2}s",
        h.changed_at.len()
    );
}

#[tokio::test]
async fn modification_times_survive_a_round_trip() {
    let mut h = Harness::new(1).await;
    std::fs::write(h.local.path().join("up.txt"), b"up").unwrap();
    set_mtime(&h.local.path().join("up.txt"), 1_500_000_000);
    std::fs::write(h.server.path().join("down.txt"), b"down").unwrap();
    set_mtime(&h.server.path().join("down.txt"), 1_400_000_000);
    let mut down = upload(&h, "down.txt");
    down.direction = Direction::Download;
    h.queue.enqueue(vec![upload(&h, "up.txt"), down]);
    h.idle().await;

    assert!(mtime(&h.server.path().join("up.txt")).abs_diff(1_500_000_000) <= 1);
    assert!(mtime(&h.local.path().join("down.txt")).abs_diff(1_400_000_000) <= 1);
}

// Real time: a paused clock runs ahead of the file I/O in a blocking thread pool.
#[tokio::test]
async fn rate_eta_and_totals_follow_a_throttled_upload() {
    let mut h = Harness::new(1).await;
    // one 64 KiB read per 100 ms: 655,360 B/s
    h.connector
        .set_throttle(64 * 1024, Duration::from_millis(100));
    std::fs::write(h.local.path().join("big"), sample_bytes(1, 2_000_000)).unwrap();
    h.queue.enqueue(vec![upload(&h, "big")]); // size not given: the worker learns it

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        h.queue.snapshot().pending[0].eta,
        None,
        "no ETA before a second of data"
    );

    tokio::time::sleep(Duration::from_millis(1700)).await;
    let snapshot = h.queue.snapshot();
    let view = &snapshot.pending[0];
    assert_eq!(view.item.size, Some(2_000_000), "the size is discovered");
    let rate = view.rate.expect("a rate after a couple of seconds");
    assert!((rate - 655_360.0).abs() < 165_000.0, "{rate}");
    let eta = view.eta.expect("an ETA after a second").as_secs_f64();
    assert!(eta > 0.2 && eta < 3.0, "{eta}");
    assert_eq!(snapshot.totals.bytes_total, 2_000_000);
    assert!(snapshot.totals.bytes_done > 500_000 && snapshot.totals.bytes_done < 1_900_000);
    assert!(snapshot.totals.rate.is_some() && snapshot.totals.eta.is_some());

    h.idle().await;
    let done = h.queue.snapshot();
    assert_eq!(
        (done.completed.len(), done.totals.bytes_total),
        (1, 0),
        "the batch resets when idle"
    );
}
