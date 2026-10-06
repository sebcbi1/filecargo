//! The queue as the bottom panel shows it: only the connected site's items. Other sites'
//! transfers keep running; they are only counted.

use std::sync::Arc;

use filecargo_config::SiteId;
use filecargo_transfer::{ItemState, QueueItemView, QueueSnapshot, Totals};

/// The site the panel is about: the connected one.
pub(crate) fn scope_of(session: &crate::state::SessionState) -> Option<SiteId> {
    match session {
        crate::state::SessionState::Connected { site, .. } => Some(*site),
        _ => None,
    }
}

fn only(site: SiteId, views: &[QueueItemView]) -> Vec<QueueItemView> {
    views
        .iter()
        .filter(|v| v.item.site == site)
        .cloned()
        .collect()
}

/// `full` reduced to `scope`'s items; empty without a scope. The totals are recomputed for
/// the scoped items that are still queued (the batch history belongs to the whole queue).
pub(crate) fn scoped(full: &QueueSnapshot, scope: Option<SiteId>) -> QueueSnapshot {
    let Some(site) = scope else {
        return QueueSnapshot {
            processing: full.processing,
            ..QueueSnapshot::default()
        };
    };
    let pending = only(site, &full.pending);
    let (mut done, mut total) = (0u64, 0u64);
    let (mut rate, mut any_rate) = (0.0, false);
    let mut eta: Option<std::time::Duration> = None;
    for view in &pending {
        let size = view.item.size.unwrap_or(view.item.transferred);
        total += size;
        done += view.item.transferred.min(size);
        if let Some(r) = view.rate {
            rate += r;
            any_rate = true;
        }
        if let Some(e) = view.eta {
            eta = Some(eta.map_or(e, |current| current.max(e)));
        }
    }
    QueueSnapshot {
        pending,
        completed: only(site, &full.completed),
        failed: only(site, &full.failed),
        processing: full.processing,
        totals: Totals {
            bytes_done: done,
            bytes_total: total,
            rate: any_rate.then_some(rate),
            eta,
        },
    }
}

/// Transfers running for sites other than `scope`.
pub(crate) fn other_sites_active(full: &QueueSnapshot, scope: Option<SiteId>) -> usize {
    full.pending
        .iter()
        .filter(|v| matches!(v.item.state, ItemState::Active { .. }))
        .filter(|v| Some(v.item.site) != scope)
        .count()
}

/// What the scoped fields of the state were built from, to rebuild them only on change.
pub(crate) struct ScopeCache {
    queue: Arc<QueueSnapshot>,
    scope: Option<SiteId>,
}

impl ScopeCache {
    pub(crate) fn matches(&self, queue: &Arc<QueueSnapshot>, scope: Option<SiteId>) -> bool {
        Arc::ptr_eq(&self.queue, queue) && self.scope == scope
    }

    pub(crate) fn new(queue: Arc<QueueSnapshot>, scope: Option<SiteId>) -> Self {
        Self { queue, scope }
    }
}
