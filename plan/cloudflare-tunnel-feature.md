# Cloudflare Tunnel Feature Plan

## Goal

Let Yanto publish a deployed service to a Cloudflare-managed domain without opening the Cloudflare dashboard.

Target flow:

```text
landingpage.domain.com -> Cloudflare HTTPS edge -> Cloudflare Tunnel -> 127.0.0.1:3000
```

Important nuance: the public visitor gets HTTPS at Cloudflare. The origin behind `cloudflared` can still be `http://127.0.0.1:3000`.

## Docs Checked

- Cloudflare Tunnel API guide: create remote tunnel, put ingress config, create CNAME, install `cloudflared`.
- Cloudflare Tunnel API reference: `POST /accounts/{account_id}/cfd_tunnel`, `PUT /accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations`, token retrieval, and supported ingress `service` values.
- Cloudflare Tunnel routing docs: public hostnames map to local services and DNS points at `<UUID>.cfargotunnel.com`.
- Cloudflare DNS routing docs: CNAME records are independent from tunnel runtime; stopped tunnels can produce `1016`.

References:

- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel-api/
- https://developers.cloudflare.com/api/node/resources/zero_trust/subresources/tunnels/subresources/cloudflared/
- https://developers.cloudflare.com/tunnel/routing/
- https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/routing-to-tunnel/dns/

## Product Scope

Add Cloudflare Tunnel management to Yanto so a user can:

- Save Cloudflare account, zone, and API token settings.
- Create or reuse a tunnel for a deployment node.
- Add a hostname route for a project service.
- Point the hostname DNS record to the tunnel.
- Run `cloudflared` on the node where the service is reachable.
- See route health and copy the final URL.

Initial example:

```text
hostname: landingpage.domain.com
service: http://127.0.0.1:3000
node: node_master_local
```

## Recommended Architecture

Use one remotely-managed Cloudflare Tunnel per deployment node.

Reasoning:

- `127.0.0.1` is local to the machine running `cloudflared`.
- Yanto already has `deployment_nodes`; tunnel ownership should attach to the node that hosts the Docker service.
- A single tunnel can publish multiple applications by adding multiple ingress rules.
- This avoids creating one long-running `cloudflared` process per project.

Runtime shape:

```text
Cloudflare API
  |
  | create tunnel, write config, create DNS
  v
Yanto master API
  |
  | stores route/tunnel metadata
  v
Target node
  |
  | runs cloudflared using tunnel token
  v
Project service on 127.0.0.1:<port>
```

## Cloudflare Operations

Settings validation:

- Require API token permissions:
  - Account: `Cloudflare Tunnel Edit` or current Cloudflare One connector write equivalent.
  - Zone: `DNS Edit`.
- Store:
  - `accountId`
  - `zoneId`
  - `zoneName`
  - API token secret

Tunnel provisioning:

```http
POST /client/v4/accounts/{account_id}/cfd_tunnel
{
  "name": "yanto-{node_id}",
  "config_src": "cloudflare"
}
```

Tunnel token:

```http
GET /client/v4/accounts/{account_id}/cfd_tunnel/{tunnel_id}/token
```

Ingress config:

```http
PUT /client/v4/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations
{
  "config": {
    "ingress": [
      {
        "hostname": "landingpage.domain.com",
        "service": "http://127.0.0.1:3000",
        "originRequest": {}
      },
      {
        "service": "http_status:404"
      }
    ]
  }
}
```

DNS route:

```http
POST /client/v4/zones/{zone_id}/dns_records
{
  "type": "CNAME",
  "proxied": true,
  "name": "landingpage.domain.com",
  "content": "{tunnel_id}.cfargotunnel.com"
}
```

Use upsert behavior in Yanto:

- Search existing DNS record by name/type.
- If it already points to this tunnel, keep it.
- If it exists but points elsewhere, require confirmation before replacing.
- If missing, create it.

## Data Model

Add tables instead of only using `app_settings`, because routes need project/node relationships and status.

