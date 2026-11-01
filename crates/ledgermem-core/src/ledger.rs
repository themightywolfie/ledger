//! The Ledger engine: the one value every surface calls.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;
use time::Duration;

use crate::caller::{Caller, Permission};
use crate::claim::{Claim, ClaimId, Scope, ScopeLevel, Source, SourceKind, Status};
use crate::clock::Clock;
use crate::error::{Error, Result};
use crate::journal::{self, EntryKind, JournalEntry, NewEntry, StoredEntry};
use crate::ops::{
    ApplyPolicy, BrokenLink, ChainReport, ClaimResult, JournalQuery, PolicyApplied, PolicyQuery,
    ReadIntent, ReadPack, Recall, RecalledClaim, ReindexReport, Remember, RememberOutcome,
    ScopeVerification,
};
use crate::policy::{Policy, PolicyVersion, invalid_policy};
use crate::store::{Commit, JournalHead, Projection, Store};

/// The tenant a local install serves, created on first launch.
pub const LOCAL_TENANT: &str = "local";

const DEFAULT_RECALL_LIMIT: usize = 10;

/// How many times reindex reads the journals again after a write lands mid-rebuild.
const REINDEX_ATTEMPTS: u32 = 3;

pub struct Ledger {
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
}

impl Ledger {
    pub fn new(store: Arc<dyn Store>, clock: Arc<dyn Clock>) -> Self {
        Ledger { store, clock }
    }

    /// Makes a tenant ready to serve. The `local` tenant is created on first launch; any other
    /// tenant must already exist.
    pub async fn open_tenant(&self, tenant: &str) -> Result<()> {
        if self.store.tenant_exists(tenant)? {
            return Ok(());
        }
        if tenant != LOCAL_TENANT {
            return Err(Error::NotFound(format!(
                "tenant {tenant:?} does not exist; create it with `ledgermem tenant create {tenant}`"
            )));
        }
        self.store.create_tenant(tenant, self.clock.now())?;
        Ok(())
    }

    /// Proposes claims. Each one is appended to its scope's journal before any read can see it.
    /// Subjects are resolved through the policy's aliases and values checked against their
    /// predicates' declared types. If any claim fails a check, none is remembered.
    pub async fn remember(&self, caller: &Caller, req: Remember) -> Result<RememberOutcome> {
        caller.require(Permission::Propose)?;
        let now = self.clock.now();
        let scope = caller.default_write_scope();
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;
        let source = Source {
            kind: SourceKind::for_channel(caller.channel),
            actor: caller.actor.clone(),
            channel: caller.channel,
        };

        let mut commit = Commit::default();
        let mut results = Vec::with_capacity(req.claims.len());
        for (index, input) in req.claims.into_iter().enumerate() {
            let admitted = policy
                .admit(&input.subject, &input.predicate, input.value)
                .map_err(|(field, reason)| {
                    Error::InvalidClaim(format!(
                        "claims[{index}].{field}: {reason}; nothing was remembered"
                    ))
                })?;
            let claim = Claim {
                id: ClaimId::generate(now),
                scope: scope.clone(),
                subject: admitted.subject,
                predicate: input.predicate,
                value: admitted.value,
                text: input.text,
                source: source.clone(),
                status: Status::Proposed,
                valid_from: now,
                valid_to: None,
                recorded_at: now,
            };
            results.push(ClaimResult {
                claim_id: claim.id.clone(),
                status: claim.status,
                trust: claim.source.kind.prior(),
                hints: admitted.hints,
            });
            let entry = NewEntry {
                id: NewEntry::generate_id(now),
                scope: scope.clone(),
                kind: EntryKind::ClaimWritten,
                actor: caller.actor.clone(),
                channel: caller.channel,
                recorded_at: now,
                content: serde_json::to_value(&claim).map_err(internal)?,
            };
            commit
                .projections
                .extend(projections_for(entry.kind, &entry.content)?);
            commit.entries.push(entry);
        }

        self.store.commit(&caller.tenant, commit)?;
        Ok(RememberOutcome { results })
    }

