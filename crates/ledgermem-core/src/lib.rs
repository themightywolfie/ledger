//! The Ledger engine: domain types, integrity rules and the ports its adapters implement.
//!
//! This crate has no database, HTTP or model dependencies. Surfaces build a [`Caller`] and call
//! the [`Ledger`]; adapters implement [`Store`], [`Clock`] and [`Verifier`].

mod caller;
mod claim;
mod clock;
mod error;
mod gate;
mod journal;
mod ledger;
mod ops;
mod policy;
mod projector;
mod store;
mod trust;
mod verifier;

pub use caller::{Caller, Channel, Permission, ScopeBounds};
pub use claim::{
    Claim, ClaimId, Evidence, EvidenceKind, RetractionReason, Scope, ScopeLevel, Source,
    SourceKind, Status,
};
pub use clock::{Clock, SystemClock, TestClock};
pub use error::{Error, Result};
pub use journal::{EntryKind, JournalEntry, NewEntry, StoredEntry, genesis_hash};
pub use ledger::{LOCAL_TENANT, Ledger};
pub use ops::{
    ApplyPolicy, Attest, AttestOutcome, BrokenLink, ChainProblem, ChainReport, ClaimInput,
    ClaimResult, EvidenceInput, EvidenceInputKind, Guidance, GuidanceCode, JournalQuery,
    PolicyApplied, PolicyQuery, ReadIntent, ReadPack, Recall, RecalledClaim, ReindexReport,
    RelayedUserStatement, Remember, RememberOutcome, ReportedSource, ScopeVerification,
    Verification,
};
pub use policy::{
    AgentClaims, AppliedVersion, Authority, Cardinality, FreshnessClass, FreshnessIntervals,
    Interval, OnUnresolved, Policy, PolicyDocument, PolicyVersion, Predicate, PredicatePack,
    ResolutionPolicy, Subjects, TrustPolicy, ValueType, VerifierName, VerifierRef,
};
pub use store::{Commit, JournalHead, Projection, Store, StoreError, StoredJournal};
pub use trust::{Trust, TrustComponents};
pub use verifier::{Check, Observation, Outcome, ScriptedVerifier, Verifier, Verifiers, Verifying};
