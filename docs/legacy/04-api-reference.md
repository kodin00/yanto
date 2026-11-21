# API Reference

All route modules are mounted at the root (no prefix); each defines its own full paths. The MCP HTTP router is mounted first.

## Global Middleware (order)

1. Body parsing (json/urlencoded, 1 MB, raw-body capture)
2. cookieParser
3. helmet (strict CSP)
4. CSRF protection (double-submit cookie; exempt: `/deploy`, `/webhooks/github`, `/mcp`, `/api/workers/*`)
5. Routers
6. Error handler (ZodError→400, HttpError→status, else 500 masked in prod)
7. Static SPA fallback

## SSE / Streaming Endpoints

1. `GET /api/deployments/:id/logs/stream` — live deployment logs (`{ logs, status, done }`).
2. `GET /api/containers/:id/logs/stream` — live `docker logs --follow` (`{ chunk }` / `{ error, done }`).
3. `GET /api/agent/tasks/:id/events/stream` — live agent task events (snapshot + live; `{ kind, payload, done }`).

All three use `text/event-stream` with `data: <JSON>\n\n` framing and a 15-second stream-authorization guard.

---

## auth.ts

| Method | Path | Auth | Body | Response |
|---|---|---|---|---|
| GET | `/api/setup/status` | none | — | setup status + `setupCodeRequired` |
| GET | `/api/auth/turnstile` | none | — | `{ enabled, siteKey }` |
| POST | `/api/setup/owner` | none (rate 10/15min) | `{ username(1-200), password, passwordConfirmation, setupCode?, turnstileToken? }` | 201 sessionUser + cookie |
| POST | `/api/auth/login` | none (rate 5/15min, skip on success) | `{ username(<=200), password, turnstileToken? }` | 200 sessionUser + cookie; 401 on failure |
| POST | `/api/auth/account/setup/preview` | none (rate 10/15min) | `{ token(1-2000) }` | account setup details |
| POST | `/api/auth/account/setup` | none (rate 10/15min) | `{ token, password, passwordConfirmation, turnstileToken? }` | 200 sessionUser + cookie |
| POST | `/api/auth/logout` | none (CSRF) | — | `{ ok: true }` + clears cookie |
| GET | `/api/auth/me` | requireAuth | — | sessionUser; 401 if not authed |

