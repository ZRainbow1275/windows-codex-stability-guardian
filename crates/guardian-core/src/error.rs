use thiserror::Error;

pub type GuardianResult<T> = Result<T, GuardianError>;

#[derive(Debug, Error)]
pub enum GuardianError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("command `{command}` failed with status {status}: {stderr}")]
    CommandFailed {
        command: String,
        status: i32,
        stderr: String,
    },
}

impl GuardianError {
    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::InvalidState(message.into())
    }
}
