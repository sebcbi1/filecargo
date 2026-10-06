//! How sizes, times and permissions are written. (The TUI has the same helpers; the front-ends
//! share only `app-core`, so each keeps its own copy.)

use std::time::{SystemTime, UNIX_EPOCH};

/// `512`, `2.1K`, `4.3M`, `1.5G`.
pub fn format_size(bytes: u64) -> String {
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

/// `text` cut to `width` characters from the left: `…html/index.php`.
pub fn truncate_left(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let tail: String = text.chars().skip(count - (width - 1)).collect();
    format!("…{tail}")
}

/// `2026-10-01 14:02` (UTC).
pub fn format_time(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return String::new();
    };
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
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// `drwxr-xr-x` from the mode bits and whether the entry is a directory.
pub fn format_permissions(mode: Option<u32>, is_dir: bool) -> String {
    let Some(mode) = mode else {
        return String::new();
    };
    let mut text = String::from(if is_dir { "d" } else { "-" });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        text.push(if bits & 4 != 0 { 'r' } else { '-' });
        text.push(if bits & 2 != 0 { 'w' } else { '-' });
        text.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_lose_their_left_side_when_too_long() {
        assert_eq!(truncate_left("/www/index.php", 20), "/www/index.php");
        assert_eq!(
            truncate_left("/var/www/html/index.php", 15),
            "…html/index.php"
        );
        assert_eq!(truncate_left("é/é/éééé", 5), "…éééé");
        assert_eq!(truncate_left("abc", 0), "");
    }

    #[test]
    fn sizes_times_and_permissions() {
        assert_eq!(format_size(340), "340");
        assert_eq!(format_size(2150), "2.1K");
        assert_eq!(format_size(1_610_612_736), "1.5G");
        assert_eq!(format_time(None), "");
        assert_eq!(
            format_time(Some(
                UNIX_EPOCH + std::time::Duration::from_secs(1_790_769_600)
            )),
            "2026-09-30 12:00"
        );
        assert_eq!(format_permissions(Some(0o755), true), "drwxr-xr-x");
        assert_eq!(format_permissions(Some(0o640), false), "-rw-r-----");
        assert_eq!(format_permissions(None, false), "");
    }
}
