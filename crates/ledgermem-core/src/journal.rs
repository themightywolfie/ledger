//! Journal entries: every change to memory is appended to its scope's journal first.
//!
//! Each entry is sealed into its scope's hash chain (ADR-0003). It commits to the hash of its
//! content and to the hash of the entry before it, and a scope's first entry links to the scope's
//! genesis hash. Hashes are SHA-256 over RFC 8785 canonical JSON, written `sha256:<hex>`.
//!
//! Sealing produces a [`StoredEntry`]: the exact text a store keeps and the hashes commit to.
//! Verification recomputes the hashes from that text without parsing it, so a tampered value that
//! no longer reads as an entry is reported as a broken link rather than failing the read.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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

/// An entry exactly as a store keeps it: the text its hashes commit to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEntry {
    pub id: String,
    /// The scope, written `level:id` or `org`.
    pub scope: String,
    pub seq: u64,
    pub kind: String,
    pub actor: String,
    pub channel: String,
    /// RFC 3339, in UTC.
    pub recorded_at: String,
    /// The content as RFC 8785 canonical JSON.
    pub content: Option<String>,
    pub content_hash: String,
    /// The hash of the entry before this one, or the scope's genesis hash for entry 1.
    pub prev_hash: String,
    /// The hash of the entry's header, which commits to every field above.
    pub entry_hash: String,
}

/// One entry in a scope's journal, numbered from 1 and sealed into the scope's hash chain, as
/// reads return it.
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
    pub content_hash: String,
    pub prev_hash: String,
    pub entry_hash: String,
}

impl NewEntry {
    pub(crate) fn generate_id(at: OffsetDateTime) -> String {
        format!("ent_{}", ulid::Ulid::from_datetime(at.into()))
    }

    /// Seals the entry into its scope's chain as entry `seq`, after the entry whose hash is
    /// `prev_hash`: the scope's [`genesis_hash`] when `seq` is 1.
    pub fn seal(self, seq: u64, prev_hash: String) -> StoredEntry {
        let content = serde_jcs::to_string(&self.content).expect("journal content is plain JSON");
        let mut entry = StoredEntry {
            id: self.id,
            scope: self.scope.to_string(),
            seq,
            kind: name_of(self.kind),
            actor: self.actor,
            channel: name_of(self.channel),
            recorded_at: self
                .recorded_at
                .to_offset(time::UtcOffset::UTC)
                .format(&Rfc3339)
                .expect("a clock time formats as RFC 3339"),
            content_hash: sha256(content.as_bytes()),
            content: Some(content),
            prev_hash,
            entry_hash: String::new(),
        };
        entry.entry_hash = entry.header_hash();
        entry
    }
}

impl StoredEntry {
    /// The entry's hash, recomputed from its header fields.
    fn header_hash(&self) -> String {
        let header = Header {
            id: &self.id,
            scope: &self.scope,
            seq: self.seq,
            kind: &self.kind,
            content_hash: &self.content_hash,
            prev_hash: &self.prev_hash,
            recorded_at: &self.recorded_at,
            actor: &self.actor,
            channel: &self.channel,
            policy_version: None,
            refs: &[],
        };
        let canonical = serde_jcs::to_vec(&header).expect("an entry header is plain JSON");
        sha256(&canonical)
    }

    /// Whether the stored content still matches the content hash.
    fn content_holds(&self) -> bool {
        self.content
            .as_ref()
            .is_some_and(|content| sha256(content.as_bytes()) == self.content_hash)
    }
}

impl TryFrom<StoredEntry> for JournalEntry {
    type Error = String;

    fn try_from(stored: StoredEntry) -> Result<Self, Self::Error> {
        let unreadable = |field: &str| {
            format!(
                "entry {} in {} has an unreadable {field}",
                stored.seq, stored.scope
            )
        };
        Ok(JournalEntry {
            scope: stored.scope.parse().map_err(|_| unreadable("scope"))?,
            kind: from_name(&stored.kind).ok_or_else(|| unreadable("kind"))?,
            channel: from_name(&stored.channel).ok_or_else(|| unreadable("channel"))?,
            recorded_at: OffsetDateTime::parse(&stored.recorded_at, &Rfc3339)
                .map_err(|_| unreadable("time"))?,
            content: match &stored.content {
                Some(content) => {
                    serde_json::from_str(content).map_err(|_| unreadable("content"))?
                }
                None => Value::Null,
            },
            id: stored.id,
            seq: stored.seq,
            actor: stored.actor,
            content_hash: stored.content_hash,
            prev_hash: stored.prev_hash,
            entry_hash: stored.entry_hash,
        })
    }
}

/// Re-derives a scope's chain from its entries, in sequence order, and returns the first entry
/// that doesn't verify.
pub(crate) fn verify_chain(scope: &str, entries: &[StoredEntry]) -> Option<BrokenLink> {
    let mut expected_prev = genesis_hash(scope);
    for (expected_seq, entry) in (1..).zip(entries) {
        let problem = if entry.seq != expected_seq {
            Some(ChainProblem::MissingEntry)
        } else if !entry.content_holds() {
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

/// The hash a scope's first entry links to: SHA-256 of `ledgermem:genesis:<scope>`, where the
/// scope is written `level:id` or `org`. It names the scope but not the tenant, so a journal still
/// verifies after it is restored into another tenant.
pub fn genesis_hash(scope: &str) -> String {
    sha256(format!("ledgermem:genesis:{scope}").as_bytes())
}

/// What an entry's hash commits to.
#[derive(Serialize)]
struct Header<'a> {
    id: &'a str,
    scope: &'a str,
    seq: u64,
    kind: &'a str,
    content_hash: &'a str,
    prev_hash: &'a str,
    recorded_at: &'a str,
    actor: &'a str,
    channel: &'a str,
    // Policy versions arrive with ticket #03, and references between entries later. Every entry
    // seals with none until then, so the entries written now keep verifying once they exist.
    policy_version: Option<&'a str>,
    refs: &'a [String],
}

/// The name a unit enum variant serializes to, such as `claim_written`.
fn name_of(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        _ => unreachable!("unit enum variants serialize as strings"),
    }
}

fn from_name<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(Value::String(name.to_owned())).ok()
}

fn sha256(bytes: &[u8]) -> String {
    let mut out = String::from("sha256:");
    for byte in Sha256::digest(bytes).iter() {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}
