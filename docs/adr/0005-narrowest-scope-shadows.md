# Across scopes, the narrowest scope shadows; conflicts are detected only within a scope

When claims about the same subject and predicate exist in more than one visible scope, a read takes the one from the narrowest scope, in the order session → agent → project → user → org. The claims it overrides are returned alongside, marked shadowed. Conflict detection and resolution run only among claims in the same scope. A project legitimately overriding an org default (a different deploy region) isn't a contradiction, and treating it as one would flood reviewers with false conflicts.

## Consequences

- A write into a narrow scope can override a broader one without any conflict being raised. The write gate's source and authority rules, not conflict detection, guard against a poisoned narrow-scope claim.
- Reads always report what they shadowed, so the override is never silent.
