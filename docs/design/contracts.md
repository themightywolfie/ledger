# Ledger contracts

Everything outside the `ledgermem` binary depends on: the MCP server, the REST API, the CLI, the server config, the policy file, the exec verifier protocol and the backup format. These are public and versioned. Changing one is a breaking change unless it only adds optional fields. Behaviour behind each contract is specified in [claims-and-integrity.md](claims-and-integrity.md). Terms follow [GLOSSARY.md](../../GLOSSARY.md).

## 1. MCP server

Targets MCP specification **2026-07-28** (stateless: no initialize handshake, no session id), implemented with `rmcp`. Both transports expose the same tools, resources and prompt.

### Transports and launch

| Command | Transport | Caller |
| --- | --- | --- |
| `ledgermem mcp` | stdio | Launch arguments (below). Channel `agent`. |
| `ledgermem mcp --transport http [--listen 127.0.0.1:7437]` | Streamable HTTP at `/mcp` | API key (`Authorization: Bearer lm_…`). |
| `ledgermem serve` | Streamable HTTP at `/mcp`, alongside REST and the visualizer | API key. |

| Launch argument | Default | Meaning |
| --- | --- | --- |
| `--tenant <id>` | `local` | The tenant to serve. Optional on both transports. Over HTTP, a key bound to a different tenant is refused. Omit it to let each key's tenant decide (multi-tenant server). The `local` tenant is created on first launch; any other tenant must exist (`ledgermem tenant create`). |
| `--user <id>` | `local` (stdio only) | User scope bound for this stdio server. |
| `--project <id>` | none | Project scope bound. Without it, writes default to user scope. |
| `--agent <id>` | `agent` | Actor id, and the agent scope. |
| `--no-relay-user` | relay allowed | Withhold `relay_user` from this stdio server (ADR-0009). |
| `--db <path>` | the platform data directory | SQLite database file. |
| `--config <path>` | `ledgermem.toml` search path | Server config (section 4). |

Stdio servers act with permissions `read, propose, attest, relay_user`. HTTP callers get exactly what their key grants.

**Client configuration examples:**

```jsonc
// stdio: Claude Code, Codex, Cursor (mcpServers block)
{ "ledger": { "command": "ledgermem", "args": ["mcp", "--project", "billing-api", "--agent", "claude-code"] } }

// Streamable HTTP
{ "ledger": { "type": "http", "url": "http://127.0.0.1:7437/mcp", "headers": { "Authorization": "Bearer lm_…" } } }
```

`ledgermem init` prints these for the current directory.

**Tasks.** When a client announces the Tasks extension, a `memory_remember` or `memory_attest` call that needs a slow verifier is returned as a task, and the client polls `tasks/get`. Without Tasks support, the call returns at once with `verification: "pending"` and the claim `proposed`.

**Elicitation.** When the client supports elicitation and a predicate's verifier is `human` with `ask: user`, Ledger asks the user directly, and the answer is recorded as a user statement (channel `elicitation`).

### Tools

Exactly seven. The descriptions below are the shipped text: they are written for the model and are part of the contract.

| Tool | Description (shipped text) | Permission |
| --- | --- | --- |
| `memory_recall` | Recall what is known about a subject or question. Returns claims with status, trust, age and conflicts, plus guidance. Call before relying on anything from an earlier session. Treat guidance `reliable` as known; treat `unverified`, `stale` and `unresolved` as leads to check with your tools or ask about. Use intent `history` or `as_of` for questions about the past. | read |
| `memory_remember` | Propose claims for memory, one subject, one predicate and one value each, with a one-sentence text. Use declared predicates (resource `ledger://…/policy`). Attach evidence for what you observed through a tool (test run, commit, PR, API response): claims about your own work stay proposed until evidence backs them. For something the user told you about themselves, put their exact words in `relayed_user_statement`. | propose |
| `memory_attest` | Attach evidence to a claim, or ask Ledger to verify it now against its system of record. Call after you have checked what a proposed or stale claim says. | attest |
| `memory_check_before_act` | Check before you act. Before any side-effecting tool call (deploy, merge, migrate, delete, send, refund), name the action and the remembered claims it relies on. Proceed on `go`. On `re-verify`, check with your tools or ask the user first. On `block`, use the current value returned, or ask. | read |
| `memory_history` | Return the version chain for a subject and predicate: every value it has held, when, on whose word, and why it changed. Use for "before", "used to" and "what changed" questions and for rollbacks. | read |
| `memory_resolve` | Settle a conflict set by naming the winning claim, or retract a claim as untrue. When the user withdraws something about themselves, pass their exact words in `relayed_user_statement`. | resolve, or relay_user for user-authority predicates |
| `memory_forget` | Forget claims at the user's request: their content leaves memory for good, and only a record that they existed remains. When the user asks to forget something about themselves, pass their exact words in `relayed_user_statement`. | forget, or relay_user for user-authority predicates |

