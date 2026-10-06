//! `<data>/queue.json`: the unfinished part of the queue, so a restart does not lose it.
//!
//! Saved: `Held`, and `Pending`, `Active` and `AwaitingDecision` (all as `Pending`, with their
//! `transferred` offset), and `Failed`. Completed items are not persisted.
//!
//! Loaded: every `Pending` and `Held` item comes back `Held`, so nothing starts after a restart
//! until the owner says so. Version 1 files (no `held` state) load the same way; a file without
//! a `version` field is read as version 1.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use filecargo_config::{ConflictRule, SiteId};
use filecargo_remote_fs::RemotePath;
use serde::{Deserialize, Serialize};

use crate::{Direction, ItemState, QueueItem, TransferError, TransferId};

/// Written by this release. Version 1 (v1.0) has no `held` state; it loads unchanged.
const VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
struct File {
    #[serde(default = "first_version")]
    version: u32,
    next_id: u64,
    items: Vec<Persisted>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    id: TransferId,
    site: SiteId,
    direction: Direction,
    local: String,
    remote: String,
    is_dir: bool,
    size: Option<u64>,
    transferred: u64,
    attempts: u32,
    parent: Option<TransferId>,
    conflict: Option<ConflictRule>,
    state: PersistedState,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PersistedState {
    Pending,
    Held,
    Failed {
        reason: String,
        retryable: bool,
        finished_unix: u64,
    },
}

fn first_version() -> u32 {
    1
}

/// What `load` found.
#[derive(Debug, Default)]
pub struct Loaded {
    pub items: Vec<QueueItem>,
    pub next_id: u64,
}

fn unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn to_persisted(item: &QueueItem) -> Option<Persisted> {
    let state = match &item.state {
        ItemState::Held => PersistedState::Held,
        ItemState::Pending | ItemState::Active { .. } | ItemState::AwaitingDecision { .. } => {
            PersistedState::Pending
        }
        ItemState::Failed {
            reason,
            retryable,
            finished,
        } => PersistedState::Failed {
            reason: reason.clone(),
            retryable: *retryable,
            finished_unix: unix(*finished),
        },
        ItemState::Completed { .. } => return None,
    };
    let Some(local) = item.local.to_str() else {
        tracing::warn!(target: "filecargo::transfer", path = %item.local.display(), "not saving a queue item whose path is not UTF-8");
        return None;
    };
    Some(Persisted {
        id: item.id,
        site: item.site,
        direction: item.direction,
        local: local.to_owned(),
        remote: item.remote.as_str().to_owned(),
        is_dir: item.is_dir,
        size: item.size,
        transferred: item.transferred,
        attempts: item.attempts,
        parent: item.parent,
        conflict: item.conflict,
        state,
    })
}

fn from_persisted(p: Persisted) -> Option<QueueItem> {
    let remote = RemotePath::parse(&p.remote).ok()?;
    let state = match p.state {
        // restored items never start by themselves
        PersistedState::Pending | PersistedState::Held => ItemState::Held,
        PersistedState::Failed {
            reason,
            retryable,
            finished_unix,
        } => ItemState::Failed {
            reason,
            retryable,
            finished: UNIX_EPOCH + Duration::from_secs(finished_unix),
        },
    };
    Some(QueueItem {
        id: p.id,
        site: p.site,
        direction: p.direction,
        local: PathBuf::from(p.local),
        remote,
        is_dir: p.is_dir,
        size: p.size,
        transferred: p.transferred,
        state,
        attempts: p.attempts,
        parent: p.parent,
        conflict: p.conflict,
    })
}

/// Serializes the persistable part of the queue.
pub fn encode(items: &[QueueItem], next_id: u64) -> Result<Vec<u8>, TransferError> {
    let file = File {
        version: VERSION,
        next_id,
        items: items.iter().filter_map(to_persisted).collect(),
    };
    serde_json::to_vec_pretty(&file).map_err(|e| TransferError::Persist(e.to_string()))
}

pub fn save(path: &Path, items: &[QueueItem], next_id: u64) -> Result<(), TransferError> {
    let bytes = encode(items, next_id)?;
    filecargo_config::atomic_write(path, &bytes).map_err(|e| TransferError::Persist(e.to_string()))
}

/// Reads the queue. A missing file is an empty queue; a corrupt or newer-version file is
/// renamed to `queue.json.bak-<unix-secs>`, a warning is logged, and the queue starts empty:
/// losing the queue must not stop the app.
pub fn load(path: &Path) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::default(),
        Err(e) => {
            tracing::warn!(target: "filecargo::transfer", path = %path.display(), error = %e, "cannot read the queue file; starting empty");
            return Loaded::default();
        }
    };
    let parsed = serde_json::from_slice::<File>(&bytes)
        .map_err(|e| e.to_string())
        .and_then(|f| {
            if (1..=VERSION).contains(&f.version) {
                Ok(f)
            } else {
                Err(format!("unsupported version {}", f.version))
            }
        });
    match parsed {
        Ok(file) => {
            let items: Vec<QueueItem> = file.items.into_iter().filter_map(from_persisted).collect();
            let next_id = items
                .iter()
                .map(|i| i.id.0 + 1)
                .max()
                .unwrap_or(0)
                .max(file.next_id);
            Loaded { items, next_id }
        }
        Err(reason) => {
            let backup = path.with_file_name(format!(
                "{}.bak-{}",
                path.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default(),
                unix(SystemTime::now())
            ));
            match std::fs::rename(path, &backup) {
                Ok(()) => {
                    tracing::warn!(target: "filecargo::transfer", path = %path.display(), backup = %backup.display(), %reason, "the queue file is unusable; backed it up and starting empty")
                }
                Err(e) => {
                    tracing::warn!(target: "filecargo::transfer", path = %path.display(), %reason, error = %e, "the queue file is unusable and could not be backed up; starting empty")
                }
            }
            Loaded::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use filecargo_remote_fs::{Entry, EntryKind};

    use super::*;
    use crate::{ConflictInfo, Outcome};

    fn item(id: u64, state: ItemState) -> QueueItem {
        QueueItem {
            id: TransferId(id),
            site: SiteId::new(),
            direction: if id.is_multiple_of(2) {
                Direction::Upload
            } else {
                Direction::Download
            },
            local: PathBuf::from(format!("/tmp/local-{id}")),
            remote: RemotePath::parse(&format!("/remote/{id}")).unwrap(),
            is_dir: id.is_multiple_of(3),
            size: Some(id * 10),
            transferred: id * 3,
            state,
            attempts: 1,
            parent: (id > 1).then_some(TransferId(1)),
            conflict: Some(ConflictRule::Resume),
        }
    }

    fn conflict() -> ConflictInfo {
        ConflictInfo {
            source: Entry::new("a", EntryKind::File),
            target: Entry::new("a", EntryKind::File),
        }
    }

    fn round_trip(items: &[QueueItem], next_id: u64) -> Loaded {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        save(&path, items, next_id).unwrap();
        load(&path)
    }

    #[test]
    fn held_and_failed_items_round_trip_exactly() {
        let finished = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let items = [
            item(1, ItemState::Held),
            item(
                2,
                ItemState::Failed {
                    reason: "no route".into(),
                    retryable: true,
                    finished,
                },
            ),
            item(
                3,
                ItemState::Failed {
                    reason: "denied".into(),
                    retryable: false,
                    finished,
                },
            ),
        ];
        let loaded = round_trip(&items, 9);
        assert_eq!(loaded.items, items);
        assert_eq!(loaded.next_id, 9);
    }

    #[test]
    fn pending_active_and_awaiting_items_come_back_held_with_their_offset() {
        let items = [
            item(0, ItemState::Pending),
            item(
                1,
                ItemState::Active {
                    started: SystemTime::now(),
                },
            ),
            item(
                2,
                ItemState::AwaitingDecision {
                    conflict: conflict(),
                },
            ),
        ];
        let loaded = round_trip(&items, 3);
        assert_eq!(loaded.items.len(), 3);
        for (saved, restored) in items.iter().zip(&loaded.items) {
            assert_eq!(
                restored.state,
                ItemState::Held,
                "nothing starts after a restart"
            );
            assert_eq!(restored.transferred, saved.transferred);
            assert_eq!(
                (restored.id, &restored.local, &restored.remote),
                (saved.id, &saved.local, &saved.remote)
            );
        }
    }

    #[test]
    fn completed_items_are_not_persisted() {
        let done = ItemState::Completed {
            outcome: Outcome::Transferred,
            finished: SystemTime::now(),
        };
        let loaded = round_trip(&[item(1, done), item(2, ItemState::Held)], 3);
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].id, TransferId(2));
    }

    #[test]
    fn next_id_never_reuses_a_loaded_id() {
        // a stale `next_id` in the file must not cause a collision
        let loaded = round_trip(&[item(7, ItemState::Held)], 2);
        assert_eq!(loaded.next_id, 8);
    }

    #[test]
    fn a_missing_file_is_an_empty_queue() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("queue.json"));
        assert!(loaded.items.is_empty());
        assert_eq!(loaded.next_id, 0);
    }

    fn backups(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("queue.json.bak-"))
            .collect()
    }

    #[test]
    fn a_corrupt_file_is_backed_up_and_the_queue_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        std::fs::write(&path, "{ not json").unwrap();
        let loaded = load(&path);
        assert!(loaded.items.is_empty());
        assert!(!path.exists());
        let found = backups(dir.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&found[0])).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn a_newer_version_is_backed_up_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        std::fs::write(&path, r#"{"version":3,"next_id":5,"items":[]}"#).unwrap();
        assert!(load(&path).items.is_empty());
        assert_eq!(backups(dir.path()).len(), 1);
    }

    #[test]
    fn a_file_written_by_v1_loads_and_its_pending_items_come_back_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        std::fs::write(&path, include_str!("../tests/fixtures/queue-v1.json")).unwrap();
        let loaded = load(&path);
        let states: Vec<_> = loaded
            .items
            .iter()
            .map(|i| (i.id.0, matches!(i.state, ItemState::Held), i.transferred))
            .collect();
        assert_eq!(states, [(1, true, 1024), (2, true, 0), (5, false, 0)]);
        assert!(matches!(
            &loaded.items[2].state,
            ItemState::Failed { reason, retryable: false, .. } if reason == "no such file"
        ));
        assert_eq!(loaded.next_id, 6);
        assert!(
            backups(dir.path()).is_empty(),
            "a v1 file is not a bad file"
        );
        assert!(path.exists(), "and it is left in place");
    }

    #[test]
    fn a_file_without_a_version_is_read_as_v1() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let text =
            include_str!("../tests/fixtures/queue-v1.json").replacen("\"version\": 1,", "", 1);
        assert!(!text.contains("version"));
        std::fs::write(&path, text).unwrap();
        assert_eq!(load(&path).items.len(), 3);
    }

    #[test]
    fn saving_writes_the_current_version_and_keeps_held_items_held() {
        let bytes = encode(&[item(1, ItemState::Held), item(2, ItemState::Pending)], 3).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\"version\": 2"), "{text}");
        assert!(text.contains("\"kind\": \"held\""), "{text}");
    }

    #[test]
    fn an_item_with_an_invalid_remote_path_is_dropped_but_the_rest_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        save(
            &path,
            &[item(1, ItemState::Pending), item(2, ItemState::Pending)],
            3,
        )
        .unwrap();
        let text =
            std::fs::read_to_string(&path)
                .unwrap()
                .replacen("/remote/1", "relative/path", 1);
        std::fs::write(&path, text).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].id, TransferId(2));
    }
}
