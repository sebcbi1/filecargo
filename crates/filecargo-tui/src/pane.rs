//! A file pane as the reducer and the view both see it, plus how entries are written out.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use filecargo_app_core::prelude::*;

use crate::ui_state::Focus;

pub(crate) struct PaneView<'a> {
    pub id: PaneId,
    pub path: String,
    pub has_parent: bool,
    pub entries: &'a [Entry],
    pub generation: u64,
    pub sort: Sort,
    pub loading: bool,
    pub error: Option<&'a str>,
}

impl PaneView<'_> {
    /// Rows shown: `..` (if any) plus the entries.
    pub fn rows(&self) -> usize {
        self.entries.len() + usize::from(self.has_parent)
    }

    /// The entry under row `row` (`None` for the `..` row).
    pub fn entry(&self, row: usize) -> Option<&Entry> {
        row.checked_sub(usize::from(self.has_parent))
            .and_then(|i| self.entries.get(i))
    }
}

pub(crate) fn pane_view(app: &AppState, focus: Focus) -> Option<PaneView<'_>> {
    match focus {
        Focus::Local => Some(PaneView {
            id: PaneId::Local,
            path: app.local.path.display().to_string(),
            has_parent: app.local.path.parent().is_some(),
            entries: &app.local.entries,
            generation: app.local.generation,
            sort: app.local.sort,
            loading: app.local.loading,
            error: app.local.error.as_deref(),
        }),
        Focus::Remote => app.remote.as_ref().map(|p| PaneView {
            id: PaneId::Remote,
            path: p.path.to_string(),
            has_parent: p.path.parent().is_some(),
            entries: &p.entries,
            generation: p.generation,
            sort: p.sort,
            loading: p.loading,
            error: p.error.as_deref(),
        }),
        _ => None,
    }
}

/// `2.1K`, `4.3M`: at most four characters for anything under a terabyte.
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["", "K", "M", "G", "T"];
    if bytes < 1024 {
        return bytes.to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0}{}", UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

/// (year, month, day, hour, minute) in UTC.
fn civil(time: SystemTime) -> (i64, i64, i64, i64, i64) {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day, rest / 3600, rest % 3600 / 60)
}

/// `10-01 14:02` for recent files, `2023-10-01` for old ones (UTC).
pub(crate) fn format_time(modified: Option<SystemTime>, now: SystemTime) -> String {
    let Some(time) = modified else {
        return String::new();
    };
    let (y, mo, d, h, mi) = civil(time);
    let age = now
        .duration_since(time)
        .or_else(|e| Ok::<_, ()>(e.duration()))
        .unwrap_or(Duration::ZERO);
    if age < Duration::from_secs(180 * 86_400) {
        format!("{mo:02}-{d:02} {h:02}:{mi:02}")
    } else {
        format!("{y:04}-{mo:02}-{d:02}")
    }
}

/// `/home/me/projects` → `~/projects` when `home` is a prefix.
pub(crate) fn abbreviate(path: &str, home: Option<&std::path::Path>) -> String {
    if let Some(home) = home.and_then(|h| h.to_str())
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(home)
        && (rest.is_empty() || rest.starts_with(['/', '\\']))
    {
        return format!("~{rest}");
    }
    path.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_short_and_human() {
        for (bytes, shown) in [
            (0, "0"),
            (999, "999"),
            (1024, "1.0K"),
            (2150, "2.1K"),
            (4_500_000, "4.3M"),
            (150 * 1024 * 1024, "150M"),
            (5 * 1024u64.pow(4), "5.0T"),
        ] {
            assert_eq!(format_size(bytes), shown, "{bytes}");
        }
    }

    #[test]
    fn recent_times_show_month_day_and_clock_old_ones_the_year() {
        let now = UNIX_EPOCH + Duration::from_secs(1_790_769_600); // 2026-09-30 12:00 UTC
        let at = |secs| Some(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(format_time(at(1_790_769_600 - 3600), now), "09-30 11:00");
        assert_eq!(
            format_time(at(1_790_769_600 - 100 * 86_400), now),
            "06-22 12:00"
        );
        assert_eq!(format_time(at(1_500_000_000), now), "2017-07-14");
        assert_eq!(format_time(None, now), "");
    }

    #[test]
    fn home_is_abbreviated_only_on_a_path_boundary() {
        let home = std::path::Path::new("/home/me");
        assert_eq!(abbreviate("/home/me", Some(home)), "~");
        assert_eq!(abbreviate("/home/me/projects", Some(home)), "~/projects");
        assert_eq!(abbreviate("/home/mellow/x", Some(home)), "/home/mellow/x");
        assert_eq!(abbreviate("/var/www", Some(home)), "/var/www");
        assert_eq!(abbreviate("/home/me", None), "/home/me");
    }
}
