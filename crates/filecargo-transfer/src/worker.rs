//! What happens to one item once it has a connection: the actual file I/O.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use filecargo_config::ConflictRule;
use filecargo_remote_fs::{Entry, EntryKind, FsError, Progress, RemoteFs, RemotePath, local};
use tokio::io::{AsyncSeekExt, BufReader, BufWriter};

use crate::conflict::{self, Decision};
use crate::{ConflictInfo, Direction, Outcome, QueueItem, TransferError};

/// Where an attempt is, shared between the worker (writer) and the scheduler (sampler).
#[derive(Debug, Default)]
pub(crate) struct ProgressCell {
    /// Where this attempt started writing (non-zero when resuming).
    offset: AtomicU64,
    /// Bytes moved by this attempt.
    written: AtomicU64,
    /// Set when the `Rename` rule picked a new name for the target: the item must follow it,
    /// so a retry resumes the renamed file instead of conflicting again.
    retarget: Mutex<Option<Retarget>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Retarget {
    pub local: PathBuf,
    pub remote: RemotePath,
}

impl ProgressCell {
    pub(crate) fn start_at(&self, offset: u64) {
        self.offset.store(offset, Ordering::Relaxed);
        self.written.store(0, Ordering::Relaxed);
    }

    /// Bytes now on the target, counting from byte 0.
    pub(crate) fn transferred(&self) -> u64 {
        self.offset.load(Ordering::Relaxed) + self.written.load(Ordering::Relaxed)
    }

    pub(crate) fn take_retarget(&self) -> Option<Retarget> {
        self.retarget
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    fn set_retarget(&self, retarget: Retarget) {
        *self.retarget.lock().unwrap_or_else(|e| e.into_inner()) = Some(retarget);
    }
}

impl Progress for ProgressCell {
    fn advance(&self, total_bytes_so_far: u64) {
        self.written.store(total_bytes_so_far, Ordering::Relaxed);
    }
}

pub(crate) struct Job {
    pub item: QueueItem,
    /// What to do if the target exists; `Ask` hands the question to the owner.
    pub rule: ConflictRule,
    pub fs: Arc<dyn RemoteFs>,
    pub progress: Arc<ProgressCell>,
}

/// A child of a directory item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Child {
    pub local: PathBuf,
    pub remote: RemotePath,
    pub is_dir: bool,
    pub size: Option<u64>,
}

// Results are moved once per item; the size of `NeedsDecision` does not matter.
#[allow(clippy::large_enum_variant)]
pub(crate) enum JobResult {
    Completed(Outcome),
    /// The directory was created; its children go right after it in the queue.
    Expanded(Vec<Child>),
    /// The target exists and the rule is `Ask`.
    NeedsDecision(ConflictInfo),
    Failed(TransferError),
}

fn local_io(path: &Path, error: &std::io::Error) -> TransferError {
    TransferError::LocalIo(format!("{}: {error}", path.display()))
}

fn from_fs(error: FsError) -> TransferError {
    match error {
        FsError::LocalIo(message) => TransferError::LocalIo(message),
        other => TransferError::Fs(other),
    }
}

pub(crate) async fn run(job: Job) -> JobResult {
    let result = if job.item.is_dir {
        run_dir(&job).await
    } else {
        run_file(&job).await
    };
    result.unwrap_or_else(JobResult::Failed)
}

/// The two ends of a file transfer: where it reads from and where it writes to.
struct Ends {
    source: Entry,
    target: Option<Entry>,
}

async fn stat_ends(
    job: &Job,
    target_local: &Path,
    target_remote: &RemotePath,
) -> Result<Ends, TransferError> {
    let item = &job.item;
    match item.direction {
        Direction::Upload => {
            let source = local::stat(&item.local)
                .await
                .map_err(from_fs)?
                .ok_or_else(|| {
                    TransferError::LocalIo(format!("{}: no such file", item.local.display()))
                })?;
            let target = job.fs.stat(target_remote).await.map_err(from_fs)?;
            Ok(Ends { source, target })
        }
        Direction::Download => {
            let source = job
                .fs
                .stat(&item.remote)
                .await
                .map_err(from_fs)?
                .ok_or_else(|| TransferError::Fs(FsError::NotFound(item.remote.to_string())))?;
            let target = local::stat(target_local).await.map_err(from_fs)?;
            Ok(Ends { source, target })
        }
    }
}

/// The first `name (n).ext` that does not exist at the target.
async fn free_name(job: &Job) -> Result<String, TransferError> {
    let item = &job.item;
    let name = match item.direction {
        Direction::Upload => item.remote.file_name().map(str::to_owned),
        Direction::Download => item
            .local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
    }
    .ok_or_else(|| TransferError::LocalIo("the target has no file name".to_owned()))?;
    for n in 1..10_000 {
        let candidate = conflict::numbered_name(&name, n);
        let taken = match item.direction {
            Direction::Upload => {
                let parent = item.remote.parent().unwrap_or_else(RemotePath::root);
                let path = parent
                    .join(&candidate)
                    .map_err(|e| TransferError::LocalIo(e.to_string()))?;
                job.fs.stat(&path).await.map_err(from_fs)?.is_some()
            }
            Direction::Download => local::stat(&item.local.with_file_name(&candidate))
                .await
                .map_err(from_fs)?
                .is_some(),
        };
        if !taken {
            return Ok(candidate);
        }
    }
    Err(TransferError::LocalIo(format!("no free name for {name}")))
}

async fn run_file(job: &Job) -> Result<JobResult, TransferError> {
    let item = &job.item;
    let (mut target_local, mut target_remote) = (item.local.clone(), item.remote.clone());
    let ends = stat_ends(job, &target_local, &target_remote).await?;

    let mut outcome = Outcome::Transferred;
    let offset = match &ends.target {
        None => 0,
        // We wrote part of this file already: it is ours, not a conflict.
        Some(target) if item.transferred > 0 => {
            if target.size <= ends.source.size {
                outcome = Outcome::Resumed;
                target.size
            } else {
                tracing::info!(target: "filecargo::transfer", id = %item.id, "the partial target grew past the source; starting over");
                0
            }
        }
        Some(target) => match conflict::decide(job.rule, &ends.source, target) {
            Decision::Write { offset } => {
                if offset > 0 {
                    outcome = Outcome::Resumed;
                }
                offset
            }
            Decision::Skip => return Ok(JobResult::Completed(Outcome::Skipped)),
            Decision::Ask => {
                return Ok(JobResult::NeedsDecision(ConflictInfo {
                    source: ends.source,
                    target: target.clone(),
                }));
            }
            Decision::Rename => {
                let name = free_name(job).await?;
                match item.direction {
                    Direction::Upload => {
                        let parent = item.remote.parent().unwrap_or_else(RemotePath::root);
                        target_remote = parent
                            .join(&name)
                            .map_err(|e| TransferError::LocalIo(e.to_string()))?;
                    }
                    Direction::Download => target_local = item.local.with_file_name(&name),
                }
                job.progress.set_retarget(Retarget {
                    local: target_local.clone(),
                    remote: target_remote.clone(),
                });
                outcome = Outcome::Renamed(name);
                0
            }
        },
    };

    job.progress.start_at(offset);
    match item.direction {
        Direction::Upload => upload(job, &target_remote, offset).await?,
        Direction::Download => download(job, &target_local, offset).await?,
    }
    Ok(JobResult::Completed(outcome))
}

async fn upload(job: &Job, target: &RemotePath, offset: u64) -> Result<(), TransferError> {
    let path = &job.item.local;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| local_io(path, &e))?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| local_io(path, &e))?;
    }
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    job.fs
        .upload(target, offset, &mut reader, job.progress.as_ref())
        .await
        .map(|_| ())
        .map_err(from_fs)
}

