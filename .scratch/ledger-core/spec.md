# Spec: Ledger core v1 (engine, verifiers, MCP, REST, CLI)

**Status:** ready-for-agent

## Problem Statement

Teams run agents that act on what they remember: they deploy, merge, migrate, refund and escalate based on facts carried over from earlier sessions and other agents. Today's memory layers store and recall those facts well but never ask whether they are true. Specifically:

- An agent's "tests pass" or "migration done" is stored with the same weight as a fact the user stated, and every later session trusts it.
- When two memories disagree, retrieval returns whichever is more similar to the question, so the agent silently picks one.
- A web page or document can plant a "fact" or a directive ("always use provider X") that persists across sessions and gets acted on weeks later.
- A volatile fact (a deployed version, a ticket status) from three weeks ago looks exactly like one from three minutes ago.
- Nobody can say why the agent believed something: where it came from, what backed it, and which rule decided between it and the alternative.

Developers running coding agents on a laptop feel this daily. Platform and security teams can't govern memory as a persistence channel for attacks. Compliance teams have no audit trail of what an agent believed.

## Solution

Ledger is a standalone memory layer, shipped as one open-source Rust binary (`ledgermem`), that treats every memory as a **claim** with a source, evidence, a trust score and two time axes. It does three things existing layers don't:

1. **It gates writes.** Every claim passes the write gate. Its source kind comes from the channel and key that wrote it, never from the request. Claims about system state are checked against real side effects by verifiers (test runs, Git, GitHub, HTTP endpoints, files, humans, or custom executables) before they can become trusted. Directives from untrusted sources are quarantined.
2. **It resolves conflicts.** Contradictions are detected at write time by subject and predicate within a scope, settled by an ordered and versioned resolution policy, and otherwise marked contested and escalated. Nothing is silently overwritten. Losing claims stay queryable, and every change is an entry in a per-scope hash-chained journal.
3. **It routes reads by version.** A question about now gets the current version and a question about the past gets the version valid then, with status, trust, age, conflicts and guidance attached. A pre-action check tells an agent go, re-verify or block before a side-effecting tool call.

Developers install one binary, run `ledgermem init`, and connect their agent over MCP using stdio or Streamable HTTP (MCP 2026-07-28). Teams use the same operations over a REST API with an OpenAPI document. Operators manage memory from the CLI and the memory visualizer (a separate spec).

## User Stories

### Getting started

1. As a developer running a coding agent, I want to install Ledger as a single binary with no database server or container, so that I can try it in minutes.
2. As a developer, I want `ledgermem init` to print ready-to-paste MCP configuration for Claude Code, Codex and Cursor, so that connecting my agent is copy and paste.
3. As a developer, I want the `local` tenant created automatically on first launch, so that I don't have to administer anything to start.
4. As a developer, I want to bind the stdio MCP server to a project, user and agent through launch arguments, so that the model never fills in scope on each call.
5. As a platform engineer, I want to launch the MCP server over Streamable HTTP per the MCP 2026-07-28 stateless spec, so that remote agents can reach Ledger through a plain load balancer.
6. As a platform engineer, I want `--tenant` to be optional when launching either transport, so that I can run a single-tenant server pinned to one tenant or a multi-tenant server where each key decides.
7. As a platform engineer, I want to create tenants explicitly from the CLI, so that tenants other than `local` exist only when I decide.
8. As a platform engineer, I want `ledgermem serve` to expose REST, MCP and the visualizer on one localhost port, so that one process covers every surface.
9. As a developer on Windows, macOS or Linux, I want the database to live in the platform's data directory by default, so that Ledger behaves like other tools on my machine.
10. As a developer, I want `ledgermem demo` to create a tenant loaded with realistic sample memory, so that I can explore Ledger before connecting an agent.

### Writing claims