### Tool inputs and outputs

Shapes are abbreviated. The JSON Schemas generated from the Rust types are authoritative, served in `tools/list` and in the OpenAPI document.

**`memory_recall`**

```jsonc
// input
{ "query": "what version of billing-api is deployed?",
  "intent": "current",                // optional: current | as_of | history | change
  "at": "2026-10-20T00:00:00Z",       // required for as_of and change
  "subject": "service:billing-api",   // optional narrowing
  "predicate": "deployed_version",    // optional narrowing
  "scopes": ["project", "user"],      // optional; within the caller's bounds
  "session": "s_981",                 // optional; makes session claims visible
  "high_stakes": false,
  "limit": 10 }
// output: a read pack (claims-and-integrity.md, section 9)
```

**`memory_remember`**

```jsonc
// input
{ "claims": [ {                      // 1 to 20 claims
    "subject": "service:billing-api",
    "predicate": "deployed_version",
    "value": "2.14.0",               // a JSON scalar, or { "type": "subject", "v": "user:u_42" }
    "text": "billing-api is running 2.14.0 in production",
    "scope": "project",              // optional: session | agent | project | user | org (within bounds)
    "valid_from": null, "valid_to": null,
    "evidence": [ { "kind": "tool_result", "tool": "kubectl.get_deployment",
                    "ref": "deployment/billing-api", "excerpt": "image: billing-api:2.14.0",
                    "observed_at": "2026-10-25T04:12:09Z" } ],
    "refines": null,                 // claim id this one makes more specific
    "relayed_user_statement": null,  // { "quote": "I'm vegetarian" }
    "reported_source": null          // web | document | other_agent (only ever lowers trust)
  } ],
  "session": "s_981" }
// output
{ "results": [ {
    "claim_id": "clm_…", "status": "verified", "trusted": true, "trust": 0.95,
    "relation": "temporal_update",   // new | duplicate | refinement | temporal_update | contradiction | compatible
    "resolution": { "rule": "system_of_record", "superseded": ["clm_…"] },
    "conflict_set": null,
    "verification": "done",          // none | done | pending
    "hints": [] } ] }                // e.g. "undeclared predicate 'tests_passing'; did you mean 'test_status'?"
```

**`memory_attest`**: input `{ "claim_id", "evidence"?: [ … ], "verify_now"?: true }`. Output `{ "claim_id", "outcome": "pass|fail|inconclusive|pending", "status", "trusted", "trust" }`.

**`memory_check_before_act`**

```jsonc
// input
{ "action": { "tool": "deploy", "description": "roll billing-api back to the previous release", "high_stakes": true },
  "relies_on": [ "clm_…", { "subject": "service:billing-api", "predicate": "deployed_version" } ] }
// output
{ "verdict": "block",                // go | re-verify | block
  "claims": [ { "ref": "clm_…", "status": "superseded", "trusted": false, "stale": false,
                "reason": "superseded: deployed_version is now 2.15.0",
                "current": { "claim_id": "clm_…", "value": "2.15.0" } } ],
  "note": null }
```

**`memory_history`**: input `{ "subject", "predicate", "scope"?, "recorded_as_of"? }`. Output `{ "chain": [ { "claim_id", "value", "valid_from", "valid_to", "status", "source", "trust", "resolution"?: { "rule", "entry_id" } } ] }`, ordered by valid time.

**`memory_resolve`**: input `{ "conflict_set"?: "cfs_…", "winner"?: "clm_…", "retract"?: "clm_…", "reason": "…", "relayed_user_statement"?: { "quote" } }`, with exactly one of `winner` (needs `conflict_set`) and `retract`. Output `{ "entry_id", "changes": [ { "claim_id", "status" } ] }`.

**`memory_forget`**: input `{ "claim_ids"?: [ … ], "subject"?, "predicate"?, "scope"?, "reason": "…", "relayed_user_statement"?: { "quote" } }`. Output `{ "forgotten": 3, "tombstones": [ "clm_…" ] }`.

### Resources

| URI | Contents |
| --- | --- |
| `ledger://{level}/{id}/conflicts` | Open conflict sets in a scope (for example `ledger://project/billing-api/conflicts`). |
| `ledger://{level}/{id}/stale` | Claims past their freshness interval. |
| `ledger://{level}/{id}/policy` | The active policy for that scope: declared predicates with cardinality, value type, freshness, authority and verifier. |

