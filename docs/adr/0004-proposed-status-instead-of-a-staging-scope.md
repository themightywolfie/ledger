# Sub-agents propose into the real scope instead of a staging scope

The product plan has sub-agents write to a separate staging scope, from which claims are promoted to the project only after verification or orchestrator approval. We drop the staging scope. Instead, sub-agents get proposer keys, which can propose claims but not promote or resolve them. Their claims land in the project scope as `proposed`. Every agent sees them labelled as unverified, none get them in their instructions, and they are promoted only when a verifier passes or a permitted actor attests them. Seeing "a sub-agent says the migration is done" is more useful to an orchestrator than not seeing it, and one status does the job of a whole extra scope.

## Consequences

- Isolation now rests on status filtering and permissions, not on visibility. Every read path must honour status.
- A proposer's own words can never promote its claims (see "User statement" in GLOSSARY.md).
