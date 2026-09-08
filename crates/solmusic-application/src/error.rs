use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("the music provider is temporarily unavailable")]
    Unavailable,
    #[error("YouTube account verification is required: {0}")]
    VerificationRequired(String),
    #[error("the requested content is not playable: {0}")]
    Unplayable(String),
    #[error("the provider response changed unexpectedly: {0}")]
    Incompatible(String),
    #[error("provider request failed: {0}")]
    Network(String),
}

#[derive(Debug, Error)]
#[error("local storage failed: {0}")]
pub struct StorageError(pub String);

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("the queue is empty")]
    EmptyQueue,
    #[error("the queue has no item in that direction")]
    QueueBoundary,
    #[error("invalid queue operation: {0}")]
    InvalidQueueOperation(String),
    #[error("a newer playback operation replaced this request")]
    StalePlaybackOperation,
}