`org` scope URIs omit the id: `ledger://org/policy`.

### Prompt: `memory-usage`

The shipped text, written as steps, each with its own completion condition:

> Ledger is your memory. Every remembered claim has a status; act on it by status.
>
> 1. Before relying on anything from an earlier session, call `memory_recall`. Treat `reliable` answers as known. Treat `unverified`, `stale` and `unresolved` answers as leads: check them with your tools, or ask the user.
> 2. When you learn something durable (a decision, the state of a system, something about the user), call `memory_remember` with declared predicates and the evidence you observed. A claim about your own work is trusted only once evidence backs it, so record what you checked, not what you intended.
> 3. Before any side-effecting tool call, call `memory_check_before_act` with the claims the action relies on. Act on `go`. On `re-verify`, check first. On `block`, use the current value it returns, or ask the user.
> 4. When the user corrects you or asks you to forget something about them, pass their exact words: `memory_resolve` to retract it, `memory_forget` to forget it.

### Errors

Tool execution errors return an MCP result with `isError: true` and structured content `{ "error": { "code", "message", "fields"? } }`, so the model can read and act on them. Malformed requests are JSON-RPC protocol errors.

## 2. REST API

Base path `/v1`. JSON in and out. Every request needs `Authorization: Bearer lm_…`. The OpenAPI 3.1 document is served at `/v1/openapi.json` and generated from the same Rust types as the MCP schemas, so the two cannot drift. The visualizer uses only these endpoints (ADR-0007).

### Memory operations (one-to-one with the MCP tools)

| Method and path | Engine operation |
| --- | --- |
| `POST /v1/recall` | recall |
| `POST /v1/remember` | remember (an operator key with `supersedes` set records a correction) |
| `POST /v1/attest` | attest |
| `POST /v1/check-before-act` | check_before_act |
| `GET /v1/history?subject=&predicate=&scope=&recorded_as_of=` | history |
| `POST /v1/resolve` | resolve |
| `POST /v1/forget` | forget |

### Review operations (operator keys)

| Method and path | Purpose |
| --- | --- |
| `GET /v1/claims?scope=&status=&subject=&predicate=&q=&include_session=&cursor=` | Browse claims. |
| `GET /v1/claims/{id}` | Claim detail: trust components, evidence, version chain, the entries that touched it. |
| `POST /v1/claims/{id}/reverify` | Enqueue re-verification (returns a job id). |
| `POST /v1/claims/{id}/release` | Release a quarantined claim. |
| `GET /v1/conflicts?scope=&state=open\|closed` · `GET /v1/conflicts/{id}` | Conflict sets. |
| `GET /v1/questions?state=open` · `POST /v1/questions/{id}/answer` | Open questions. The answer is `confirm`, `reject`, `pick` (a winner) or `value`. |
| `GET /v1/journal?scope=&after_seq=&limit=` | Journal entries for one scope. |
| `GET /v1/journal/verify` | Verify every chain in the tenant: `{ "ok", "scopes": [ { "scope", "entries", "head_hash", "broken_at"? } ] }`. |
| `GET /v1/policy` · `GET /v1/policy/versions` · `POST /v1/policy` | Read the policy, list its versions, apply a new one (YAML or JSON body). |
| `GET /v1/predicates/undeclared` | Undeclared predicates in use, with counts and the nearest declared predicate. |
| `GET /v1/aliases/suggestions` | Suggested subject aliases, for an operator to accept through a policy change. |
| `GET /v1/jobs/{id}` | Job status. |

### Administration (operator keys with `admin`)

| Method and path | Purpose |
| --- | --- |
| `POST /v1/backup` | Streams a backup bundle (`application/vnd.ledgermem.backup`, section 6). |
| `POST /v1/restore` | Body is a bundle. Restores into the key's tenant, which must be empty. Returns a restore report. |
| `GET /v1/tenants` · `POST /v1/tenants` | List and create tenants. |
| `GET /v1/keys` · `POST /v1/keys` · `DELETE /v1/keys/{id}` | Manage API keys. Revoking a key is a credential change, not a memory change, so `DELETE` is allowed here. |

`GET /healthz` needs no authentication.

### Keys and permissions

A key has a **kind**, which fixes its channel, and a set of **permissions**.

| Kind | Channel | Typical permissions |
| --- | --- | --- |
| agent | agent | `read, propose, attest`, plus `relay_user` if granted (off by default for HTTP keys) |
| agent (proposer preset) | agent | `read, propose` |
| integration | integration | `read, propose, attest, declare_source` |
| operator | visualizer | `read, propose, attest, resolve, forget, operator`, plus `admin` if granted |

