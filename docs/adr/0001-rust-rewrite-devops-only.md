# Rewrite in Rust as a devops-only tool; remove all AI features

Yanto is being rewritten from Node/TypeScript to Rust, and its scope narrows to VPS management: deploying, operating, and exposing workloads. All AI functionality (AI providers, Codex, agent tasks, agent worktrees, commit/push from the UI) is dropped, not ported. Git-sourced deploys stay, since pulling and deploying a branch is CD, not development.

## Consequences

- MCP survives only as an interface onto devops operations, not as an AI feature (see later ADRs).
- The rewrite happens on the `rewrite/rust` branch; the TypeScript tree stays on `master` until the parity checklist in `docs/rewrite/` is complete.
