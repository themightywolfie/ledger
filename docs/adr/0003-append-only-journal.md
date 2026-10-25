# Memory changes only by appending entries to a hash-chained journal

Every scope has a journal: an append-only sequence of entries, each committing to the hash of the one before it. Nothing in Ledger, including the visualizer, edits or deletes a claim in place. A person "fixing" a memory writes a correction (a new user-stated claim that supersedes the old one), a retraction, a resolution or a forget, each as a new entry. Entries commit to a content hash rather than the content itself, so a forget can purge a claim's content and leave a tombstone while the chain still verifies.

## Considered Options

- Mutable rows plus an audit table: simpler to edit, but the audit trail can drift from the data and nothing proves it is complete.
- Chaining over the full content: tamper-evident, but a forget would break verification of every later entry.

## Consequences

- There is no edit or hard-delete endpoint, by design. Do not add one.
- Restore verifies every chain and refuses a backup that fails.
