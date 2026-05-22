pub type AppResult<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AppError {
    #[error("core protocol error: {0}")]
    Core(#[from] hivemind_core::Error),

    #[error("identity error: {0}")]
    Identity(String),

    #[error("clock error: {0}")]
    Clock(String),

    #[error("object store error: {0}")]
    ObjectStore(String),

    #[error("chunk store error: {0}")]
    ChunkStore(String),
}
