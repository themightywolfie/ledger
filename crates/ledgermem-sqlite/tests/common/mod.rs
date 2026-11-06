//! What the store integration tests share.

use std::sync::Mutex;

use ledgermem_core::{
    Claim, ClaimId, Commit, Job, JournalHead, PolicyVersion, Projection, Scope, Store, StoreError,
    StoredEntry, StoredJournal,
};
use ledgermem_sqlite::SqliteStore;
use time::OffsetDateTime;

/// A store that lets one more write land just before the next write that depends on what was
/// read, a reindex's rebuild or a guarded commit, the way a `ledgermem mcp` process sharing the
/// database could.
pub struct LateWriteStore {
    pub inner: SqliteStore,
    pub late_write: Mutex<Option<Commit>>,
}

impl LateWriteStore {
    pub fn new(inner: SqliteStore) -> Self {
        LateWriteStore {
            inner,
            late_write: Mutex::new(None),
        }
    }

    fn land_late_write(&self, tenant: &str) -> Result<(), StoreError> {
        match self.late_write.lock().unwrap().take() {
            Some(late_write) => self.inner.commit(tenant, late_write),
            None => Ok(()),
        }
    }
}

impl Store for LateWriteStore {
    fn tenant_exists(&self, tenant: &str) -> Result<bool, StoreError> {
        self.inner.tenant_exists(tenant)
    }
    fn create_tenant(&self, tenant: &str, at: OffsetDateTime) -> Result<(), StoreError> {
        self.inner.create_tenant(tenant, at)
    }
    fn commit(&self, tenant: &str, commit: Commit) -> Result<(), StoreError> {
        self.inner.commit(tenant, commit)
    }
    fn commit_at(
        &self,
        tenant: &str,
        heads: &[JournalHead],
        commit: Commit,
    ) -> Result<bool, StoreError> {
        self.land_late_write(tenant)?;
        self.inner.commit_at(tenant, heads, commit)
    }
    fn claim(
        &self,
        tenant: &str,
        id: &ClaimId,
    ) -> Result<Option<(Claim, JournalHead)>, StoreError> {
        self.inner.claim(tenant, id)
    }
    fn lease_job(
        &self,
        now: OffsetDateTime,
        until: OffsetDateTime,
    ) -> Result<Option<Job>, StoreError> {
        self.inner.lease_job(now, until)
    }
    fn lease_job_for(
        &self,
        tenant: &str,
        claim_id: &ClaimId,
        now: OffsetDateTime,
        until: OffsetDateTime,
    ) -> Result<Option<Job>, StoreError> {
        self.inner.lease_job_for(tenant, claim_id, now, until)
    }
    fn job_waits(&self, tenant: &str, claim_id: &ClaimId) -> Result<bool, StoreError> {
        self.inner.job_waits(tenant, claim_id)
    }
    fn search(
        &self,
        tenant: &str,
        scopes: &[Scope],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, StoreError> {
        self.inner.search(tenant, scopes, query, limit)
    }
    fn policies(&self, tenant: &str, scopes: &[Scope]) -> Result<Vec<PolicyVersion>, StoreError> {
        self.inner.policies(tenant, scopes)
    }
    fn journal(&self, tenant: &str, scope: &str) -> Result<Vec<StoredEntry>, StoreError> {
        self.inner.journal(tenant, scope)
    }
    fn journals(&self, tenant: &str) -> Result<Vec<StoredJournal>, StoreError> {
        self.inner.journals(tenant)
    }
    fn replace_projections(
        &self,
        tenant: &str,
        heads: &[JournalHead],
        projections: Vec<Projection>,
    ) -> Result<bool, StoreError> {
        self.land_late_write(tenant)?;
        self.inner.replace_projections(tenant, heads, projections)
    }
}