## users.ts — all `requireOwner`

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/users` | — | array of managed users |
| POST | `/api/users` | `{ username(1-200), projectAccess: [{ projectId, permissions[] }] }` (max 10000) | 201 `{ user, ...resetUrl }` |
| PUT | `/api/users/:id/access` | `{ projectAccess[] }` | updated user |
| PATCH | `/api/users/:id/status` | `{ status: "active"\|"disabled" }` | updated user |
| POST | `/api/users/:id/reset-link` | — | `{ resetUrl }` |
| DELETE | `/api/users/:id` | — | 204 |

## projects.ts

| Method | Path | Auth | Body | Response |
|---|---|---|---|---|
| GET | `/api/projects` | requireAuth | — | array of project views (shapes per permission) |
| POST | `/api/projects` | requireOwner | `projectInput` | 201 `{ ...publicProject, deployToken }` |
| PATCH | `/api/projects/:id` | requireAuth + `config` | `projectInput.partial()` | updated project; 403 if member changes folderName/targetNodeId |
| GET | `/api/projects/:id/deploy-token` | requireAuth + `secrets` | — | `{ deployToken }` |
| DELETE | `/api/projects/:id` | requireOwner | — | 204 |
| POST | `/api/projects/:id/deploy` | requireAuth + `deploy` (+`secrets` if env) | `deploymentInput`: `{ targetRef?, envContent?, envVariables?: [{key,value?,masked?}], envFile? }` | 202/200 `{ deployment, reused }` |
| POST | `/api/projects/:id/rollback/preview` | requireAuth + `deploy` | `rollbackInput`: `{ deploymentId?, targetRef? }` | rollback preview |
| POST | `/api/projects/:id/rollback` | requireAuth + `deploy` | `rollbackInput` | 202/200 `{ deployment, reused }` |
| GET | `/api/projects/:id/compose/content` | requireAuth + `config` | — | compose content |
| GET | `/api/projects/:id/env` | requireAuth + `secrets` | `?envFile=` | env variable list (masked) |
| GET | `/api/projects/:id/env/content` | requireAuth + `secrets` | `?envFile=` | env content (masked) |
| GET | `/api/projects/:id/env/preview` | requireAuth + `secrets` | `?envFile=` | env preview |
| POST | `/api/projects/:id/env/preview` | requireAuth + `secrets` | `{ envFile?, content }` | parsed preview |
| PUT | `/api/projects/:id/env` | requireAuth + `secrets` | `envInput`: `{ envFile?, content }` | `{ envFile, entryCount, ... }` |
| PATCH | `/api/projects/:id/env` | requireAuth + `secrets` | `{ envFile?, variables: [{key,value?,masked?}] }` | `{ ok: true, preview }` |
| POST | `/api/projects/:id/stop` | requireAuth + `runtime` | — | `{ ok: true, logs }`; 400 if worker node |
| POST | `/api/projects/:id/restart` | requireAuth + `runtime` | — | `{ ok: true, logs }`; 400 if worker node |

**`projectInput`:** `{ name(1+), source?(<=500), gitUrl?, dockerImage?(<=500), branch?="master", folderName?="", composeFile?(1+), composeContent?, envFile?(1+), autoStart?=true, manualDeployEnabled?=true, githubWebhookEnabled?=true, targetNodeId?(1+), agentImage?(<=500)="" }`.

**GET `/api/projects` shaping:** `composeContent` nulled without `config`; `cloudflareRoutes` emptied without `hostnames`; ambiguous folder → non-owners get `containerCount: 0`, owners get `containerMappingWarning`.

## deployments.ts — all `requireAuth`

| Method | Path | Params | Response |
|---|---|---|---|
| GET | `/api/deployments` | `?page&pageSize` (default 10, max 500) | paginated deployments (filtered by accessible projects) |
| GET | `/api/deployments/:id` | — | deployment; 404 if not found/no access |
| GET | `/api/deployments/:id/logs` | — | `text/plain` logs |
| GET | `/api/deployments/:id/logs/stream` | — | SSE `{ logs, status, done }` |

## containers.ts — all `requireAuth`

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/containers` | — | container list (owner: all; member: unambiguous-folder projects) |
| GET | `/api/containers/:id/logs` | — | `text/plain` |
| GET | `/api/containers/:id/logs/stream` | — | SSE `{ chunk }` / `{ error, done }` |
| POST | `/api/containers/:id/stop` | — | `{ ok: true }` (needs `runtime` via projectForContainer) |
| POST | `/api/containers/:id/start` | — | `{ ok: true }` |
| POST | `/api/containers/:id/exec` | `{ command(1-10000) }` | `{ ok, exitCode, ...result }` |
| POST | `/api/containers/:id/restart` | — | `{ ok: true }` |

`:id` may be a container ID or name.

## backups.ts — all `requireAuth`

| Method | Path | Body/Params | Response |
|---|---|---|---|
| GET | `/api/backups` | `?page&pageSize` (default 10, max 200) | paginated backups (filtered by `backups` perm) |
| GET | `/api/backups/postgres-targets` | — | Postgres targets (owner: all; member: filtered) |
| POST | `/api/backups` | `{ containerId? }` | 201 backup. No containerId → only owner may back up Yanto DB |
| POST | `/api/backups/postgres-targets/:containerId/restore` | raw body = SQL dump; header `x-filename` (<=160); limit `backupUploadMaxBytes` | `{ ok: true, target }` |
| POST | `/api/backups/:id/upload-r2` | — | R2 upload result |
| GET | `/api/backups/:id/download` | — | file download; marks downloaded; 404 if not successful |
| DELETE | `/api/backups/:id` | — | 204 |

## workers.ts — no session auth / CSRF-exempt

