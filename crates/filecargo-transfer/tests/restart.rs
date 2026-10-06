#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use filecargo_transfer::{Direction, ItemState, NewTransfer, Outcome, Queue, QueueLimits};
use support::{Fault, Harness, TestConnector, remote, sample_bytes, sha256_hex};

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

async fn wait_for(
    h: &Harness,
    what: &str,
    mut condition: impl FnMut(&filecargo_transfer::QueueSnapshot) -> bool,
) {
    for _ in 0..200 {
        if condition(&h.queue.snapshot()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}: {:#?}", h.queue.snapshot());
}

#[tokio::test]
async fn pending_active_and_failed_items_survive_a_restart_held_and_the_partial_one_resumes_when_started()
 {
    let mut h = Harness::new(1).await;
    let data = sample_bytes(7, 6_000_000);
    std::fs::write(h.local.path().join("partial.bin"), &data).unwrap();
    std::fs::write(h.local.path().join("waiting.bin"), b"waiting").unwrap();

    // a failed item first
    let failed_id = h.queue.enqueue(vec![upload(&h, "missing.bin")])[0];
    h.idle().await;
    assert_eq!(h.queue.snapshot().failed.len(), 1);

    // one transfer stuck mid-file, one behind it
    h.connector.flaky.inject(Fault::HangAfter(2_000_000));
    let ids = h
        .queue
        .enqueue(vec![upload(&h, "partial.bin"), upload(&h, "waiting.bin")]);
    wait_for(&h, "the transfer to be under way", |s| {
        s.pending
            .first()
            .is_some_and(|v| v.item.transferred >= 2_000_000)
    })
    .await;

    let store = h.data.path().join("queue.json");
    let mut h = h.restart(1).await; // shuts the first queue down, then starts a new one

    // what the shutdown wrote
    let saved = std::fs::read_to_string(&store).unwrap();
    assert!(
        saved.contains("partial.bin")
            && saved.contains("waiting.bin")
            && saved.contains("missing.bin"),
        "{saved}"
    );

    // the new queue restored all three; the partial one is held with its offset, and nothing
    // starts until the owner says so
    tokio::time::sleep(Duration::from_millis(500)).await;
    let restored = h.queue.snapshot();
    assert_eq!(restored.failed.len(), 1);
    assert_eq!(restored.failed[0].item.id, failed_id);
    assert_eq!(restored.pending.len(), 2);
    assert!(
        restored
            .pending
            .iter()
            .all(|v| matches!(v.item.state, ItemState::Held)),
        "{restored:#?}"
    );
    assert!(restored.pending[0].item.transferred >= 2_000_000);
    assert!(!h.server.path().join("waiting.bin").exists());
    h.queue.start_held(h.site);
    h.idle().await;

    let arrived = std::fs::read(h.server.path().join("partial.bin")).unwrap();
    assert_eq!(sha256_hex(&arrived), sha256_hex(&data));
    assert_eq!(
        h.connector.flaky.calls().get(1),
        Some(&("upload", 2_000_000)),
        "after the restart the first call must continue at the offset, not start over"
    );
    assert_eq!(
        std::fs::read(h.server.path().join("waiting.bin")).unwrap(),
        b"waiting"
    );
    let after = h.queue.snapshot();
    assert_eq!(
        (
            after.completed.len(),
            after.failed.len(),
            after.pending.len()
        ),
        (2, 1, 0)
    );
    let partial = after
        .completed
        .iter()
        .find(|v| v.item.id == ids[0])
        .unwrap();
    assert!(matches!(
        partial.item.state,
        ItemState::Completed {
            outcome: Outcome::Resumed,
            ..
        }
    ));
}

#[tokio::test]
async fn ids_continue_after_a_restart_and_completed_items_are_not_restored() {
    let mut h = Harness::new(2).await;
    std::fs::write(h.local.path().join("a"), b"a").unwrap();
    let first = h.queue.enqueue(vec![upload(&h, "a")]);
    h.idle().await;
    let mut h = h.restart(2).await;
    assert!(
        h.queue.snapshot().completed.is_empty(),
        "completed items are not persisted"
    );
    std::fs::write(h.local.path().join("b"), b"b").unwrap();
    let second = h.queue.enqueue(vec![upload(&h, "b")]);
    assert!(second[0] > first[0], "{first:?} then {second:?}");
    h.idle().await;
}

#[tokio::test]
async fn state_changes_reach_the_file_within_about_a_second_without_a_write_per_change() {
    let h = Harness::new(1).await;
    let store = h.data.path().join("queue.json");
    h.queue.set_processing(false);
    for i in 0..50 {
        h.queue.enqueue(vec![upload(&h, &format!("f{i}"))]);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let early = std::fs::metadata(&store).map(|m| m.len()).unwrap_or(0);
    assert!(
        early < 20_000,
        "the debounce should not have written 50 times"
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let saved = std::fs::read_to_string(&store).unwrap();
    assert_eq!(
        saved.matches("\"local\"").count(),
        50,
        "the latest state reaches the file"
    );
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

#[tokio::test]
async fn a_corrupt_queue_file_is_backed_up_the_queue_starts_empty_and_a_warning_is_logged() {
    let data = tempfile::tempdir().unwrap();
    let store = data.path().join("queue.json");
    std::fs::write(&store, "{ definitely not json").unwrap();
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    let (queue, _events) =
        Queue::start(TestConnector::new(), store.clone(), QueueLimits::default())
            .await
            .unwrap();
    drop(guard);

    let snapshot = queue.snapshot();
    assert!(snapshot.pending.is_empty() && snapshot.failed.is_empty());
    let backups: Vec<_> = std::fs::read_dir(data.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("queue.json.bak-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read_to_string(backups[0].path()).unwrap(),
        "{ definitely not json"
    );
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("WARN") && log.contains("backed it up"),
        "{log}"
    );
    queue.shutdown().await;
}

#[tokio::test]
async fn held_items_survive_a_restart_as_held_and_a_v1_file_loads() {
    let h = Harness::new(1).await;
    std::fs::write(h.local.path().join("a"), b"a").unwrap();
    h.queue.enqueue_held(vec![upload(&h, "a")]);
    wait_for(&h, "the held item", |s| s.pending.len() == 1).await;
    let mut h = h.restart(1).await;
    let restored = h.queue.snapshot();
    assert_eq!(restored.pending.len(), 1);
    assert!(matches!(restored.pending[0].item.state, ItemState::Held));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!h.server.path().join("a").exists());
    h.queue.start_held(h.site);
    h.idle().await;
    assert_eq!(std::fs::read(h.server.path().join("a")).unwrap(), b"a");

    // a queue.json written by v1.0 starts as a queue of held items
    h.queue.shutdown().await;
    let data = tempfile::tempdir().unwrap();
    let store = data.path().join("queue.json");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/queue-v1.json"),
        &store,
    )
    .unwrap();
    let (queue, _events) = Queue::start(TestConnector::new(), store, QueueLimits::default())
        .await
        .unwrap();
    let snapshot = queue.snapshot();
    assert_eq!((snapshot.pending.len(), snapshot.failed.len()), (2, 1));
    assert!(
        snapshot
            .pending
            .iter()
            .all(|v| matches!(v.item.state, ItemState::Held))
    );
    queue.shutdown().await;
}