11. As an agent, I want to propose structured claims (subject, predicate, value and a one-sentence text), so that Ledger detects conflicts by key rather than by similarity.
12. As an agent, I want to attach evidence for what I observed through a tool, so that my claims can be verified and trusted.
13. As an agent, I want each write's response to tell me the claim's status, trust, relation to existing claims and any conflict set, so that I know whether to rely on it.
14. As an agent, I want a hint when I use an undeclared predicate that resembles a declared one, so that I converge on the shared vocabulary.
15. As an agent, I want to write up to 20 claims in one call, so that writing back after a task is one round trip.
16. As an agent, I want to declare that a claim refines an existing one, so that "uses Postgres 16 on RDS" is linked to "uses Postgres" instead of contradicting it.
17. As an agent, I want to give a claim a valid time when I know it, so that later questions about the past get correct answers.
18. As an agent, I want to relay the user's exact words when they tell me something about themselves, so that their preferences are remembered as theirs.
19. As an agent, I want to mark that a fact came from a web page, a document or another agent, so that Ledger trusts it no more than it deserves.
20. As an engineering lead, I want an agent's unverified "tests pass" to stay proposed, so that it never becomes shared truth.
21. As an engineering lead, I want a claim that its verifier refutes to be retracted with the evidence attached, so that false completion reports are visible as false.
22. As an agent builder, I want claims on undeclared predicates accepted conservatively rather than rejected, so that memory isn't lost while the policy catches up.

### Poisoning defenses

23. As a security engineer, I want every claim's source kind derived from its channel and key, never from the request, so that injected content can't pass itself off as the user.
24. As a security engineer, I want directives arriving from web, document or other-agent sources quarantined, so that "always use provider X" can't become a preference.
25. As a security engineer, I want low-trust sources unable to set user-authority predicates, so that a document can't declare the user an admin or VIP.
26. As a security engineer, I want repeats from the same actor to count once and agents never to corroborate each other, so that fabricated consensus can't raise trust.
27. As a security engineer, I want relaying user statements to require an explicit permission that is off by default on HTTP keys, so that remote agents can't speak for users unless I allow it.
28. As an operator, I want to release a quarantined claim only by deliberately recording it as my own statement, so that nothing leaves quarantine by accident.

### Multi-agent memory

29. As an orchestrator, I want sub-agents to hold proposer keys whose claims land as proposed in the shared project, so that I see their reports labelled unverified without them becoming truth.
30. As an orchestrator, I want to promote a sub-agent's claim by attesting it, so that verified work becomes shared knowledge.
31. As an orchestrator, I want claims from different agents distinguishable by actor, so that I know which sub-agent said what.

### Verification

32. As an agent, I want cheap verifiers to run inline during a write, so that a correct claim is trusted straight away.
33. As an agent, I want slow verifiers to run in the background, or as an MCP task when my client supports Tasks, so that my write isn't blocked.
34. As an agent, I want to ask Ledger to verify a claim now, so that I can refresh a stale fact before relying on it.
35. As a developer, I want a test-run verifier that reads the JUnit report of a configured repo, so that "tests pass" is checked against the actual run rather than the agent's words.
36. As a developer, I want to opt in to the test-run verifier re-running tests asynchronously, so that I can get a fresh result when the report is old.
37. As a developer, I want a Git verifier for commits, branches, merges and file content in my local repo, so that "pushed", "merged" and "the file now says X" are checked.
38. As a developer, I want a GitHub verifier for pull request state and CI status, so that "PR merged" and "CI green" are checked against GitHub.
39. As a platform engineer, I want an HTTP verifier that issues read-only GETs to endpoints I register, so that service state is checked against our own APIs.
40. As a developer, I want a file verifier for existence, contents and hashes, so that claims about files are checked.
41. As a platform engineer, I want to register custom verifiers as executables in any language speaking a documented JSON protocol, so that we can verify domain-specific claims without forking Ledger.
42. As a platform engineer, I want verifier credentials kept in server config and environment variables, never in policy or the journal, so that backups and policies hold no secrets.
43. As a reviewer, I want claims whose verifier is `human` held as proposed with an open question, so that I can confirm or reject them.
44. As an end user, I want Ledger to ask me directly through my MCP client when a fact needs my confirmation, so that confirmations really come from me.
45. As a platform engineer, I want an optional LLM-judge verifier whose verdicts are labelled and capped at a lower trust ceiling, so that claims without a real verifier get some signal without being over-trusted.
46. As a platform engineer, I want verifier timeouts and failures recorded as inconclusive rather than as passes or errors, so that a flaky dependency never promotes a claim.

### Conflicts and resolution

