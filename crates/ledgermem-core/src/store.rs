//! The store port, in the engine's terms. No SQL appears above it (ADR-0001).

use time::OffsetDateTime;

use crate::claim::{Claim, Scope};
use crate::journal::{NewEntry, StoredEntry};
use crate::policy::PolicyVersion;

/// Durable storage for journals and their projections.
pub trait Store: Send + Sync {
    fn tenant_exists(&self, tenant: &str) -> Result<bool, StoreError>;

    fn create_tenant(&self, tenant: &str, created_at: OffsetDateTime) -> Result<(), StoreError>;

    /// Appends the entries to their scopes' journals, sealing each with [`NewEntry::seal`] after
    /// the scope's last entry and storing the result verbatim, then applies the projection
    /// updates. Everything commits together or not at all.
    fn commit(&self, tenant: &str, commit: Commit) -> Result<(), StoreError>;

    /// Full-text search over claim text and subject within the given scopes, best match first.
    /// Equally good matches come in claim id order, so results don't depend on storage order.
    /// Quarantined claims are never served as memory, so search never returns one.
    fn search(
        &self,
        tenant: &str,
        scopes: &[Scope],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, StoreError>;

    /// The policy version in effect in each of `scopes` that has one, the last applied there,
    /// in the order of `scopes`. Read as one snapshot, so an apply never lands between them.
    fn policies(&self, tenant: &str, scopes: &[Scope]) -> Result<Vec<PolicyVersion>, StoreError>;

    /// Every entry in one scope's journal, exactly as stored, in sequence order. `scope` is
    /// written `level:id` or `org`.
    fn journal(&self, tenant: &str, scope: &str) -> Result<Vec<StoredEntry>, StoreError>;

    /// Every journal in the tenant, entries exactly as stored, read as one snapshot so that no
    /// write lands between one journal and the next. Journals come in scope order.
    fn journals(&self, tenant: &str) -> Result<Vec<StoredJournal>, StoreError>;

    /// Drops every projection in the tenant and applies `projections` in order, but only if the
    /// tenant's journals still end where `heads` says they did when they were read. Returns false,
    /// changing nothing, if a write landed since. Everything commits together or not at all, and
    /// journals are untouched.
    fn replace_projections(
        &self,
        tenant: &str,
        heads: &[JournalHead],
        projections: Vec<Projection>,
    ) -> Result<bool, StoreError>;
}

/// One scope's journal, entries exactly as stored, in sequence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJournal {
    /// The scope as stored, written `level:id` or `org`.
    pub scope: String,
    pub entries: Vec<StoredEntry>,
}

/// Where one scope's journal ended when it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalHead {
    /// The scope as stored, written `level:id` or `org`.
    pub scope: String,
    /// The last entry's sequence number.
    pub seq: u64,
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
    /// Makes a policy version the one in effect in its scope.
    PutPolicy(PolicyVersion),
}

#[derive(Debug, thiserror::Error)]
#[error("store: {0}")]
pub struct StoreError(Box<dyn std::error::Error + Send + Sync>);

impl StoreError {
    pub fn new(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        StoreError(error.into())
    }
}
