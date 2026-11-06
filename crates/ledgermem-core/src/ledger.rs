//! The Ledger engine: the one value every surface calls.

use std::sync::Arc;

use serde_json::{Map, Value};
use time::{Duration, OffsetDateTime};

use crate::caller::{Caller, Permission};
use crate::claim::{
    Claim, ClaimId, Evidence, EvidenceKind, RetractionReason, Scope, ScopeLevel, Source, Status,
};
use crate::clock::Clock;
use crate::error::{Error, Result, internal};
use crate::gate;
use crate::journal::{self, EntryKind, JournalEntry, NewEntry, StoredEntry};
use crate::ops::{
    ApplyPolicy, Attest, AttestOutcome, AttestedOutcome, BrokenLink, ChainReport, ClaimResult,
    Guidance, GuidanceCode, JobsRun, JournalQuery, PolicyApplied, PolicyQuery, ReadIntent,
    ReadPack, Recall, RecalledClaim, ReindexReport, Remember, RememberOutcome, ScopeVerification,
    Verification,
};
use crate::policy::{FreshnessClass, Policy, PolicyVersion, VerifierName, invalid_policy};
use crate::projector::{self, ClaimRetracted, Claims, EvidenceAttached, StatusChanged};
use crate::store::{Commit, Job, JournalHead, NewJob, Store};
use crate::trust;
use crate::verifier::{Check, Outcome, Verifier, Verifiers};

/// The tenant a local install serves, created on first launch.
pub const LOCAL_TENANT: &str = "local";

const DEFAULT_RECALL_LIMIT: usize = 10;

/// How many times reindex reads the journals again after a write lands mid-rebuild.
const REINDEX_ATTEMPTS: u32 = 3;

/// How many times a write that depends on a claim's status reads the claim again after another
/// write lands in its scope first.
const WRITE_ATTEMPTS: u32 = 3;

/// The most jobs one run of the job runner takes, so a long queue can't hold it for good.
const JOBS_PER_RUN: u64 = 100;

/// How long a runner holds a job before another may take it over. Longer than any check lasts.
const JOB_LEASE: Duration = Duration::minutes(15);

pub struct Ledger {
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    verifiers: Verifiers,
}

impl Ledger {
    pub fn new(store: Arc<dyn Store>, clock: Arc<dyn Clock>) -> Self {
        Ledger {
            store,
            clock,
            verifiers: Verifiers::default(),
        }
    }

