# FRP & Cloudflare (Tunnels, DNS, R2, SSH)

Yanto exposes two distinct mechanisms to make a service reachable from the internet:

1. **FRP (Fast Reverse Proxy)** — raw TCP/UDP port-forwarding for cases like Minecraft and SSH, where direct TCP/UDP to a public VPS is required. Yanto manages the **server side** (frps in a Docker container); the **client side** (frpc) is the user's responsibility — Yanto only generates the config and an install script.
2. **Cloudflare Tunnels (cloudflared)** — HTTP/HTTPS reverse proxy via Cloudflare's network, used to publish web services on a public hostname. Yanto fully manages the cloudflared connector as a Docker container, creates/deletes tunnels via the Cloudflare API, manages DNS CNAME records, and wires the connector into the project's Docker network.

Both are modeled per **deployment node** (a node is a host that runs containers). A project targets a specific node (`targetNodeId`), and tunnels/routes are bound to that node.

---

## 1. FRP (Fast Reverse Proxy)

### Role and model

FRP provides raw TCP/UDP port forwarding from a public VPS to a service on a NATed client machine. Yanto manages the **server side** (frps in a Docker container) and stores tunnel *definitions* in the database, but the **client side** (frpc) is the user's responsibility — Yanto only generates a copyable `frpc.toml` and a bash install script.

### frps server management

- frps runs in a Docker container named `yanto-frps` (configurable via `FRP_CONTAINER_NAME`).
- Yanto controls the container lifecycle: `controlFrpServer("start"|"stop"|"restart")` shells out to `docker start/stop/restart yanto-frps`.
- Server status is derived two ways: `docker inspect --format '{{json .State}}'` for the container state (status + startedAt for uptime), and the frps **dashboard HTTP API** (default `http://frps:7500`, configurable via `FRP_DASHBOARD_URL`) for version, traffic totals, clients, and proxies.

### frps.toml config structure (template, rendered from env vars)

- `bindAddr = "0.0.0.0"`, `bindPort = {{ .Envs.FRP_BIND_PORT }}` (default 7000) — the frpc control connection port.
- `transport.tls.force = true` — TLS forced on all client connections.
- `maxPortsPerClient = 20` — caps how many proxies a single frpc may register.
- `allowPorts = [{ start = {{ .Envs.FRP_PORT_START }}, end = {{ .Envs.FRP_PORT_END }} }]` — restricts public forward ports to a configured range (default 25560–25600).
- Auth: `auth.method = "token"`, `auth.additionalScopes = ["HeartBeats","NewWorkConns"]`, token read from a file at `/data/frp/auth-token` (`auth.tokenSource.type = "file"`).
- Dashboard/web server on port 7500, user `yanto`, password from `FRP_DASHBOARD_PASSWORD` env var.
- Logging to console at info level.

### Token / auth

- The shared auth token lives at `config.frpTokenPath` (default `/data/frp/auth-token`), read via `readFrpToken()`. If absent/empty, dashboard requests throw.
- Dashboard API requests use HTTP Basic auth: `Authorization: Basic base64("yanto:" + token)`, with a 2.5s timeout.
- The same token is embedded into the generated `frpc.toml` so the client can authenticate to frps.

### Port allocation

- Configurable range `FRP_PORT_START`..`FRP_PORT_END` (default 25560–25600). `validateFrpRemotePort` rejects ports outside the range with HTTP 400.
- Uniqueness is enforced by a DB unique index on `(protocol, remotePort)` — a duplicate insert yields HTTP 409 "That public port is already used by another tunnel with the same protocol."

### Tunnel lifecycle (DB records)

Tunnels are rows in `frp_tunnels`: `{ id, nodeId, name, protocol(tcp|udp), localHost, localPort, remotePort, enabled, syncStatus, lastError, lastSyncedAt, ... }`.

- **Create:** validates remote port, requires the public host setting to be saved if the tunnel is enabled, inserts a row. `syncStatus` starts as `offline` (if enabled) or `disabled`.
- **Update:** re-validates, resets `syncStatus` to `offline`/`disabled`, clears `lastError`.
- **Delete:** just removes the DB row. There is no remote frpc to push to — the user must update/restart frpc on the client.
- Enabling a tunnel requires the "VPS public endpoint" (`frp.public_host` app setting) to be configured first.

