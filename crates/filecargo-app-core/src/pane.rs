//! The local pane: background listing, navigation and sorting. A newer navigation supersedes
//! an older listing that is still running; the late result is discarded.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filecargo_remote_fs::{Entry, local};

use crate::app::{Core, Msg};
use crate::sort::{retain_visible, sort_entries};
use crate::state::Sort;

/// What a listing task found, or why it failed.
pub(crate) struct LocalListing {
    pub seq: u64,
    pub result: Result<(PathBuf, Vec<Entry>), String>,
}

/// `\\?\C:\x` → `C:\x`: canonical paths on Windows carry a verbatim prefix nobody wants to read.
fn tidy(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(windows)]
fn hide_windows_hidden(dir: &Path, entries: &mut Vec<Entry>) {
    use std::os::windows::fs::MetadataExt;
    entries.retain(|e| {
        std::fs::symlink_metadata(dir.join(&e.name))
            .map(|m| m.file_attributes() & 0x2 == 0)
            .unwrap_or(true)
    });
}

#[cfg(not(windows))]
fn hide_windows_hidden(_: &Path, _: &mut Vec<Entry>) {}

async fn list(target: PathBuf, show_hidden: bool) -> Result<(PathBuf, Vec<Entry>), String> {
    let canonical = tokio::fs::canonicalize(&target)
        .await
        .map(tidy)
        .map_err(|e| format!("{}: {e}", target.display()))?;
    let mut entries = local::read_dir(&canonical)
        .await
        .map_err(|e| e.to_string())?;
    if !show_hidden {
        hide_windows_hidden(&canonical, &mut entries);
    }
    retain_visible(&mut entries, show_hidden);
    Ok((canonical, entries))
}

/// `~` and `~/x` mean the home directory; otherwise absolute, or relative to `base`.
fn resolve(base: &Path, typed: &str) -> PathBuf {
    let typed = typed.trim();
    if let Some(rest) = typed.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with(['/', '\\']))
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest.trim_start_matches(['/', '\\']));
    }
    let path = PathBuf::from(typed);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

impl Core {
    /// Starts a listing of `target` for the local pane, superseding any running one.
    pub(crate) fn list_local(&mut self, target: PathBuf) {
        self.local_seq += 1;
        let seq = self.local_seq;
        let show_hidden = self.state.settings.ui.show_hidden;
        self.state.local.loading = true;
        self.changed();
        let messages = self.messages.clone();
        tokio::spawn(async move {
            let result = list(target, show_hidden).await;
            let _ = messages.send(Msg::LocalListed(LocalListing { seq, result }));
        });
    }

    pub(crate) fn navigate_local(&mut self, typed: &str) {
        let target = resolve(&self.state.local.path, typed);
        self.list_local(target);
    }

    pub(crate) fn up_local(&mut self) {
        if let Some(parent) = self.state.local.path.parent() {
            let parent = parent.to_path_buf();
            self.list_local(parent);
        }
    }

    pub(crate) fn refresh_local(&mut self) {
        let path = self.state.local.path.clone();
        self.list_local(path);
    }

    pub(crate) fn on_local_listed(&mut self, listing: LocalListing) {
        if listing.seq != self.local_seq {
            return; // a newer navigation superseded this listing
        }
        let pane = &mut self.state.local;
        pane.loading = false;
        match listing.result {
            Ok((path, mut entries)) => {
                sort_entries(&mut entries, pane.sort);
                pane.path = path;
                pane.entries = Arc::from(entries);
                pane.error = None;
                pane.generation += 1;
            }
            Err(error) => {
                // the previous entries stay on screen
                tracing::info!(target: "filecargo::app", %error, "local listing failed");
                pane.error = Some(error);
            }
        }
        self.changed();
    }

    pub(crate) fn sort_local(&mut self, sort: Sort) {
        let pane = &mut self.state.local;
        let mut entries: Vec<Entry> = pane.entries.to_vec();
        sort_entries(&mut entries, sort);
        pane.sort = sort;
        pane.entries = Arc::from(entries);
        pane.generation += 1;
        self.changed();
    }
}
