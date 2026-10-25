# Ledger

**The memory layer that checks before it believes.**

Ledger is an open-source memory layer for AI agents. Recalling a fact is only half its job. The other half is knowing whether the fact is still true, who said it, and whether anyone checked. Every memory is a **claim** with a source, evidence, a trust score and two time axes, and Ledger:

- **gates writes.** An agent's "tests pass" or "migration done" stays *proposed* until a verifier confirms it against the real system: a test run, Git, GitHub, an HTTP endpoint, a file, a person, or your own check in any language.
- **resolves conflicts.** Contradictions are caught when they are written, settled by explicit, versioned rules, and otherwise marked *contested* for a person to decide. Nothing is silently overwritten, and every change is an entry in a hash-chained journal.
- **routes reads by version.** A question about now gets the current value, and a question about the past gets the value that held then. Every answer comes with its status, trust, age and conflicts, so the agent can tell what is known from what is merely believed.

It ships as a single Rust binary, `ledgermem`. It includes an MCP server (stdio and Streamable HTTP, MCP 2026-07-28), a REST API, a CLI, and a small web app, the **memory visualizer**, where you can see, correct, forget, back up and restore memories.

> **Status: design phase.** The design is written and agreed, but there is no code yet. The specs and tickets below are the build plan. The commands below show the intended experience and don't work yet.

## The intended experience

```bash
cargo install ledgermem
ledgermem init          # writes config, applies predicate packs, prints MCP config for your agent
```

```jsonc
// Claude Code, Codex or Cursor: MCP config
{ "ledger": { "command": "ledgermem", "args": ["mcp", "--project", "billing-api", "--agent", "claude-code"] } }
```

```bash
ledgermem serve         # REST + MCP over HTTP + the memory visualizer on http://127.0.0.1:7437
```

## Documentation

| Read this | To learn |
| --- | --- |
| [GLOSSARY.md](GLOSSARY.md) | The domain language: claim, evidence, attestation, journal, trusted, shadowed, … |
| [docs/design/architecture.md](docs/design/architecture.md) | How it's built: one deep engine module, ports and adapters, surfaces, workspace layout, storage. |
| [docs/design/claims-and-integrity.md](docs/design/claims-and-integrity.md) | Every rule: lifecycle, trust scoring, relations, resolution policy, read routing, pre-action checks, journal hashing. |
| [docs/design/contracts.md](docs/design/contracts.md) | The public contracts: MCP tools, REST, CLI, config, policy file, exec verifier protocol, backup format. |
| [docs/adr/](docs/adr/) | Architecture decisions and why they were made. |
| [.scratch/ledger-core/spec.md](.scratch/ledger-core/spec.md) | The v1 spec for the engine, verifiers, MCP, REST and CLI. |
| [.scratch/memory-visualizer/spec.md](.scratch/memory-visualizer/spec.md) | The v1 spec for the memory visualizer. |

## Repository layout (planned)

```
crates/ledgermem-core        engine, domain types, ports (no database, HTTP or model dependencies)
crates/ledgermem-sqlite      SQLite store adapter
crates/ledgermem-verifiers   built-in verifiers and the exec protocol
crates/ledgermem-models      embedders and optional language-model providers
crates/ledgermem-server      REST, MCP and the embedded visualizer
crates/ledgermem             the binary and CLI
crates/ledgermem-scenarios   integrity scenario suite (not published)
web/                         the memory visualizer (React + TypeScript)
policies/                    predicate packs
```

## How this project is planned

Design work follows a grill → spec → tickets flow. Decisions are made in conversation, domain terms are recorded in `GLOSSARY.md`, and hard-to-reverse choices become ADRs. Each feature then gets a spec under `.scratch/<feature>/spec.md`.

Tickets are tracked on the [Ledger Trello board](https://trello.com/b/mXDl1YY8/ledger). Each one is a thin end-to-end slice, numbered in dependency order and listing its blockers, with its acceptance criteria as a checklist. Conventions are in [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md).

## License

[Apache-2.0](LICENSE).