47. As an agent builder, I want conflicts detected at write time by subject and predicate within a scope, so that contradictions are caught before anyone reads them.
48. As an agent builder, I want duplicates merged as corroboration rather than stored twice, so that memory doesn't fill with repeats.
49. As an end user, I want a newer statement about where I live to supersede the old one while keeping it as history, so that "where did I live before Mumbai?" has an answer.
50. As an engineering lead, I want evidence-backed claims to beat bare assertions, so that a CI result beats an agent's "tests pass".
51. As a platform engineer, I want a predicate's system of record to beat any agent's narration, so that our status API decides what is deployed.
52. As an end user, I want my own word to decide my preferences and identity, so that documents and other agents can't override me.
53. As an agent builder, I want claims the policy can't settle marked contested, never silently overwritten, so that disagreements are visible and nothing is lost.
54. As a reviewer, I want contested sets re-verified automatically first and only then escalated to me as an open question, so that I see only what machines couldn't settle.
55. As a reviewer or orchestrator with resolve permission, I want to settle a conflict set by naming the winner, so that agents can move on.
56. As a developer, I want every resolution recorded with the rule and policy version that decided it, so that I can audit why the agent believes something.
57. As an agent builder, I want a project's claim to shadow an org default for the same predicate, with the shadowed claim reported, so that legitimate overrides aren't conflicts but are never silent.
58. As a team lead, I want to choose what happens when no rule decides (contested, ask the user, or prefer the newer claim), so that the policy fits our risk appetite.

### Reading

59. As an agent, I want recall to return claims with status, trust, age, conflicts and guidance, so that I can tell what is known from what is believed and what is disputed.
60. As an agent, I want to ask for a value as of a past moment, so that "what did we decide last Tuesday?" returns that version.
61. As an agent, I want the version chain for a subject and predicate, so that I can roll back to "the release before the current one".
62. As an agent, I want to ask what changed since a moment, so that I can summarise changes since the last release.
63. As an agent, I want the read intent inferred from my question when I don't give one, so that simple questions need no extra parameters.
64. As an agent, I want stale volatile claims flagged, with re-verification queued automatically, so that a three-week-old deploy status isn't treated as current.
65. As an agent making a high-stakes read, I want Ledger to abstain and return both candidates when the answer is contested, stale or unverified, so that I check or ask instead of guessing.
66. As an agent, I want recall to match by keyword, by meaning and by subject or alias, so that I find claims however I phrase the question.
67. As an agent builder, I want recall to answer within 150 ms at p95 locally, so that memory doesn't slow my agent down.
68. As a framework integrator, I want each returned claim marked injectable or not, so that my adapter puts only fresh, verified, uncontested claims into instructions.
69. As an agent, I want every trust score returned with its components, so that I can explain to the user why I believe something.
70. As an auditor, I want to ask what Ledger believed at a past recorded time, so that I can reconstruct an agent's view when it acted.
71. As an agent, I want session-scoped claims visible only when I name my session, so that one run's scratch work doesn't leak into others.

### Pre-action check

72. As an agent, I want to check a planned side-effecting action against the claims it relies on, so that I don't deploy, merge or refund on bad memory.
73. As an agent, I want stale or proposed relied-on claims re-verified inline within a time budget, so that a go is based on fresh evidence.
74. As an agent, I want a block to tell me the current value when my claim has been superseded, so that I can correct course in one step.
75. As an agent, I want to name relied-on claims by subject and predicate as well as by id, so that I can check without recalling ids first.
76. As an agent, I want a reason for every relied-on claim in the verdict, so that I can explain a re-verify or a block to the user.

### Retraction and forgetting

77. As an end user, I want to retract something I said about myself through my agent, so that it is never used again.
78. As an end user, I want to ask my agent to forget something about me, so that its content is gone for good.
79. As a compliance officer, I want every forget recorded as a tombstone with actor and reason, so that removals are auditable without keeping the data.
80. As a compliance officer, I want forgotten content purged from recall indexes and evidence payloads, so that no path can recall it.
81. As a compliance officer, I want restores to re-apply every forget the instance knows about, so that forgotten data doesn't come back.

### Journal

82. As a security engineer, I want every change appended to a per-scope hash-chained journal, so that tampering is detectable.
83. As a platform engineer, I want `ledgermem verify` to report the first broken link in any chain, so that I can detect corruption or tampering.
84. As a platform engineer, I want `ledgermem reindex` to rebuild all derived data from the journal, so that a projection bug never loses memory.
85. As a compliance officer, I want forgetting to leave every chain verifiable, so that compliance with deletion requests doesn't destroy tamper evidence.

### Policy

