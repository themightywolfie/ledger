//! The Ledger engine: domain types, integrity rules and the ports its adapters implement.
//!
//! This crate has no database, HTTP or model dependencies. Surfaces build a [`Caller`] and call
//! the [`Ledger`]; adapters implement [`Store`] and [`Clock`].

mod caller;
mod claim;
mod clock;
mod error;
mod journal;
mod ledger;
mod ops;
mod store;

pub use caller::{Caller, Channel, Permission, ScopeBounds};
pub use claim::{Claim, ClaimId, Scope, ScopeLevel, Source, SourceKind, Status};
pub use clock::{Clock, SystemClock, TestClock};
pub use error::{Error, Result};
pub use journal::{EntryKind, JournalEntry, NewEntry, StoredEntry, genesis_hash};
pub use ledger::{LOCAL_TENANT, Ledger};
pub use ops::{
    BrokenLink, ChainProblem, ChainReport, ClaimInput, ClaimResult, JournalQuery, ReadIntent,
    ReadPack, Recall, RecalledClaim, ReindexReport, Remember, RememberOutcome, ScopeVerification,
};
pub use store::{Commit, JournalHead, Projection, Store, StoreError};
