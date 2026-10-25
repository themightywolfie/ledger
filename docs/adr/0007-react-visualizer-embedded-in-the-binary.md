# The visualizer is a React + TypeScript app compiled into the Rust binary

The rest of Ledger is Rust, but the memory visualizer is a React + TypeScript single-page app built with Vite. Its static files are embedded into the `ledgermem` binary at build time and served by `ledgermem serve` on localhost. React has the visualization and component libraries a timeline and review UI need, and far more potential contributors than Rust frontend frameworks. Embedding it keeps "one binary" true. The visualizer talks only to the public REST API, so it can't depend on anything an integrator can't use.

## Considered Options

- Leptos (all-Rust, compiled to WASM): one language, but a smaller ecosystem for charts and timelines and fewer contributors.
- Server-rendered HTML with htmx: no JavaScript build, but a poor fit for interactive timelines and conflict review.

## Consequences

- Building the binary requires Node for the web build. A prebuilt copy of the web assets keeps Rust-only builds possible.
