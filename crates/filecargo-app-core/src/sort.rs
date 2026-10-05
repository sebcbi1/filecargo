//! Ordering of pane entries: directories first, then by the chosen key.

use std::cmp::Ordering;

use filecargo_remote_fs::Entry;

use crate::state::{Sort, SortKey};

/// Case-insensitive comparison where runs of digits compare as numbers: `file2` < `file10`.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    digits
                };
                let (da, db) = (take(&mut a), take(&mut b));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let order = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if order != Ordering::Equal {
                    return order;
                }
                // equal value: fewer leading zeros first, so the order is total
                let order = da.len().cmp(&db.len());
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn by_name(a: &Entry, b: &Entry) -> Ordering {
    natural_cmp(&a.name, &b.name).then_with(|| a.name.cmp(&b.name))
}

/// Directories first; within each group by `sort.key`, reversed when descending. Ties fall
/// back to the name so the order is deterministic.
pub(crate) fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        b.is_dir().cmp(&a.is_dir()).then_with(|| {
            let primary = match sort.key {
                SortKey::Name => by_name(a, b),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Modified => a.modified.cmp(&b.modified),
            };
            let primary = if sort.ascending {
                primary
            } else {
                primary.reverse()
            };
            // ties are always by name, ascending: equal sizes keep a stable, readable order
            primary.then_with(|| by_name(a, b))
        })
    });
}

/// Hides dotfiles unless `show_hidden`.
pub(crate) fn retain_visible(entries: &mut Vec<Entry>, show_hidden: bool) {
    if !show_hidden {
        entries.retain(|e| !e.name.starts_with('.'));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use filecargo_remote_fs::EntryKind;

    use super::*;

    fn entry(name: &str, dir: bool, size: u64, modified: Option<u64>) -> Entry {
        let mut e = Entry::new(name, if dir { EntryKind::Dir } else { EntryKind::File });
        e.size = size;
        e.modified = modified.map(|s| UNIX_EPOCH + Duration::from_secs(s));
        e
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn natural_order_compares_digit_runs_as_numbers_and_ignores_case() {
        fn sorted(mut v: Vec<&str>) -> Vec<&str> {
            v.sort_by(|a, b| natural_cmp(a, b));
            v
        }
        assert_eq!(
            sorted(vec!["file10", "file2", "file1"]),
            ["file1", "file2", "file10"]
        );
        assert_eq!(sorted(vec!["b", "A", "c", "B"]), ["A", "b", "B", "c"]);
        assert_eq!(
            sorted(vec!["img007", "img7", "img07"]),
            ["img7", "img07", "img007"]
        );
        assert_eq!(
            sorted(vec!["a2b10", "a2b9", "a10b1"]),
            ["a2b9", "a2b10", "a10b1"]
        );
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("same", "SAME"), Ordering::Equal);
    }

    #[test]
    fn directories_come_first_whatever_the_key_and_direction() {
        let make = || {
            vec![
                entry("zeta.txt", false, 5, Some(30)),
                entry("beta", true, 0, Some(10)),
                entry("alpha.txt", false, 50, Some(20)),
                entry("Gamma", true, 0, Some(40)),
            ]
        };
        let sorted = |key, ascending| {
            let mut v = make();
            sort_entries(&mut v, Sort { key, ascending });
            v.iter().map(|e| e.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(
            sorted(SortKey::Name, true),
            ["beta", "Gamma", "alpha.txt", "zeta.txt"]
        );
        assert_eq!(
            sorted(SortKey::Name, false),
            ["Gamma", "beta", "zeta.txt", "alpha.txt"]
        );
        assert_eq!(
            sorted(SortKey::Size, true),
            ["beta", "Gamma", "zeta.txt", "alpha.txt"]
        );
        assert_eq!(
            sorted(SortKey::Size, false),
            ["beta", "Gamma", "alpha.txt", "zeta.txt"]
        );
        assert_eq!(
            sorted(SortKey::Modified, true),
            ["beta", "Gamma", "alpha.txt", "zeta.txt"]
        );
        assert_eq!(
            sorted(SortKey::Modified, false),
            ["Gamma", "beta", "zeta.txt", "alpha.txt"]
        );
    }

    #[test]
    fn unknown_times_sort_before_known_ones_and_ties_fall_back_to_the_name() {
        let mut v = vec![
            entry("b", false, 1, Some(5)),
            entry("a", false, 1, None),
            entry("c", false, 1, Some(5)),
        ];
        sort_entries(
            &mut v,
            Sort {
                key: SortKey::Modified,
                ascending: true,
            },
        );
        assert_eq!(names(&v), ["a", "b", "c"]);
        let mut v = vec![entry("b", false, 7, None), entry("a", false, 7, None)];
        sort_entries(
            &mut v,
            Sort {
                key: SortKey::Size,
                ascending: true,
            },
        );
        assert_eq!(names(&v), ["a", "b"]);
    }

    #[test]
    fn dotfiles_are_hidden_unless_asked_for() {
        let make = || {
            vec![
                entry(".git", true, 0, None),
                entry(".env", false, 1, None),
                entry("src", true, 0, None),
            ]
        };
        let mut hidden = make();
        retain_visible(&mut hidden, false);
        assert_eq!(names(&hidden), ["src"]);
        let mut shown = make();
        retain_visible(&mut shown, true);
        assert_eq!(shown.len(), 3);
    }
}
