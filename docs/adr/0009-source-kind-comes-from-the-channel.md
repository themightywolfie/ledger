# A claim's source kind comes from its channel and actor, never from its payload

Over MCP the caller is a language model, so any `source.kind` field in a request is the model's opinion, and a prompt-injected model will happily label a web page's suggestion as `user_stated`. Ledger therefore ignores any source kind the caller supplies and derives it from the channel the write arrived through and the permissions of the actor's key. A write from an agent is an agent claim. The agent may mark it as a relayed user statement, quoting the user's words, but that counts only with the `relay_user` permission and only for predicates the user has authority over. Truly user-stated claims come only from channels the user controls: MCP elicitation, the visualizer, the command line, or a framework integration key, where the framework rather than the model is the caller.

## Consequences

- "The user said so" can't be forged by content, only by a holder of a relay-permitted key. Local stdio keys allow relaying by default, and HTTP keys don't.
- Users of local coding agents can state preferences in chat without extra confirmation steps. Remote deployments get the strict behaviour.
- An agent's account of where a fact came from (`reported_source: web`) can only lower a claim's trust, never raise it.