| Method | Path | Auth | Body | Response |
|---|---|---|---|---|
| POST | `/api/workers/register` | join token (rate 10/15min) | `{ joinToken(1-1000), name?(<=200), dockerVersion?(<=200), labels?(<=64 entries) }` | 201 `{ node, token }`; 401 if invalid |
| POST | `/api/workers/heartbeat` | Bearer worker token | `{ name?, dockerVersion?, labels? }` | `{ ok: true, node }` |
| GET | `/api/workers/jobs/next` | Bearer worker token | — | `{ ...job, project } \| null` |
| POST | `/api/workers/deployments/:id/logs` | Bearer worker token | `{ chunk(<=256KB) }` | `{ ok: true }` |
| PATCH | `/api/workers/deployments/:id` | Bearer worker token | `{ status?, exitCode?, commitSha?(<=128), commitMessage?(<=2000), targetRef?(<=1000) }` | `{ ok: true }` |
| POST | `/deploy` | deploy token (rate 10/15min) | `?id=<projectId>` + `Authorization: Bearer <deployToken>` | 202/200 `{ deployment, reused }`; 403 if manual deploy disabled |
| POST | `/webhooks/github` | HMAC signature (rate 60/min) | `?id=`/`?projectId=` + `x-hub-signature-256` | see below |

**GitHub webhook behavior:** verify HMAC against project deploy token using raw body; `ping` → `{ ok }`; disabled webhook → 202 ignored; non-`push` → 202 ignored; `deleted` ref → 202 ignored; branch mismatch → 202 ignored; match → start deployment with trigger `github`.

## settings.ts — all `requireOwner`

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/settings` | — | aggregated settings overview |
| POST | `/api/settings/runtime` | `runtimeSettingsInput` | `{ ok: true, runtime }` |
| POST | `/api/settings/r2` | `r2SettingsInput` | `{ ok: true, r2 }` |
| POST | `/api/settings/cloudflare` | `cloudflareSettingsInput`: `{ accountId?, zoneId?, apiToken? }` | `{ ok: true, cf }` |
| POST | `/api/settings/cloudflare/validate` | `cloudflareSettingsInput` | validation result |
| POST | `/api/settings/ssh-key` | `{ privateKey(1+) }` | `{ ok: true, sshKey }` |
| POST | `/api/settings/ssh-key/generate` | — | `{ ok: true, sshKey }` |
| POST | `/api/settings/setup-wizard` | `{ action: "completed"\|"dismissed" }` | `{ ok: true, setupWizard }` |
| POST | `/api/settings/multi-node` | `{ enabled?=false }` | `{ ok: true, multiNode }` |

**`runtimeSettingsInput`:** `{ appBaseUrl (http/https URL), cookieSecure, turnstileSiteKey?(<=500), turnstileSecretKey?(<=1000), clearTurnstileSecretKey?=false, commandTimeoutMs [1000, 86400000], commandOutputMaxBytes [16384, 100MB], deploymentLogMaxChars [10000, 10000000] }`.

**`r2SettingsInput`:** `{ enabled?=false, accountId? (32 hex), bucket? (3-63 lowercase), accessKeyId?, secretAccessKey?, prefix?="postgres-dumps" }`.

## cloudflare.ts

### Owner-only: Tunnels
| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/cloudflare/tunnels` | — | tunnel list |
| POST | `/api/cloudflare/tunnels` | `{ clientId(1+), name(1-100) }` | 201 publicTunnel |
| DELETE | `/api/cloudflare/tunnels/:id` | `?force=true` | 204 |
| GET | `/api/cloudflare/tunnels/node/:nodeId` | — | `{ tunnel, runtime, health }`; 404 if none |
| POST | `/api/cloudflare/tunnels/node/:nodeId/start` | — | `{ ok: true }` |
| POST | `/api/cloudflare/tunnels/node/:nodeId/stop` | — | `{ ok: true }` |
| POST | `/api/cloudflare/tunnels/node/:nodeId/restart` | — | `{ ok: true }` |