86. As a team lead, I want our predicate policy kept as a YAML file in our repo and applied with one command, so that memory rules are versioned with the project.
87. As a team lead, I want predicate packs for software delivery and personal assistants, so that we start from a good vocabulary.
88. As a team lead, I want a project overlay on the tenant policy, so that projects can add or override predicates.
89. As a team lead, I want an invalid policy rejected whole with the offending path named, so that a typo never half-applies.
90. As an operator, I want to declare subject aliases in the policy, so that "billing-api", "billing" and the repo resolve to one subject.
91. As an operator, I want Ledger to suggest likely aliases without merging anything, so that I decide every merge.
92. As an agent, I want to read the active policy as an MCP resource, so that I use declared predicates.

### Backup and restore

93. As an operator, I want to back up a tenant to one portable file in a documented format, so that I can move or recover memory.
94. As an operator, I want restore to refuse a backup whose chains don't verify, so that I never restore tampered memory.
95. As an operator, I want restore to go only into an empty tenant, so that a restore never silently mixes with existing memory.
96. As an operator, I want backups to exclude tenants and API keys, so that a backup file never leaks credentials.
97. As an operator, I want a restore report (scopes, entries, claims, forgets re-applied, warnings), so that I know exactly what came back.

### Access control

98. As a platform engineer, I want API keys bound to one tenant with separate permissions, so that a coding agent can propose without being able to resolve or forget.
99. As a platform engineer, I want any request outside a key's tenant or scope bounds refused at the server, so that isolation never depends on prompts.
100. As a platform engineer, I want keys shown once and stored hashed, so that a leaked database doesn't leak credentials.
101. As a platform engineer, I want the HTTP server bound to localhost unless I opt in, so that I don't expose memory by accident.

### Surfaces

102. As an integrator, I want a REST API that mirrors the MCP tools, with an OpenAPI document, so that I can generate clients and build framework adapters.
103. As an integrator, I want identical behaviour over MCP and REST, so that integrity never depends on the surface.
104. As an agent, I want a `memory-usage` prompt that teaches how to treat each status, so that I use memory correctly without custom instructions.
105. As an agent, I want tool errors returned as readable results with stable codes, so that I can recover from them.
106. As a script author, I want every CLI command to support `--json`, so that I can automate Ledger.
107. As an operator, I want CLI commands to list and answer open questions, so that I can review without a browser.

### Recall models

108. As a developer, I want local embeddings by default with no API key, so that semantic recall works offline after the first model download.
109. As a developer, I want to point embeddings at an OpenAI-compatible endpoint such as Ollama, or turn them off, so that I control cost and privacy.
110. As a developer, I want recall to keep working with full-text and subject matching when the embedding model is unavailable, so that recall degrades instead of failing.

### Contributors

111. As an open-source contributor, I want behaviour described by a readable integrity scenario suite, so that I can change internals confidently.
112. As a contributor, I want the engine to compile without database, HTTP or model dependencies, so that integrity logic stays in one place.
113. As a contributor, I want CI to run the scenario suite, adapter conformance tests and latency checks on Windows, macOS and Linux, so that I learn about regressions before release.

## Implementation Decisions

The design documents (see Further Notes) hold the exact rules and contracts. These are the decisions this spec commits to.

**Modules**

- **Ledger engine**: the one deep module. Its interface is the seven memory operations (remember, attest, recall, check before act, history, resolve, forget), the review operations (browse, claim detail, answer question, apply policy) and the administration operations (backup, restore, verify journals, tenants and keys). Every operation takes a Caller (tenant, actor, channel, permissions, scope bounds) built by a surface, never by the model. The write gate, conflict engine, trust scorer, read planner, policy, projector, journal, job runner and guard are internal to it.
- **Ports**, each with at least two adapters: Store (SQLite in v1, Postgres next), Verifier (the built-ins, exec and a scripted test adapter), Embedder (local ONNX, OpenAI-compatible, none, deterministic test adapter), Language-model provider (OpenAI-compatible, Anthropic, scripted, off by default), Clock (system and test). Nothing in the engine reads the wall clock or issues SQL.
- **Surfaces** are thin adapters: MCP (stdio and Streamable HTTP via rmcp, targeting the 2026-07-28 spec, with the Tasks extension and elicitation), REST (axum, OpenAPI generated from the same types as the MCP schemas), and the CLI. None contains integrity logic.
- **Workspace**: separate crates for the engine, the SQLite store, the verifiers, the models, the server surfaces and the binary, plus an unpublished scenario-suite crate. The engine crate's dependency list enforces the seams.

**Architectural decisions** (each recorded as an ADR)

