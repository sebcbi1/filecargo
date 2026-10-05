//! `LIST` fallback for servers without MLSD: the library's Unix and DOS parsers, with a
//! name-only entry for anything neither understands, so no file ever vanishes from a listing.

use suppaftp::list::{File, ListParser, PosixPexQuery};

use crate::{Entry, EntryKind};

fn mode_bits(file: &File) -> u32 {
    let mut mode = 0;
    for (who, shift) in [
        (PosixPexQuery::Owner, 6),
        (PosixPexQuery::Group, 3),
        (PosixPexQuery::Others, 0),
    ] {
        mode |= u32::from(file.can_read(who)) << (shift + 2);
        mode |= u32::from(file.can_write(who)) << (shift + 1);
        mode |= u32::from(file.can_execute(who)) << shift;
    }
    mode
}

/// `None` for lines that are not entries (`total 12`, blanks).
pub(crate) fn parse_line(line: &str) -> Option<Entry> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() || line.starts_with("total ") {
        return None;
    }
    let (file, posix) = match ListParser::parse_posix(line) {
        Ok(file) => (file, true),
        Err(_) => match ListParser::parse_dos(line) {
            Ok(file) => (file, false),
            Err(_) => return Some(Entry::new(line.trim(), EntryKind::Other)),
        },
    };
    if file.name() == "." || file.name() == ".." {
        return None;
    }
    let kind = if file.is_directory() {
        EntryKind::Dir
    } else if file.is_symlink() {
        EntryKind::Symlink {
            target: file.symlink().map(|p| p.to_string_lossy().into_owned()),
        }
    } else {
        EntryKind::File
    };
    Some(Entry {
        name: file.name().to_owned(),
        size: if kind == EntryKind::Dir {
            0
        } else {
            file.size() as u64
        },
        kind,
        modified: Some(file.modified()),
        permissions: posix.then(|| mode_bits(&file)),
        owner: None,
        group: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_file_directory_and_symlink() {
        let file =
            parse_line("-rw-r--r--   1 user group     1234 Nov  5 13:46 example.txt").unwrap();
        assert_eq!(
            (file.name.as_str(), file.kind, file.size, file.permissions),
            ("example.txt", EntryKind::File, 1234, Some(0o644))
        );
        let dir = parse_line("drwxr-xr-x   2 user group     4096 Nov  5  2024 my dir").unwrap();
        assert_eq!(
            (dir.name.as_str(), dir.kind, dir.size, dir.permissions),
            ("my dir", EntryKind::Dir, 0, Some(0o755))
        );
        let link =
            parse_line("lrwxrwxrwx   1 user group        8 Nov  5 13:46 current -> releases")
                .unwrap();
        assert_eq!(link.name, "current");
        assert_eq!(
            link.kind,
            EntryKind::Symlink {
                target: Some("releases".into())
            }
        );
    }

    #[test]
    fn dos_format() {
        let file = parse_line("11-05-24  01:46PM                 1234 report.txt").unwrap();
        assert_eq!(
            (file.name.as_str(), file.kind, file.size, file.permissions),
            ("report.txt", EntryKind::File, 1234, None)
        );
        let dir = parse_line("11-05-24  01:46PM       <DIR>          archive").unwrap();
        assert_eq!((dir.name.as_str(), dir.kind), ("archive", EntryKind::Dir));
    }

    #[test]
    fn total_lines_blank_lines_and_dot_entries_are_not_entries() {
        assert_eq!(parse_line("total 12"), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(
            parse_line("drwxr-xr-x   2 user group     4096 Nov  5  2024 ."),
            None
        );
        assert_eq!(
            parse_line("drwxr-xr-x   2 user group     4096 Nov  5  2024 .."),
            None
        );
    }

    #[test]
    fn unparseable_lines_become_name_only_entries() {
        let e = parse_line("some odd vendor format 123").unwrap();
        assert_eq!(
            (e.name.as_str(), e.kind),
            ("some odd vendor format 123", EntryKind::Other)
        );
    }
}
