use filecargo_remote_fs::FsError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    #[error("cannot connect: {0}")]
    Connect(String),
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error("local file error: {0}")]
    LocalIo(String),
    #[error("site was deleted")]
    SiteDeleted,
    #[error("cannot save the queue: {0}")]
    Persist(String),
}

impl TransferError {
    /// Whether retrying on a fresh connection can help (connect problems and the retryable
    /// kinds of [`FsError`]).
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Connect(_) => true,
            Self::Fs(e) => e.is_retryable(),
            Self::LocalIo(_) | Self::SiteDeleted | Self::Persist(_) => false,
        }
    }
}
