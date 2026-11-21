# MCP Server

Yanto exposes its entire management plane (projects, deployments, containers, Cloudflare tunneling, FRP tunneling, backups, settings) over the **Model Context Protocol (MCP)** so that an external MCP-compatible agent/client can drive the platform programmatically. The MCP surface is a faithful projection of the same service-layer operations the HTTP REST API uses; it is not a separate feature set.

---

## 1. Transport Modes

Yanto supports **two** MCP transports, both backed by the same server factory.

### HTTP / Streamable HTTP (stateless)

- Mounted on the main app at `POST /mcp`.
- **Stateless**: each request creates a fresh MCP server, handles the request, then closes the server. No session streaming/SSE sessions are kept across requests.
- Non-POST methods on `/mcp` return `405 Use POST for stateless Streamable HTTP MCP requests.`
- Security gates (in order):
  1. `nodeRole === "master"` only — worker nodes get `404`.
  2. **Host/Origin validation** against an allow-list built from localhost variants, the configured `appBaseUrl` host/origin, plus CSV env overrides `MCP_ALLOWED_HOSTS` / `MCP_ALLOWED_ORIGINS`. Mismatch → `403`.
  3. **Bearer token** required in `Authorization` header → `401` if missing/invalid.
  4. **Rate limiting** on auth failures: 20 rejected requests per 15 min per IP (skip successful requests).
- `/mcp` is exempted from the app-wide CSRF protection.

### stdio

- A standalone process entrypoint (`start:mcp:stdio`), intended to be spawned by an MCP client as a child process communicating over stdin/stdout via `StdioServerTransport`.
- Boot sequence: warn on unsafe defaults → require `nodeRole === "master"` → require `YANTO_MCP_TOKEN` env var → run DB migrations → ensure local master node → authenticate the token → build server → connect stdio transport.
- Handles `SIGINT`/`SIGTERM` by closing the server and ending the DB pool.

Both transports require a valid MCP access token; there is no anonymous access.

---

## 2. MCP Tools (88 tools total)

Every tool is registered through a single `registerTool` primitive. Each tool carries an MCP `ToolAnnotations` hint (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) derived from one of six presets: `readOnly`, `externalRead`, `write`, `externalWrite`, `destructive`, `externalDestructive` (the "external" variants set `openWorldHint: true`, signaling the tool touches outside-the-system resources like Cloudflare/R2/remote nodes).

**Destructive/admin tools require `confirm: true` in the input** (enforced by `requireConfirm`, returning HTTP 400 otherwise). All write/admin tools record an audit log entry with actor `mcp:<tokenName>`.

Tool result shape: handlers return a text content block plus a `structuredContent` object. Errors are caught and converted to MCP error results with `isError` flagging rather than throwing to the client.

### System / Health / Audit
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_health` | read | `{}` | Platform health status. |
| `yanto_system_usage` | read | `{}` | System resource usage. |
| `yanto_system_logs` | read | `{ tailChars?: 1..200000 }` | Recent in-memory system log history, truncated to `tailChars` (default 80000). |
| `yanto_audit_list` | read | `{ limit?: 1..500, projectId?: string }` | Lists audit log entries (default 100), optionally filtered by project. |
| `yanto_cleanup_preview` | read | `{}` | Previews what a protected Docker cleanup would remove. |
| `yanto_cleanup` | admin (destructive) | `{ confirm? }` | Runs protected Docker cleanup; requires confirm. |

### Nodes / Multi-node
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_nodes_list` | read | `{}` | Lists deployment nodes. |
| `yanto_worker_join_command_reveal` | admin (destructive) | `{ confirm? }` | Ensures/returns the worker join token plus a curl-pipe-bash install command. Requires confirm. |

