//! Journal entries: every change to memory is appended to its scope's journal first.
//!
//! Entries are not hash-chained yet. Content hashes and chain hashes (ADR-0003) arrive with
//! ticket #02, before anything is released.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::caller::Channel;
use crate::claim::Scope;

/// What an entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    ClaimWritten,
}

/// An entry the engine has built and the store has not yet numbered.
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

/// One entry in a scope's journal, numbered from 1.
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
}

impl NewEntry {
    pub(crate) fn generate_id(at: OffsetDateTime) -> String {
        format!("ent_{}", ulid::Ulid::from_datetime(at.into()))
    }

    /// The entry as it reads once the store has given it its sequence number.
    pub fn numbered(self, seq: u64) -> JournalEntry {
        JournalEntry {
            id: self.id,
            scope: self.scope,
            seq,
            kind: self.kind,
            actor: self.actor,
            channel: self.channel,
            recorded_at: self.recorded_at,
            content: self.content,
        }
    }
}
