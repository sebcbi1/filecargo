//! What happens to one item once it has a connection: the actual file I/O.

use std::io::SeekFrom;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use filecargo_remote_fs::{Entry, EntryKind, FsError, Progress, RemoteFs, RemotePath, local};
use tokio::io::{AsyncSeekExt, BufReader, BufWriter};

use crate::{Direction, Outcome, QueueItem, TransferError};

/// Where an attempt is, shared between the worker (writer) and the scheduler (sampler).
#[derive(Debug, Default)]
pub(crate) struct ProgressCell {
    /// Where this attempt started writing (non-zero when resuming).
    offset: AtomicU64,
    /// Bytes moved by this attempt.
    written: AtomicU64,
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
}

impl Progress for ProgressCell {
    fn advance(&self, total_bytes_so_far: u64) {
        self.written.store(total_bytes_so_far, Ordering::Relaxed);
    }
}

pub(crate) struct Job {
    pub item: QueueItem,
    pub fs: Arc<dyn RemoteFs>,
    pub progress: Arc<ProgressCell>,
}

/// A child of a directory item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Child {
    pub local: std::path::PathBuf,
    pub remote: RemotePath,
    pub is_dir: bool,
    pub size: Option<u64>,
}

pub(crate) enum JobResult {
    Completed(Outcome),
    /// The directory was created; its children go right after it in the queue.
    Expanded(Vec<Child>),
    Failed(TransferError),
}

fn local_io(path: &std::path::Path, error: &std::io::Error) -> TransferError {
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

async fn run_file(job: &Job) -> Result<JobResult, TransferError> {
    let item = &job.item;
    job.progress.start_at(0);
    match item.direction {
        Direction::Upload => upload(job, 0).await,
        Direction::Download => download(job, 0).await,
    }
    .map(|()| JobResult::Completed(Outcome::Transferred))
}

async fn upload(job: &Job, offset: u64) -> Result<(), TransferError> {
    let item = &job.item;
    let mut file = tokio::fs::File::open(&item.local)
        .await
        .map_err(|e| local_io(&item.local, &e))?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| local_io(&item.local, &e))?;
    }
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    job.fs
        .upload(&item.remote, offset, &mut reader, job.progress.as_ref())
        .await
        .map(|_| ())
        .map_err(from_fs)
}

async fn download(job: &Job, offset: u64) -> Result<(), TransferError> {
    let item = &job.item;
    if let Some(parent) = item.local.parent() {
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
        .open(&item.local)
        .await
        .map_err(|e| local_io(&item.local, &e))?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| local_io(&item.local, &e))?;
    }
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    job.fs
        .download(&item.remote, offset, &mut writer, job.progress.as_ref())
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