Keys also carry scope bounds (for example `project:billing-api`, `user:u_42`), and calls can only narrow them. An operator key represents a person, and writes made with it are recorded as user-stated.

### Error mapping

| Engine error | HTTP | MCP |
| --- | --- | --- |
| ScopeDenied, PermissionDenied | 403 | tool error |
| NotFound | 404 | tool error |
| InvalidClaim | 422 (with `fields`) | tool error |
| ChainBroken, RestoreTargetNotEmpty | 409 | tool error |
| VerifierUnavailable | 424 | tool error |
| Busy | 503 with `Retry-After` | tool error |
| Internal | 500 | tool error |

Error bodies are `{ "error": { "code": "scope_denied", "message": "…", "fields"?: { … } } }`.

## 3. CLI

```
ledgermem init                                   # write ledgermem.toml, apply predicate packs, print MCP config snippets
ledgermem mcp [--transport stdio|http] [launch arguments]
ledgermem serve [--listen 127.0.0.1:7437] [--tenant T] [--no-ui] [--allow-remote]
ledgermem tenant create <id> | list
ledgermem key create --tenant T --kind agent|integration|operator [--preset proposer] [--permission …] [--scope project:billing-api …]
ledgermem key list | revoke <key_id>
ledgermem policy apply <file.yaml> [--project P] | show [--project P] | packs
ledgermem claims list [filters] | show <claim_id>
ledgermem questions list | answer <qst_id> --confirm|--reject|--pick <clm_id>|--value <v>
ledgermem backup [--tenant T] -o <file.lmbk>
ledgermem restore <file.lmbk> [--tenant T]
ledgermem verify [--tenant T]
ledgermem reindex [--tenant T]
ledgermem demo [--tenant demo]                   # create a tenant loaded with the scenario suite's demo dataset
```

`ledgermem serve` prints a sign-in link for the visualizer: `http://127.0.0.1:7437/ui/#key=lm_…`. The key belongs to a local operator key that `serve` creates on first run. It sits in the URL fragment, so it never reaches request logs. The visualizer keeps it in session storage, and also accepts a pasted operator key for a remote server.

CLI commands act as the local operator (channel `cli`) on the database they open. Without `--tenant`, they use `local`. Every command supports `--json` for machine-readable output.

## 4. Server config (`ledgermem.toml`)

Owned by whoever runs the binary. Never journaled. Holds verifier credentials by reference to environment variables, never inline.

```toml
[storage]
path = "{data_dir}/ledgermem.db"   # %APPDATA%\ledgermem, ~/.local/share/ledgermem, ~/Library/Application Support/ledgermem

[server]
listen = "127.0.0.1:7437"
ui = true

[embedder]
kind = "local"                      # local | openai_compatible | none
model = "bge-small-en-v1.5"
# base_url = "http://localhost:11434/v1"; api_key_env = "OPENAI_API_KEY"   (openai_compatible)

[llm]
kind = "off"                        # off | openai_compatible | anthropic
# model = "claude-haiku-5-5"; api_key_env = "ANTHROPIC_API_KEY"

[verifiers.git.repos]
"repo:acme/billing-api" = "C:/src/billing-api"

[verifiers.test_run.repos."repo:acme/billing-api"]
junit = "C:/src/billing-api/target/junit.xml"
rerun = { command = ["cargo", "test"], cwd = "C:/src/billing-api", timeout_ms = 120000 }   # opt-in, always async

[verifiers.github]
token_env = "GITHUB_TOKEN"

[verifiers.http.status_api]
base_url = "https://status.internal.acme.dev"
headers_env = { Authorization = "STATUS_API_TOKEN" }
timeout_ms = 800

[verifiers.exec.feature_flag]
command = ["python", "verifiers/feature_flag.py"]
timeout_ms = 2000
env_allow = ["FLAGS_API_TOKEN"]

[verifiers.llm_judge]
enabled = false
```

## 5. Policy file

YAML, applied with `ledgermem policy apply` or `POST /v1/policy`. Holds no secrets. Every applied version is journaled, and a project file is applied as an overlay on the tenant policy.