    /// Recalls the claims that hold now and match the query, best match first.
    pub async fn recall(&self, caller: &Caller, req: Recall) -> Result<ReadPack> {
        caller.require(Permission::Read)?;
        let now = self.clock.now();
        let limit = req.limit.unwrap_or(DEFAULT_RECALL_LIMIT);
        let claims =
            self.store
                .search(&caller.tenant, &caller.visible_scopes(), &req.query, limit)?;

        let answer_set = claims
            .into_iter()
            .map(|claim| RecalledClaim {
                trust: claim.source.kind.prior(),
                age: format_age(now - claim.recorded_at),
                claim_id: claim.id,
                scope: claim.scope,
                subject: claim.subject,
                predicate: claim.predicate,
                value: claim.value,
                text: claim.text,
                status: claim.status,
                recorded_at: claim.recorded_at,
            })
            .collect();
        Ok(ReadPack {
            intent: ReadIntent::Current,
            answer_set,
        })
    }

    /// Every entry in one scope's journal, in sequence order. The scope must be within the
    /// caller's bounds. This is the journal view of the review operations' browse.
    pub async fn journal(&self, caller: &Caller, query: JournalQuery) -> Result<Vec<JournalEntry>> {
        caller.require(Permission::Read)?;
        caller.require_visible(&query.scope)?;
        self.store
            .journal(&caller.tenant, &query.scope.to_string())?
            .into_iter()
            .map(readable)
            .collect()
    }

    /// Applies a policy document to the tenant, or to one project as an overlay on the tenant's.
    /// The new version is journaled in the scope it applies to, and takes effect at once.
    pub async fn apply_policy(&self, caller: &Caller, req: ApplyPolicy) -> Result<PolicyApplied> {
        caller.require(Permission::Operator)?;
        let now = self.clock.now();
        let scope = policy_scope(req.project.as_deref())
            .map_err(|reason| invalid_policy("project", reason))?;
        let version = PolicyVersion::new(scope.clone(), &req.document, now)?;
        let declared = Policy::layered(std::slice::from_ref(&version));
        let entry = NewEntry {
            id: NewEntry::generate_id(now),
            scope: scope.clone(),
            kind: EntryKind::PolicyApplied,
            actor: caller.actor.clone(),
            channel: caller.channel,
            recorded_at: now,
            content: serde_json::to_value(&version).map_err(internal)?,
        };
        let commit = Commit {
            projections: projections_for(entry.kind, &entry.content)?,
            entries: vec![entry],
        };
        self.store.commit(&caller.tenant, commit)?;
        Ok(PolicyApplied {
            policy_version: version.id,
            scope,
            predicates: declared.predicates.len() as u64,
            aliases: declared.alias_count() as u64,
        })
    }

    /// The policy in effect for the tenant, or for one project with its overlay. Operators may
    /// read any project's; other callers only one within their bounds.
    pub async fn policy(&self, caller: &Caller, query: PolicyQuery) -> Result<Policy> {
        caller.require(Permission::Read)?;
        let scope = policy_scope(query.project.as_deref()).map_err(Error::NotFound)?;
        if !caller.permissions.contains(&Permission::Operator) {
            caller.require_visible(&scope)?;
        }
        self.policy_in(&caller.tenant, query.project.as_deref())
    }

    /// Re-derives every journal in the tenant and reports the first broken link in each.
    pub async fn verify_journals(&self, caller: &Caller) -> Result<ChainReport> {
        caller.require(Permission::Operator)?;
        let scopes: Vec<_> = self
            .read_journals(&caller.tenant)?
            .into_iter()
            .map(|journal| ScopeVerification {
                entries: journal.entries.len() as u64,
                head_hash: journal.entries.last().map(|entry| entry.entry_hash.clone()),
                broken_at: journal.broken_at,
                scope: journal.scope,
            })
            .collect();
        Ok(ChainReport {
            ok: scopes.iter().all(|scope| scope.broken_at.is_none()),
            scopes,
        })
    }

    /// Drops every projection in the tenant and rebuilds it by replaying the journals. Refuses,
    /// changing nothing, if any journal doesn't verify. A write that lands mid-rebuild sends it
    /// back to read the journals again, so the rebuild never drops it.
    pub async fn reindex(&self, caller: &Caller) -> Result<ReindexReport> {
        caller.require(Permission::Admin)?;
        for _ in 0..REINDEX_ATTEMPTS {
            if let Some(report) = self.try_reindex(&caller.tenant)? {
                return Ok(report);
            }
        }
        Err(Error::Busy(format!(
            "writes kept landing while reindex ran, {REINDEX_ATTEMPTS} times; try again when \
             agents are quieter"
        )))
    }