### Public settings

`publicFrpSettings` / `saveFrpSettings` manage the `frp.public_host` app setting — the public IP or DNS-only hostname of the VPS. The guide explicitly warns **not** to use a Cloudflare-proxied hostname here, because Minecraft TCP/UDP must hit the VPS directly.

### Tunnel status / overview

`frpOverview` joins `frp_tunnels` to `deployment_nodes` and correlates each tunnel with a proxy reported by the frps dashboard:
- Matches a dashboard proxy to a tunnel by proxy name (matching tunnel id, tunnel name, or the suffix after `.`/`/`), by protocol type, or by `remotePort`.
- `tunnelStatus`: `disabled` if not enabled; `error` if `lastError`/`syncStatus==="error"`; `online` if the matched dashboard proxy status is "online"; else `offline`.
- Per-tunnel traffic (`todayTrafficIn/Out`, fallback `trafficIn/Out`) and current connections (`curConns`) are surfaced.

### frpc config + install script generation

`frpClientSetup` produces everything the user needs to run frpc on the client machine:

- `buildFrpcToml` generates a complete `frpc.toml`:
  - `serverAddr`, `serverPort` (bind port), `loginFailExit = false`, fixed `clientID`/`user = "yanto-manual-frpc"`.
  - `[auth]` method token, the auth token (or placeholder `PASTE_FRP_TOKEN_HERE`), `additionalScopes = ["HeartBeats","NewWorkConns"]`.
  - `[transport]` `wireProtocol = "v2"`, `[transport.tls] enable = true`.
  - `[log]` to console, info.
  - One `[[proxies]]` block per enabled tunnel: `name = tunnel.id`, `type`, `localIP = localHost`, `localPort`, `remotePort`.
  - If no enabled tunnels exist, a sample SSH tunnel (local 127.0.0.1:22 → remote 6000 or port-start) is emitted.
- `buildFrpcInstallScript` wraps the toml in a bash script that: requires root, detects arch (amd64/arm64/arm), downloads frp v0.69.0 from GitHub releases, installs `frpc` to `/opt/frp`, writes the toml to `/etc/frp/frpc.toml` (mode 600), and creates+enables a systemd unit `frpc.service` (`Restart=always`) — or prints a manual run command if systemd is absent.

### How tunnels map to deployments/containers

FRP tunnels are associated with a `nodeId` (deployment node) but are **not** wired into Docker networks by Yanto. The tunnel's `localHost:localPort` is whatever the user specifies (typically `127.0.0.1:<service port>` on the client machine running frpc). Yanto does not run frpc; it only records the desired forward and gives the user the config to run it.

---

## 2. Cloudflare Integration

### Cloudflare features used

- **Cloudflare Tunnels (`cloudflared` / `cfd_tunnel`)** — managed remotely via the Cloudflare REST API (`https://api.cloudflare.com/client/v4`), and locally as a Docker container per tunnel.
- **DNS records** — CNAME (and arbitrary A/AAAA/MX/TXT/etc.) record CRUD against a zone.
- **Zones** — listed/validated per account.
- **Tunnel configurations (ingress rules)** — pushed via the API to define hostname→service routing inside the tunnel.

### Settings & clients

- Global Cloudflare settings (`cloudflare.tunnel` app setting): `{ accountId, zoneId, apiToken }`, with the API token **encrypted at rest** and never returned in public views (only `hasApiToken`).
- `saveCloudflareSettings` also upserts a `cloudflare_clients` row (keyed by `accountId`) so multiple Cloudflare accounts/clients can coexist. Each client stores its own `accountId`, `zoneId`, encrypted `apiToken`.
- `validateCloudflareSettings` / `validateCloudflareClient` verify credentials by calling `GET /accounts/{id}` and `GET /zones/{id}` (or listing zones) and returning the account/zone names.
- Clients can be listed/created/updated/deleted; deletion is blocked if tunnels still reference the client.

### Cloudflare API client (`cfFetch`)

