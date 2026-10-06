//! File operations on the names of a pane's current directory. The remote pane runs them on
//! the session's filesystem; the local pane runs the same code on a `RootedFs` over the pane's
//! directory, so errors and notices are the same for both.

use std::sync::Arc;

use filecargo_remote_fs::{Entry, EntryKind, FsError, RemoteFs, RemotePath, local::RootedFs};

use crate::app::{Core, Msg};
use crate::command::Command;
use crate::prompt::PromptAction;
use crate::state::{Level, PaneId, PromptKind};

/// What a finished operation reports back.
pub(crate) struct OpDone {
    pub pane: PaneId,
    /// "delete 2 items", for the notice and the log.
    pub what: String,
    pub outcome: Result<(), FsError>,
}

/// A name that is one plain path component (no separators, not `.` / `..`).
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

fn plural(count: usize) -> String {
    if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    }
}

impl Core {
    /// The directory the names of `pane` live in, as a path on the pane's filesystem: the
    /// remote path, or the root of a `RootedFs` over the local directory.
    fn pane_dir(&self, pane: PaneId) -> Option<RemotePath> {
        match pane {
            PaneId::Remote => self.state.remote.as_ref().map(|p| p.path.clone()),
            PaneId::Local => Some(RemotePath::root()),
        }
    }

    fn pane_entries(&self, pane: PaneId) -> &[Entry] {
        match pane {
            PaneId::Remote => self.state.remote.as_ref().map_or(&[], |p| &p.entries[..]),
            PaneId::Local => &self.state.local.entries[..],
        }
    }

    fn spawn_op<F, Fut>(&mut self, pane: PaneId, what: String, op: F)
    where
        F: FnOnce(Arc<dyn RemoteFs>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), FsError>> + Send + 'static,
    {
        let fs: Arc<dyn RemoteFs> = match pane {
            PaneId::Remote => {
                let Some(live) = &self.live else { return };
                live.fs.clone()
            }
            PaneId::Local => match RootedFs::new(&self.state.local.path) {
                Ok(fs) => Arc::new(fs),
                Err(error) => {
                    self.notice(Level::Error, format!("Could not {what}: {error}"));
                    return;
                }
            },
        };
        let messages = self.messages.clone();
        tracing::info!(target: "filecargo::app", operation = %what, ?pane, "file operation");
        tokio::spawn(async move {
            let outcome = op(fs).await;
            let _ = messages.send(Msg::OpDone(OpDone {
                pane,
                what,
                outcome,
            }));
        });
    }

    fn refresh_pane(&mut self, pane: PaneId) {
        match pane {
            PaneId::Remote => self.refresh_remote(),
            PaneId::Local => self.refresh_local(),
        }
    }

    pub(crate) fn on_op_done(&mut self, done: OpDone) {
        if let Err(error) = &done.outcome {
            tracing::warn!(target: "filecargo::app", operation = %done.what, %error, "file operation failed");
            self.notice(Level::Error, format!("Could not {}: {error}", done.what));
            if done.pane == PaneId::Remote && error.is_retryable() {
                self.session_lost(error);
            }
        }
        // after a partial failure the pane may be out of date too
        self.refresh_pane(done.pane);
    }

    pub(crate) fn file_operation(&mut self, command: Command) {
        let pane = match &command {
            Command::Mkdir { pane, .. }
            | Command::Rename { pane, .. }
            | Command::Delete { pane, .. }
            | Command::Chmod { pane, .. } => *pane,
            _ => return,
        };
        let command = match pane {
            PaneId::Remote => match self.ready_for_remote(command) {
                Some(command) => command,
                None => return,
            },
            PaneId::Local => command,
        };
        let Some(dir) = self.pane_dir(pane) else {
            return;
        };
        match command {
            Command::Mkdir { name, .. } => {
                let Some(path) = self.child(&dir, &name) else {
                    return;
                };
                self.spawn_op(
                    pane,
                    format!("create the folder \"{name}\""),
                    move |fs| async move { fs.mkdir(&path).await },
                );
            }
            Command::Rename { from, to, .. } => {
                let (Some(old), Some(new)) = (self.child(&dir, &from), self.child(&dir, &to))
                else {
                    return;
                };
                self.spawn_op(
                    pane,
                    format!("rename \"{from}\" to \"{to}\""),
                    move |fs| async move { fs.rename(&old, &new).await },
                );
            }
            Command::Delete { names, .. } => self.delete(pane, names),
            Command::Chmod { names, mode, .. } => {
                if pane == PaneId::Local && cfg!(windows) {
                    self.notice(
                        Level::Warning,
                        "Permissions cannot be changed on the local pane on Windows.".to_owned(),
                    );
                    return;
                }
                let paths = self.existing(pane, &dir, &names);
                if paths.is_empty() {
                    return;
                }
                let what = format!("change the permissions of {}", plural(paths.len()));
                self.spawn_op(pane, what, move |fs| async move {
                    for (path, _) in paths {
                        fs.chmod(&path, mode).await?;
                    }
                    Ok(())
                });
            }
            _ => {}
        }
    }

    fn child(&mut self, dir: &RemotePath, name: &str) -> Option<RemotePath> {
        let path = valid_name(name).then(|| dir.join(name).ok()).flatten();
        if path.is_none() {
            self.notice(
                Level::Error,
                format!("\"{name}\" is not a valid file name."),
            );
        }
        path
    }

    /// The selected names that exist in the pane's current listing, with their kinds; unknown
    /// names are ignored.
    fn existing(
        &self,
        pane: PaneId,
        dir: &RemotePath,
        names: &[String],
    ) -> Vec<(RemotePath, EntryKind)> {
        let entries = self.pane_entries(pane);
        names
            .iter()
            .filter_map(|name| {
                let entry = entries.iter().find(|e| &e.name == name)?;
                Some((dir.join(name).ok()?, entry.kind.clone()))
            })
            .collect()
    }

    fn delete(&mut self, pane: PaneId, names: Vec<String>) {
        let Some(dir) = self.pane_dir(pane) else {
            return;
        };
        let targets = self.existing(pane, &dir, &names);
        if targets.is_empty() {
            return;
        }
        let names: Vec<String> = targets
            .iter()
            .filter_map(|(path, _)| path.file_name().map(str::to_owned))
            .collect();
        if self.state.settings.ui.confirm_delete {
            let recursive = targets.iter().any(|(_, kind)| *kind == EntryKind::Dir);
            let id = self.enqueue_prompt(PromptKind::ConfirmDelete {
                pane,
                names: names.clone(),
                recursive,
            });
            self.actions
                .insert(id, PromptAction::Delete { pane, names });
        } else {
            self.run_delete(pane, names);
        }
    }

    /// Deletes names of `pane`'s directory, directories recursively.
    pub(crate) fn run_delete(&mut self, pane: PaneId, names: Vec<String>) {
        let Some(dir) = self.pane_dir(pane) else {
            return;
        };
        let targets = self.existing(pane, &dir, &names);
        if targets.is_empty() {
            return;
        }
        let what = format!("delete {}", plural(targets.len()));
        self.spawn_op(pane, what, move |fs| async move {
            for (path, kind) in targets {
                if kind == EntryKind::Dir {
                    fs.remove_all(&path).await?;
                } else {
                    fs.remove_file(&path).await?;
                }
            }
            Ok(())
        });
    }
}