### Owner-only: Clients
| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/cloudflare/clients` | — | client list |
| POST | `/api/cloudflare/clients/validate` | `cloudflareClientInput` (apiToken required) | validation result |
| POST | `/api/cloudflare/clients` | `cloudflareClientInput` (apiToken required) | 201 client |
| PATCH | `/api/cloudflare/clients/:id` | `cloudflareClientInput.partial()` | client |
| DELETE | `/api/cloudflare/clients/:id` | — | 204 |
| GET | `/api/cloudflare/clients/:id/zones` | — | zone list |

### Owner-only: Client DNS records
| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/cloudflare/clients/:clientId/dns-records` | — | record list |
| POST | `/api/cloudflare/clients/:clientId/dns-records` | `cloudflareDnsRecordInput` | 201 record |
| PATCH | `/api/cloudflare/clients/:clientId/dns-records/:id` | `cloudflareDnsRecordInput` | record |
| DELETE | `/api/cloudflare/clients/:clientId/dns-records/:id` | — | 204 |

### Owner-only: Assignments
| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/cloudflare/assignments` | `?tunnelId=` | assignment list |
| POST | `/api/cloudflare/assignments` | `{ tunnelId(1+), projectId?, composeProject?, composeService?, containerName? }` | 201 assignment |
| DELETE | `/api/cloudflare/assignments/:id` | — | 204 |

### Hostnames (requireAuth + `hostnames`; owner bypasses)
| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/cloudflare/hostnames` | — | hostname list |
| POST | `/api/cloudflare/hostnames` | `{ tunnelId(1+), assignmentId(1+), zoneId(1+), hostname(1+), protocol: "http"\|"https", port[1-65535], noTlsVerify?=false }` | 201 hostname |
| DELETE | `/api/cloudflare/hostnames/:id` | — | 204, or `{ ok: true, warnings }` |
| POST | `/api/cloudflare/hostnames/:id/retry` | — | hostname |

### Owner-only: global DNS records
| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/cloudflare/dns-records` | — | record list |
| POST | `/api/cloudflare/dns-records` | `cloudflareDnsRecordInput` | 201 record |
| PATCH | `/api/cloudflare/dns-records/:id` | `cloudflareDnsRecordInput` | record |
| DELETE | `/api/cloudflare/dns-records/:id` | — | 204 |

### Project-scoped routes (requireAuth + `hostnames`)
| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/cloudflare/routes/diagnostics` | — | diagnostics list |
| GET | `/api/projects/:id/cf-routes` | — | route list |
| POST | `/api/projects/:id/cf-routes` | `cloudflareRouteInput`: `{ hostname, serviceTarget (scheme://host:port), noTlsVerify?=false, nodeId? }` | 201 route |
| PATCH | `/api/cloudflare/routes/:id/enable` | — | route |
| PATCH | `/api/cloudflare/routes/:id/disable` | — | route |
| DELETE | `/api/cloudflare/routes/:id` | — | 204 |

**`cloudflareDnsRecordInput`:** `{ type: "A"\|"AAAA"\|"CNAME"\|"TXT"\|"MX"\|"NS", name(1-255), content(1-4096), ttl?[1-2147483647]=1, proxied?=false, priority?[0-65535], comment?(<=500) }`.

## frp.ts — all `requireOwner`

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/frp/overview` | — | overview |
| PUT | `/api/frp/settings` | `{ publicHost (IP/hostname or "") }` | settings |
| GET | `/api/frp/client-setup` | — | setup object |
| POST | `/api/frp/server/start` | — | server status |
| POST | `/api/frp/server/stop` | — | server status |
| POST | `/api/frp/server/restart` | — | server status |
| POST | `/api/frp/tunnels` | `{ name(1-100), nodeId?, protocol: "tcp"\|"udp", localHost, localPort[1-65535], remotePort[1-65535], enabled?=true }` | 201 tunnel |
| PATCH | `/api/frp/tunnels/:id` | partial (non-empty) | tunnel |
| DELETE | `/api/frp/tunnels/:id` | — | 204 |

## mcp-tokens.ts — all `requireOwner`

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/mcp-tokens` | — | token list |
| POST | `/api/mcp-tokens` | `{ name(1-100), accessLevel: "read"\|"write"\|"admin" }` | 201 `{ accessToken, ...result }` (raw token returned once) |
| DELETE | `/api/mcp-tokens/:id` | — | 204 |