- Bearer token auth (`Authorization: Bearer <apiToken>`), JSON content type.
- Base URL `https://api.cloudflare.com/client/v4`.
- Parses the standard `{ success, result, errors }` envelope; on `success=false` throws HTTP 400 with the joined error messages; on non-OK HTTP throws HTTP 400 with the response text (truncated to 500 chars).

### Tunnel management

**Create managed tunnel** (`createManagedTunnel`):
1. `POST /accounts/{accountId}/cfd_tunnel` with `{ name, config_src: "cloudflare" }` → returns Cloudflare tunnel id + name.
2. `GET /accounts/{accountId}/cfd_tunnel/{id}/token` → returns the connector token (stored encrypted in DB).
3. Creates a dedicated Docker network `yanto-cf-<id>` (labeled `yanto.cloudflared.tunnel=<id>`).
4. Inserts a `cloudflare_tunnels` row.
5. Pushes the initial tunnel config (`putTunnelConfig`) and starts the cloudflared container.
6. On any failure: removes the Docker network, deletes the Cloudflare tunnel via API, and rethrows.

**Delete managed tunnel** (`deleteManagedTunnel`): blocks unless `force` is set when routes/assignments exist; with `force` it deletes each route's DNS record, removes the cloudflared container, disconnects everything from the tunnel network, removes the network, `DELETE /accounts/{accountId}/cfd_tunnel/{cfTunnelId}`, deletes the DB row.

**Ensure tunnel for node** (`ensureTunnelForNode`): one tunnel per node (looked up by `nodeId`); if none, creates one named `yanto-<nodeId>` using the first available client.

### Tunnel ingress config (`putTunnelConfig`)

- Reads all routes for the tunnel, takes the **enabled** ones, and builds an ingress array: one entry per route `{ hostname, service: serviceTarget, originRequest: { noTLSVerify: true } | {} }`, terminated by a catch-all `{ service: "http_status:404" }`.
- Pushed via `PUT /accounts/{accountId}/cfd_tunnel/{cfTunnelId}/configurations` with `{ config: { ingress } }`.

### cloudflared runtime (Docker)

- Container name: `yanto-cloudflared-<tunnelId>` (legacy name `yanto-cloudflared-<nodeId>` is also cleaned up).
- `startCloudflared`: removes any stale container, ensures the Docker network exists, writes a per-tunnel env file `<cloudflaredDir>/<tunnelId>.env` containing `TUNNEL_TOKEN=<token>` (mode 600), then `docker run -d --name ... --restart unless-stopped --label yanto.cloudflared.tunnel=<id> --network <dockerNetworkName> --env-file <env> cloudflare/cloudflared:latest tunnel --no-autoupdate run`. Marks status `active`.
- `stopCloudflared` / `restartCloudflared` / `getCloudflaredStatus` (parses `docker ps --filter name=... --format {{json .}}`) / `ensureCloudflaredRunning`.
- `ensureEnabledCloudflaredConnectors`: for every tunnel that has enabled routes, ensures the container is running and re-connects it to each enabled route's project network.

### Tunnel network assignments

A tunnel can expose specific containers/compose-services by attaching them to the tunnel's Docker network (`cloudflareTunnelAssignments`):
- `targetType` is `compose_service` (uses `composeProject` + `composeService`, filtered by Docker compose labels, connected with a network alias derived from `<project>-<service>`) or `container` (filtered by name).
- `createTunnelAssignment` inserts the row then `docker network connect [--alias <host>] <tunnelNetwork> <containerId>`; rolls back the row on failure.
- `deleteTunnelAssignment` blocks if a route still uses the assignment; otherwise disconnects the container(s) and deletes the row.
- `reconcileTunnelAssignments` recreates missing networks, ensures cloudflared running, and reconnects all assignments (used for self-healing).

### Connecting cloudflared to a project network

`connectCloudflaredToProjectNetwork`: ensures the cloudflared container is running, checks if it's already on the project's compose network (`<folderName>_default`), and if not runs `docker network connect <projectNetwork> <cloudflaredContainer>` (tolerating "already exists"). This is what lets the tunnel reach the project's containers by service name.

