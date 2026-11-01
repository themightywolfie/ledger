//! The Ledger engine: the one value every surface calls.

use std::sync::Arc;

use serde_json::Value;
use time::{Duration, OffsetDateTime};

use crate::caller::{Caller, Permission};
use crate::claim::{Claim, ClaimId, Evidence, RetractionReason, Scope, ScopeLevel, Source, Status};
use crate::clock::Clock;
use crate::error::{Error, Result, internal};
use crate::gate;
use crate::journal::{self, EntryKind, JournalEntry, NewEntry, StoredEntry};
use crate::ops::{
    ApplyPolicy, Attest, AttestOutcome, BrokenLink, ChainReport, ClaimResult, Guidance,
    GuidanceCode, JournalQuery, PolicyApplied, PolicyQuery, ReadIntent, ReadPack, Recall,
    RecalledClaim, ReindexReport, Remember, RememberOutcome, ScopeVerification, Verification,
};
use crate::policy::{Policy, PolicyVersion, VerifierName, invalid_policy};
use crate::projector::{self, ClaimRetracted, Claims, EvidenceAttached, StatusChanged};
use crate::store::{Commit, JournalHead, Store};
use crate::trust;
use crate::verifier::{Check, Outcome, Verifiers};

/// The tenant a local install serves, created on first launch.
pub const LOCAL_TENANT: &str = "local";

const DEFAULT_RECALL_LIMIT: usize = 10;

/// How many times reindex reads the journals again after a write lands mid-rebuild.
const REINDEX_ATTEMPTS: u32 = 3;

