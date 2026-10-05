#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::SystemTime;

use filecargo_app_core::{Command, LogBuffer, LogLayer, LogLine};
use filecargo_config::LogLevel;
use support::Fixture;
use tracing_subscriber::layer::SubscriberExt;

fn with_layer<R>(layer: LogLayer, f: impl FnOnce() -> R) -> R {
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, f)
}

#[test]
fn protocol_lines_arrive_with_their_target_level_and_fields() {
    let buffer = LogBuffer::new();
    with_layer(LogLayer::new(buffer.clone()), || {
        tracing::info!(target: "filecargo::protocol", site = "prod", "connecting over SSH");
        tracing::warn!(target: "filecargo::transfer", "something odd");
    });
    let lines = buffer.snapshot();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        (lines[0].level, lines[0].target.as_str()),
        (LogLevel::Info, "filecargo::protocol")
    );
    assert!(
        lines[0].message.starts_with("connecting over SSH"),
        "{}",
        lines[0].message
    );
    assert!(
        lines[0].message.contains("site=prod"),
        "{}",
        lines[0].message
    );
    assert_eq!(lines[1].level, LogLevel::Warn);
    assert!(buffer.generation() >= 2);
}

#[test]
fn the_ring_drops_the_oldest_lines_beyond_5000() {
    let buffer = LogBuffer::new();
    for i in 0..5_100 {
        buffer.push(LogLine {
            time: SystemTime::now(),
            level: LogLevel::Info,
            target: "t".into(),
            message: format!("line {i}"),
        });
    }
    assert_eq!(buffer.len(), 5_000);
    buffer.with_lines(|lines| {
        assert_eq!(lines.front().unwrap().message, "line 100");
        assert_eq!(lines.back().unwrap().message, "line 5099");
    });
}

#[test]
fn events_below_the_configured_level_are_dropped() {
    let buffer = LogBuffer::new(); // info by default
    with_layer(LogLayer::new(buffer.clone()), || {
        tracing::debug!("too chatty");
        tracing::info!("kept");
        buffer.set_level(LogLevel::Error);
        tracing::warn!("now too chatty");
        tracing::error!("still kept");
        buffer.set_level(LogLevel::Trace);
        tracing::trace!("everything");
    });
    let messages: Vec<_> = buffer.snapshot().into_iter().map(|l| l.message).collect();
    assert_eq!(messages, ["kept", "still kept", "everything"]);
}

#[test]
fn update_settings_changes_the_log_level_live() {
    let fx = Fixture::new();
    let buffer = fx.app.log();
    with_layer(LogLayer::new(buffer.clone()), || {
        tracing::debug!("before");
        let mut settings = (*fx.state().settings).clone();
        settings.log.level = LogLevel::Debug;
        fx.app.send(Command::UpdateSettings(settings));
        fx.wait_for("the new level", |s| s.settings.log.level == LogLevel::Debug);
        tracing::debug!("after");
    });
    let messages: Vec<_> = buffer.snapshot().into_iter().map(|l| l.message).collect();
    assert_eq!(messages, ["after"]);
}

#[test]
fn the_log_generation_in_the_snapshot_follows_new_lines() {
    let fx = Fixture::new();
    let before = fx.state().log_generation;
    with_layer(LogLayer::new(fx.app.log()), || tracing::info!("hello"));
    // any state change publishes a snapshot that carries the current generation
    fx.app
        .send(Command::DismissNotice(filecargo_app_core::NoticeId(0)));
    let mut settings = (*fx.state().settings).clone();
    settings.transfers.max_concurrent = 7;
    fx.app.send(Command::UpdateSettings(settings));
    let state = fx.wait_for("a snapshot with the new generation", |s| {
        s.log_generation > before
    });
    assert!(state.log_generation > before);
}

#[test]
fn lines_are_appended_to_the_log_file_when_one_is_configured() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("filecargo.log");
    std::fs::write(&path, "existing line\n").unwrap();
    let buffer = LogBuffer::new();
    with_layer(LogLayer::new(buffer.clone()).with_file(&path), || {
        tracing::info!(target: "filecargo::protocol", "first");
        tracing::error!("second");
    });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("existing line\n"), "{text}");
    assert!(text.contains("INFO  filecargo::protocol: first"), "{text}");
    assert!(text.contains("ERROR") && text.contains("second"), "{text}");
    assert_eq!(buffer.len(), 2, "the memory buffer gets the same lines");
}
