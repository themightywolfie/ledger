//! The store port, in the engine's terms. No SQL appears above it (ADR-0001).
//!
//! Besides journals and their projections, a store keeps the job queue: operational state, not
//! memory, so it is never journaled and a reindex leaves it alone.

use time::OffsetDateTime;

use crate::claim::{Claim, ClaimId, Scope};
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

    /// Commits like [`Store::commit`], but only if each journal in `heads` still ends where it
    /// says. Returns false, changing nothing, if a write landed in one since.
    fn commit_at(
        &self,
        tenant: &str,
        heads: &[JournalHead],
        commit: Commit,
    ) -> Result<bool, StoreError>;

    /// The claim `id` names as it stands now, and where its scope's journal ends, read as one
    /// snapshot. None when the tenant has no such claim.
    fn claim(&self, tenant: &str, id: &ClaimId)
    -> Result<Option<(Claim, JournalHead)>, StoreError>;

    /// Leases the queued job that has waited longest, in any tenant, among those no runner holds:
    /// never leased, or with a lease that ran out by `now`. The lease lasts until `until`, so a
    /// runner that stops before it finishes the job leaves it for another. None when no job is
    /// waiting.
    fn lease_job(
        &self,
        now: OffsetDateTime,
        until: OffsetDateTime,
    ) -> Result<Option<Job>, StoreError>;

    /// Leases the job queued for `claim_id`, as [`Store::lease_job`] would, so whoever is waiting
    /// on that claim's check can run it now. None when no job is queued for it, or a runner holds
    /// the one that is.
    fn lease_job_for(
        &self,
        tenant: &str,
        claim_id: &ClaimId,
        now: OffsetDateTime,
        until: OffsetDateTime,
    ) -> Result<Option<Job>, StoreError>;

    /// Whether a job is still queued for `claim_id`, whether or not a runner holds it.
    fn job_waits(&self, tenant: &str, claim_id: &ClaimId) -> Result<bool, StoreError>;

    /// Full-text search over claim text and subject within the given scopes, best match first.
    /// Equally good matches come in claim id order, so results don't depend on storage order.
    /// Quarantined and retracted claims are never served as memory, so search never returns one.
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

/// One write: the journal entries, the projection updates built from them, and the jobs it
/// queues or finishes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Commit {
    pub entries: Vec<NewEntry>,
    pub projections: Vec<Projection>,
    /// Jobs the write calls for. A claim already waiting on a job isn't queued again.
    pub queued: Vec<NewJob>,
    /// The ids of the jobs the write finishes, which leave the queue.
    pub finished: Vec<String>,
}

/// A job to queue: checking one claim again against its predicate's verifier, in the background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    /// `job_` followed by a ULID.
    pub id: String,
    pub claim_id: ClaimId,
    pub queued_at: OffsetDateTime,
}

/// A queued job, as a runner leases it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub tenant: String,
    pub claim_id: ClaimId,
}

impl NewJob {
    /// A job to check `claim_id` again, queued at `at`.
    pub(crate) fn check(claim_id: ClaimId, at: OffsetDateTime) -> Self {
        NewJob {
            id: format!("job_{}", ulid::Ulid::from_datetime(at.into())),
            claim_id,
            queued_at: at,
        }
    }
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
