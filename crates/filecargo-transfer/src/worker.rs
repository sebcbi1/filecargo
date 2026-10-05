//! What happens to one item once it has a connection: the actual file I/O.

use std::io::SeekFrom;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use filecargo_remote_fs::{FsError, Progress, RemoteFs};
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

pub(crate) enum JobResult {
    Completed(Outcome),
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
        Err(TransferError::LocalIo(
            "directories are not supported yet".to_owned(),
        ))
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
