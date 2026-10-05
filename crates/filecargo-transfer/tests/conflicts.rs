#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::PathBuf;

use filecargo_config::ConflictRule;
use filecargo_transfer::{
    ConflictDecision, Direction, ItemState, NewTransfer, Outcome, QueueEvent,
};
use support::{Harness, remote, set_mtime};

const NEW: &[u8] = b"NEW-CONTENT-THAT-IS-LONGER";

/// One file `name` on each side: `source_bytes` where the transfer reads, `target_bytes` where it
/// writes. Returns the transfer and the path of the target on disk.
fn scenario(
    h: &Harness,
    direction: Direction,
    name: &str,
    source: &[u8],
    target: &[u8],
) -> (NewTransfer, PathBuf) {
    let local = h.local.path().join(name);
    let server = h.server.path().join(name);
    let (source_path, target_path) = match direction {
        Direction::Upload => (&local, &server),
        Direction::Download => (&server, &local),
    };
    std::fs::write(source_path, source).unwrap();
    std::fs::write(target_path, target).unwrap();
    let transfer = NewTransfer {
        site: h.site,
        direction,
        local: local.clone(),
        remote: remote(&format!("/{name}")),
        is_dir: false,
        size: None,
        conflict: None,
    };
    (transfer, target_path.clone())
}

fn with_rule(mut transfer: NewTransfer, rule: ConflictRule) -> NewTransfer {
    transfer.conflict = Some(rule);
    transfer
}

fn outcome(h: &Harness) -> Outcome {
    match &h.queue.snapshot().completed[0].item.state {
        ItemState::Completed { outcome, .. } => outcome.clone(),
        other => panic!("{other:?}"),
    }
}

const BOTH: [Direction; 2] = [Direction::Upload, Direction::Download];

#[tokio::test]
async fn overwrite_replaces_the_target() {
    for direction in BOTH {
        let mut h = Harness::new(1).await;
        let (transfer, target) = scenario(&h, direction, "f.txt", NEW, b"old");
        h.queue
            .enqueue(vec![with_rule(transfer, ConflictRule::Overwrite)]);
        h.idle().await;
        assert_eq!(std::fs::read(target).unwrap(), NEW, "{direction:?}");
        assert_eq!(outcome(&h), Outcome::Transferred);
    }
}

#[tokio::test]
async fn skip_leaves_the_target_alone() {
    for direction in BOTH {
        let mut h = Harness::new(1).await;
        let (transfer, target) = scenario(&h, direction, "f.txt", NEW, b"old");
        h.queue
            .enqueue(vec![with_rule(transfer, ConflictRule::Skip)]);
        h.idle().await;
        assert_eq!(std::fs::read(target).unwrap(), b"old", "{direction:?}");
        assert_eq!(outcome(&h), Outcome::Skipped);
    }
}

#[tokio::test]
async fn overwrite_if_newer_only_replaces_with_a_newer_source() {
    for direction in BOTH {
        for (source_time, target_time, replaced) in [
            (2_000, 1_000, true),
            (1_000, 2_000, false),
            (1_500, 1_500, false),
        ] {
            let mut h = Harness::new(1).await;
            let (transfer, target) = scenario(&h, direction, "f.txt", NEW, b"old");
            let source = match direction {
                Direction::Upload => transfer.local.clone(),
                Direction::Download => h.server.path().join("f.txt"),
            };
            set_mtime(&source, source_time);
            set_mtime(&target, target_time);
            h.queue
                .enqueue(vec![with_rule(transfer, ConflictRule::OverwriteIfNewer)]);
            h.idle().await;
            let label = format!("{direction:?} source {source_time} target {target_time}");
            if replaced {
                assert_eq!(std::fs::read(&target).unwrap(), NEW, "{label}");
                assert_eq!(outcome(&h), Outcome::Transferred, "{label}");
            } else {
                assert_eq!(std::fs::read(&target).unwrap(), b"old", "{label}");
                assert_eq!(outcome(&h), Outcome::Skipped, "{label}");
            }
        }
    }
}

#[tokio::test]
async fn resume_continues_a_shorter_target_skips_an_equal_one_and_restarts_a_longer_one() {
    for direction in BOTH {
        // shorter: a prefix of the source
        let mut h = Harness::new(1).await;
        let (transfer, target) = scenario(&h, direction, "f.txt", NEW, &NEW[..9]);
        h.queue
            .enqueue(vec![with_rule(transfer, ConflictRule::Resume)]);
        h.idle().await;
        assert_eq!(
            std::fs::read(&target).unwrap(),
            NEW,
            "{direction:?} shorter"
        );
        assert_eq!(outcome(&h), Outcome::Resumed);

        // equal size: nothing to do
        let mut h = Harness::new(1).await;
        let same_size = vec![b'x'; NEW.len()];
        let (transfer, target) = scenario(&h, direction, "f.txt", NEW, &same_size);
        h.queue
            .enqueue(vec![with_rule(transfer, ConflictRule::Resume)]);
        h.idle().await;
        assert_eq!(
            std::fs::read(&target).unwrap(),
            same_size,
            "{direction:?} equal"
        );
        assert_eq!(outcome(&h), Outcome::Skipped);

        // longer: overwritten from the start
        let mut h = Harness::new(1).await;
        let longer = vec![b'y'; NEW.len() + 50];
        let (transfer, target) = scenario(&h, direction, "f.txt", NEW, &longer);
        h.queue
            .enqueue(vec![with_rule(transfer, ConflictRule::Resume)]);
        h.idle().await;
        assert_eq!(std::fs::read(&target).unwrap(), NEW, "{direction:?} longer");
        assert_eq!(outcome(&h), Outcome::Transferred);
    }
}

