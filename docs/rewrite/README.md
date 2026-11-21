> Classification: INTERNAL

# Yanto Rust rewrite — plan

Start with [00-scope](00-scope.md). It fixes every decision. The other docs only elaborate on it.

| Doc | Contents |
|---|---|
| [00-scope](00-scope.md) | Fixed decisions D1–D28, review resolutions, feature parity checklist, drops, roadmap |
| [01-architecture](01-architecture.md) | Workspace, service layer + adapters, runtime, Host integration, testing |
| [02-data-model](02-data-model.md) | SQLite schema |
| [03-interfaces](03-interfaces.md) | Auth/RBAC, dashboard routes, REST `/api/v1`, MCP tools, CLI |
| [04-features](04-features.md) | Per-feature behavior, side effects, failure modes, acceptance criteria |
| [05-install-and-ops](05-install-and-ops.md) | `install.sh`, `yanto install`, systemd, releases, mise, CI |
| [06-milestones](06-milestones.md) | Milestones M0–M9, execution model, risks |
| [07-open-questions](07-open-questions.md) | Consolidated open questions OQ-n with recommended answers and the milestone each blocks |
| [08-conventions](08-conventions.md) | Rules for parallel milestones: migration ranges, module registration, templates, errors, matrix tests, test helpers, ports and fakes |

Related: [`CONTEXT.md`](../../CONTEXT.md) (glossary), [`docs/adr/`](../adr/) (decisions), [`docs/legacy/`](../legacy/) (how the TS system behaves today).
