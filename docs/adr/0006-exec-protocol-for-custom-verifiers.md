# Custom verifiers are external executables speaking JSON over stdin/stdout

Built-in verifiers (test run, Git, GitHub, HTTP, file, human) are compiled into Ledger. Custom verifiers use an `exec` protocol instead: Ledger runs a registered executable with no shell and a timeout, writes the claim to its stdin as JSON, and reads a verdict back from its stdout as JSON. Teams can write a verifier in any language without recompiling Ledger or touching Rust.

## Considered Options

- Compiled-in Rust plugins: fastest, but every custom verifier would mean forking and rebuilding Ledger.
- WASM components: sandboxed and portable, but the toolchain is too much to ask for a 20-line check. This stays a candidate for later.

## Consequences

- The JSON protocol is a public contract and is versioned from day one.
- v1 doesn't sandbox exec verifiers. They run with Ledger's own permissions, and the documentation says so plainly.
