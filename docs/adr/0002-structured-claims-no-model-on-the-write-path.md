# Callers write structured claims; Ledger runs no model on the write path

The product plan has Ledger extract claims from raw input with a small model and classify how claims relate with a small classifier. In v1 the caller writes structured claims (subject, predicate, value, claim text, optional valid time and evidence) instead. Whatever calls `memory_remember` over MCP is already a language model and can structure its own claim, so Ledger gains nothing by running a second model and loses latency, cost and determinism. Relation classification is a fixed set of rules over predicate cardinality, normalized value comparison and valid-time ordering. A refinement is recorded only when the caller declares one. Free-text extraction and the LLM judge verifier sit behind an optional language-model provider that is off by default.

## Consequences

- Write latency and outcomes are deterministic and testable without a model.
- The quality of extraction depends on the caller, so tool descriptions and the `memory-usage` prompt must teach the predicate vocabulary well.
- Refinements the caller doesn't declare are treated as duplicates or contradictions, which the conservative "contested, not overwrite" default contains.
