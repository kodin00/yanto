# Yanto — Functional Overview

> Purpose of this document set: capture **what yanto does** (its functionality), independent of the current TypeScript/Node implementation, to guide a future Rust reimplementation. Implementation-language details are intentionally omitted.

## What Yanto Is

Yanto Deploy is a **self-hosted, small-VPS deployment platform / PaaS control plane**. It targets a trusted operator who runs one VPS (and optionally remote worker nodes) and wants a web dashboard to deploy, observe, and operate containerized services — plus run AI-assisted coding tasks against their Git projects.

It is a single product "PaaS-in-a-box" with these core capabilities:

- **Project deployments** — define a project backed by a Git repo (clone/update a configured branch) or a Docker image, then run Docker Compose and store the result and logs. Each project gets a token-protected deploy webhook.
- **Dashboard** for deployments, containers, logs, host usage.
- **FRP forwarding** — expose TCP/UDP services behind CGNAT by running an `frps` server and generating `frpc` client scripts/config for the remote machine.
- **Cloudflare Tunnels** — publish a deployed local service to a Cloudflare-managed HTTPS hostname without leaving the dashboard; one remotely-managed tunnel per deployment node.
- **AI Tasks** (owner-only) — run Codex, OpenAI-compatible, or Anthropic-backed coding tasks against Git projects in isolated git worktrees. Streaming tool activity, conversation continuation, change review, commit, push, worktree cleanup.
- **MCP server** — scoped tokens (`ymcp_...`) with `read`/`write`/`admin` scopes over HTTP at `POST /mcp`, plus a stdio entrypoint.
- **R2 backups**, **Postgres backup targets**, **GitHub webhooks**, **audit logging**, **user/role management** (single owner + invited members; invitations/resets expire in 24h).
- **Multi-node workers** — remote worker nodes that poll the master for deployment jobs.

## High-Level Stack

| Layer | Technology (current) | Functional role |
|---|---|---|
| Client | React 19 SPA | Browser dashboard, served as static assets by the server |
| HTTP API | Express 5 | REST API (`/api/...`), deploy webhook (`/deploy`), GitHub webhook (`/webhooks/github`), MCP (`/mcp`) |
| Database | PostgreSQL 17 (Drizzle ORM) | All persistent state |
| Orchestration | Docker CLI (via mounted `docker.sock`) | Deployments, container management, AI-task runner containers, cloudflared |
| Tunnels | FRP (`frps`/`frpc`) + Cloudflare Tunnels (`cloudflared`) | Ingress: raw TCP/UDP (FRP) and HTTP/HTTPS (Cloudflare) |
| Object storage | Cloudflare R2 (S3-compatible) | Backup storage |
| AI | Anthropic, OpenAI (Chat + Responses), Codex SDK | Coding agent execution |

## Component Diagram