`cloudflare_tunnels`

- `id`
- `nodeId`
- `cloudflareTunnelId`
- `name`
- `status`
- `configVersion`
- `createdAt`
- `updatedAt`
- `lastCheckedAt`
- `lastError`

`cloudflare_routes`

- `id`
- `projectId`
- `nodeId`
- `tunnelId`
- `hostname`
- `service`
- `enabled`
- `dnsRecordId`
- `status`
- `lastCheckedAt`
- `lastError`
- `createdAt`
- `updatedAt`

Settings in `app_settings`:

- `cloudflare.tunnel`
- Public shape masks token presence like existing R2 settings.
- Secret token should be kept out of API responses.

Future hardening:

- Encrypt stored API token and tunnel tokens with `JWT_SECRET` or a new `SETTINGS_ENCRYPTION_KEY`.
- Add token rotation support.

## Backend Services

Add `src/server/services/cloudflare.ts`.

Responsibilities:

- `getCloudflareSettings`
- `saveCloudflareSettings`
- `validateCloudflareSettings`
- `ensureTunnelForNode`
- `getTunnelToken`
- `putTunnelConfig`
- `upsertTunnelDnsRecord`
- `publishProjectRoute`
- `disableProjectRoute`
- `syncTunnelIngress`
- `checkTunnelStatus`

Implementation notes:

- Use native `fetch`, matching the repo style.
- Wrap Cloudflare API errors with clear user-facing messages.
- Generate ingress config from enabled `cloudflare_routes` for the tunnel plus a final `http_status:404` catch-all.
- Keep writes transactional where local DB state changes depend on successful Cloudflare calls.
- Record audit events:
  - `settings.cloudflare.save`
  - `cloudflare.tunnel.create`
  - `cloudflare.route.publish`
  - `cloudflare.route.disable`
  - `cloudflare.dns.upsert`

## Node Runtime

Master node MVP:

- Ensure `cloudflared` runs in the Yanto Docker Compose stack when Cloudflare Tunnel is enabled.
- Prefer a sidecar service using the official `cloudflare/cloudflared` image:

```yaml
cloudflared:
  image: cloudflare/cloudflared:latest
  command: tunnel --no-autoupdate run --token ${CLOUDFLARED_TOKEN}
  restart: unless-stopped
```

But because tunnel tokens are created after settings are saved, Yanto needs one of these approaches:

- Write a managed env file in the app data volume, then restart the sidecar.
- Or run `cloudflared` from Yanto using Docker API / `docker run` with labels.

Recommended MVP:

- Use Docker-managed `cloudflared` container per node.
- Label it with `yanto.cloudflared.node=<nodeId>`.
- Store token in a root-only file under `/data/cloudflared/{nodeId}.env`.
- Restart container when token or tunnel changes.

Worker-node follow-up:

- Add worker job type `cloudflare_tunnel_sync`.
- Master creates tunnel/config/DNS.
- Worker receives tunnel token and desired runtime state.
- Worker starts or restarts local `cloudflared`.

## API Routes

Settings:

- `GET /api/settings` includes `cloudflareTunnel`.
- `POST /api/settings/cloudflare-tunnel` saves account/zone/token.
- `POST /api/settings/cloudflare-tunnel/validate` validates token and zone access.

Tunnels:

- `GET /api/cloudflare/tunnels`
- `POST /api/cloudflare/tunnels/:nodeId/ensure`
- `POST /api/cloudflare/tunnels/:id/sync`
- `GET /api/cloudflare/tunnels/:id/status`

Routes:

- `GET /api/projects/:id/cloudflare-routes`
- `POST /api/projects/:id/cloudflare-routes`
- `PATCH /api/projects/:id/cloudflare-routes/:routeId`
- `DELETE /api/projects/:id/cloudflare-routes/:routeId`

Create route payload:

```json
{
  "hostname": "landingpage.domain.com",
  "service": "http://127.0.0.1:3000",
  "nodeId": "node_master_local"
}
```

