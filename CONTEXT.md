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

**Hosted Registry**:
The private container image registry Yanto runs on the Master when the operator enables it. Projects can deploy images pushed to it.
_Avoid_: Docker Hub, repo, local registry

**Registry Credential**:
Stored login for an external registry (e.g. GHCR, Docker Hub), used to pull private images.
_Avoid_: Registry login, pull secret

**Cron Entry**:
A line in the host crontab that Yanto created and manages.
_Avoid_: Scheduled job, task, timer

**Backup**:
A local dump of a Project's data (volumes or database) or of Yanto's own database, which the operator can download.
_Avoid_: Snapshot, export

### Access

**Deploy Key**:
An SSH key Yanto uses to clone a Project's private Git repository.
_Avoid_: SSH key (ambiguous), Git key

**Authorized Key**:
A public key allowed to SSH into a Node, managed by Yanto in that Node's `authorized_keys`.
_Avoid_: SSH key (ambiguous), login key

**API Token**:
A scoped bearer token (`read` < `write` < `admin`) that authenticates REST and MCP clients.
_Avoid_: MCP token, PAT, access key

### People

**Owner**:
The single account with unrestricted access, created first.
_Avoid_: Admin, root, superuser

**Member**:
Any non-Owner account; it sees only the Projects it is granted, with per-Project permissions.
_Avoid_: User (ambiguous), operator, guest

## Out of scope

AI providers, coding agents, agent tasks, and Git worktrees are not part of Yanto. The word "agent" is reserved and must not name any Yanto concept.