### Projects
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_projects_list` | read | `{}` | Lists projects with container counts. |
| `yanto_project_get` | read | `{ id }` | Gets one project. |
| `yanto_project_create` | write | `projectInput` | Creates a project. |
| `yanto_project_update` | write | `projectInput.partial() + { id }` | Updates a project. |
| `yanto_project_delete` | admin (destructive) | `{ id, confirm? }` | Deletes project and its directory; requires confirm. |
| `yanto_project_compose_read` | read | `{ projectId }` | Reads project compose file content. |
| `yanto_project_env_read` | read | `{ projectId, envFile? }` | Reads project env variables with **values masked**. |
| `yanto_project_env_content_read` | read | `{ projectId, envFile? }` | Reads project env file content with **values masked**. |
| `yanto_project_env_preview` | read | `{ content, envFile? }` | Parses/preview an env file content (no project needed). |
| `yanto_project_env_write` | write | `{ projectId, content, envFile? }` | Writes full env file content. |
| `yanto_project_env_variables_write` | write | `{ projectId, variables[], envFile? }` | Patches env variables; masked values are preserved. |
| `yanto_project_deploy_token_reveal` | admin (destructive) | `{ id, confirm? }` | Reveals the project's deploy token; requires confirm. |
| `yanto_project_deploy` | write (externalWrite) | `deploymentInput + { projectId }` | Starts a manual deployment; refuses if `manualDeployEnabled` is false; returns promptly with deployment ID (or "already running"). |
| `yanto_project_rollback_preview` | read (externalRead) | `{ projectId, targetRef }` | Previews rollback target. |
| `yanto_project_rollback` | admin (externalDestructive) | `rollbackInput + { projectId, confirm? }` | Executes a rollback deployment; requires confirm. |
| `yanto_project_stop` | admin (destructive) | `{ id, confirm? }` | Stops a **local** project compose stack (worker nodes unsupported); requires confirm. |
| `yanto_project_restart` | admin (destructive) | `{ id, confirm? }` | Restarts a **local** project compose stack; requires confirm. |

### Deployments
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_deployments_list` | read | `{ limit?: 1..500 }` | Lists latest deployments (default 500). |
| `yanto_deployment_get` | read | `{ id }` | Gets one deployment. |
| `yanto_deployment_logs` | read | `{ id, tailChars?: 1..200000 }` | Reads deployment logs (truncated, default 80000) plus status. |

### Containers (Docker)
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_containers_list` | read | `{}` | Lists Docker containers. |
| `yanto_container_logs` | read | `{ id, tailChars? }` | Reads container logs (truncated). |
| `yanto_container_start` | write | `{ id }` | Starts a container. |
| `yanto_container_stop` | admin (destructive) | `{ id, confirm? }` | Stops a container; requires confirm. |
| `yanto_container_restart` | admin (destructive) | `{ id, confirm? }` | Restarts a container; requires confirm. |

### Backups (Postgres + R2)
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_backups_list` | read | `{ limit?: 1..200 }` | Lists backups (default 50). |
| `yanto_backups_postgres_targets_list` | read | `{}` | Lists Postgres containers eligible for backup. |
| `yanto_backup_create` | write | `{ containerId? }` | Creates a Postgres backup. |
| `yanto_backup_upload_r2` | write (externalWrite) | `{ id }` | Uploads a backup to R2. |
| `yanto_backup_delete` | admin (destructive) | `{ id, confirm? }` | Deletes a backup; requires confirm. |

