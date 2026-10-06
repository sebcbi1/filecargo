#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Lines emitted by a transfer worker carry the item's site through the `site` span field.
//! Own test binary: the capture layer is the process-wide subscriber, because workers run on
//! other threads.

mod support;

use std::sync::{Mutex, OnceLock};

use filecargo_transfer::{Direction, NewTransfer};
use support::{Harness, remote};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// `(message, site of the nearest span that has one)`.
type Captured = (String, Option<String>);

/// Every event seen so far.
static EVENTS: OnceLock<Mutex<Vec<Captured>>> = OnceLock::new();

struct SiteField(String);

#[derive(Default)]
struct Grab {
    site: Option<String>,
    message: String,
}

impl Visit for Grab {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "site" => self.site = Some(format!("{value:?}")),
            "message" => self.message = format!("{value:?}"),
            _ => {}
        }
    }
}

struct Capture;

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Capture {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let mut grab = Grab::default();
        attrs.record(&mut grab);
        if let (Some(site), Some(span)) = (grab.site, ctx.span(id)) {
            span.extensions_mut().insert(SiteField(site));
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let mut grab = Grab::default();
        event.record(&mut grab);
        let site = ctx.event_scope(event).and_then(|scope| {
            scope
                .into_iter()
                .find_map(|span| span.extensions().get::<SiteField>().map(|s| s.0.clone()))
        });
        EVENTS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .push((grab.message, site));
    }
}

#[tokio::test]
async fn a_line_from_a_spawned_worker_is_tagged_with_the_items_site() {
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Capture)).unwrap();
    let mut h = Harness::new(1).await;
    std::fs::write(h.local.path().join("a.txt"), "hello").unwrap();
    h.queue.enqueue(vec![NewTransfer {
        site: h.site,
        direction: Direction::Upload,
        local: h.local.path().join("a.txt"),
        remote: remote("/a.txt"),
        is_dir: false,
        size: None,
        conflict: None,
    }]);
    h.idle().await;
    let events = EVENTS.get_or_init(Mutex::default).lock().unwrap().clone();
    let started = events
        .iter()
        .find(|(message, _)| message.contains("starting"))
        .unwrap_or_else(|| panic!("no worker line in {events:?}"));
    assert_eq!(started.1.as_deref(), Some(h.site.to_string().as_str()));
}