## ai-providers.ts — all `requireOwner`

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/ai/providers` | — | provider list + models |
| GET | `/api/ai/models/available` | — | enabled providers + enabled models |
| GET | `/api/ai/codex/status` | — | codex status |
| POST | `/api/ai/codex/login/start` | — | login info |
| POST | `/api/ai/codex/login/cancel` | — | `{ ok: true }` |
| POST | `/api/ai/codex/logout` | — | `{ ok: true }` |
| POST | `/api/ai/codex/models/refresh` | — | `{ ok: true, models }`; 409 if not connected |
| POST | `/api/ai/providers` | `providerInput` | 201 provider |
| PATCH | `/api/ai/providers/:id` | `providerInput.partial()` | provider |
| DELETE | `/api/ai/providers/:id` | — | 204 |
| POST | `/api/ai/providers/:id/discover` | — | `{ ok: true, models }` |
| POST | `/api/ai/providers/:id/models` | `{ modelId(1-250), displayName?(<=250) }` | 201 model |
| PATCH | `/api/ai/models/:id` | `{ displayName?(1-250), enabled? }` | model |
| DELETE | `/api/ai/models/:id` | — | 204 |

**`providerInput`:** `{ name(1-100), protocol: "openai_responses"\|"openai_chat"\|"anthropic_messages", baseUrl?(<=500), apiKey?(<=10000), enabled?, defaultModelId?(1-100) }`.

## agent-tasks.ts — effectively all `requireOwner`

| Method | Path | Body/Params | Response |
|---|---|---|---|
| GET | `/api/agent/tasks` | `?archived=true\|false` | task list |
| GET | `/api/agent/worktrees` | — | worktree list |
| DELETE | `/api/agent/worktrees/:taskId` | `?force=true` | 204 |
| POST | `/api/agent/tasks` | `taskInput` | 201 task |
| GET | `/api/agent/tasks/:id` | — | task; 404 |
| PATCH | `/api/agent/tasks/:id` | `{ title?, modelId?, autoCommit?, autoPush?, autoCleanup? }` | task |
| PATCH | `/api/agent/tasks/:id/archive` | `{ archived }` | task |
| DELETE | `/api/agent/tasks/:id` | `?force=true` | 204 |
| POST | `/api/agent/tasks/:id/run` | `{ message?(1-200000) }` | 202 run |
| POST | `/api/agent/tasks/:id/stop` | — | `{ ok: true }` |
| GET | `/api/projects/:id/agent-branches` | — | branch list |
| GET | `/api/agent/tasks/:id/git` | — | git preview |
| POST | `/api/agent/tasks/:id/commit` | `{ message(1-500), paths?(<=500 entries, <=1000 each) }` | `{ sha }` |
| POST | `/api/agent/tasks/:id/push` | — | `{ sha }` |
| DELETE | `/api/agent/tasks/:id/worktree` | `?force=true` | 204 |
| GET | `/api/agent/tasks/:id/events/stream` | — | SSE events |

**`taskInput`:** `{ projectId(1+), modelId(1+), title(1-200), prompt(1-200000), sourceBranch(1-250), taskBranch(1-250), resumeExistingBranch?=false, autoCommit?=false, autoPush?=false, autoCleanup?=false }`.

## system.ts

| Method | Path | Auth | Response |
|---|---|---|---|
| GET | `/api/health` | none | 200 if ok else 503; `{ ok, checks: { [name]: { ok } } }` |
| GET | `/api/nodes` | requireOwner | node list |
| POST | `/api/nodes/join-token` | requireOwner | `{ token, command }` (curl\|bash one-liner) |
| GET | `/api/audit-logs` | requireAuth | paginated (`?page&pageSize` max 500, `?projectId=`) |
| GET | `/api/system/usage` | requireOwner | usage object |
| GET | `/api/system/logs` | requireOwner | `text/plain` |
| GET | `/api/system/cleanup/preview` | requireOwner | `{ logs }` |
| POST | `/api/system/cleanup` | requireOwner | `{ logs }` |
