//! The closed set of errors every surface maps to its own conventions.

use crate::store::StoreError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    NotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// The stable code surfaces put in error bodies, such as `not_found`.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::Internal(_) => "internal",
        }
    }
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Error::Internal(error.to_string())
    }
}