### Settings
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_settings_read` | read | `{}` | Returns a settings snapshot. |
| `yanto_settings_r2_update` | admin (externalWrite) | `r2SettingsInput + { confirm? }` | Updates R2 settings; credentials write-only; requires confirm. |
| `yanto_settings_cloudflare_update` | admin (externalWrite) | `cloudflareSettingsInput + { confirm? }` | Updates legacy Cloudflare settings; API token write-only; requires confirm. |
| `yanto_settings_cloudflare_validate` | admin (externalRead) | `cloudflareSettingsInput + { confirm? }` | Validates legacy Cloudflare credentials; requires confirm. |
| `yanto_settings_ssh_key_save` | admin (destructive) | `{ privateKey, confirm? }` | Saves managed SSH private key (write-only); requires confirm. |
| `yanto_settings_ssh_key_generate` | admin (destructive) | `{ confirm? }` | Generates a managed SSH key; requires confirm. |
| `yanto_settings_setup_state_update` | write | `{ action: "completed"\|"dismissed" }` | Updates setup wizard state. |
| `yanto_settings_multi_node_update` | write | `{ enabled? }` | Updates multi-node setting. |

### Cloudflare — Clients
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_cloudflare_clients_list` | read | `{}` | Lists Cloudflare clients. |
| `yanto_cloudflare_client_validate` | admin (externalRead) | `cloudflareClientInput + { confirm? }` | Validates a client token; requires confirm. |
| `yanto_cloudflare_client_create` | admin (externalWrite) | `cloudflareClientInput + { confirm? }` | Creates a client; apiToken required, write-only; requires confirm. |
| `yanto_cloudflare_client_update` | admin (externalWrite) | `cloudflareClientInput.partial() + { id, confirm? }` | Updates a client; token write-only; requires confirm. |
| `yanto_cloudflare_client_delete` | admin (externalDestructive) | `{ id, confirm? }` | Deletes a client; requires confirm. |
| `yanto_cloudflare_zones_list` | read (externalRead) | `{ clientId }` | Lists zones for a client. |

### Cloudflare — Tunnels & Assignments
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_cloudflare_tunnels_list` | read | `{}` | Lists managed Cloudflare tunnels. |
| `yanto_cloudflare_tunnel_create` | write (externalWrite) | `{ clientId, name }` | Creates a managed tunnel. |
| `yanto_cloudflare_tunnel_delete` | admin (externalDestructive) | `{ id, force?, confirm? }` | Deletes (or force-deletes) a tunnel; requires confirm. |
| `yanto_cloudflare_assignments_list` | read | `{ tunnelId? }` | Lists tunnel assignments. |
| `yanto_cloudflare_assignment_create` | write | `cloudflareAssignmentInput` | Creates a tunnel assignment. |
| `yanto_cloudflare_assignment_delete` | admin (destructive) | `{ id, confirm? }` | Deletes an assignment; requires confirm. |
| `yanto_cloudflare_tunnel_node_status` | read (externalRead) | `{ nodeId }` | Returns tunnel runtime + health for a node (404 if none). |
| `yanto_cloudflare_tunnel_start` | write (externalWrite) | `{ nodeId }` | Starts cloudflared for a node. |
| `yanto_cloudflare_tunnel_stop` | admin (externalDestructive) | `{ nodeId, confirm? }` | Stops cloudflared; requires confirm. |
| `yanto_cloudflare_tunnel_restart` | admin (externalDestructive) | `{ nodeId, confirm? }` | Restarts cloudflared; requires confirm. |

### Cloudflare — Managed Hostnames
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_cloudflare_hostnames_list` | read | `{}` | Lists managed hostnames. |
| `yanto_cloudflare_hostname_create` | write (externalWrite) | `cloudflareHostnameInput` | Creates a managed hostname route. |
| `yanto_cloudflare_hostname_retry` | write (externalWrite) | `{ id }` | Retries a managed hostname sync. |
| `yanto_cloudflare_hostname_delete` | admin (externalDestructive) | `{ id, confirm? }` | Deletes a managed hostname (returns warnings); requires confirm. |

### Cloudflare — Legacy DNS (global Cloudflare settings)
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_cloudflare_dns_list` | read (externalRead) | `{}` | Lists legacy DNS records. |
| `yanto_cloudflare_dns_create` | write (externalWrite) | `cloudflareDnsRecordInput` | Creates a legacy DNS record. |
| `yanto_cloudflare_dns_update` | write (externalWrite) | `cloudflareDnsRecordInput + { id }` | Updates a legacy DNS record. |
| `yanto_cloudflare_dns_delete` | admin (externalDestructive) | `{ id, confirm? }` | Deletes a legacy DNS record; requires confirm. |

### Cloudflare — Per-client DNS
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_cloudflare_client_dns_list` | read (externalRead) | `{ clientId }` | Lists DNS records for a client. |
| `yanto_cloudflare_client_dns_create` | write (externalWrite) | `{ clientId } + cloudflareDnsRecordInput` | Creates a DNS record under a client. |
| `yanto_cloudflare_client_dns_update` | write (externalWrite) | `{ clientId, id } + cloudflareDnsRecordInput` | Updates a client DNS record. |
| `yanto_cloudflare_client_dns_delete` | admin (externalDestructive) | `{ clientId, id, confirm? }` | Deletes a client DNS record; requires confirm. |

