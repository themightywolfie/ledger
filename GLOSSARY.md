# Ledger

Ledger is a memory layer for agents that keeps memory true. Every remembered statement is a claim with provenance and evidence: checked before it is believed, reconciled when it conflicts, and dated so both the past and the present can be asked about.

## Claims

**Claim**:
One structured statement about the world (a subject, a predicate and a value) together with who said it, what backs it and when it was true. The unit of memory.
_Avoid_: memory, fact, belief, chunk, record

**Subject**:
The thing a claim is about, named canonically so that every alias of it resolves to the same subject.
_Avoid_: entity, topic, target

**Alias**:
Another name that refers to the same subject (a short name, a repo name, a nickname).

**Predicate**:
The property or relationship a claim asserts about its subject, such as "deployed version" or "lives in".
_Avoid_: attribute, key, field

**Value**:
What a claim says the predicate is for its subject.

**Claim text**:
The natural-language rendering of a claim, kept so it can be found by meaning as well as by subject and predicate.

**Cardinality**:
Whether a predicate holds exactly one value at a time (single-valued) or several at once (multi-valued). Two values for a multi-valued predicate never conflict.

**Undeclared predicate**:
A predicate used by a claim but absent from the policy. Treated as single-valued and slow to go stale until an operator declares it.

## Policy

**Policy**:
A tenant's declared rules for its memory: the predicates it knows and how each behaves, the aliases of its subjects, and its resolution policy. Every version applied is kept.
_Avoid_: config, schema, settings

**Policy overlay**:
A project's additions to, or overrides of, its tenant's policy.

**Predicate pack**:
A ready-made set of predicate declarations for one domain, such as software delivery or a personal assistant.

## Provenance

**Source**:
Where a claim came from: its source kind, the actor who made it, and the session and turn it arrived in.
_Avoid_: origin

**Source kind**:
The category a claim's source falls into: user-stated, agent claim, tool result, document, web, other agent, or system. Decided by the channel a claim arrived through, never by what the content says about itself.

**Actor**:
The person, agent or system that made a claim.

**Evidence**:
A pointer to something observable that backs or refutes a claim (a tool result, a test run, a diff, an API response, a human confirmation), fixed by a content hash and the time it was observed.
_Avoid_: proof, citation, reference

**Channel**:
The path a write arrived through: an agent, an integration acting for a framework, the visualizer, the command line, or a question put directly to the user. The channel and the actor's permissions fix a claim's source kind; what the claim says about itself never does.

**User statement**:
Evidence consisting of the user's own words, which counts only for predicates the user has authority over and only when it arrives through a user-stated channel. An agent's own words are never evidence for its own claim.

**Relayed user statement**:
An agent's report of the user's exact words. It counts as a user statement only for predicates the user has authority over, and only from an actor permitted to relay.

**Attestation**:
One act of checking a claim against reality, whose outcome is recorded as evidence.
_Avoid_: validation, fact-check

**Verifier**:
A read-only check that attests claims of a declared shape against a real system: a test run, a Git repository, an HTTP endpoint, a file, a cluster, or a human.
_Avoid_: validator, checker

**LLM judge**:
A verifier of last resort that asks a language model whether a claim holds; its verdicts are labelled as such and can only lift trust to a lower ceiling.

**System of record**:
The registered source of truth for a predicate that describes system state, whose word beats any actor's narration of it.

**Authority**:
The party whose word decides a predicate, such as the user for their own preferences and identity.

## Trust

**Trust score**:
A bounded number expressing how far a claim should be believed, built from source prior, evidence strength, corroboration, recency and contradictions, and always explainable by those components.
_Avoid_: confidence, credibility

**Source prior**:
The starting trust a claim gets from its source kind alone.

**Corroboration**:
Agreement with a claim from an independent source. Repeats from the same source count once.

**Freshness class**:
How quickly claims about a predicate go out of date: stable, slow or volatile, each with a default re-verify interval.

**Stale**:
Said of a claim whose last attestation is older than its freshness class allows.

## Lifecycle

**Status**:
Where a claim stands in its lifecycle.

**Proposed**:
Stored but not yet verified. Visible only as labelled data, never as something to act on.

**Verified**:
Backed by at least one passing attestation.

**Trusted**:
Verified, with a trust score at or above the trusted threshold, and not part of an open conflict set.

