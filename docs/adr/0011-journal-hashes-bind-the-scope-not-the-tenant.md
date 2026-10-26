# Journal hashes bind the scope, not the tenant

A journal's genesis hash and every entry hash name the scope (`project:billing-api`, `user:u_42`, `org`) but not the tenant it belongs to. A backup can then be restored into a tenant with a different name and every chain still verifies, which `ledgermem restore` relies on: it restores into whichever empty tenant the operator chooses. Tenant isolation is enforced where it already lives, by the store and the Caller on every operation, not by the hashes.

## Considered Options

- Binding the tenant into the genesis hash: two tenants with identical journals would get different hashes, but a journal could only ever be restored under its original tenant name, and renaming a tenant would break every chain in it.

## Consequences

- Two tenants holding identical journals have identical hashes. The hashes prove a journal's history hasn't been edited, not which tenant it belongs to.
- The hash format is public (claims-and-integrity.md, section 11) and becomes a one-way door at the first release.
