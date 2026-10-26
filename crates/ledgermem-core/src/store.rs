//! The store port, in the engine's terms. No SQL appears above it (ADR-0001).

use time::OffsetDateTime;

use crate::claim::{Claim, Scope};
use crate::journal::{JournalEntry, NewEntry};

/// Durable storage for journals and their projections.
pub trait Store: Send + Sync {
    fn tenant_exists(&self, tenant: &str) -> Result<bool, StoreError>;

    fn create_tenant(&self, tenant: &str, created_at: OffsetDateTime) -> Result<(), StoreError>;

    /// Appends the entries to their scopes' journals, numbering each from the scope's last
    /// sequence, then applies the projection updates. Everything commits together or not at all.
    fn commit(&self, tenant: &str, commit: Commit) -> Result<(), StoreError>;

    /// Full-text search over claim text and subject within the given scopes, best match first.
    /// Equally good matches come in claim id order, so results don't depend on storage order.
    fn search(
        &self,
        tenant: &str,
        scopes: &[Scope],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, StoreError>;

    /// Every entry in one scope's journal, in sequence order.
    fn journal(&self, tenant: &str, scope: &Scope) -> Result<Vec<JournalEntry>, StoreError>;

    /// Every scope in the tenant that has a journal, in scope order.
    fn scopes(&self, tenant: &str) -> Result<Vec<Scope>, StoreError>;

    /// Drops every projection in the tenant, then applies `projections` in order. Everything
    /// commits together or not at all. Journals are untouched.
    fn replace_projections(
        &self,
        tenant: &str,
        projections: Vec<Projection>,
    ) -> Result<(), StoreError>;
}

/// One write: the journal entries and the projection updates built from them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Commit {
    pub entries: Vec<NewEntry>,
    pub projections: Vec<Projection>,
}

/// A change to the tables reads are served from.
#[derive(Debug, Clone, PartialEq)]
pub enum Projection {
    PutClaim(Claim),
}

#[derive(Debug, thiserror::Error)]
#[error("store: {0}")]
pub struct StoreError(Box<dyn std::error::Error + Send + Sync>);

impl StoreError {
    pub fn new(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        StoreError(error.into())
    }
}
