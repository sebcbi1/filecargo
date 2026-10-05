//! Our own MLSD / MLST fact parser. The FTP library's rejects real-world lines: symlink types
//! (`OS.unix=slink:<target>`), fractional `modify` times and names containing `;`.

use crate::{Entry, EntryKind};

use super::time::parse_timestamp;

/// What one listing line turned into.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed {
    Entry(Entry),
    /// `cdir` / `pdir` / blank: not part of the directory's contents.
    Skip,
}

/// Parses one MLSD line (or the fact line of an MLST reply): `fact=value;fact=value; name`.
/// The first space ends the facts, so names may contain `;` and spaces.
pub(crate) fn parse_line(line: &str) -> Parsed {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return Parsed::Skip;
    }
    let Some((facts, name)) = line.split_once(' ') else {
        // No separator at all: not a fact line. Keep it visible rather than drop it.
        return Parsed::Entry(Entry::new(line, EntryKind::Other));
    };
    if name.is_empty() {
        return Parsed::Skip;
    }

    let mut kind = EntryKind::Other;
    let mut entry = Entry::new(name, EntryKind::Other);
    for fact in facts.split(';').filter(|f| !f.is_empty()) {
        let Some((key, value)) = fact.split_once('=') else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "type" => {
                let lowered = value.to_ascii_lowercase();
                kind = match lowered.as_str() {
                    "file" => EntryKind::File,
                    "dir" => EntryKind::Dir,
                    "cdir" | "pdir" => return Parsed::Skip,
                    other => match other.strip_prefix("os.unix=") {
                        Some("symlink") => EntryKind::Symlink { target: None },
                        Some(rest) if rest.starts_with("slink") => EntryKind::Symlink {
                            // keep the original case of the target
                            target: value
                                .split_once(':')
                                .map(|(_, t)| t.to_owned())
                                .filter(|t| !t.is_empty()),
                        },
                        _ => EntryKind::Other,
                    },
                };
            }
            "size" => entry.size = value.parse().unwrap_or(0),
            "modify" => entry.modified = parse_timestamp(value),
            "unix.mode" => {
                entry.permissions = u32::from_str_radix(value, 8).ok().map(|m| m & 0o7777);
            }
            "unix.owner" if !value.chars().all(|c| c.is_ascii_digit()) => {
                entry.owner = Some(value.to_owned());
            }
            "unix.group" if !value.chars().all(|c| c.is_ascii_digit()) => {
                entry.group = Some(value.to_owned());
            }
            _ => {}
        }
    }
    if kind == EntryKind::Dir {
        entry.size = 0;
    }
    entry.kind = kind;
    Parsed::Entry(entry)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn entry(line: &str) -> Entry {
        match parse_line(line) {
            Parsed::Entry(e) => e,
            Parsed::Skip => panic!("{line:?} was skipped"),
        }
    }

    #[test]
    fn regular_file_with_all_facts() {
        let e = entry(
            "type=file;size=1024;modify=20260930120000;UNIX.mode=0644;UNIX.owner=alice;UNIX.group=staff; index.php",
        );
        assert_eq!(e.name, "index.php");
        assert_eq!(
            (e.kind.clone(), e.size, e.permissions),
            (EntryKind::File, 1024, Some(0o644))
        );
        assert_eq!(
            e.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_790_769_600))
        );
        assert_eq!(
            (e.owner.as_deref(), e.group.as_deref()),
            (Some("alice"), Some("staff"))
        );
    }

    #[test]
    fn numeric_owner_and_group_are_not_names() {
        let e = entry("type=file;size=1;UNIX.owner=1000;UNIX.group=100; f");
        assert_eq!((e.owner, e.group), (None, None));
    }

    #[test]
    fn directory_has_zero_size_and_fractional_time_is_accepted() {
        let e = entry("type=dir;size=4096;modify=20260930120000.123;perm=flcdmpe; html");
        assert_eq!((e.kind, e.size), (EntryKind::Dir, 0));
        assert!(e.modified.is_some());
    }

    #[test]
    fn unix_symlink_types_keep_the_target() {
        let e = entry("type=OS.unix=slink:/etc/Real;size=4;modify=20240101000000; link");
        assert_eq!(
            e.kind,
            EntryKind::Symlink {
                target: Some("/etc/Real".into())
            }
        );
        let e = entry("type=OS.unix=symlink;size=4; other");
        assert_eq!(e.kind, EntryKind::Symlink { target: None });
    }

    #[test]
    fn names_may_contain_semicolons_and_spaces() {
        let e = entry("type=file;size=5; name; with semicolon.txt");
        assert_eq!(e.name, "name; with semicolon.txt");
        let e = entry("type=file;size=5;  leading space");
        assert_eq!(e.name, " leading space");
    }

    #[test]
    fn fact_names_are_case_insensitive() {
        let e = entry("Type=File;Size=9;Modify=20260101000000; Upper.txt");
        assert_eq!((e.kind, e.size), (EntryKind::File, 9));
    }

    #[test]
    fn current_and_parent_dir_lines_are_skipped() {
        assert_eq!(
            parse_line("type=cdir;modify=20260101000000; ."),
            Parsed::Skip
        );
        assert_eq!(
            parse_line("type=pdir;modify=20260101000000; .."),
            Parsed::Skip
        );
        assert_eq!(parse_line(""), Parsed::Skip);
        assert_eq!(parse_line("type=file;size=3; "), Parsed::Skip);
    }

    #[test]
    fn unknown_types_and_lines_without_facts_stay_visible() {
        assert_eq!(
            entry("type=OS.unix=socket;size=0; sock").kind,
            EntryKind::Other
        );
        let e = entry(" just-a-name");
        assert_eq!((e.name.as_str(), e.kind), ("just-a-name", EntryKind::Other));
        let e = entry("garbage");
        assert_eq!((e.name.as_str(), e.kind), ("garbage", EntryKind::Other));
    }

    #[test]
    fn crlf_is_stripped() {
        assert_eq!(entry("type=file;size=1; a\r\n").name, "a");
    }
}