async fn download(job: &Job, target: &Path, offset: u64) -> Result<(), TransferError> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| local_io(parent, &e))?;
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true);
    if offset == 0 {
        options.truncate(true);
    }
    let mut file = options
        .open(target)
        .await
        .map_err(|e| local_io(target, &e))?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| local_io(target, &e))?;
    }
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    job.fs
        .download(&job.item.remote, offset, &mut writer, job.progress.as_ref())
        .await
        .map(|_| ())
        .map_err(from_fs)
}

/// A name from a listing that is safe to join onto a local directory: exactly one normal path
/// component, so a hostile server cannot make us write outside the target (`..`, `/etc/x`,
/// `C:\x`, ...).
fn safe_name(name: &str) -> bool {
    use std::path::{Component, Path};
    if name.is_empty() || name.contains(['/', '\\', '\0']) {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// Turns a listing into children, skipping what v1 does not transfer: symlinks, special
/// files, and names that are not safe to use.
fn children(item: &QueueItem, entries: Vec<Entry>) -> Vec<Child> {
    let mut children = Vec::with_capacity(entries.len());
    for entry in entries {
        let is_dir = match entry.kind {
            EntryKind::File => false,
            EntryKind::Dir => true,
            EntryKind::Symlink { .. } => {
                tracing::info!(target: "filecargo::transfer", name = %entry.name, "skipping a symlink");
                continue;
            }
            EntryKind::Other => {
                tracing::info!(target: "filecargo::transfer", name = %entry.name, "skipping a special file");
                continue;
            }
        };
        if !safe_name(&entry.name) {
            tracing::warn!(target: "filecargo::transfer", name = %entry.name, "skipping an unsafe file name");
            continue;
        }
        let Ok(remote) = item.remote.join(&entry.name) else {
            tracing::warn!(target: "filecargo::transfer", name = %entry.name, "skipping a name the server path cannot hold");
            continue;
        };
        children.push(Child {
            local: item.local.join(&entry.name),
            remote,
            is_dir,
            size: (!is_dir).then_some(entry.size),
        });
    }
    children
}

/// Creates the target directory (an existing one is fine: directories merge) and lists the
/// source.
async fn run_dir(job: &Job) -> Result<JobResult, TransferError> {
    let item = &job.item;
    let entries = match item.direction {
        Direction::Upload => {
            match job.fs.mkdir(&item.remote).await {
                Ok(()) => {}
                Err(FsError::AlreadyExists(_)) => {
                    let existing = job.fs.stat(&item.remote).await.map_err(from_fs)?;
                    if !existing.is_some_and(|e| e.is_dir()) {
                        return Err(TransferError::Fs(FsError::NotADirectory(
                            item.remote.to_string(),
                        )));
                    }
                }
                Err(error) => return Err(from_fs(error)),
            }
            local::read_dir(&item.local).await.map_err(from_fs)?
        }
        Direction::Download => {
            tokio::fs::create_dir_all(&item.local)
                .await
                .map_err(|e| local_io(&item.local, &e))?;
            job.fs.list(&item.remote).await.map_err(from_fs)?
        }
    };
    Ok(JobResult::Expanded(children(item, entries)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_plain_components_are_safe_names() {
        for good in ["file.txt", "a b", "é", ".hidden", "name.with.dots", "..."] {
            assert!(safe_name(good), "{good:?} should be safe");
        }
        for bad in ["", ".", "..", "a/b", "../x", "/etc/passwd", "a\\b", "x\0y"] {
            assert!(!safe_name(bad), "{bad:?} should be refused");
        }
    }
}
