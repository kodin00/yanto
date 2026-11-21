# Vendored assets

Served from the binary (embedded via `rust-embed`). No npm; updates are manual.

| File | Project | Version | Source |
|---|---|---|---|
| `htmx.min.js` | htmx | 2.0.11 | https://cdn.jsdelivr.net/npm/htmx.org@2.0.11/dist/htmx.min.js |

SHA-256 of `htmx.min.js` as vendored:
`d6fdc75f204e6bdefa99b69bf1e6d4ac69b8a364f77929f45c13476b4000f717`

To update: download the new pinned release from the same CDN path, replace the
file, and update this table and the hash. The CSP is `default-src 'self'`, so
assets must never be loaded from a CDN at runtime.
