# Vendored assets

Served from the binary (embedded via `rust-embed`). No npm; updates are manual.

| File | Project | Version | Source |
|---|---|---|---|
| `htmx.min.js` | htmx | 2.0.11 | https://cdn.jsdelivr.net/npm/htmx.org@2.0.11/dist/htmx.min.js |
| `xterm.js` | @xterm/xterm (MIT) | 6.0.0 | https://cdn.jsdelivr.net/npm/@xterm/xterm@6.0.0/lib/xterm.js |
| `xterm.css` | @xterm/xterm (MIT) | 6.0.0 | https://cdn.jsdelivr.net/npm/@xterm/xterm@6.0.0/css/xterm.css |
| `xterm-addon-fit.js` | @xterm/addon-fit (MIT) | 0.11.0 | https://cdn.jsdelivr.net/npm/@xterm/addon-fit@0.11.0/lib/addon-fit.js |

SHA-256 of `htmx.min.js` as vendored:
`d6fdc75f204e6bdefa99b69bf1e6d4ac69b8a364f77929f45c13476b4000f717`

SHA-256 of the xterm files as vendored (downloaded 2026-11-21, unmodified; the trailing
`sourceMappingURL` comment points at a `.map` file that is not vendored, which only affects devtools):

```
14903579ff54664cd72f8e8699e6961a6272c21863ec1c3b118cdc8af5d4a972  xterm.js
854a7c0fb70e8b1a083c16797ab827299fb18744f5ad34f227b48337e33293c6  xterm.css
ba3ea256ce0620a0992a197d6c9baea64823fc93d8da07a9e366ca9943c18527  xterm-addon-fit.js
```

The two JS files are the UMD builds: they define the globals `Terminal` and `FitAddon.FitAddon`,
used by `assets/js/exec-terminal.js`.

To update: download the new pinned release from the same CDN path, replace the
file, and update this table and the hash. The CSP is `default-src 'self'`, so
assets must never be loaded from a CDN at runtime.