### Cloudflare — Project Routes & Diagnostics
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_cloudflare_routes_diagnostics` | read (externalRead) | `{}` | Lists route diagnostics. |
| `yanto_project_cloudflare_routes_list` | read | `{ projectId }` | Lists Cloudflare routes for a project. |
| `yanto_project_cloudflare_route_publish` | write (externalWrite) | `{ projectId } + cloudflareRouteInput` | Publishes a Cloudflare route for a project. |
| `yanto_cloudflare_route_enable` | write (externalWrite) | `{ id }` | Enables a project route. |
| `yanto_cloudflare_route_disable` | write (externalWrite) | `{ id }` | Disables a project route. |
| `yanto_cloudflare_route_delete` | admin (externalDestructive) | `{ id, confirm? }` | Deletes a project route; requires confirm. |

### FRP
| Tool | Level | Input | Behavior |
|---|---|---|---|
| `yanto_frp_overview` | read | `{}` | Reads FRP overview. |
| `yanto_frp_settings_update` | write | `{ publicHost }` | Updates FRP settings (public host). |
| `yanto_frp_server_start` | write | `{}` | Starts the FRP server. |
| `yanto_frp_server_stop` | admin (destructive) | `{ confirm? }` | Stops the FRP server; requires confirm. |
| `yanto_frp_server_restart` | admin (destructive) | `{ confirm? }` | Restarts the FRP server; requires confirm. |
| `yanto_frp_tunnel_create` | write | `frpTunnelInput` | Creates an FRP tunnel. |
| `yanto_frp_tunnel_update` | write | `frpTunnelInput.partial() + { id }` | Updates an FRP tunnel. |
| `yanto_frp_tunnel_delete` | admin (destructive) | `{ id, confirm? }` | Deletes an FRP tunnel; requires confirm. |

---

## 3. MCP Resources (6)

Registered only when the token has `read` access.

| URI / Template | Description | MIME | Content |
|---|---|---|---|
| `yanto://overview` | Yanto overview | `application/json` | The settings snapshot, pretty-printed JSON. |
| `yanto://projects/{id}/compose` (template) | Project compose | `text/yaml` | The project's compose file content. 404 if project missing. |
| `yanto://projects/{id}/env` (template) | Project masked environment | `text/plain` | Project env file content with values masked. |
| `yanto://deployments/{id}/logs` (template) | Deployment logs | `text/plain` | Full deployment logs; 404 if deployment missing. |
| `yanto://containers/{id}/logs` (template) | Container logs | `text/plain` | Live container logs. |
| `yanto://system/logs` | System logs | `text/plain` | In-memory system log history. |

Resource templates use no list callback; clients must address them by URI.

---

## 4. MCP Token Management

### Access levels

`McpAccessLevel`: `"read" | "write" | "admin"`, ranked `read < write < admin`. `hasMcpAccess(actual, required)` is a rank comparison — a token's level grants all lower levels too.

### Token format

Tokens are strings of form `ymcp_<idPrefix>_<secret>` (prefix `ymcp_`). `parseMcpToken` splits off the id (`ymcp_<idPrefix>`) and the secret (everything after the second `_`, which itself may contain underscores). Tokens that don't start with `ymcp_` or have fewer than 3 `_`-delimited parts are rejected.

### Issuance (`createMcpAccessToken`)

- Generates a random id (`createId("ymcp")`) and a 32-byte random secret (base64url).
- Stores a **hash** of `id.secret` salted with `config.mcpTokenSecret` — the raw secret is never stored.
- Returns the full token string `${id}_${secret}` once to the caller, plus the public token row.
- REST endpoint: `POST /api/mcp-tokens` (owner-only). Audit-logged as `mcp_token.create`.