    /// Checks claims with `verifiers`. A claim whose predicate names a verifier the ledger
    /// doesn't have stays as the write gate left it.
    pub fn with_verifiers(mut self, verifiers: Verifiers) -> Self {
        self.verifiers = verifiers;
        self
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
    /// predicates' declared types. Each claim's source kind comes from the caller's channel and
    /// key, and the request can only lower it. If any claim fails a check, none is remembered.
    /// The verifier a claim's predicate declares then checks it inline: a pass verifies it, a
    /// fail retracts it as refuted, and an inconclusive result changes nothing. A slow verifier
    /// checks it in the background instead: the claim is written as it stands, with its check
    /// queued in the same write, and its verification is pending until the job runner has run it.
    pub async fn remember(&self, caller: &Caller, req: Remember) -> Result<RememberOutcome> {
        caller.require(Permission::Propose)?;
        let now = self.clock.now();
        let scope = caller.default_write_scope();
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;

        // Every claim passes the policy and the write gate before any verifier runs, so a request
        // that fails a check costs no verification.
        let mut written = Vec::with_capacity(req.claims.len());
        for (index, input) in req.claims.into_iter().enumerate() {
            let invalid = |(field, reason)| {
                Error::InvalidClaim(format!(
                    "claims[{index}].{field}: {reason}; nothing was remembered"
                ))
            };
            let admitted = policy
                .admit(&input.subject, &input.predicate, input.value.clone())
                .map_err(invalid)?;
            let classified = gate::classify(caller, &input, &admitted, &policy).map_err(invalid)?;
            let mut evidence: Vec<_> = classified
                .evidence
                .into_iter()
                .map(|kind| Evidence::observed(kind, now))
                .collect();
            evidence.extend(
                gate::attached_evidence(&input.evidence, now).map_err(|wrong| {
                    Error::InvalidClaim(format!("claims[{index}].{wrong}; nothing was remembered"))
                })?,
            );
            let claim = Claim {
                id: ClaimId::generate(now),
                scope: scope.clone(),
                subject: admitted.subject,
                predicate: input.predicate,
                value: admitted.value,
                text: input.text,
                source: Source {
                    kind: classified.kind,
                    actor: caller.actor.clone(),
                    channel: caller.channel,
                },
                evidence,
                status: classified.status,
                reason: None,
                valid_from: now,
                valid_to: None,
                recorded_at: now,
            };
            let hints: Vec<_> = admitted.hints.into_iter().chain(classified.hints).collect();
            written.push((claim, hints));
        }

        let mut commit = Commit::default();
        let mut claims = Claims::new();
        let mut results = Vec::with_capacity(written.len());
        for (mut claim, mut hints) in written {
            let verification = self
                .verify_as_written(&mut claim, &policy, &mut hints)
                .await;
            if verification == Verification::Pending {
                commit.queued.push(NewJob::check(claim.id.clone(), now));
            }
            let standing = trust::standing(&claim, &policy, now);
            results.push(ClaimResult {
                claim_id: claim.id.clone(),
                status: claim.status,
                reason: claim.reason,
                verification,
                trusted: standing.trusted,
                trust: standing.trust,
                hints,
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
                .push(projector::project(entry.kind, &entry.content, &mut claims)?);
            commit.entries.push(entry);
        }

        self.store.commit(&caller.tenant, commit)?;
        Ok(RememberOutcome { results })
    }

    /// Runs now, rather than on the job runner's next turn, the checks a remember left pending,
    /// and returns what the remember would have returned had they run inline. A check another
    /// runner holds is left to it, and its claim stays pending, for the caller to ask again. For
    /// a surface that can wait without holding up its client, such as an MCP task.
    pub async fn finish_remember(
        &self,
        caller: &Caller,
        mut outcome: RememberOutcome,
    ) -> Result<RememberOutcome> {
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;
        for result in &mut outcome.results {
            if result.verification != Verification::Pending {
                continue;
            }
            let Some(claim) = self.finish_check(&caller.tenant, &result.claim_id).await? else {
                continue;
            };
            result.verification = match last_check(&claim) {
                Some((check, _)) => {
                    result.hints.extend(check_hint(check));
                    Verification::Done
                }
                None => Verification::Unchecked,
            };
            let standing = trust::standing(&claim, &policy, self.clock.now());
            result.status = claim.status;
            result.reason = claim.reason;
            result.trusted = standing.trusted;
            result.trust = standing.trust;
        }
        Ok(outcome)
    }

    /// Checks a claim as it is written, unless the write gate quarantined it, and records what
    /// the verifier found on the claim itself, so the entry that writes the claim fixes it. A
    /// slow check is left pending.
    async fn verify_as_written(
        &self,
        claim: &mut Claim,
        policy: &Policy,
        hints: &mut Vec<String>,
    ) -> Verification {
        if claim.status == Status::Quarantined {
            return Verification::Unchecked;
        }
        let declared = match self.declared_verifier(policy, claim) {
            Ok(declared) => declared,
            Err(Unchecked::Undeclared) => return Verification::Unchecked,
            Err(unavailable) => {
                hints.push(format!(
                    "{}, so the claim wasn't checked",
                    unavailable.why(claim)
                ));
                return Verification::Unchecked;
            }
        };
        if declared.in_background() {
            return Verification::Pending;
        }
        let finding = declared.check(claim, self.clock.as_ref()).await;
        hints.extend(finding.hint);
        if let Some(status) = status_after(claim.status, finding.outcome) {
            claim.status = status;
            claim.reason = (status == Status::Retracted).then_some(RetractionReason::Refuted);
        }
        claim.evidence.push(finding.evidence);
        Verification::Done
    }

    /// The verifier `claim`'s predicate declares, or why there is none to check it with.
    fn declared_verifier(&self, policy: &Policy, claim: &Claim) -> Result<Declared<'_>, Unchecked> {
        let predicate = policy.predicate(&claim.predicate);
        let declared = predicate.verifier.clone().ok_or(Unchecked::Undeclared)?;
        let verifier = self
            .verifiers
            .get(&declared.verifier)
            .ok_or_else(|| Unchecked::Unavailable(declared.verifier.clone()))?;
        Ok(Declared {
            system_of_record: predicate.system_of_record.as_ref() == Some(&declared.verifier),
            name: declared.verifier,
            params: declared.params,
            verifier,
        })
    }

    /// Attaches a caller's evidence to a claim, and checks the claim against its predicate's
    /// verifier now unless asked not to. A pass verifies a proposed claim and renews a verified
    /// one's freshness, a fail retracts it as refuted, and an inconclusive result changes
    /// nothing. A slow verifier checks it in the background instead, queued in the same write,
    /// and the outcome is pending. The caller's own evidence is recorded, hashed, but attests
    /// nothing.
    pub async fn attest(&self, caller: &Caller, req: Attest) -> Result<AttestOutcome> {
        caller.require(Permission::Attest)?;
        let now = self.clock.now();
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;
        let (claim, head) = self.attestable(caller, &req.claim_id)?;

        let mut evidence = gate::attached_evidence(&req.evidence, now)
            .map_err(|wrong| Error::InvalidClaim(format!("{wrong}; nothing was attached")))?;
        let mut outcome = Outcome::Inconclusive;
        let mut attested = AttestedOutcome::Inconclusive;
        let mut queued = None;
        let mut hints = Vec::new();
        if req.verify_now {
            match self.declared_verifier(&policy, &claim) {
                Ok(declared) if declared.in_background() => {
                    queued = Some(NewJob::check(claim.id.clone(), now));
                    attested = AttestedOutcome::Pending;
                }
                Ok(declared) => {
                    let finding = declared.check(&claim, self.clock.as_ref()).await;
                    outcome = finding.outcome;
                    attested = finding.outcome.into();
                    hints.extend(finding.hint);
                    evidence.push(finding.evidence);
                }
                Err(unchecked) if evidence.is_empty() => {
                    return Err(Error::VerifierUnavailable(format!(
                        "{}, so Ledger can't check {}; attach the evidence you observed instead",
                        unchecked.why(&claim),
                        req.claim_id
                    )));
                }
                Err(unchecked) => hints.push(format!(
                    "{}, so the evidence was attached unchecked",
                    unchecked.why(&claim)
                )),
            }
        } else if evidence.is_empty() {
            return Err(Error::InvalidClaim(
                "evidence: attach what you observed, or ask to verify_now; nothing was attached"
                    .into(),
            ));
        }

        let attestation = Attestation {
            by: caller.clone(),
            evidence,
            outcome,
            queued,
            finished: None,
            at: self.clock.now(),
        };
        let reread = || self.attestable(caller, &req.claim_id).map(Some);
        let claim = self
            .record(&attestation, (claim, head), reread)?
            .ok_or_else(|| internal(format!("{} vanished while it was attested", req.claim_id)))?;
        let standing = trust::standing(&claim, &policy, attestation.at);
        Ok(AttestOutcome {
            claim_id: claim.id,
            outcome: attested,
            status: claim.status,
            reason: claim.reason,
            trusted: standing.trusted,
            trust: standing.trust,
            evidence: attestation.evidence,
            hints,
        })
    }

    /// Runs now, rather than on the job runner's next turn, the check an attest left pending, and
    /// returns what the attest would have returned had it run inline: the outcome of the claim's
    /// latest check. A check another runner holds is left to it, and the outcome stays pending,
    /// for the caller to ask again. For a surface that can wait without holding up its client,
    /// such as an MCP task.
    pub async fn finish_attest(
        &self,
        caller: &Caller,
        mut outcome: AttestOutcome,
    ) -> Result<AttestOutcome> {
        if outcome.outcome != AttestedOutcome::Pending {
            return Ok(outcome);
        }
        let Some(claim) = self.finish_check(&caller.tenant, &outcome.claim_id).await? else {
            return Ok(outcome);
        };
        match last_check(&claim) {
            Some((check, found)) => {
                outcome.outcome = found.into();
                outcome.hints.extend(check_hint(check));
                outcome.evidence.push(check.clone());
            }
            None => outcome.outcome = AttestedOutcome::Inconclusive,
        }
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;
        let standing = trust::standing(&claim, &policy, self.clock.now());
        outcome.status = claim.status;
        outcome.reason = claim.reason;
        outcome.trusted = standing.trusted;
        outcome.trust = standing.trust;
        Ok(outcome)
    }

    /// Runs the check queued for `claim_id` now, unless another runner holds it, and returns the
    /// claim once no check is queued for it. None while another runner still holds its check.
    async fn finish_check(&self, tenant: &str, claim_id: &ClaimId) -> Result<Option<Claim>> {
        let now = self.clock.now();
        match self
            .store
            .lease_job_for(tenant, claim_id, now, now + JOB_LEASE)?
        {
            Some(job) => self.run_job(job).await?,
            None if self.store.job_waits(tenant, claim_id)? => return Ok(None),
            None => {}
        }
        let (claim, _) = self
            .store
            .claim(tenant, claim_id)?
            .ok_or_else(|| Error::NotFound(format!("there is no claim {claim_id}")))?;
        Ok(Some(claim))
    }

    /// Records `attestation` against the claim `read` holds, as of where its scope's journal
    /// ends. The outcome moves the claim on from the status it has when the attestation commits,
    /// so if another write lands in the scope first, `reread` reads the claim again and the
    /// outcome is decided again. Returns the claim as the attestation leaves it, or None if
    /// `reread` finds nothing left to attest.
    fn record(
        &self,
        attestation: &Attestation,
        read: (Claim, JournalHead),
        reread: impl Fn() -> Result<Option<(Claim, JournalHead)>>,
    ) -> Result<Option<Claim>> {
        let tenant = &attestation.by.tenant;
        let mut read = Some(read);
        for _ in 0..WRITE_ATTEMPTS {
            let (claim, head) = match read.take() {
                Some(read) => read,
                None => match reread()? {
                    Some(read) => read,
                    None => return Ok(None),
                },
            };
            let id = claim.id.clone();
            let entries = attestation.entries(&claim)?;
            let mut claims = Claims::from([(id.clone(), claim)]);
            let projections = entries
                .iter()
                .map(|entry| projector::project(entry.kind, &entry.content, &mut claims))
                .collect::<Result<_>>()?;
            let commit = Commit {
                entries,
                projections,
                queued: attestation.queued.iter().cloned().collect(),
                finished: attestation.finished.iter().cloned().collect(),
            };
            if self.store.commit_at(tenant, &[head], commit)? {
                return Ok(claims.remove(&id));
            }
        }
        Err(Error::Busy(format!(
            "other writes kept landing in the claim's scope while it was attested, \
             {WRITE_ATTEMPTS} times; try again"
        )))
    }

    /// The claim `id` names, and where its scope's journal ends, if the caller may see it and it
    /// is still memory.
    fn attestable(&self, caller: &Caller, id: &ClaimId) -> Result<(Claim, JournalHead)> {
        let visible = caller.visible_scopes();
        let found = self
            .store
            .claim(&caller.tenant, id)?
            .filter(|(claim, _)| visible.contains(&claim.scope));
        let Some((claim, head)) = found else {
            return Err(Error::NotFound(format!(
                "there is no claim {id} within the scope bounds of {}",
                caller.actor
            )));
        };
        match claim.status {
            Status::Quarantined => Err(Error::InvalidClaim(format!(
                "{id} is quarantined: it isn't served as memory, and only an operator can \
                 release it"
            ))),
            Status::Retracted => Err(Error::InvalidClaim(format!(
                "{id} is retracted, and a retracted claim is never used again; remember it anew \
                 if it holds now"
            ))),
            _ => Ok((claim, head)),
        }
    }

    /// The job runner: runs the jobs that are due, in every tenant, and says how many ran. Each
    /// re-verifies one claim against its verifier, and records what it found as Ledger's own, as
    /// an attestation would. A job that fails stays leased, so it runs again once its lease runs
    /// out, and the others still run. The process serving the ledger calls this from time to
    /// time; nothing in the engine runs it otherwise.
    pub async fn run_jobs(&self) -> Result<JobsRun> {
        let mut run = JobsRun::default();
        while run.ran + (run.failed.len() as u64) < JOBS_PER_RUN {
            let now = self.clock.now();
            let Some(job) = self.store.lease_job(now, now + JOB_LEASE)? else {
                break;
            };
            match self.run_job(job).await {
                Ok(()) => run.ran += 1,
                Err(error) => run.failed.push(error.to_string()),
            }
        }
        Ok(run)
    }

    /// Re-verifies a job's claim and records what the check found, finishing the job in the same
    /// write. A claim no longer proposed, verified or contested, or that nothing can check any
    /// more, just finishes the job. The claim is checked under the policy of its own scope.
    async fn run_job(&self, job: Job) -> Result<()> {
        let checkable = || -> Result<Option<(Claim, JournalHead)>> {
            Ok(self
                .store
                .claim(&job.tenant, &job.claim_id)?
                .filter(|(claim, _)| still_checked(claim.status)))
        };
        if let Some((claim, head)) = checkable()? {
            let policy = self.policy_for(&job.tenant, &claim)?;
            if let Ok(declared) = self.declared_verifier(&policy, &claim) {
                let finding = declared.check(&claim, self.clock.as_ref()).await;
                let attestation = Attestation {
                    by: Caller::ledger_itself(&job.tenant),
                    evidence: vec![finding.evidence],
                    outcome: finding.outcome,
                    queued: None,
                    finished: Some(job.id.clone()),
                    at: self.clock.now(),
                };
                if self
                    .record(&attestation, (claim, head), checkable)?
                    .is_some()
                {
                    return Ok(());
                }
            }
        }
        let finished = Commit {
            finished: vec![job.id],
            ..Commit::default()
        };
        Ok(self.store.commit(&job.tenant, finished)?)
    }

    /// The policy in effect in `claim`'s own scope: the tenant policy, with the claim's project
    /// overlay when it lives in a project.
    fn policy_for(&self, tenant: &str, claim: &Claim) -> Result<Policy> {
        let project = match (&claim.scope.level, &claim.scope.id) {
            (ScopeLevel::Project, Some(project)) => Some(project.as_str()),
            _ => None,
        };
        self.policy_in(tenant, project)
    }

    /// Recalls the claims that hold now and match the query, best match first, each scored at
    /// the time of the read under the policy in effect for the caller. The stale volatile claims
    /// it returns get their re-verification queued, and the read never waits for it.
    pub async fn recall(&self, caller: &Caller, req: Recall) -> Result<ReadPack> {
        caller.require(Permission::Read)?;
        let now = self.clock.now();
        let limit = req.limit.unwrap_or(DEFAULT_RECALL_LIMIT);
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;
        let claims =
            self.store
                .search(&caller.tenant, &caller.visible_scopes(), &req.query, limit)?;

        let mut reverify = Vec::new();
        let mut answer_set = Vec::with_capacity(claims.len());
        for claim in claims {
            let standing = trust::standing(&claim, &policy, now);
            let volatile = policy.predicate(&claim.predicate).freshness == FreshnessClass::Volatile;
            // Queued only if the job runner, which checks the claim under the policy of its own
            // scope, will find a verifier for it there.
            if standing.stale
                && volatile
                && still_checked(claim.status)
                && self
                    .declared_verifier(&self.policy_for(&caller.tenant, &claim)?, &claim)
                    .is_ok()
            {
                reverify.push(NewJob::check(claim.id.clone(), now));
            }
            answer_set.push(RecalledClaim {
                trusted: standing.trusted,
                trust: standing.trust,
                age: format_age(now - claim.recorded_at),
                stale: standing.stale,
                injectable: standing.injectable,
                claim_id: claim.id,
                scope: claim.scope,
                subject: claim.subject,
                predicate: claim.predicate,
                value: claim.value,
                text: claim.text,
                status: claim.status,
                recorded_at: claim.recorded_at,
            });
        }
        // Queueing is the read's only side effect, and the read still answers if it fails.
        let ids = reverify.iter().map(|job| job.claim_id.clone()).collect();
        let queue = Commit {
            queued: reverify,
            ..Commit::default()
        };
        let queued = queue.queued.is_empty() || self.store.commit(&caller.tenant, queue).is_ok();
        let stale = if queued { ids } else { Vec::new() };
        Ok(ReadPack {
            intent: ReadIntent::Current,
            guidance: guidance(&answer_set, req.high_stakes),
            answer_set,
            stale,
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
            projections: vec![projector::project(
                entry.kind,
                &entry.content,
                &mut Claims::new(),
            )?],
            entries: vec![entry],
            ..Commit::default()
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
        let mut claims = Claims::new();
        let mut projections = Vec::new();
        for stored in journals.into_iter().flat_map(|journal| journal.entries) {
            let entry = readable(stored)?;
            projections.push(projector::project(entry.kind, &entry.content, &mut claims)?);
        }
        if !self
            .store
            .replace_projections(tenant, &heads, projections)?
        {
            return Ok(None);
        }
        Ok(Some(ReindexReport {
            scopes: heads.len() as u64,
            entries: heads.iter().map(|head| head.seq).sum(),
            claims: claims.len() as u64,
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

/// Why nothing could check a claim.
enum Unchecked {
    /// The claim's predicate declares no verifier.
    Undeclared,
    /// It declares this one, which the ledger doesn't have.
    Unavailable(VerifierName),
}

impl Unchecked {
    /// Why nothing could check `claim`, in words for whoever wrote it.
    fn why(&self, claim: &Claim) -> String {
        match self {
            Unchecked::Undeclared => format!("{} declares no verifier", claim.predicate),
            Unchecked::Unavailable(name) => format!(
                "{}'s verifier {name} isn't available on this server",
                claim.predicate
            ),
        }
    }
}

/// The verifier a claim's predicate declares, as this ledger has it.
struct Declared<'a> {
    name: VerifierName,
    params: Map<String, Value>,
    /// Whether it is the predicate's system of record.
    system_of_record: bool,
    verifier: &'a dyn Verifier,
}

impl Declared<'_> {
    /// Whether its check takes long enough that it runs in the background.
    fn in_background(&self) -> bool {
        self.verifier.runs_in_background(&self.params)
    }

    /// What it finds about `claim` now, recorded as evidence observed when the check finished.
    async fn check(&self, claim: &Claim, clock: &dyn Clock) -> Finding {
        let check = Check {
            claim,
            params: &self.params,
        };
        let observation = self.verifier.verify(check).await;
        let outcome = observation.outcome;
        let evidence =
            observation.into_evidence(self.name.clone(), self.system_of_record, clock.now());
        Finding {
            outcome,
            hint: check_hint(&evidence),
            evidence,
        }
    }
}

/// One attestation to record against a claim.
struct Attestation {
    /// Who made it: a caller, or Ledger itself.
    by: Caller,
    evidence: Vec<Evidence>,
    /// What checking the claim found, which may move it on. Inconclusive when nothing checked it.
    outcome: Outcome,
    /// A slow check it leaves to the job runner.
    queued: Option<NewJob>,
    /// The job it finishes, when the job runner made it.
    finished: Option<String>,
    at: OffsetDateTime,
}

impl Attestation {
    /// The entries that record it against `claim`: the evidence attached, if any, then the
    /// status the outcome moves the claim on to, if any.
    fn entries(&self, claim: &Claim) -> Result<Vec<NewEntry>> {
        let entry = |kind, content: Result<Value, serde_json::Error>| -> Result<NewEntry> {
            Ok(NewEntry {
                id: NewEntry::generate_id(self.at),
                scope: claim.scope.clone(),
                kind,
                actor: self.by.actor.clone(),
                channel: self.by.channel,
                recorded_at: self.at,
                content: content.map_err(internal)?,
            })
        };
        let claim_id = claim.id.clone();
        let mut entries = Vec::new();
        if !self.evidence.is_empty() {
            entries.push(entry(
                EntryKind::EvidenceAttached,
                serde_json::to_value(EvidenceAttached {
                    claim_id: claim_id.clone(),
                    evidence: self.evidence.clone(),
                }),
            )?);
        }
        match status_after(claim.status, self.outcome) {
            Some(Status::Retracted) => entries.push(entry(
                EntryKind::ClaimRetracted,
                serde_json::to_value(ClaimRetracted {
                    claim_id,
                    reason: RetractionReason::Refuted,
                }),
            )?),
            Some(status) => entries.push(entry(
                EntryKind::StatusChanged,
                serde_json::to_value(StatusChanged { claim_id, status }),
            )?),
            None => {}
        }
        Ok(entries)
    }
}

/// What a claim's verifier found.
struct Finding {
    outcome: Outcome,
    /// The evidence that records it.
    evidence: Evidence,
    /// Why the claim didn't pass, for whoever wrote it.
    hint: Option<String>,
}

/// Why a verifier's evidence didn't pass the claim, for whoever wrote it.
fn check_hint(evidence: &Evidence) -> Option<String> {
    let EvidenceKind::Verifier {
        verifier,
        outcome,
        summary,
        ..
    } = &evidence.kind
    else {
        return None;
    };
    match outcome {
        Outcome::Pass => None,
        Outcome::Fail => Some(format!("{verifier} refuted the claim: {summary}")),
        Outcome::Inconclusive => Some(format!("{verifier} couldn't check the claim: {summary}")),
    }
}

/// The latest check a verifier made of `claim`, and its outcome.
fn last_check(claim: &Claim) -> Option<(&Evidence, Outcome)> {
    claim
        .evidence
        .iter()
        .rev()
        .find_map(|evidence| match evidence.kind {
            EvidenceKind::Verifier { outcome, .. } => Some((evidence, outcome)),
            _ => None,
        })
}

/// Whether a claim with `status` is still memory that re-verification can speak to. A check of a
/// contested claim is recorded, though only a resolution moves it on.
fn still_checked(status: Status) -> bool {
    matches!(
        status,
        Status::Proposed | Status::Verified | Status::Contested
    )
}

/// The status a verifier's outcome moves a claim with `status` on to, if any
/// (claims-and-integrity.md, section 4). A pass verifies a proposed claim. A fail retracts a
/// proposed or verified claim as refuted. An inconclusive result changes nothing, and so does
/// any outcome for a claim in another status: a contested claim leaves its conflict set only
/// through a resolution.
fn status_after(status: Status, outcome: Outcome) -> Option<Status> {
    match (outcome, status) {
        (Outcome::Pass, Status::Proposed) => Some(Status::Verified),
        (Outcome::Fail, Status::Proposed | Status::Verified) => Some(Status::Retracted),
        _ => None,
    }
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

/// How far a read's best answer can be relied on (claims-and-integrity.md, section 9). The other
/// candidates carry their own labels.
fn guidance(answer_set: &[RecalledClaim], high_stakes: bool) -> Guidance {
    let verdict = |code, text: &str| Guidance {
        code,
        text: text.to_owned(),
    };
    let Some(best) = answer_set.first() else {
        return verdict(
            GuidanceCode::NoAnswer,
            "no memory: say you don't know rather than guess",
        );
    };
    // Conflicts other than the best answer's own come with the conflict engine (ticket #09).
    if best.status == Status::Contested {
        return verdict(
            GuidanceCode::Unresolved,
            "unresolved: the best answer is contested; verify it through a tool or ask the user",
        );
    }
    let (shortfall, text) = if best.status == Status::Verified && best.stale {
        (
            GuidanceCode::Stale,
            "stale: it was last checked longer ago than its freshness allows, so check it before \
             relying on it",
        )
    } else if !best.trusted {
        (
            GuidanceCode::Unverified,
            "unverified: check it with your tools or ask the user before relying on it",
        )
    } else {
        return verdict(
            GuidanceCode::Reliable,
            "verified and fresh: rely on it as known",
        );
    };
    if high_stakes {
        return verdict(
            GuidanceCode::Unresolved,
            &format!(
                "unresolved: the best answer is {shortfall}, and a high-stakes read shouldn't \
                 rest on it; verify it through a tool or ask the user"
            ),
        );
    }
    verdict(shortfall, text)
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
