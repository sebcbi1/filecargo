//! UTC date arithmetic for FTP timestamps (`YYYYMMDDHHMMSS[.sss]`), without a date crate.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Parses `YYYYMMDDHHMMSS` with an optional `.fraction` (ignored). `None` when malformed.
pub(crate) fn parse_timestamp(text: &str) -> Option<SystemTime> {
    let digits = text.split('.').next()?;
    if digits.len() != 14 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let field = |range: std::ops::Range<usize>| digits[range].parse::<i64>().ok();
    let (year, month, day) = (field(0..4)?, field(4..6)?, field(6..8)?);
    let (hour, minute, second) = (field(8..10)?, field(10..12)?, field(12..14)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(secs)
        .ok()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

/// Formats as `YYYYMMDDHHMMSS` (UTC), the argument of `MFMT`.
pub(crate) fn format_timestamp(time: SystemTime) -> Option<String> {
    let secs = i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rest = secs.rem_euclid(86_400);
    Some(format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(time: SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    #[test]
    fn parses_known_instants() {
        assert_eq!(secs(parse_timestamp("19700101000000").unwrap()), 0);
        assert_eq!(
            secs(parse_timestamp("20000229235959").unwrap()),
            951_868_799
        );
        assert_eq!(
            secs(parse_timestamp("20260930120000").unwrap()),
            1_790_769_600
        );
    }

    #[test]
    fn fractions_are_ignored() {
        assert_eq!(
            parse_timestamp("20260930120000.987"),
            parse_timestamp("20260930120000")
        );
    }

    #[test]
    fn malformed_values_are_rejected() {
        for bad in [
            "",
            "2026",
            "2026093012000x",
            "20261301000000",
            "20260932000000",
            "20260930250000",
        ] {
            assert_eq!(parse_timestamp(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn format_round_trips_across_leap_days_and_epochs() {
        for text in [
            "19700101000000",
            "20000229235959",
            "20240229000000",
            "21001231235959",
            "20260930120000",
        ] {
            assert_eq!(
                format_timestamp(parse_timestamp(text).unwrap()).as_deref(),
                Some(text)
            );
        }
    }
}
