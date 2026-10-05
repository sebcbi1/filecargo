#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::{Duration, Instant};

use filecargo_transfer::{Direction, ItemState, NewTransfer, Outcome};
use support::{Fault, Harness, remote, sample_bytes, sha256_hex};

const TEN_MB: usize = 10 * 1024 * 1024;
const VIRTUAL_HOUR: Duration = Duration::from_secs(3600);

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
        direction: Direction::Download,
        local: h.local.path().join("down").join(name),
        ..upload(h, name)
    }
}

fn only_completed(h: &Harness) -> (Outcome, u32) {
    let snapshot = h.queue.snapshot();
    assert_eq!(
        (snapshot.completed.len(), snapshot.failed.len()),
        (1, 0),
        "{snapshot:#?}"
    );
    match &snapshot.completed[0].item.state {
        ItemState::Completed { outcome, .. } => {
            (outcome.clone(), snapshot.completed[0].item.attempts)
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_dropped_upload_retries_and_resumes_at_the_targets_current_size() {
    let mut h = Harness::new(1).await;
    let data = sample_bytes(1, TEN_MB);
    std::fs::write(h.local.path().join("big.bin"), &data).unwrap();
    h.connector.flaky.inject(Fault::FailAfter(4_000_000));
    h.queue.enqueue(vec![upload(&h, "big.bin")]);
    h.idle_for(VIRTUAL_HOUR).await;

    let arrived = std::fs::read(h.server.path().join("big.bin")).unwrap();
    assert_eq!(sha256_hex(&arrived), sha256_hex(&data));
    assert_eq!(
        h.connector.flaky.calls(),
        [("upload", 0), ("upload", 4_000_000)],
        "the retry must resume, not restart"
    );
    assert_eq!(
        h.connector.stats.connects(),
        2,
        "a fresh connection after the drop"
    );
    assert_eq!(only_completed(&h), (Outcome::Resumed, 1));
}

#[tokio::test(start_paused = true)]
async fn a_dropped_download_retries_and_resumes_at_the_targets_current_size() {
    let mut h = Harness::new(1).await;
    let data = sample_bytes(2, TEN_MB);
    std::fs::write(h.server.path().join("big.bin"), &data).unwrap();
    h.connector.flaky.inject(Fault::FailAfter(3_000_000));
    h.queue.enqueue(vec![download(&h, "big.bin")]);
    h.idle_for(VIRTUAL_HOUR).await;

    let got = std::fs::read(h.local.path().join("down/big.bin")).unwrap();
    assert_eq!(sha256_hex(&got), sha256_hex(&data));
    assert_eq!(
        h.connector.flaky.calls(),
        [("download", 0), ("download", 3_000_000)]
    );
    assert_eq!(only_completed(&h), (Outcome::Resumed, 1));
}

#[tokio::test(start_paused = true)]
async fn three_consecutive_failures_fail_the_item_and_a_manual_retry_finishes_it() {
    let mut h = Harness::new(1).await;
    let data = sample_bytes(3, TEN_MB);
    std::fs::write(h.local.path().join("big.bin"), &data).unwrap();
    for _ in 0..3 {
        h.connector.flaky.inject(Fault::FailAfter(1_000_000));
    }
    let ids = h.queue.enqueue(vec![upload(&h, "big.bin")]);
    h.idle_for(VIRTUAL_HOUR).await;

    let snapshot = h.queue.snapshot();
    assert_eq!((snapshot.completed.len(), snapshot.failed.len()), (0, 1));
    match &snapshot.failed[0].item.state {
        ItemState::Failed {
            reason, retryable, ..
        } => {
            assert!(reason.contains("connection reset by peer"), "{reason}");
            assert!(retryable);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(snapshot.failed[0].item.attempts, 3);
    assert_eq!(snapshot.failed[0].item.transferred, 3_000_000);
    assert_eq!(
        h.connector.flaky.calls(),
        [("upload", 0), ("upload", 1_000_000), ("upload", 2_000_000)]
    );

    // manual retry: attempts reset, resumes from the partial file
    h.queue.retry(ids[0]);
    h.idle_for(VIRTUAL_HOUR).await;
    let arrived = std::fs::read(h.server.path().join("big.bin")).unwrap();
    assert_eq!(sha256_hex(&arrived), sha256_hex(&data));
    assert_eq!(
        h.connector.flaky.calls().last(),
        Some(&("upload", 3_000_000))
    );
    assert_eq!(only_completed(&h), (Outcome::Resumed, 0));
}

#[tokio::test(start_paused = true)]
async fn a_failure_that_cannot_be_fixed_by_retrying_fails_at_once() {
    let mut h = Harness::new(1).await;
    h.queue.enqueue(vec![upload(&h, "does-not-exist")]);
    h.idle_for(VIRTUAL_HOUR).await;
    let snapshot = h.queue.snapshot();
    assert_eq!(snapshot.failed.len(), 1);
    assert_eq!(snapshot.failed[0].item.attempts, 1, "no automatic retries");
    assert_eq!(h.connector.stats.connects(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_connection_failure_is_retried_after_the_back_off() {
    let mut h = Harness::new(1).await;
    std::fs::write(h.local.path().join("f"), b"payload").unwrap();
    h.connector
        .flaky
        .fail_connects
        .store(2, std::sync::atomic::Ordering::SeqCst);
    h.queue.enqueue(vec![upload(&h, "f")]);
    h.idle_for(VIRTUAL_HOUR).await;
    assert_eq!(
        std::fs::read(h.server.path().join("f")).unwrap(),
        b"payload"
    );
    assert_eq!(
        only_completed(&h).1,
        2,
        "two failed connects, the third attempt worked"
    );
}

#[tokio::test(start_paused = true)]
async fn the_back_off_is_two_then_ten_seconds() {
    let mut h = Harness::new(1).await;
    std::fs::write(h.local.path().join("f"), b"payload").unwrap();
    h.connector
        .flaky
        .fail_connects
        .store(2, std::sync::atomic::Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    h.queue.enqueue(vec![upload(&h, "f")]);
    h.idle_for(VIRTUAL_HOUR).await;
    let waited = started.elapsed();
    assert!(waited >= Duration::from_secs(12), "{waited:?}");
    assert!(waited < Duration::from_secs(13), "{waited:?}");
}

#[tokio::test]
async fn remove_cancels_an_active_item_within_a_second_and_the_broken_connection_is_not_reused() {
    let mut h = Harness::new(1).await;
    std::fs::write(h.local.path().join("stuck.bin"), sample_bytes(4, TEN_MB)).unwrap();
    std::fs::write(h.local.path().join("next.bin"), b"next").unwrap();
    h.connector.flaky.inject(Fault::HangAfter(1_000));
    let ids = h
        .queue
        .enqueue(vec![upload(&h, "stuck.bin"), upload(&h, "next.bin")]);

    // wait until the first item is really transferring
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = h.queue.snapshot();
        if snapshot
            .pending
            .first()
            .is_some_and(|v| v.item.transferred > 0)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the transfer never started: {snapshot:#?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let removed = Instant::now();
    h.queue.remove(ids[0]);
    h.idle().await;
    assert!(
        removed.elapsed() < Duration::from_secs(1),
        "cancelling took {:?}",
        removed.elapsed()
    );

    // the freed slot ran the next item, on a *new* connection (the cancelled one is broken)
    assert_eq!(
        std::fs::read(h.server.path().join("next.bin")).unwrap(),
        b"next"
    );
    assert_eq!(h.connector.stats.connects(), 2);
    let snapshot = h.queue.snapshot();
    assert_eq!(
        (
            snapshot.completed.len(),
            snapshot.failed.len(),
            snapshot.pending.len()
        ),
        (1, 0, 0)
    );
    // the partial file is left as it is
    assert!(h.server.path().join("stuck.bin").exists());
}

#[tokio::test(start_paused = true)]
async fn retry_and_retry_all_failed_requeue_failed_items_at_the_end() {
    let mut h = Harness::new(1).await;
    let ids = h.queue.enqueue(vec![upload(&h, "a"), upload(&h, "b")]);
    h.idle_for(VIRTUAL_HOUR).await;
    assert_eq!(h.queue.snapshot().failed.len(), 2);

    std::fs::write(h.local.path().join("a"), b"a").unwrap();
    std::fs::write(h.local.path().join("b"), b"b").unwrap();
    std::fs::write(h.local.path().join("c"), b"c").unwrap();
    h.queue.set_processing(false);
    h.queue.enqueue(vec![upload(&h, "c")]);
    h.queue.retry(ids[0]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let order: Vec<String> = h
        .queue
        .snapshot()
        .pending
        .iter()
        .map(|v| v.item.remote.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(
        order,
        ["c", "a"],
        "a retried item goes to the end of the queue"
    );
    assert_eq!(h.queue.snapshot().failed.len(), 1);

    h.queue.retry_all_failed();
    h.queue.set_processing(true);
    h.idle_for(VIRTUAL_HOUR).await;
    let snapshot = h.queue.snapshot();
    assert_eq!((snapshot.completed.len(), snapshot.failed.len()), (3, 0));
    for name in ["a", "b", "c"] {
        assert_eq!(
            std::fs::read(h.server.path().join(name)).unwrap(),
            name.as_bytes()
        );
    }
}
