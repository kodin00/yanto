# Server-rendered HTML + htmx instead of the React SPA

The dashboard is rewritten as server-rendered templates with htmx and SSE for live updates, embedded in the binary. This removes the Node/npm toolchain from the build entirely, and an ops dashboard (tables, forms, log streams) doesn't need a client-side app.

## Considered Options

- Keep the React SPA and embed its build output: lowest-risk, but keeps npm in the build and keeps a ~12k-line client that would still need AI parts cut out.
- Leptos/Dioxus WASM SPA: pure Rust, but adds a WASM toolchain and more complexity than an ops dashboard needs.
