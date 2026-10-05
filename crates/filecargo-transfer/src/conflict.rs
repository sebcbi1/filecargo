//! What to do when the target of a file transfer already exists.

use filecargo_config::ConflictRule;
use filecargo_remote_fs::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Write from `offset` (0 truncates and starts over).
    Write {
        offset: u64,
    },
    Skip,
    /// Write under the first free numbered name.
    Rename,
    /// Hand the question to the owner.
    Ask,
}

/// The conflict table of the spec. Unknown times count as "not newer".
pub(crate) fn decide(rule: ConflictRule, source: &Entry, target: &Entry) -> Decision {
    match rule {
        ConflictRule::Ask => Decision::Ask,
        ConflictRule::Overwrite => Decision::Write { offset: 0 },
        ConflictRule::OverwriteIfNewer => match (source.modified, target.modified) {
            (Some(source), Some(target)) if source > target => Decision::Write { offset: 0 },
            _ => Decision::Skip,
        },
        ConflictRule::Resume => match target.size.cmp(&source.size) {
            std::cmp::Ordering::Less => Decision::Write {
                offset: target.size,
            },
            std::cmp::Ordering::Equal => Decision::Skip,
            std::cmp::Ordering::Greater => Decision::Write { offset: 0 },
        },
        ConflictRule::Skip => Decision::Skip,
        ConflictRule::Rename => Decision::Rename,
    }
}

/// `report.txt` → `report (2).txt`; `archive.tar.gz` → `archive.tar (2).gz`;
/// `.profile` and `README` get the number at the end.
pub(crate) fn numbered_name(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use filecargo_remote_fs::EntryKind;

    use super::*;

    fn file(size: u64, modified: Option<u64>) -> Entry {
        let mut entry = Entry::new("f", EntryKind::File);
        entry.size = size;
        entry.modified = modified.map(|s| UNIX_EPOCH + Duration::from_secs(s));
        entry
    }

    #[test]
    fn ask_skip_overwrite_and_rename_ignore_sizes_and_times() {
        let (source, target) = (file(10, Some(5)), file(99, Some(500)));
        assert_eq!(decide(ConflictRule::Ask, &source, &target), Decision::Ask);
        assert_eq!(decide(ConflictRule::Skip, &source, &target), Decision::Skip);
        assert_eq!(
            decide(ConflictRule::Overwrite, &source, &target),
            Decision::Write { offset: 0 }
        );
        assert_eq!(
            decide(ConflictRule::Rename, &source, &target),
            Decision::Rename
        );
    }

    #[test]
    fn overwrite_if_newer_needs_a_strictly_newer_known_source() {
        let rule = ConflictRule::OverwriteIfNewer;
        let target = file(1, Some(100));
        assert_eq!(
            decide(rule, &file(1, Some(101)), &target),
            Decision::Write { offset: 0 }
        );
        assert_eq!(
            decide(rule, &file(1, Some(100)), &target),
            Decision::Skip,
            "equal is not newer"
        );
        assert_eq!(decide(rule, &file(1, Some(99)), &target), Decision::Skip);
        assert_eq!(
            decide(rule, &file(1, None), &target),
            Decision::Skip,
            "unknown source time"
        );
        assert_eq!(
            decide(rule, &file(1, Some(101)), &file(1, None)),
            Decision::Skip,
            "unknown target time"
        );
        assert_eq!(decide(rule, &file(1, None), &file(1, None)), Decision::Skip);
    }

    #[test]
    fn resume_continues_a_smaller_target_skips_an_equal_one_and_restarts_a_larger_one() {
        let source = file(100, None);
        assert_eq!(
            decide(ConflictRule::Resume, &source, &file(40, None)),
            Decision::Write { offset: 40 }
        );
        assert_eq!(
            decide(ConflictRule::Resume, &source, &file(0, None)),
            Decision::Write { offset: 0 }
        );
        assert_eq!(
            decide(ConflictRule::Resume, &source, &file(100, None)),
            Decision::Skip
        );
        assert_eq!(
            decide(ConflictRule::Resume, &source, &file(101, None)),
            Decision::Write { offset: 0 }
        );
    }

    #[test]
    fn numbered_names_keep_the_extension_last() {
        assert_eq!(numbered_name("report.txt", 1), "report (1).txt");
        assert_eq!(numbered_name("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered_name(".profile", 1), ".profile (1)");
        assert_eq!(numbered_name("README", 3), "README (3)");
        assert_eq!(numbered_name("trailing.", 1), "trailing (1).");
    }
}
