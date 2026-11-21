> Classification: INTERNAL

# Yanto Rust rewrite — plan

Start with [00-scope](00-scope.md). It fixes every decision. The other docs only elaborate on it.

| Doc | Contents |
|---|---|
| [00-scope](00-scope.md) | Fixed decisions D1–D21, feature parity checklist, drops, roadmap |
| [01-architecture](01-architecture.md) | Workspace, service layer + adapters, runtime, Host integration, testing |
| [02-data-model](02-data-model.md) | SQLite schema |
| [03-interfaces](03-interfaces.md) | Auth/RBAC, dashboard routes, REST `/api/v1`, MCP tools, CLI |
| [04-features](04-features.md) | Per-feature behavior, side effects, failure modes, acceptance criteria |
| [05-install-and-ops](05-install-and-ops.md) | `install.sh`, `yanto install`, systemd, releases, mise, CI |
| [06-milestones](06-milestones.md) | Milestones M0–M9, execution model, risks |

Related: [`CONTEXT.md`](../../CONTEXT.md) (glossary), [`docs/adr/`](../adr/) (decisions), [`docs/legacy/`](../legacy/) (how the TS system behaves today).