Validation:

- Hostname must belong to configured zone.
- Service must use a Cloudflare-supported scheme.
- For MVP, allow `http://`, `https://`, `tcp://`, `ssh://`, `rdp://`.
- Require confirmation if DNS record exists and is not owned by Yanto.

## Frontend UX

Settings page:

- Add `Cloudflare Tunnel` panel near existing R2 settings.
- Fields:
  - Enable Cloudflare Tunnel
  - Account ID
  - Zone ID
  - Zone name
  - API token
  - Validate button
- Show token saved state without revealing token.

Project page:

- Add a `Public hostnames` section to each project detail modal.
- Fields:
  - Hostname
  - Service URL
  - Target node
- Actions:
  - Publish
  - Sync
  - Disable
  - Copy URL
- Show statuses:
  - DNS created
  - Tunnel running
  - Last sync failed

Good default:

- If a container exposes one host port, suggest `http://127.0.0.1:<hostPort>`.
- Otherwise let the user type the service manually.

## Implementation Phases

### Phase 1: Backend Cloudflare Client

- Add route schemas for Cloudflare settings and route creation.
- Add Cloudflare settings storage using existing `app_settings` style.
- Add typed Cloudflare API client helpers.
- Add validation endpoint.
- Unit test settings masking and API error handling.

### Phase 2: Database and Route Publishing

- Add Drizzle migration for `cloudflare_tunnels` and `cloudflare_routes`.
- Implement `ensureTunnelForNode`.
- Implement DNS record upsert.
- Implement route create/disable.
- Generate tunnel ingress from DB routes with final `http_status:404`.
- Audit all mutating actions.

### Phase 3: Local cloudflared Runtime

- Add service to start/restart/stop a Docker-managed `cloudflared` container for master node.
- Persist token in `/data/cloudflared`.
- Add runtime health check.
- Show tunnel status in API.

### Phase 4: Frontend

- Add Settings panel.
- Add project route UI.
- Add status badges, error states, and copy URL action.
- Keep R2 and SSH panels unchanged.

### Phase 5: Worker Nodes

- Extend worker job model for tunnel runtime sync.
- Worker starts local `cloudflared` with the node tunnel token.
- Worker reports connector status to master.

## Tests

Backend:

- Settings save masks token in public response.
- Hostname zone validation accepts `landingpage.domain.com` for `domain.com`.
- Service validation rejects unsupported schemes.
- Ingress config always appends `http_status:404`.
- DNS upsert creates missing record.
- DNS upsert refuses conflicting record unless confirmed.
- Route disable removes ingress and optionally leaves DNS record with warning.

Runtime:

- Starting cloudflared creates a labeled container.
- Restarting cloudflared replaces stale token/env.
- Missing Docker reports actionable error.

Frontend:

- Settings form preserves saved token when token field is blank.
- Project route form blocks invalid hostname/service.
- Successful publish shows HTTPS URL.

## Edge Cases

- Multi-level subdomains may need an advanced Cloudflare certificate.
- Existing DNS records may point elsewhere.
- Tunnel can be configured while `cloudflared` is stopped; users may see Cloudflare `1016`.
- `127.0.0.1` must be interpreted from the target node, not the browser or master unless the master is the target node.
- If a project moves nodes, its Cloudflare route must move to that node tunnel.
- If a user deletes a project, disable its route and resync ingress.
- Avoid deleting DNS automatically unless the user asks; DNS may be shared intentionally.

## MVP Acceptance Criteria

- Admin can save Cloudflare account, zone, and token.
- Admin can publish `landingpage.domain.com` to `http://127.0.0.1:3000`.
- Yanto creates/reuses a node tunnel.
- Yanto writes tunnel ingress config.
- Yanto creates proxied CNAME to `{tunnel_id}.cfargotunnel.com`.
- Yanto starts `cloudflared` for the local node.
- Project page shows the final `https://landingpage.domain.com` URL.
- Audit log records settings, tunnel, DNS, and route changes.
