use comsat_types::ValidationError;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("invalid persistence value: {0}")]
    Validation(String),
    #[error("persistence conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("persistence backend failed: {0}")]
    Backend(String),
}

impl From<ValidationError> for StoreError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}