```
                          +---------------------------+
                          |   Browser (React SPA)      |
                          |   served from dist/client  |
                          +-------------+-------------+
                                        | HTTPS (cookies, CSRF)
                                        v
+-----------------------------------------------------------+
| Yanto MASTER process                                       |
|  - HTTP API  (/api, /deploy, /mcp)                         |
|  - Auth (JWT + httpOnly cookies, RBAC, rate-limit)         |
|  - Drizzle ORM -> PostgreSQL                                |
|  - Deployment runner (Git clone/update OR image pull,      |
|    docker compose up)  -> stores result + logs             |
|  - AI Task orchestrator (Codex / OpenAI / Anthropic)        |
|      worktrees under <agentWorktreesRoot>                   |
|  - MCP server (HTTP POST /mcp + stdio entrypoint)          |
|  - FRP manager                                             |
|  - Cloudflare Tunnel client -> Cloudflare API              |
|  - Worker scheduler (/api/workers/register|jobs|heartbeat)|
|  - Backups (R2 / Postgres), audit log, settings            |
+-----+----------------+----------------+-----------+-------+
      | docker.sock    | /projects      | /data/*   | HTTPS
      v                v                v           v
+-----------+   +----------------+  volumes   +----------------+
| Docker    |   | Git repos /    | (ssh,      | Cloudflare API |
| daemon    |   | worktrees      |  backups,  | (tunnels, DNS)  |
| (host)    |   | (host mount)   |  codex,    +-------+---------+
+-----+-----+   +----------------+  frp,             |
      |                              config)         | tunnel token
      | runs deployed projects                       v
      v                                    +------------------+
+-------------------+                       | cloudflared       |
| Deployed project  |<-- FRP / Tunnel ------| container (per    |
| containers        |    (optional)         |  node)             |
+-------------------+                       +------------------+

  +------------------------------------------------------------+
  | FRPS container                                              |
  |  - binds :7000/tcp (control), port range tcp+udp (fwd)     |
  |  - dashboard :7500 (auth = token)                          |
  |  - frpc clients (on remote/CGNAT machines) connect :7000   |
  +------------------------------------------------------------+

  +------------------------------------------------------------+
  | WORKER process (role=worker)                               |
  |  - NO HTTP listener, NO DB                                  |
  |  - registers with master (join token) -> gets worker token  |
  |  - polls GET /api/workers/jobs/next, runs deployments       |
  |  - streams logs + status back to master                     |
  |  - heartbeats; exponential backoff on master errors        |
  +------------------------------------------------------------+

  +------------------------------------------------------------+
  | PostgreSQL                                                  |
  |  - schema managed by migrations run at server startup       |
  |  - tables: projects, deployments, agent_*, cloudflare_*,  |
  |    frp_tunnels, app_settings, audit_logs, users, etc.      |
  +------------------------------------------------------------+
```

## Master vs Worker Roles

The same binary runs two roles, selected by `YANTO_NODE_ROLE`:

- **master** — the full HTTP API + dashboard + AI task orchestration + MCP + FRP management. Owns all state, scheduling, and the UI.
- **worker** — a remote execution agent. Does NOT run an HTTP listener or database. It registers with the master via a join token, polls `GET /api/workers/jobs/next` for deployment jobs, executes them locally (Docker Compose against the mounted `/projects` and `docker.sock`), streams logs back, and reports terminal status. Uses exponential backoff on master errors (capped 60s) and gracefully releases the master's running-deployment lock on shutdown.

## Document Index

| Doc | Topic |
|---|---|
| [01-architecture-and-config.md](01-architecture-and-config.md) | Server bootstrap, lifecycle, configuration system, deployment model, install/entrypoint |
| [02-auth-and-rbac.md](02-auth-and-rbac.md) | Authentication, sessions, roles, project permissions, CSRF, account tokens |
| [03-database-schema.md](03-database-schema.md) | Full database schema — every table, column, index, relationship |
| [04-api-reference.md](04-api-reference.md) | Complete REST API surface (endpoints, bodies, responses, auth) |
| [05-projects-and-deployments.md](05-projects-and-deployments.md) | Projects, deployments, Docker, Compose, nodes, paths, commands, worker runtime |
| [06-frp-and-cloudflare.md](06-frp-and-cloudflare.md) | FRP tunnels, Cloudflare tunnels/DNS, R2, SSH |
| [07-ai-agents.md](07-ai-agents.md) | Agent lifecycle, tasks, tools, events, AI providers, Codex |
| [08-mcp-server.md](08-mcp-server.md) | MCP transports, tools, resources, tokens |
| [09-services-misc.md](09-services-misc.md) | Accounts, audit, backups, crypto, passwords, settings, system, tokens, turnstile, GitHub webhooks |
| [10-frontend-ui.md](10-frontend-ui.md) | Client views, components, API client, UI surface |
| [11-security-and-direction.md](11-security-and-direction.md) | Security model, known limitations, improvement direction |
