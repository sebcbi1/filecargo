//! Server-tree commands: thin wrappers over `ConfigStore` that turn errors into prompts.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use filecargo_config::{ImportOptions, ImportReport, SecretKey, SecretString, SiteId, TreeOp};

use crate::app::Core;
use crate::state::Level;

/// `2026-10-05` for a point in time, UTC (Howard Hinnant's civil-from-days).
pub(crate) fn iso_date(time: SystemTime) -> String {
    let days = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs() / 86_400).unwrap_or(0));
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
    format!("{year:04}-{month:02}-{day:02}")
}

fn describe(report: &ImportReport) -> (Level, String) {
    let Some(root) = &report.root_folder else {
        return (
            Level::Info,
            "Nothing was imported: the file contains no usable sites.".to_owned(),
        );
    };
    let mut body = format!(
        "Imported {} sites into the folder \"{root}\".",
        report.imported.len()
    );
    if !report.skipped.is_empty() {
        body.push_str(&format!("\n\nSkipped {}:", report.skipped.len()));
        for skipped in &report.skipped {
            body.push_str(&format!("\n  • {}: {}", skipped.site, skipped.reason));
        }
    }
    if !report.passwords_skipped.is_empty() {
        body.push_str(&format!(
            "\n\nPasswords not imported for {} sites:",
            report.passwords_skipped.len()
        ));
        for skipped in &report.passwords_skipped {
            body.push_str(&format!("\n  • {}: {}", skipped.site, skipped.reason));
        }
    }
    let level = if report.skipped.is_empty() && report.passwords_skipped.is_empty() {
        Level::Info
    } else {
        Level::Warning
    };
    (level, body)
}

impl Core {
    pub(crate) fn apply_tree_op(&mut self, op: TreeOp) {
        let Some(store) = self.store.as_mut() else {
            self.message(
                Level::Error,
                "Cannot change the server list",
                "The configuration could not be loaded; reset it first.",
            );
            return;
        };
        match store.apply(op) {
            Ok(_) => {
                self.state.servers = Arc::new(store.tree().clone());
                self.changed();
            }
            Err(error) => self.message(
                Level::Error,
                "Cannot change the server list",
                error.to_string(),
            ),
        }
    }

    pub(crate) fn import_filezilla(&mut self, path: &Path, import_passwords: bool) {
        let Some(store) = self.store.as_mut() else {
            self.message(
                Level::Error,
                "Cannot import",
                "The configuration could not be loaded; reset it first.",
            );
            return;
        };
        let folder_name = format!("FileZilla import {}", iso_date(SystemTime::now()));
        match store.import_filezilla(
            path,
            ImportOptions {
                import_passwords,
                folder_name,
            },
        ) {
            Ok(report) => {
                self.state.servers = Arc::new(store.tree().clone());
                tracing::info!(target: "filecargo::app", imported = report.imported.len(), skipped = report.skipped.len(), "FileZilla import finished");
                let (level, body) = describe(&report);
                self.message(level, "FileZilla import", body);
            }
            Err(error) => self.message(Level::Error, "Import failed", error.to_string()),
        }
    }

    pub(crate) fn set_site_password(&mut self, site: SiteId, secret: &SecretString) {
        if let Err(error) = self.secrets.set(&SecretKey::Password(site), secret) {
            self.message(Level::Error, "Password not saved", error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn dates_are_utc_calendar_dates() {
        let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
        assert_eq!(iso_date(at(0)), "1970-01-01");
        assert_eq!(iso_date(at(951_868_799)), "2000-02-29");
        assert_eq!(iso_date(at(1_790_769_600)), "2026-09-30");
        assert_eq!(iso_date(at(4_102_444_800)), "2100-01-01");
    }
}