- SQLite first, behind a store interface expressed in engine terms, so that Postgres can follow (ADR-0001).
- Callers write structured claims. No model on the write path in v1. Relation classification by fixed rules (ADR-0002).
- Memory changes only by appending to a per-scope hash-chained journal, with chains committing to content hashes so a forget preserves verifiability (ADR-0003).
- The journal is the source of truth, and claims, conflict sets, questions and recall indexes are deterministic projections rebuilt by reindex and restore (ADR-0010).
- Sub-agents propose into the real scope with proposer keys. No staging scope (ADR-0004).
- The narrowest scope shadows across scopes. Conflicts are detected only within one scope (ADR-0005).
- Source kind is derived from channel and key, and relaying user statements needs `relay_user`: on for local stdio, off for HTTP keys (ADR-0009).
- Custom verifiers use the versioned exec JSON protocol, unsandboxed in v1, and documented as such (ADR-0006).
- The crates and binary are named `ledgermem`, the product Ledger (ADR-0008). Licensed Apache-2.0.

**Domain rules** (exact values in the claims-and-integrity design doc)

- Lifecycle statuses: proposed, verified, contested, superseded, retracted, quarantined, plus forgotten tombstones. "Trusted" and "injectable" are computed at read time, never stored.
- Trust score = max(source prior, best attestation strength) + bounded corroboration − contradiction penalty, scaled down when stale, capped when only an LLM judge vouches. The trusted threshold defaults to 0.80, and every score carries its components.
- Relation rules in order: declared refinement, duplicate, multi-valued compatible, disjoint-interval history, temporal update, contradiction. Resolution rules in order: evidence beats assertion, system of record, authority, recency with evidence, trust margin, then the configured fallback.
- Freshness classes are stable, slow (90 days) and volatile (1 hour). Staleness is computed at read time. Recall queues re-verification and never waits. The pre-action check re-verifies inline within a 2-second budget.
- Read intents are current, as of, history and change, given explicitly or inferred by fixed rules. Recall fuses full-text, vector and subject matches by reciprocal rank fusion before version routing, shadowing and the integrity filter.
- Guidance codes are reliable, unverified, stale, unresolved and none. Pre-action verdicts are go, re-verify and block.
- Undeclared predicates are treated as single-valued and slow, with a nearest-predicate hint. Subject aliases come only from policy, never from agents.
- Value comparison is type-aware (text normalisation, semver, alias-resolved subject references).

**Contracts** (exact shapes in the contracts design doc)

- Exactly seven MCP tools, three resource families (conflicts, stale, policy) and one `memory-usage` prompt. Tool descriptions are shipped text written for models.
- REST mirrors the tools one-to-one under `/v1`, plus review and administration endpoints. Errors are a closed set mapped consistently to HTTP statuses and MCP tool errors.
- API keys have a kind (agent, integration, operator), which fixes the channel, plus explicit permissions (read, propose, attest, resolve, forget, relay_user, declare_source, operator, admin) and scope bounds. They are stored as argon2 hashes.
- Server config (TOML) holds storage, listen address, embedder, model provider and verifier instances with credentials referenced by environment variable. The policy (YAML) holds predicates, aliases and resolution settings, with no secrets, and is journaled per version.
- Backup bundle format `ledgermem.backup/1`: a gzip-compressed tar of the manifest, per-scope journals, evidence metadata, policy versions and the forget register. Restore into an empty tenant only.
- Exec verifier protocol `ledgermem.verifier/1`: claim and params on stdin, outcome, observed value and evidence on stdout, with a timeout and a stdout cap.

**Storage (SQLite adapter)**: truth tables (entries, evidence, policy versions, tenants, keys), projection tables (claims, conflict sets, questions, full-text index, vectors) and an operational job queue. A single writer per tenant assigns sequence numbers and chain hashes. Reads are concurrent. Vector search starts as an in-process cosine scan behind an internal seam of the adapter.

**Predicate packs**: software delivery (deployed version, test status, PR state, CI status, branch exists, migration status, owner, uses dependency, decision) and personal assistant (lives in, works at, dietary preference, timezone, preferred name, prefers). Each declares cardinality, value type, freshness, authority and a default verifier where one applies.

## Testing Decisions

**What makes a good test here**: it exercises behaviour through an interface a caller uses and asserts on observable outcomes (statuses, guidance, verdicts, journal contents, chain verification), never on internal tables or private functions. It survives an internal refactor unchanged. Time-dependent behaviour is tested by moving the test clock, never by sleeping.

**Seams under test**, highest first. These were agreed during design.

