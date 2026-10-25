# The product is Ledger; the crates and binary are `ledgermem`

The `ledger` crate name is taken on crates.io, and a `ledger` command clashes with ledger-cli, the plain-text accounting tool installed on many developer machines. The product keeps the name Ledger in prose. The published crates are `ledgermem` and `ledgermem-*`, and the binary is `ledgermem`. Renaming after publishing would break every user's MCP configuration, so we pick the name once, now.
