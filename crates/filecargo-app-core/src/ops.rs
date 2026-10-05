//! Remote file operations on the names of the remote pane's current directory.

use filecargo_remote_fs::{EntryKind, FsError, RemoteFs, RemotePath};

use crate::app::{Core, Msg};
use crate::command::Command;
use crate::prompt::PromptAction;
use crate::state::{Level, PaneId, PromptKind};

/// What a finished operation reports back.
pub(crate) struct OpDone {
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
    fn remote_dir(&self) -> Option<RemotePath> {
        self.state.remote.as_ref().map(|p| p.path.clone())
    }

    fn spawn_op<F, Fut>(&mut self, what: String, op: F)
    where
        F: FnOnce(std::sync::Arc<dyn RemoteFs>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), FsError>> + Send + 'static,
    {
        let Some(live) = &self.live else { return };
        let fs = live.fs.clone();
        let messages = self.messages.clone();
        tracing::info!(target: "filecargo::app", operation = %what, "remote operation");
        tokio::spawn(async move {
            let outcome = op(fs).await;
            let _ = messages.send(Msg::OpDone(OpDone { what, outcome }));
        });
    }

    pub(crate) fn on_op_done(&mut self, done: OpDone) {
        match done.outcome {
            Ok(()) => self.refresh_remote(),
            Err(error) => {
                tracing::warn!(target: "filecargo::app", operation = %done.what, %error, "remote operation failed");
                self.notice(Level::Error, format!("Could not {}: {error}", done.what));
                if error.is_retryable() {
                    self.session_lost(&error);
                }
                // the pane may be out of date after a partial failure
                self.refresh_remote();
            }
        }
    }

    pub(crate) fn remote_operation(&mut self, command: Command) {
        let Some(command) = self.ready_for_remote(command) else {
            return;
        };
        let Some(dir) = self.remote_dir() else { return };
        match command {
            Command::Mkdir { name } => {
                let Some(path) = self.child(&dir, &name) else {
                    return;
                };
                self.spawn_op(
                    format!("create the folder \"{name}\""),
                    move |fs| async move { fs.mkdir(&path).await },
                );
            }
            Command::Rename { from, to } => {
                let (Some(old), Some(new)) = (self.child(&dir, &from), self.child(&dir, &to))
                else {
                    return;
                };
                self.spawn_op(
                    format!("rename \"{from}\" to \"{to}\""),
                    move |fs| async move { fs.rename(&old, &new).await },
                );
            }
            Command::Delete { names } => self.delete(names),
            Command::Chmod { names, mode } => {
                let paths = self.existing(&dir, &names);
                if paths.is_empty() {
                    return;
                }
                let what = format!("change the permissions of {}", plural(paths.len()));
                self.spawn_op(what, move |fs| async move {
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
    fn existing(&self, dir: &RemotePath, names: &[String]) -> Vec<(RemotePath, EntryKind)> {
        let Some(pane) = &self.state.remote else {
            return Vec::new();
        };
        names
            .iter()
            .filter_map(|name| {
                let entry = pane.entries.iter().find(|e| &e.name == name)?;
                Some((dir.join(name).ok()?, entry.kind.clone()))
            })
            .collect()
    }

    fn delete(&mut self, names: Vec<String>) {
        let Some(dir) = self.remote_dir() else { return };
        let targets = self.existing(&dir, &names);
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
                pane: PaneId::Remote,
                names: names.clone(),
                recursive,
            });
            self.actions.insert(id, PromptAction::Delete { names });
        } else {
            self.run_delete(names);
        }
    }

    /// Deletes names of the remote pane's directory, directories recursively.
    pub(crate) fn run_delete(&mut self, names: Vec<String>) {
        let Some(dir) = self.remote_dir() else { return };
        let targets = self.existing(&dir, &names);
        if targets.is_empty() {
            return;
        }
        let what = format!("delete {}", plural(targets.len()));
        self.spawn_op(what, move |fs| async move {
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