### DNS record management

Two layers:
- **Generic DNS CRUD** (per stored settings or per client): `listDnsRecords` (`GET /zones/{zoneId}/dns_records?per_page=200&order=type`), `createDnsRecord` (POST), `updateDnsRecord` (PATCH), `deleteDnsRecord` (DELETE). Supports types A/AAAA/CNAME/MX/TXT etc.; `proxied` only applied to A/AAAA/CNAME; MX carries `priority`; optional `comment`. Both a global-settings variant and a per-client variant exist.
- **Tunnel-specific CNAME upsert/delete** (`upsertTunnelDnsRecord`, `deleteTunnelDnsRecord`): manages the CNAME that points a hostname to `<cfTunnelId>.cfargotunnel.com` (proxied=true). Upsert: lists existing CNAMEs for the hostname, updates content if it differs, or creates one. Delete: matches by id or by content=target and removes.

### Tunnel health

`getTunnelHealth`: `GET /accounts/{cfAccountId}/cfd_tunnel/{cfTunnelId}` → `{ healthy: status==="healthy", connectors: connections.length, status }`.

### Diagnostics

`listRouteDiagnostics` builds a per-route diagnostic report. For each route it gathers:
- The expected DNS target (`<cfTunnelId>.cfargotunnel.com`) and the actual DNS records for the hostname (from a single zone-wide DNS list call, best-effort).
- Tunnel runtime running state and Cloudflare-reported health, cached per-tunnel across the loop.
- A preliminary classification, then — only if the route is enabled, DNS is ok, and the tunnel is running — performs an HTTPS reachability check (`HEAD` then `GET` on `https://<hostname>` with a 3.5s timeout; status <400 = ok, 405 triggers a GET fallback).
- Final classification via `classifyRouteDiagnostic`.

