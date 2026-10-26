//! The journal's hash format is a public contract (claims-and-integrity.md, section 11): backups
//! carry these hashes and other tools verify them. These values were worked out by hand, not by
//! this code: the canonical JSON was written per RFC 8785 and hashed with `sha256sum`.

use ledgermem_core::{Channel, EntryKind, NewEntry, Scope, ScopeLevel, genesis_hash};
use serde_json::json;
use time::macros::datetime;

/// Content that exercises RFC 8785: keys out of order, a key with a non-ASCII character, `4.50`
/// (written `4.5`), `1e30` (written `1e+30`), a literal array and non-ASCII text.
fn sealed_entry() -> ledgermem_core::StoredEntry {
    NewEntry {
        id: "ent_01JTEST".into(),
        scope: Scope::new(ScopeLevel::Project, "billing-api"),
        kind: EntryKind::ClaimWritten,
        actor: "agent:coder".into(),
        channel: Channel::Agent,
        recorded_at: datetime!(2026-10-26 09:00 UTC),
        content: json!({
            "value": 1e30,
            "text": "€ 4.50",
            "count": 4.50,
            "flags": [true, null],
            "Zeta": "z",
            "aé": 1,
        }),
    }
    .seal(1, genesis_hash("project:billing-api"))
}

#[test]
fn content_is_stored_as_rfc_8785_canonical_json() {
    assert_eq!(
        sealed_entry().content.as_deref(),
        Some(
            r#"{"Zeta":"z","aé":1,"count":4.5,"flags":[true,null],"text":"€ 4.50","value":1e+30}"#
        )
    );
}

#[test]
fn the_content_hash_is_sha256_of_the_canonical_content() {
    assert_eq!(
        sealed_entry().content_hash,
        "sha256:6e656b6c4a5b165d46a834fc18c5a6af42811070715858f39d5ef59e3dd65bed"
    );
}

#[test]
fn the_entry_hash_is_sha256_of_the_canonical_header() {
    // The header that hashes to this value:
    // {"actor":"agent:coder","channel":"agent","content_hash":"sha256:6e65…5bed",
    //  "id":"ent_01JTEST","kind":"claim_written","policy_version":null,
    //  "prev_hash":"sha256:d217…533f","recorded_at":"2026-10-26T09:00:00Z","refs":[],
    //  "scope":"project:billing-api","seq":1}
    assert_eq!(
        sealed_entry().entry_hash,
        "sha256:5c4c5f477263a455cd08bb2419072d4d996557ea5f68f0d754ac530d3f09519e"
    );
}