**Promotion**:
A proposed claim becoming verified, because a verifier passed or a permitted actor attested it.

**Contested**:
Part of a conflict that the resolution policy could not settle.

**Superseded**:
Replaced by a later claim for the same subject and predicate. Still answers questions about the time it was valid.

**Retracted**:
Withdrawn as untrue, by its authority or by evidence. Never used again.

**Quarantined**:
Held back by the write gate as likely poisoning, such as a directive arriving from web content. Never served as memory.

## Time

**Valid time**:
The period during which a claim was true in the world.

**Recorded time**:
The moment Ledger learned a claim.

**Version chain**:
The ordered claims that have held, one after another, for the same subject and predicate in a scope.
_Avoid_: revision list, history (as a name for the chain itself)

## Conflicts

**Conflict**:
Two or more claims in the same scope that cannot all be true.

**Conflict set**:
The claims taking part in one conflict, open until a resolution settles it.

**Relation**:
How a newly written claim stands to an existing one about the same subject and predicate: duplicate, refinement, temporal update, contradiction, or compatible.

**Duplicate**:
A claim that says the same thing as an existing one. Counts toward corroboration only when its source is independent.

**Refinement**:
A claim that says the same thing as an existing one, more specifically.

**Temporal update**:
A claim that replaces an existing one because the world changed.

**Contradiction**:
A claim that disagrees with an existing one about the same moment.

**Resolution**:
The decision that settles a conflict set, recorded with the rule or reviewer that made it.

**Resolution policy**:
The ordered rules, versioned with the project, that settle conflicts without a human.

**Reviewer**:
A person or orchestrating agent permitted to settle conflicts by hand.

**Open question**:
A conflict or claim waiting on a human answer, such as a contested set escalated to a reviewer or a claim held for human confirmation.

## The journal

**Journal**:
The append-only, hash-chained sequence of everything that has happened in one scope. Nothing in it is ever edited or removed; every change is a new entry.
_Avoid_: ledger, log, history

**Entry**:
One record in a journal: a claim written, evidence attached, a resolution made, a policy version applied, a claim forgotten.
_Avoid_: event, row, record

**Correction**:
A user-stated claim, made by a person reviewing memory, that supersedes an existing claim they judge wrong.
_Avoid_: edit, update, modification

**Backup**:
A portable copy of a tenant's journals, policy versions and evidence metadata, from which the tenant can be restored.
_Avoid_: dump, snapshot

**Restore**:
Rebuilding a tenant from a backup, only into an empty tenant and only when every journal in the backup verifies.

## Writing

**Write gate**:
The fixed sequence every write passes through (intake, extraction, classification, verification, conflict check, scoring and commit) before a claim can be believed.

**Extraction**:
Turning raw input such as a conversation turn or a tool output into claims.

## Reading

**Read intent**:
The time a question is about: the current value, the value as of a moment, the whole version chain, or what changed since a moment.
_Avoid_: query type, temporal mode

**Answer set**:
The claims a read returns as its answer, each carrying status, trust and age.

**Guidance**:
A read's verdict on how far its answer set can be relied on, such as "verified and fresh" or "unresolved".

**Abstention**:
A read declining to pick an answer, returning the competing candidates as unresolved instead.

**Pre-action check**:
A check of a planned side-effecting action against the claims it relies on, answering go, re-verify or block.
_Avoid_: guard check, preflight

## Scope

**Tenant**:
One isolated installation or customer. Nothing is ever visible across tenants.

**Scope**:
The visibility boundary a claim lives in within a tenant: org, user, project, agent or session.

**Shadowed**:
Said of a claim overridden on a read by a claim for the same subject and predicate in a narrower scope. Returned alongside the answer, not hidden.

**Operator**:
A person looking after a tenant's memory through the visualizer or the command line, permitted to review, correct, resolve, forget, back up and restore.
_Avoid_: admin, user (the user is whom the agents act for)

**Proposer**:
An actor whose permissions let it propose claims but not promote or resolve them, such as a sub-agent.

## Removal

**Forget**:
Removing a claim's content on request so that it can no longer be recalled, while recording that the removal happened.
_Avoid_: delete, purge, erase

**Tombstone**:
What remains of a forgotten claim: the record that it existed and was forgotten, without its content.
