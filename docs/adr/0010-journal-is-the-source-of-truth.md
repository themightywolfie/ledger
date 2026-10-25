# The journal is the source of truth; claims and the recall index are projections of it

Every change to memory is first appended to a scope's journal (ADR-0003). The claim tables Ledger reads from, including current status, version chains, open conflict sets and the full-text and vector recall indexes, are projections built from journal entries, and `ledgermem reindex` can rebuild them from scratch. Restore is therefore "replay the journal into an empty tenant", and a backup is a copy of journals plus the policy and evidence metadata they reference, never a dump of derived tables.

## Considered Options

- Treating the claim tables as primary and the journal as an audit log: simpler queries, but restore would need two formats to agree and nothing would guarantee the audit log is complete.

## Consequences

- Projections must be deterministic functions of the journal plus the policy version each entry recorded. A projection that reads the wall clock or the live policy is a bug.
- Trust is never stored as truth. The stored components are re-scored at read time (see the trusted status in GLOSSARY.md).
- A schema change to a projection is a reindex, not a migration of user data.
