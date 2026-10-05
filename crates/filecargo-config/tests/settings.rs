#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::fs;

use filecargo_config::{ConfigError, ConfigStore, ConflictRule, LogLevel, Paths, Settings};

fn paths(dir: &tempfile::TempDir) -> Paths {
    Paths::from_override(Some(dir.path().join("cfg")))
}

fn settings_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("cfg/settings.toml")
}

fn write_settings(dir: &tempfile::TempDir, text: &str) {
    fs::create_dir_all(dir.path().join("cfg")).unwrap();
    fs::write(settings_file(dir), text).unwrap();
}

#[test]
fn missing_file_loads_defaults_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::open(paths(&dir)).unwrap();
    assert_eq!(store.settings(), &Settings::default());
    assert_eq!(store.settings().transfers.max_concurrent, 2);
    assert!(store.settings().ui.confirm_delete);
    assert!(!settings_file(&dir).exists());
}

#[test]
fn partial_file_merges_with_defaults_and_unknown_keys_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    write_settings(
        &dir,
        "version = 1\nmystery = true\n[transfers]\nmax_concurrent = 5\n[log]\nlevel = \"debug\"\n[future_section]\nx = 1\n",
    );
    let store = ConfigStore::open(paths(&dir)).unwrap();
    let s = store.settings();
    assert_eq!(s.transfers.max_concurrent, 5);
    assert_eq!(s.transfers.default_conflict, ConflictRule::Ask);
    assert_eq!(s.log.level, LogLevel::Debug);
    assert_eq!(s.connection.timeout_secs, 20);
}

#[test]
fn update_persists_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = ConfigStore::open(paths(&dir)).unwrap();
    store
        .update_settings(|s| {
            s.transfers.max_concurrent = 4;
            s.transfers.default_conflict = ConflictRule::OverwriteIfNewer;
            s.ui.show_hidden = true;
            s.ui.local_start_dir = "/work".into();
        })
        .unwrap();

    let reopened = ConfigStore::open(paths(&dir)).unwrap();
    assert_eq!(reopened.settings(), store.settings());
    assert_eq!(reopened.settings().transfers.max_concurrent, 4);
    let text = fs::read_to_string(settings_file(&dir)).unwrap();
    assert!(text.starts_with("version = 1\n"), "{text}");
    assert!(
        text.contains("default_conflict = \"overwrite_if_newer\""),
        "{text}"
    );
}

#[test]
fn out_of_range_values_are_rejected_with_the_field_name_and_nothing_is_written() {
    type Edit = fn(&mut Settings);
    let cases: [(&str, Edit); 4] = [
        ("transfers.max_concurrent", |s| {
            s.transfers.max_concurrent = 0
        }),
        ("transfers.max_concurrent", |s| {
            s.transfers.max_concurrent = 11
        }),
        ("connection.timeout_secs", |s| s.connection.timeout_secs = 0),
        ("connection.keepalive_secs", |s| {
            s.connection.keepalive_secs = 0
        }),
    ];
    for (field, edit) in cases {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::open(paths(&dir)).unwrap();
        store.update_settings(|s| s.ui.show_hidden = true).unwrap();
        let before = fs::read(settings_file(&dir)).unwrap();

        let err = store.update_settings(edit).unwrap_err();
        assert!(err.to_string().contains(field), "{field}: {err}");
        assert!(matches!(err, ConfigError::Invalid(_)), "{err}");
        assert_eq!(fs::read(settings_file(&dir)).unwrap(), before, "{field}");
        assert!(store.settings().ui.show_hidden);
        assert_eq!(store.settings().transfers.max_concurrent, 2, "{field}");
    }
}

#[test]
fn invalid_file_value_is_an_error_not_a_silent_default() {
    let dir = tempfile::tempdir().unwrap();
    write_settings(&dir, "version = 1\n[transfers]\nmax_concurrent = 99\n");
    let err = ConfigStore::open(paths(&dir)).unwrap_err();
    assert!(
        err.to_string().contains("transfers.max_concurrent"),
        "{err}"
    );

    write_settings(
        &dir,
        "version = 1\n\n[transfers]\ndefault_conflict = \"explode\"\n",
    );
    match ConfigStore::open(paths(&dir)).unwrap_err() {
        ConfigError::Parse { line, .. } => assert_eq!(line, 4),
        other => panic!("expected parse error, got {other}"),
    }
}

#[test]
fn newer_settings_version_is_rejected_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    write_settings(&dir, "version = 2\n");
    let err = ConfigStore::open(paths(&dir)).unwrap_err();
    assert!(
        matches!(err, ConfigError::UnsupportedVersion { found: 2, .. }),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(settings_file(&dir)).unwrap(),
        "version = 2\n"
    );
}

#[test]
fn settings_changes_from_another_instance_are_picked_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = ConfigStore::open(paths(&dir)).unwrap();
    let mut b = ConfigStore::open(paths(&dir)).unwrap();
    a.update_settings(|s| s.ui.show_hidden = true).unwrap();
    b.update_settings(|s| s.transfers.max_concurrent = 7)
        .unwrap();

    let merged = ConfigStore::open(paths(&dir)).unwrap();
    assert!(
        merged.settings().ui.show_hidden,
        "a's edit must survive b's write"
    );
    assert_eq!(merged.settings().transfers.max_concurrent, 7);
}
