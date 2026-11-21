# Yanto

Yanto is a self-hosted VPS management tool: it deploys and operates containerized workloads on servers the operator owns. It is devops-only — it never writes or edits application code.

## Language

### Workloads

**Project**:
A named deployable unit backed by either a Git source or a container image, run on a Node with Docker Compose.
_Avoid_: App, service, stack

**Deployment**:
One attempt to bring a Project to a desired revision, with its own status and log.
_Avoid_: Build, release, run

**Deploy Token**:
A per-Project secret that lets an external caller (CI, webhook) trigger a Deployment.
_Avoid_: Webhook key, deploy key

### Infrastructure

**Node**:
A server Yanto deploys to. The Node running the dashboard is the **Master**; every other Node is a **Worker**.
_Avoid_: Host, agent, server, machine

### People

**Owner**:
The single account with unrestricted access, created first.
_Avoid_: Admin, root, superuser

**Member**:
Any non-Owner account; it sees only the Projects it is granted, with per-Project permissions.
_Avoid_: User (ambiguous), operator, guest

## Out of scope

AI providers, coding agents, agent tasks, and Git worktrees are not part of Yanto. The word "agent" is reserved and must not name any Yanto concept.
