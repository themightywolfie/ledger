//! The Ledger engine: the one value every surface calls.

use std::sync::Arc;

use time::Duration;

use crate::caller::{Caller, Permission};
use crate::claim::{Claim, ClaimId, Source, SourceKind, Status};
use crate::clock::Clock;
use crate::error::{Error, Result};
use crate::journal::{EntryKind, JournalEntry, NewEntry};
use crate::ops::{
    ClaimResult, JournalQuery, ReadIntent, ReadPack, Recall, RecalledClaim, Remember,
    RememberOutcome,
};
use crate::store::{Commit, Projection, Store};

/// The tenant a local install serves, created on first launch.
pub const LOCAL_TENANT: &str = "local";

const DEFAULT_RECALL_LIMIT: usize = 10;

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
    pub async fn remember(&self, caller: &Caller, req: Remember) -> Result<RememberOutcome> {
        caller.require(Permission::Propose)?;
        let now = self.clock.now();
        let scope = caller.default_write_scope();
        let source = Source {
            kind: SourceKind::for_channel(caller.channel),
            actor: caller.actor.clone(),
            channel: caller.channel,
        };

        let mut commit = Commit::default();
        let mut results = Vec::with_capacity(req.claims.len());
        for input in req.claims {
            let claim = Claim {
                id: ClaimId::generate(now),
                scope: scope.clone(),
                subject: input.subject,
                predicate: input.predicate,
                value: input.value,
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
            commit.projections.extend(projections_for(&entry)?);
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
        Ok(self.store.journal(&caller.tenant, &query.scope)?)
    }
}

/// The projector: the projection updates a journal entry implies (ADR-0010).
fn projections_for(entry: &NewEntry) -> Result<Vec<Projection>> {
    match entry.kind {
        EntryKind::ClaimWritten => {
            let claim = serde_json::from_value(entry.content.clone()).map_err(internal)?;
            Ok(vec![Projection::PutClaim(claim)])
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