```yaml
version: 1
extends: [software-delivery, personal-assistant]   # predicate packs shipped in policies/

subjects:
  aliases:
    service:billing-api: [service:billing, repo:acme/billing-api]

predicates:
  deployed_version:
    cardinality: single            # single | multi
    value_type: version            # text | number | boolean | datetime | version | subject | {enum: [...]}
    freshness: volatile            # stable | slow | volatile
    reverify_after: 1h             # optional override
    system_of_record: http.status_api
    verifier: { use: http.status_api, params: { path: "/deploys/{subject_name}", pointer: "/version" } }
  test_status:
    cardinality: single
    value_type: { enum: [passing, failing] }
    freshness: volatile
    verifier: { use: test_run }
    agent_claims: require_evidence # agent claims without passing evidence never win a resolution
  dietary_preference:
    cardinality: multi
    authority: user
    freshness: slow
  preferred_provider:
    authority: user
    instruction: true              # web/document/other-agent sources are quarantined (claims-and-integrity.md, section 3)

resolution:
  trust_margin: 0.25
  on_unresolved: contested         # contested | ask_user | prefer_newer

trust:
  trusted_threshold: 0.80

freshness:
  slow: 90d
  volatile: 1h
```

Validation errors name the offending path (`predicates.test_status.value_type`) and reject the whole document. Nothing is partially applied.

**Built-in verifier parameters:**

| Verifier | Params | Checks |
| --- | --- | --- |
| `test_run` | `report: junit` (default), `rerun: bool` | Reads the subject repo's JUnit report from server config, and compares failures and errors against the claimed value. `rerun` re-runs the configured command asynchronously. |
| `git` | `check: commit_exists \| branch_exists \| merged_into \| file_contains`, plus `ref`, `into`, `path`, `pattern` | Against the subject's configured local repo. |
| `github` | `check: pr_state \| ci_status`, `repo` | Through the GitHub REST API with the configured token. |
| `http.<name>` | `path` (template), `pointer` (JSON Pointer), `compare: equals \| contains` | GET only, against the named endpoint in server config. |
| `file` | `check: exists \| contains \| sha256`, `path` (template) | The local filesystem. |
| `human` | `ask: operator \| user` | Opens an open question, or asks the user through MCP elicitation. |
| `exec.<name>` | free-form, passed through | Section 6. |
| `llm_judge` | `instructions` | Only when enabled. Trust is capped at 0.60. |

Templates may use `{subject_name}` (the part after `kind:`), `{subject}`, `{value}` and `{predicate}`.

## 6. Exec verifier protocol (`ledgermem.verifier/1`)

Ledger runs the configured command with no shell, the working directory set to the config file's directory, the environment reduced to `env_allow`, and a hard timeout. It writes one JSON document to stdin and closes it.

```jsonc
// stdin
{ "protocol": "ledgermem.verifier/1",
  "claim": { "id": "clm_…", "scope": "project:billing-api", "subject": "service:billing-api",
             "predicate": "feature_flag_state", "value": "on", "text": "…",
             "valid_from": "…", "evidence": [ … ] },
  "params": { "flag": "new-invoices" },
  "deadline_ms": 2000 }

// stdout (exactly one JSON document)
{ "protocol": "ledgermem.verifier/1",
  "outcome": "pass",               // pass | fail | inconclusive
  "observed": { "value": "on", "summary": "flag new-invoices is on in prod" },
  "evidence": { "kind": "api_response", "ref": "flags/new-invoices", "payload": { … } },
  "system_of_record": true }
```

A non-zero exit, a timeout, more than 1 MiB on stdout, or output that doesn't parse is recorded as `inconclusive` with the failure reason. stderr is captured to the server log, truncated to 8 KiB. The observed payload is hashed into the evidence. A future protocol `/2` will be added alongside `/1`, never in place of it.

## 7. Backup bundle (`ledgermem.backup/1`, `.lmbk`)

A gzip-compressed tar archive:

```
manifest.json
journal/<level>--<id>.jsonl      one entry per line, in sequence order; content null for forgotten entries
evidence.jsonl                   evidence metadata and retained payloads, keyed by content hash
policy/<pol_id>.yaml             every applied policy version
forget_register.jsonl            ids and content hashes forgotten in this tenant (no content)
```

```jsonc
// manifest.json
{ "format": "ledgermem.backup/1",
  "ledgermem_version": "0.1.0",
  "tenant": "acme",
  "created_at": "2026-10-25T09:00:00Z",
  "scopes": [ { "scope": "project:billing-api", "entries": 1842, "head_seq": 1842, "head_hash": "sha256:…" } ],
  "policy_versions": ["pol_1", "pol_7"],
  "counts": { "claims": 913, "evidence": 1204, "forgotten": 12 } }
```

Tenants and API keys are never included, because they are credentials. **Restore** checks the format version, verifies every journal against its manifest head, refuses unless the target tenant is empty, replays entries through the projector, re-applies the target instance's forget register, and reports `{ "scopes", "entries", "claims", "forgets_reapplied", "warnings" }`. The format is documented so other tools (including a future Mem0 or Zep importer) can produce and read it.