    /// One reindex attempt. Returns `None`, changing nothing, if a write landed after the
    /// journals were read.
    fn try_reindex(&self, tenant: &str) -> Result<Option<ReindexReport>> {
        let journals = self.read_journals(tenant)?;
        if let Some((scope, broken)) = journals
            .iter()
            .find_map(|journal| Some((&journal.scope, journal.broken_at.as_ref()?)))
        {
            return Err(Error::ChainBroken(format!(
                "{scope} is broken at entry {}: {}. Reindex rebuilds only from journals that \
                 verify; run `ledgermem verify` for every scope's status",
                broken.seq, broken.problem
            )));
        }
        let heads: Vec<_> = journals
            .iter()
            .map(|journal| JournalHead {
                scope: journal.scope.clone(),
                seq: journal.entries.len() as u64,
            })
            .collect();
        // Replay journal by journal. Every entry about a claim lives in its scope's journal,
        // because conflicts and supersession stay within one scope (ADR-0005), so replaying
        // each journal in order rebuilds every claim as live writes left it.
        let mut projections = Vec::new();
        for stored in journals.into_iter().flat_map(|journal| journal.entries) {
            let entry = readable(stored)?;
            projections.extend(projections_for(entry.kind, &entry.content)?);
        }
        let claims = distinct_claims(&projections);
        if !self
            .store
            .replace_projections(tenant, &heads, projections)?
        {
            return Ok(None);
        }
        Ok(Some(ReindexReport {
            scopes: heads.len() as u64,
            entries: heads.iter().map(|head| head.seq).sum(),
            claims,
        }))
    }

    /// The tenant policy, with `project`'s overlay on top when there is one.
    fn policy_in(&self, tenant: &str, project: Option<&str>) -> Result<Policy> {
        let overlay = project.map(|project| Scope::new(ScopeLevel::Project, project));
        let scopes: Vec<_> = std::iter::once(Scope::org()).chain(overlay).collect();
        Ok(Policy::layered(&self.store.policies(tenant, &scopes)?))
    }

    /// Every journal in the tenant as stored, read as one snapshot, each with its first broken
    /// link, if any.
    fn read_journals(&self, tenant: &str) -> Result<Vec<Journal>> {
        Ok(self
            .store
            .journals(tenant)?
            .into_iter()
            .map(|stored| Journal {
                broken_at: journal::verify_chain(&stored.scope, &stored.entries),
                scope: stored.scope,
                entries: stored.entries,
            })
            .collect())
    }
}

/// One scope's journal as stored, and the first entry in it that doesn't verify.
struct Journal {
    scope: String,
    entries: Vec<StoredEntry>,
    broken_at: Option<BrokenLink>,
}

/// A stored entry as reads return it. One that no longer parses has been tampered with.
fn readable(stored: StoredEntry) -> Result<JournalEntry> {
    JournalEntry::try_from(stored)
        .map_err(|reason| Error::Internal(format!("{reason}; run `ledgermem verify`")))
}

/// How many claims the projections put, counting each claim once.
fn distinct_claims(projections: &[Projection]) -> u64 {
    projections
        .iter()
        .filter_map(|projection| match projection {
            Projection::PutClaim(claim) => Some(claim.id.as_str()),
            Projection::PutPolicy(_) => None,
        })
        .collect::<BTreeSet<_>>()
        .len() as u64
}

/// The scope a policy applies to: the project it overlays, or `org` for the tenant policy.
/// The error says why the project id names no project.
fn policy_scope(project: Option<&str>) -> Result<Scope, String> {
    match project {
        None => Ok(Scope::org()),
        Some("") => {
            Err("a project id can't be empty; name the project, such as billing-api".into())
        }
        Some(project) => Ok(Scope::new(ScopeLevel::Project, project)),
    }
}

/// The projector: the projection updates a journal entry implies (ADR-0010). Live writes and
/// reindex both go through it, so a rebuild lands exactly where the writes did.
fn projections_for(kind: EntryKind, content: &Value) -> Result<Vec<Projection>> {
    match kind {
        EntryKind::ClaimWritten => {
            let claim = serde_json::from_value(content.clone()).map_err(internal)?;
            Ok(vec![Projection::PutClaim(claim)])
        }
        EntryKind::PolicyApplied => {
            let version = serde_json::from_value(content.clone()).map_err(internal)?;
            Ok(vec![Projection::PutPolicy(version)])
        }
    }
}

/// A compact age in its largest whole unit: `45s`, `38m`, `3h` or `12d`.
fn format_age(age: Duration) -> String {
    let seconds = age.whole_seconds().max(0);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

fn internal(error: impl std::fmt::Display) -> Error {
    Error::Internal(error.to_string())
}