`classifyRouteDiagnostic` classifies:
- **DNS status:** `ok` (single matching CNAME to expected target), `missing` (no records), `mismatch` (CNAME points elsewhere), `conflict` (matching CNAME plus other same-name records, or non-CNAME records), `unknown` (DNS couldn't be checked).
- **Tunnel status:** `missing`, `stopped` (runtime not running), `unhealthy` (Cloudflare reports unhealthy), `running`, `unknown`.
- **Reachability status:** `ok` / `failed` / `skipped` / `unknown`.
- Produces human-readable `messages` and `recommendedFixes` (e.g., "Create a CNAME for X pointing to Y", "Restart cloudflared", "Check the project container, service target, and Cloudflare route").

---

## 3. Cloudflare Route Publication Flow

The publication of a project to a public hostname is orchestrated by `runRoutePublicationLifecycle`, a small ordered-step coordinator that keeps the DB route recoverable when non-transactional external systems fail. Steps, in order:

1. `publishDns()` → upsert the CNAME for the hostname to `<cfTunnelId>.cfargotunnel.com`; returns the DNS record id (or null if no zone configured).
2. `publishIngress()` → push the tunnel ingress config for the new tunnel; if the route moved from a different tunnel, also re-push the previous tunnel's config to drop the hostname.
3. `prepareRuntime()` → ensure cloudflared running + connect cloudflared to the project network.
4. `cleanupPrevious()` → if the hostname changed, delete the old CNAME from the previous tunnel's target.
5. `markActive(dnsRecordId)` → set route `syncStatus="active"`, `cfDnsRecordId`, `lastPublishedAt`, clear `lastError`.

On any error: `markError(message, dnsRecordId)` rolls back — deletes the just-created DNS record (if the hostname changed / new route), restores the route's previous DB fields (or marks a new route `enabled=false`/`syncStatus="error"` with the message), and re-pushes tunnel configs to revert ingress. The original error is rethrown.

### `publishProjectRoute` (the caller)

- Resolves the project, picks the target node (`nodeId` arg or `project.targetNodeId`), and `ensureTunnelForNode` (creates a tunnel for that node if needed).
- Loads stored Cloudflare settings and any existing route for the project (one route per project).
- Normalizes `noTlsVerify` (only meaningful for `https://` service targets); derives `protocol` and `port` from the `serviceTarget` string.
- Either updates the existing route row or inserts a new one (`syncStatus="pending"`), then runs the lifecycle above.

### Related route operations

- `disableProjectRoute` / `enableProjectRoute`: flip `enabled`, re-push tunnel ingress; enable also ensures cloudflared running and reconnects to the project network.
- `deleteProjectRoute`: delete the DNS CNAME, delete the DB row, re-push tunnel ingress.
- `createManagedHostname` / `retryManagedHostname` / `deleteManagedHostname`: a parallel "managed hostnames" path tied to a **tunnel assignment** (compose service or container) rather than a project. `createManagedHostname` builds `serviceTarget = "<protocol>://<assignmentHost>:<port>"`, inserts the route, pushes ingress, upserts DNS, marks active; on failure marks `syncStatus="error"` and re-pushes ingress. `deleteManagedHostname` deletes DNS, the row, and re-pushes ingress, returning any warnings.

---

## 4. R2 (Cloudflare Object Storage)

### Purpose

R2 is used for **backup storage**. `uploadFileToR2` is called to upload backup artifacts.

### Settings

Stored under the `cloudflare.r2` app setting: `{ enabled, accountId, bucket, accessKeyId, secretAccessKey, prefix }`. Public view redacts secrets. Upload is gated on `enabled` and completeness of `accountId`/`bucket`/`accessKeyId`/`secretAccessKey`; `accountId` must be 32 hex chars; `bucket` must be a valid 3–63 char lowercase DNS-compliant name. `prefix` defaults to `"postgres-dumps"`.

### Upload mechanism

`uploadFileToR2` performs a single `PUT` to the R2 S3-compatible endpoint `https://<accountId>.r2.cloudflarestorage.com/<bucket>/<objectKey>` using **AWS Signature V4** computed by hand (region `auto`, service `s3`):
- Streams the file body, computes SHA-256 of the file for `x-amz-content-sha256`.
- Builds the canonical request, string-to-sign, derives the signing key (`AWS4<secret>` → date → `auto` → `s3` → `aws4_request` via HMAC-SHA256), and sends `Authorization: AWS4-HMAC-SHA256 Credential=.../.../auto/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=...`.
- `objectKey` = `<safePrefix(settings.prefix)>/<filename>`, where `safePrefix` lowercases and slugifies each path segment.
- Returns `{ bucket, key, endpoint, filename, size }`.

There is **no download/list/delete** implementation — only upload. (Downloads happen via direct R2 URLs or another path.)

---

## 5. SSH

### Purpose

SSH is used for **Git access** (cloning/pulling project repos over SSH) and per-project deploy keys, not for running arbitrary remote commands on nodes.

### Key management

- `ensureSshKey(projectId)`: ensures `config.sshKeysDir` (default `/tmp/yanto-ssh`, mode 700) exists; if `<projectId>.pub` doesn't exist, runs `ssh-keygen -t ed25519 -N "" -C yanto-<projectId> -f <projectId>`, then chmod 600 the private key. Returns `{ privateKeyPath, publicKey }`. This generates a per-project ed25519 key pair (a deploy key).
- **Managed key** (a single global key for Git operations):
  - `generateManagedSshPrivateKey`: creates `config.managedSshPrivateKeyPath` (default `/tmp/yanto-ssh/id_ed25519`) via `ssh-keygen -t ed25519 -N "" -C yanto-managed`, chmod 600. Refuses if it already exists.
  - `saveManagedSshPrivateKey(privateKey)`: validates the PEM has `-----BEGIN ... PRIVATE KEY-----`, writes it (mode 600), then derives the public key via `ssh-keygen -y -f <private>` and writes `<path>.pub`. Cleans up the private key if public derivation fails.
  - `managedSshKeyStatus`: reports `hasManagedKey`, `hasMountedKey` (a host-mounted key at `config.sshPrivateKeyPath`, default `/root/.ssh/id_ed25519`), `activePrivateKeyPath` (resolved by `resolveGitPrivateKeyPath`: prefers managed, falls back to mounted, else null), and the managed public key.
- `gitSshEnv(privateKeyPath)`: builds the `GIT_SSH_COMMAND` env var: `ssh -o IdentitiesOnly=yes -o StrictHostKeyChecking=<config.sshStrictHostKeyChecking> -o UserKnownHostsFile=/tmp/yanto_known_hosts [-i <privateKeyPath>]`. `StrictHostKeyChecking` defaults to `accept-new`. This env is passed to Git invocations so they authenticate with the right key and don't hang on host-key prompts.

### What it connects to

SSH here is for **Git remotes** (e.g., private GitHub/GitLab repos used as project sources) — there is no general "run command over SSH on a node" facility. Remote node execution is handled by worker nodes polling the master over HTTP using `WORKER_TOKEN`, not SSH.

---

## 6. How FRP, Cloudflare, and Deployments Relate

- **Deployment nodes** are the unit a project targets (`projects.targetNodeId`) and that tunnels bind to. Both FRP tunnels (`frp_tunnels.nodeId`) and Cloudflare tunnels (`cloudflare_tunnels.nodeId`, one per node) reference a node.
- **FRP** is the **direct TCP/UDP** path: VPS public IP + public port → frps (Docker on master) → frpc (user-run on NATed client) → local service. Yanto manages frps and the tunnel definitions, and gives the user a ready-to-run frpc config/script. It does **not** touch Docker networks or the project's containers. Use cases: Minecraft Java (TCP 25565), Minecraft Bedrock (UDP 19132), SSH, etc. The public host must be a direct (non-Cloudflare-proxied) address.
- **Cloudflare Tunnels** are the **HTTP/HTTPS web** path: public hostname → Cloudflare edge → cloudflared (Docker on the node, managed by Yanto) → project container over a shared Docker network. Yanto fully manages: tunnel creation/deletion (API), the cloudflared container, a per-tunnel Docker network, attaching project containers/compose-services to that network (so cloudflared can reach them by service name), the tunnel ingress config (hostname→service mapping), and the DNS CNAME (hostname → `<cfTunnelId>.cfargotunnel.com`, proxied).
- A **project** gets **one Cloudflare route** (`cloudflare_routes.projectId`, unique per project via the publication flow) published to a hostname on the node's tunnel. There is also a more general "managed hostname" path tied to a tunnel **assignment** (any compose service/container) rather than a project.
- The two systems are **independent and complementary**: FRP for raw ports on a VPS, Cloudflare for web hostnames. They share the `deployment_nodes` abstraction and the app-settings/config infrastructure but do not call into each other.
- **R2** is orthogonal storage for **backups** of project data, using S3 Signature V4 against the R2 endpoint; only upload is implemented.
- **SSH** is for Git-based project source access (per-project deploy keys + one managed key), feeding the deployment pipeline rather than remote node control.

## Integration Contracts Summary

| Subsystem | External API / Process | Key endpoints / commands |
|---|---|---|
| FRP server | Docker (frps container) | `docker inspect/start/stop/restart yanto-frps` |
| FRP dashboard | HTTP (frps) | `GET /api/serverinfo`, `/api/clients`, `/api/proxy/tcp`, `/api/proxy/udp` (Basic auth `yanto:<token>`) |
| FRP client | none (user-run) | Yanto emits `frpc.toml` + bash install script (downloads frp v0.69.0, systemd unit) |
| Cloudflare API | HTTPS REST | `POST/GET/DELETE /accounts/{id}/cfd_tunnel`, `GET .../token`, `PUT .../configurations`, `GET /accounts/{id}`, `GET/POST/PATCH/DELETE /zones/{id}/dns_records`, `GET /zones?account.id=...` |
| cloudflared | Docker | `docker run/rm/stop/ps/inspect/network create\|connect\|disconnect\|rm` for `yanto-cloudflared-<id>` and `yanto-cf-<id>` networks |
| R2 | HTTPS (S3 SigV4) | `PUT https://<accountId>.r2.cloudflarestorage.com/<bucket>/<key>` |
| SSH | local `ssh-keygen` | `ssh-keygen -t ed25519 -y/-f ...`; `GIT_SSH_COMMAND=ssh -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/tmp/yanto_known_hosts -i <key>` |
