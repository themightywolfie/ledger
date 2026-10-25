# SQLite first, Postgres second, behind one store interface

The product plan specifies Postgres with pgvector, run as an embedded Postgres inside a container for local use. We ship SQLite first instead: one database file inside a single `ledgermem` binary, with SQLite full-text search for keyword recall and vector search for semantic recall. The first users are developers running coding agents on a laptop, and `cargo install ledgermem` plus one line of MCP config is far less friction than running a container. Postgres with pgvector follows as a second adapter behind the same store interface, for self-hosted teams that need a shared server.

## Consequences

- The store interface is a real seam with two adapters, so nothing above it may use SQLite-only features (no SQL in the engine).
- The tests run against SQLite in memory, which is also the shipping store, so the main test path exercises real storage.
- SQLite allows one writer at a time. This is fine for local and small-team use. Multi-node deployments need Postgres.