1. **The engine interface** carries almost everything. An **integrity scenario suite** runs YAML scenarios against an engine built with in-memory SQLite, the test clock, scripted verifiers and the deterministic embedder. Each scenario sets up policy and verifier outcomes, plays a sequence of operations as named callers, and asserts on outcomes. The suite covers every integrity event family from the product plan (temporal update, history-to-action, unresolved contradiction, unverified agent claim, external poisoning, staleness, multi-agent contamination, retraction) plus controls, forget, backup and restore round-trips, scope shadowing, tenant isolation, and permission refusals. The scenario format is the seed of the OutcomeMem benchmark. A scenario looks like this:

   ```yaml
   name: unverified "tests pass" is refuted, and a deploy that relies on it is blocked
   clock: 2026-10-25T09:00:00Z
   policy: { extends: [software-delivery] }
   callers:
     coder: { kind: agent, project: billing-api, permissions: [read, propose, attest] }
   verifiers:
     test_run: { "repo:acme/billing-api": { outcome: fail, observed: failing } }
   steps:
     - as: coder
       remember: { subject: "repo:acme/billing-api", predicate: test_status, value: passing, text: "all tests pass" }
       expect: { status: retracted, reason: refuted }
     - as: coder
       check_before_act:
         action: { tool: deploy, description: "deploy billing-api" }
         relies_on: [{ subject: "repo:acme/billing-api", predicate: test_status }]
       expect: { verdict: block }
   ```

2. **The store conformance suite** is one set of tests, written against the store port, that every store adapter must pass: append atomicity, sequence continuity, candidate lookup, hybrid search ordering, journal paging, job queue semantics. SQLite runs it now, and Postgres must pass it unchanged.
3. **Built-in verifiers** are each tested against real local stand-ins: a temporary Git repository, a loopback HTTP server, JUnit fixture files, temporary files, and a small exec verifier script. The GitHub verifier is tested against recorded API responses.
4. **The wire contract**: a small set of tests per surface checks that each MCP tool and REST route reaches its engine operation and that response JSON matches the published schema (snapshot tests on the generated schemas, plus one round-trip per tool). There are MCP tests over both stdio and Streamable HTTP, including a Tasks round-trip.
5. **The CLI**: smoke tests that run the binary against a temporary database for init, policy apply, backup, restore, verify, reindex and demo.

**Latency checks**: a load scenario with 100k claims records p95 recall and write-acknowledgement times in CI and fails on regressions against the targets.

**Prior art**: none in this repository yet (greenfield). The scenario style borrows from STATE-Bench's deterministic state assertions, and the store conformance suite follows the usual pattern of one shared trait-level test suite run against each adapter.

## Out of Scope

- The memory visualizer (its own spec, which consumes this one's REST API).
- The Postgres + pgvector store adapter (the next milestone; the store port and conformance suite are designed for it now).
- The Kubernetes verifier (the next milestone).
- gRPC, and the Python and TypeScript SDKs.
- Framework adapters for LangChain/LangGraph, Microsoft Agent Framework, LlamaIndex and Pydantic AI (the REST API and integration keys are designed for them).
- The OutcomeMem benchmark harness, baselines, and the paper (the scenario format seeds it).
- Free-text extraction from raw conversation turns (the model-provider port exists, but only the LLM judge uses it in v1).
- OAuth 2.1 authorization (API keys only in v1).
- Managed cloud, Helm charts and multi-node deployment.
- Per-scope encryption at rest and field-level PII redaction.
- Sandboxing of exec verifiers, and WASM verifiers.
- Merge-restore, and importers from Mem0 or Zep.
- Automatic subject merging, and scheduled staleness sweeps.

## Further Notes

- Design documents: `docs/design/architecture.md` (modules, seams, surfaces, workspace, storage), `docs/design/claims-and-integrity.md` (every rule and default), `docs/design/contracts.md` (MCP, REST, CLI, config, policy, exec protocol, backup format). Domain language: `GLOSSARY.md`. Decisions: `docs/adr/`.
- The product plan's success metrics apply to v1 acceptance: harmful-action reduction on planted scenarios, under 10% over-caution on controls, and the latency targets. Recall parity with Mem0 on LongMemEval and LoCoMo is measured once the OutcomeMem harness exists.
- rmcp 3.7 implements the 2026-07-28 spec, stateless Streamable HTTP and the Tasks extension. The first MCP ticket should confirm the Tasks and elicitation APIs against docs.rs before building on them.
- sqlite-vec is still alpha, which is why vector search starts as an in-process scan.
