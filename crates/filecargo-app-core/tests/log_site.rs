#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Session lines carry the site of the session. Own test binary: connecting runs on the app's
//! threads, so the layer has to be the process-wide subscriber.

mod support;

use filecargo_app_core::{Command, LogLayer};
use support::{Fixture, TestFactory, server_tree, site_for};
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn lines_logged_while_connecting_are_tagged_with_the_site() {
    let server = server_tree();
    let factory = TestFactory::new();
    factory.serve("host", server.path().to_path_buf(), None);
    let fx = Fixture::with_factory(factory);
    let buffer = fx.app.log();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(LogLayer::new(buffer.clone())),
    )
    .unwrap();
    tracing::info!("before connecting");
    let id = fx.add_site(site_for("s", "host"));
    fx.app.send(Command::Connect(id));
    fx.wait_for("connected", |s| s.remote.is_some());
    let lines = buffer.snapshot();
    let site_of = |text: &str| {
        lines
            .iter()
            .find(|l| l.message.starts_with(text))
            .unwrap_or_else(|| panic!("no {text:?} in {lines:#?}"))
            .site
    };
    assert_eq!(site_of("connecting"), Some(id));
    assert_eq!(site_of("connected"), Some(id));
    assert_eq!(site_of("before connecting"), None);
}