/// How many times a write that depends on a claim's status reads the claim again after another
/// write lands in its scope first.
const WRITE_ATTEMPTS: u32 = 3;

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
    /// fail retracts it as refuted, and an inconclusive result changes nothing.
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

    /// Checks a claim as it is written, unless the write gate quarantined it, and records what
    /// the verifier found on the claim itself, so the entry that writes the claim fixes it.
    async fn verify_as_written(
        &self,
        claim: &mut Claim,
        policy: &Policy,
        hints: &mut Vec<String>,
    ) -> Verification {
        if claim.status == Status::Quarantined {
            return Verification::Unchecked;
        }
        let finding = match self.check(claim, policy, claim.recorded_at).await {
            Ok(finding) => finding,
            Err(Unchecked::Undeclared) => return Verification::Unchecked,
            Err(unavailable) => {
                hints.push(format!(
                    "{}, so the claim wasn't checked",
                    unavailable.why(claim)
                ));
                return Verification::Unchecked;
            }
        };
        hints.extend(finding.hint);
        if let Some(status) = status_after(claim.status, finding.outcome) {
            claim.status = status;
            claim.reason = (status == Status::Retracted).then_some(RetractionReason::Refuted);
        }
        claim.evidence.push(finding.evidence);
        Verification::Done
    }

    /// Asks the verifier `claim`'s predicate declares what it finds now, recorded as evidence
    /// observed at `at`, or says why nothing could.
    async fn check(
        &self,
        claim: &Claim,
        policy: &Policy,
        at: OffsetDateTime,
    ) -> Result<Finding, Unchecked> {
        let predicate = policy.predicate(&claim.predicate);
        let declared = predicate.verifier.as_ref().ok_or(Unchecked::Undeclared)?;
        let name = &declared.verifier;
        let verifier = self
            .verifiers
            .get(name)
            .ok_or_else(|| Unchecked::Unavailable(name.clone()))?;
        let check = Check {
            claim,
            params: &declared.params,
        };
        let observation = verifier.verify(check).await;
        let hint = match observation.outcome {
            Outcome::Pass => None,
            Outcome::Fail => Some(format!("{name} refuted the claim: {}", observation.summary)),
            Outcome::Inconclusive => Some(format!(
                "{name} couldn't check the claim: {}",
                observation.summary
            )),
        };
        let system_of_record = predicate.system_of_record.as_ref() == Some(name);
        Ok(Finding {
            outcome: observation.outcome,
            hint,
            evidence: observation.into_evidence(name.clone(), system_of_record, at),
        })
    }

    /// Attaches a caller's evidence to a claim, and checks the claim against its predicate's
    /// verifier now unless asked not to. A pass verifies a proposed claim and renews a verified
    /// one's freshness, a fail retracts it as refuted, and an inconclusive result changes
    /// nothing. The caller's own evidence is recorded, hashed, but attests nothing.
    pub async fn attest(&self, caller: &Caller, req: Attest) -> Result<AttestOutcome> {
        caller.require(Permission::Attest)?;
        let now = self.clock.now();
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;
        let (claim, head) = self.attestable(caller, &req.claim_id)?;

        let mut evidence = gate::attached_evidence(&req.evidence, now)
            .map_err(|wrong| Error::InvalidClaim(format!("{wrong}; nothing was attached")))?;
        let mut outcome = Outcome::Inconclusive;
        let mut hints = Vec::new();
        if req.verify_now {
            match self.check(&claim, &policy, now).await {
                Ok(finding) => {
                    outcome = finding.outcome;
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

        // The outcome moves the claim on from the status it has when the attestation commits,
        // so if another write lands in its scope first, read the claim again and decide again.
        let mut read = Some((claim, head));
        for _ in 0..WRITE_ATTEMPTS {
            let (claim, head) = match read.take() {
                Some(read) => read,
                None => self.attestable(caller, &req.claim_id)?,
            };
            let entries = attestation_entries(caller, &claim, evidence.clone(), outcome, now)?;
            let mut claims = Claims::from([(claim.id.clone(), claim)]);
            let projections = entries
                .iter()
                .map(|entry| projector::project(entry.kind, &entry.content, &mut claims))
                .collect::<Result<_>>()?;
            let commit = Commit {
                entries,
                projections,
            };
            if !self.store.commit_at(&caller.tenant, &[head], commit)? {
                continue;
            }
            let claim = claims.remove(&req.claim_id).expect("the attested claim");
            let standing = trust::standing(&claim, &policy, now);
            return Ok(AttestOutcome {
                claim_id: claim.id,
                outcome,
                status: claim.status,
                reason: claim.reason,
                trusted: standing.trusted,
                trust: standing.trust,
                evidence,
                hints,
            });
        }
        Err(Error::Busy(format!(
            "other writes kept landing in {}'s scope while it was attested, {WRITE_ATTEMPTS} \
             times; try again",
            req.claim_id
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

    /// Recalls the claims that hold now and match the query, best match first, each scored at
    /// the time of the read under the policy in effect for the caller.
    pub async fn recall(&self, caller: &Caller, req: Recall) -> Result<ReadPack> {
        caller.require(Permission::Read)?;
        let now = self.clock.now();
        let limit = req.limit.unwrap_or(DEFAULT_RECALL_LIMIT);
        let policy = self.policy_in(&caller.tenant, caller.bounds.project.as_deref())?;
        let claims =
            self.store
                .search(&caller.tenant, &caller.visible_scopes(), &req.query, limit)?;

        let answer_set = claims
            .into_iter()
            .map(|claim| {
                let standing = trust::standing(&claim, &policy, now);
                RecalledClaim {
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
                }
            })
            .collect::<Vec<_>>();
        Ok(ReadPack {
            intent: ReadIntent::Current,
            guidance: guidance(&answer_set, req.high_stakes),
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
            projections: vec![projector::project(
                entry.kind,
                &entry.content,
                &mut Claims::new(),
            )?],
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

/// The entries that record an attestation of `claim`: the evidence attached, then the status the
/// outcome moves the claim on to, if any.
fn attestation_entries(
    caller: &Caller,
    claim: &Claim,
    evidence: Vec<Evidence>,
    outcome: Outcome,
    at: OffsetDateTime,
) -> Result<Vec<NewEntry>> {
    let entry = |kind, content: Result<Value, serde_json::Error>| -> Result<NewEntry> {
        Ok(NewEntry {
            id: NewEntry::generate_id(at),
            scope: claim.scope.clone(),
            kind,
            actor: caller.actor.clone(),
            channel: caller.channel,
            recorded_at: at,
            content: content.map_err(internal)?,
        })
    };
    let claim_id = claim.id.clone();
    let mut entries = vec![entry(
        EntryKind::EvidenceAttached,
        serde_json::to_value(EvidenceAttached {
            claim_id: claim_id.clone(),
            evidence,
        }),
    )?];
    match status_after(claim.status, outcome) {
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

/// What a claim's verifier found.
struct Finding {
    outcome: Outcome,
    /// The evidence that records it.
    evidence: Evidence,
    /// Why the claim didn't pass, for whoever wrote it.
    hint: Option<String>,
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