#[tokio::test]
async fn rename_writes_to_the_first_free_numbered_name() {
    for direction in BOTH {
        let mut h = Harness::new(1).await;
        let (transfer, target) = scenario(&h, direction, "report.txt", NEW, b"original");
        let dir = target.parent().unwrap().to_path_buf();
        h.queue
            .enqueue(vec![with_rule(transfer.clone(), ConflictRule::Rename)]);
        h.idle().await;
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"original",
            "{direction:?}: the existing file is untouched"
        );
        assert_eq!(std::fs::read(dir.join("report (1).txt")).unwrap(), NEW);
        assert_eq!(outcome(&h), Outcome::Renamed("report (1).txt".to_owned()));

        // the same transfer again: (1) is taken, so (2)
        h.queue
            .enqueue(vec![with_rule(transfer, ConflictRule::Rename)]);
        h.idle().await;
        assert_eq!(
            std::fs::read(dir.join("report (2).txt")).unwrap(),
            NEW,
            "{direction:?}"
        );
        assert_eq!(outcome(&h), Outcome::Renamed("report (2).txt".to_owned()));
    }
}

#[tokio::test]
async fn ask_pauses_only_that_item_and_resolve_continues_it() {
    for direction in BOTH {
        let mut h = Harness::new(2).await;
        let (conflicting, target) = scenario(&h, direction, "clash.txt", NEW, b"old");
        let (free, _) = scenario(&h, direction, "other.txt", b"other", b"other");
        // `other.txt` conflicts too, but the rule says skip; only `clash.txt` asks
        h.queue
            .enqueue(vec![conflicting, with_rule(free, ConflictRule::Skip)]);

        let (id, info) = h.next_conflict().await;
        assert_eq!(info.source.size, NEW.len() as u64);
        assert_eq!(info.target.size, 3);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"old",
            "nothing is written before the answer"
        );
        let snapshot = h.queue.snapshot();
        assert!(
            snapshot
                .pending
                .iter()
                .any(|v| v.item.id == id
                    && matches!(v.item.state, ItemState::AwaitingDecision { .. })),
            "{:?}",
            snapshot.pending
        );

        h.queue.resolve(
            id,
            ConflictDecision {
                rule: ConflictRule::Overwrite,
                apply_to_all: false,
            },
        );
        h.idle().await;
        assert_eq!(std::fs::read(&target).unwrap(), NEW, "{direction:?}");
        assert_eq!(h.conflicts.len(), 1);
    }
}

#[tokio::test]
async fn apply_to_all_answers_the_remaining_conflicts_without_asking_again() {
    for direction in BOTH {
        let mut h = Harness::new(1).await;
        let mut targets = Vec::new();
        let mut transfers = Vec::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            let (transfer, target) = scenario(&h, direction, name, NEW, b"old");
            transfers.push(transfer);
            targets.push(target);
        }
        h.queue.enqueue(transfers);

        let (id, _) = h.next_conflict().await;
        h.queue.resolve(
            id,
            ConflictDecision {
                rule: ConflictRule::Skip,
                apply_to_all: true,
            },
        );
        h.idle().await;

        assert_eq!(
            h.conflicts.len(),
            1,
            "{direction:?}: only the first conflict is asked"
        );
        assert_eq!(h.finished_ok, 3);
        for target in targets {
            assert_eq!(std::fs::read(target).unwrap(), b"old");
        }
        // the override ends with the queue: the next conflict is asked again
        let (transfer, _) = scenario(&h, direction, "d.txt", NEW, b"old");
        h.queue.enqueue(vec![transfer]);
        let (again, _) = h.next_conflict().await;
        h.queue.resolve(
            again,
            ConflictDecision {
                rule: ConflictRule::Skip,
                apply_to_all: false,
            },
        );
        h.idle().await;
    }
}

#[tokio::test]
async fn apply_to_all_also_answers_conflicts_that_are_already_waiting() {
    let mut h = Harness::new(3).await;
    let mut transfers = Vec::new();
    let mut targets = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        let (transfer, target) = scenario(&h, Direction::Upload, name, NEW, b"old");
        transfers.push(transfer);
        targets.push(target);
    }
    h.queue.enqueue(transfers);
    // three slots: all three ask before anyone answers
    let (first, _) = h.next_conflict().await;
    h.next_conflict().await;
    h.next_conflict().await;
    h.queue.resolve(
        first,
        ConflictDecision {
            rule: ConflictRule::Overwrite,
            apply_to_all: true,
        },
    );
    h.idle().await;
    assert_eq!(h.conflicts.len(), 3);
    for target in targets {
        assert_eq!(std::fs::read(target).unwrap(), NEW);
    }
}

#[tokio::test]
async fn an_answer_of_ask_is_ignored() {
    let mut h = Harness::new(1).await;
    let (transfer, target) = scenario(&h, Direction::Upload, "f.txt", NEW, b"old");
    h.queue.enqueue(vec![transfer]);
    let (id, _) = h.next_conflict().await;
    h.queue.resolve(
        id,
        ConflictDecision {
            rule: ConflictRule::Ask,
            apply_to_all: true,
        },
    );
    // still waiting: the proper answer then completes it
    h.queue.resolve(
        id,
        ConflictDecision {
            rule: ConflictRule::Skip,
            apply_to_all: false,
        },
    );
    h.idle().await;
    assert_eq!(std::fs::read(target).unwrap(), b"old");
    assert_eq!(h.conflicts.len(), 1);
    let _ = QueueEvent::Idle;
}
