//! Journal entries: every change to memory is appended to its scope's journal first.
//!
//! Each entry is sealed into its scope's hash chain (ADR-0003). It commits to the hash of its
//! content and to the hash of the entry before it, and a scope's first entry links to the scope's
//! genesis hash. Hashes are SHA-256 over RFC 8785 canonical JSON, written `sha256:<hex>`.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::caller::Channel;
use crate::claim::Scope;
use crate::ops::{BrokenLink, ChainProblem};

/// What an entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    ClaimWritten,
}

/// An entry the engine has built and the store has not yet sealed into its scope's chain.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEntry {
    pub id: String,
    pub scope: Scope,
    pub kind: EntryKind,
    pub actor: String,
    pub channel: Channel,
    pub recorded_at: OffsetDateTime,
    pub content: Value,
}

/// One entry in a scope's journal, numbered from 1 and sealed into the scope's hash chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub scope: Scope,
    pub seq: u64,
    pub kind: EntryKind,
    pub actor: String,
    pub channel: Channel,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
    pub content: Value,
    /// The hash of `content`.
    pub content_hash: String,
    /// The hash of the entry before this one, or the scope's genesis hash for entry 1.
    pub prev_hash: String,
    /// The hash of the entry's header, which commits to every field above.
    pub entry_hash: String,
}

impl NewEntry {
    pub(crate) fn generate_id(at: OffsetDateTime) -> String {
        format!("ent_{}", ulid::Ulid::from_datetime(at.into()))
    }

    /// Seals the entry into its scope's chain as entry `seq`, after the entry whose hash is
    /// `prev_hash`: the scope's [`genesis_hash`] when `seq` is 1.
    pub fn seal(self, seq: u64, prev_hash: String) -> JournalEntry {
        let mut entry = JournalEntry {
            content_hash: hash(&self.content),
            id: self.id,
            scope: self.scope,
            seq,
            kind: self.kind,
            actor: self.actor,
            channel: self.channel,
            recorded_at: self.recorded_at,
            content: self.content,
            prev_hash,
            entry_hash: String::new(),
        };
        entry.entry_hash = entry.header_hash();
        entry
    }
}

impl JournalEntry {
    /// The entry's hash, recomputed from its header fields.
    fn header_hash(&self) -> String {
        hash(&Header {
            id: &self.id,
            scope: &self.scope,
            seq: self.seq,
            kind: self.kind,
            content_hash: &self.content_hash,
            prev_hash: &self.prev_hash,
            recorded_at: self.recorded_at,
            actor: &self.actor,
            channel: self.channel,
            policy_version: None,
            refs: &[],
        })
    }
}

/// Re-derives a scope's chain from its entries, in sequence order, and returns the first entry
/// that doesn't verify.
pub(crate) fn verify_chain(scope: &Scope, entries: &[JournalEntry]) -> Option<BrokenLink> {
    let mut expected_prev = genesis_hash(scope);
    for (expected_seq, entry) in (1..).zip(entries) {
        let problem = if entry.seq != expected_seq {
            Some(ChainProblem::MissingEntry)
        } else if entry.content_hash != hash(&entry.content) {
            Some(ChainProblem::ContentHashMismatch)
        } else if entry.prev_hash != expected_prev {
            Some(ChainProblem::PrevHashMismatch)
        } else if entry.entry_hash != entry.header_hash() {
            Some(ChainProblem::EntryHashMismatch)
        } else {
            None
        };
        if let Some(problem) = problem {
            return Some(BrokenLink {
                seq: expected_seq,
                problem,
            });
        }
        expected_prev.clone_from(&entry.entry_hash);
    }
    None
}

/// The hash a scope's first entry links to: SHA-256 of `ledgermem:genesis:<scope>`. It names the
/// scope but not the tenant, so a journal still verifies after it is restored into another tenant.
pub fn genesis_hash(scope: &Scope) -> String {
    sha256(format!("ledgermem:genesis:{scope}").as_bytes())
}

/// What an entry's hash commits to.
#[derive(Serialize)]
struct Header<'a> {
    id: &'a str,
    scope: &'a Scope,
    seq: u64,
    kind: EntryKind,
    content_hash: &'a str,
    prev_hash: &'a str,
    #[serde(with = "time::serde::rfc3339")]
    recorded_at: OffsetDateTime,
    actor: &'a str,
    channel: Channel,
    // Policy versions arrive with ticket #03, and references between entries later. Every entry
    // seals with none until then, so the entries written now keep verifying once they exist.
    policy_version: Option<&'a str>,
    refs: &'a [String],
}

/// SHA-256 of the value's RFC 8785 canonical JSON.
fn hash(value: &impl Serialize) -> String {
    let canonical = serde_jcs::to_vec(value).expect("journal values are plain JSON");
    sha256(&canonical)
}

fn sha256(bytes: &[u8]) -> String {
    let mut out = String::from("sha256:");
    for byte in Sha256::digest(bytes).iter() {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}
