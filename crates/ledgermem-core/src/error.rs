//! The closed set of errors every surface maps to its own conventions.

use crate::store::StoreError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request names a scope outside the caller's bounds. Never filtered silently.
    #[error("{0}")]
    ScopeDenied(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    NotFound(String),
    /// A claim breaks its predicate's declared rules, such as a value outside a declared enum.
    #[error("{0}")]
    InvalidClaim(String),
    /// A policy document doesn't parse or validate. Nothing in it was applied.
    #[error("{0}")]
    InvalidPolicy(String),
    /// A journal doesn't verify, so nothing may be rebuilt or restored from it.
    #[error("{0}")]
    ChainBroken(String),
    /// Other writes kept the operation from finishing. Retrying later can succeed.
    #[error("{0}")]
    Busy(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// The stable code surfaces put in error bodies, such as `not_found`.
    pub fn code(&self) -> &'static str {
        match self {
            Error::ScopeDenied(_) => "scope_denied",
            Error::PermissionDenied(_) => "permission_denied",
            Error::NotFound(_) => "not_found",
            Error::InvalidClaim(_) => "invalid_claim",
            Error::InvalidPolicy(_) => "invalid_policy",
            Error::ChainBroken(_) => "chain_broken",
            Error::Busy(_) => "busy",
            Error::Internal(_) => "internal",
        }
    }
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Error::Internal(error.to_string())
    }
}