### Validation (`authenticateMcpToken`)

- Parses token; looks up the row by id where `revokedAt IS NULL`.
- Compares stored hash to computed hash using **constant-time comparison**.
- On success, updates `lastUsedAt` and returns an `McpAuthContext = { tokenId, tokenName, accessLevel, actor: "mcp:<tokenName>" }`.
- Returns `null` on any failure.

### Listing & Revocation

- `GET /api/mcp-tokens` (owner-only) returns all tokens (public projection; secrets/hashes never exposed).
- `revokeMcpAccessToken(id)`: soft-revoke by setting `revokedAt`. REST: `DELETE /api/mcp-tokens/:id` (owner-only), 404 if not found, audit-logged as `mcp_token.revoke`. Revoked tokens fail authentication.

### What tokens authorize

The `accessLevel` gates which tools/resources are registered for that connection (per-connection filtering, not per-call authorization). The `actor` string `mcp:<tokenName>` is stamped into every audit log entry produced by tool actions.

---

## 5. Tool Registration System

- `registerTool` is the single registration primitive. It takes a `ToolLevel` (`read|write|admin`), the tool name, description, input schema, MCP `ToolAnnotations`, and handler.
- **Access gating:** if `!hasMcpAccess(ctx.accessLevel, level)` the tool is **silently not registered** for that connection. So a `read`-level token only sees read-level tools; an `admin` token sees everything.
- The handler is wrapped: args are parsed, and any thrown error is converted to an MCP error result.
- **Annotation presets** encode the destructive/external nature of each tool (see §2).
- **Discovery:** there is no dynamic registry/list endpoint; discovery happens through the MCP protocol itself (`tools/list`), which the SDK serves from the registered tools.

---

## 6. The Context Object

`YantoMcpContext` is a type alias for `McpAuthContext`:

```
{ tokenId: string; tokenName: string; accessLevel: McpAccessLevel; actor: string }
```

- `ctx.accessLevel` — drives tool/resource registration gating.
- `ctx.actor` (string `mcp:<tokenName>`) — passed as the `actor` field into every `recordAuditLog` call so MCP-driven actions are attributable in the audit trail.

No request-scoped HTTP objects, user session, or DB handles are passed; handlers obtain the DB via the imported singleton and services directly. The context is purely an auth/identity carrier.

---

## 7. Relationship to the Main HTTP Server and to Agents

**To the main HTTP server:**
- The MCP HTTP transport is mounted **inside** the main Express app at `/mcp`, alongside all the REST routers. It shares the same Express middleware stack except for two explicit exemptions: `/mcp` is exempt from CSRF protection, and auth is via MCP bearer token (not the cookie/session auth used by REST).
- The MCP server **reuses the same service layer** as the REST routers — projects, deployments, cloudflare, frp, docker, backups, settings, audit, etc. MCP is therefore a parallel control surface over identical business logic, not a separate subsystem. Every operation an MCP tool performs could equally be done via the REST API (and vice versa), with the same audit logging.
- The MCP HTTP handler is **stateless and ephemeral**: a fresh MCP server is built per request from the authenticated context, the request is handled, and the server is closed.
- MCP is **master-only**: both HTTP and stdio refuse to run on worker nodes.

**To agents:**
- The agent subsystem does **not** currently consume the MCP server. Agents are driven through their own task/tool runner and AI-provider abstractions, not via MCP.
- The MCP surface is aimed at **external** MCP clients (e.g., an IDE-integrated assistant or a separate agent runtime) that authenticate with an MCP token and drive Yanto as a managed deployment/tunneling platform.
- MCP is an **outward-facing control API**; the agent subsystem is an **inward-facing execution feature** that MCP tools can manage (e.g., `yanto_project_create` accepts an `agentImage`). A reimplementation should treat MCP as a self-contained protocol surface that delegates to the same domain services the REST API uses, gated by token access levels, with master-only enforcement and per-request stateless server construction.
