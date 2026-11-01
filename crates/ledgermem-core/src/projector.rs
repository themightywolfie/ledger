//! The projector: turns journal entries into the projection updates reads are served from
//! (ADR-0010). Live writes and reindex both go through it, so a rebuild lands exactly where the
//! writes did.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claim::{Claim, ClaimId, Evidence, RetractionReason, Status};
use crate::error::{Error, Result, internal};
use crate::journal::EntryKind;
use crate::store::Projection;

/// What an `evidence_attached` entry records.
#[derive(Serialize, Deserialize)]
pub(crate) struct EvidenceAttached {
    pub claim_id: ClaimId,
    pub evidence: Vec<Evidence>,
}

/// What a `status_changed` entry records.
#[derive(Serialize, Deserialize)]
pub(crate) struct StatusChanged {
    pub claim_id: ClaimId,
    pub status: Status,
}

/// What a `claim_retracted` entry records.
#[derive(Serialize, Deserialize)]
pub(crate) struct ClaimRetracted {
    pub claim_id: ClaimId,
    pub reason: RetractionReason,
}

/// The claims projected so far, so that an entry about a claim already written can be applied
/// to it.
pub(crate) type Claims = BTreeMap<ClaimId, Claim>;

/// The projection update one journal entry implies, given the claims projected before it.
pub(crate) fn project(kind: EntryKind, content: &Value, claims: &mut Claims) -> Result<Projection> {
    let claim = match kind {
        EntryKind::PolicyApplied => return Ok(Projection::PutPolicy(read(content)?)),
        EntryKind::ClaimWritten => {
            let claim: Claim = read(content)?;
            let id = claim.id.clone();
            claims.insert(id.clone(), claim);
            written(claims, &id)?
        }
        EntryKind::EvidenceAttached => {
            let attached: EvidenceAttached = read(content)?;
            let claim = written(claims, &attached.claim_id)?;
            claim.evidence.extend(attached.evidence);
            claim
        }
        EntryKind::StatusChanged => {
            let changed: StatusChanged = read(content)?;
            let claim = written(claims, &changed.claim_id)?;
            claim.status = changed.status;
            claim
        }
        EntryKind::ClaimRetracted => {
            let retracted: ClaimRetracted = read(content)?;
            let claim = written(claims, &retracted.claim_id)?;
            claim.status = Status::Retracted;
            claim.reason = Some(retracted.reason);
            claim
        }
    };
    Ok(Projection::PutClaim(claim.clone()))
}

/// The claim an entry is about, which an earlier entry must have written.
fn written<'a>(claims: &'a mut Claims, id: &ClaimId) -> Result<&'a mut Claim> {
    claims.get_mut(id).ok_or_else(|| {
        Error::Internal(format!(
            "the journal changes claim {id} before any entry writes it"
        ))
    })
}

fn read<T: DeserializeOwned>(content: &Value) -> Result<T> {
    serde_json::from_value(content.clone()).map_err(internal)
}
