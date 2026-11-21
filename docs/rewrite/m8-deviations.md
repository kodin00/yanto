> Classification: INTERNAL

# M8 deviations and open items (MCP)

Recorded by M8 in the style of [m6-deviations](m6-deviations.md). Behavior decisions stay in [00-scope](00-scope.md) and 03 section d.

## What was added

`yanto-server::mcp` (module, not crate): `ToolSet` (one definition, mounted by both transports), `McpServer` (the `rmcp` handler), `http::router` (nested at `/mcp`, Bearer middleware) and `serve_stdio`. Tools live one module per feature area in `mcp/tools/`; `tools::register` has one line per area, so M7 adds `mod hostnames; ...` plus one `hostnames::register(set)` line. `yanto mcp` in `yanto-cli`. No migration. 67 tools, snapshot in `crates/yanto-server/tests/snapshots/mcp__tool_list_snapshot.snap`.

`rmcp` 3.11.0, features `server`, `transport-streamable-http-server`, `transport-io` (client feature only in dev-dependencies). Pinned by `Cargo.lock`; all `rmcp` types stay inside `mcp/`.

## Deviations from 00-07

- **Hand-written `ServerHandler`, no `#[tool_router]` macros.** One `ToolDef` per tool (typed constructor, schemars input schema, scope, class, `confirm`) is table data, which gives per-scope `tools/list` filtering, the `confirm` property in the schema, and parity tests against `service::all_operations()` without 67 macro methods. `list_tools`/`call_tool` are the only handler methods.
- **Stateless HTTP.** `legacy_session_mode = false` plus `json_response = true`: one JSON-RPC exchange per POST, no `Mcp-Session-Id`, `initialize` optional. Matches the legacy server.
- **Host/Origin allow-list** is loopback plus `server.public_url` only. "Configured extras" has no setting yet (ASSUMPTION: add an `app_settings` key when a deployment needs one).
- **stdio auth (OQ-16 decision).** In-process, token from `YANTO_API_TOKEN_FILE` (`--token-file`, refused unless mode 0600) or `YANTO_API_TOKEN`; no token argument exists. Authenticated once at startup: revoking the token does not end a running stdio session (it ends when the client disconnects). Deployments it enqueues wait for `yanto serve`.
- **Errors** are `isError` results `{"error":{"code","message","fields?"}}` with the REST codes plus `confirmation_required`. Unknown tool names and malformed JSON-RPC are protocol errors.
- **`confirm` is checked after a scope pre-check** that mirrors `OpSpec::min_scope` (the service still re-checks), so a read token calling an admin tool gets `forbidden`, never a hint about `confirm`.
- **Service-level phrase guards pass through** instead of being satisfied by `confirm: true`: `project_delete.confirm_slug`, `cleanup.containers_confirm`, `registry_disable.delete_data`, `authorized_keys_delete.confirm_user`.
- **Redaction.** `yanto_project_create` omits the Deploy Token; `yanto_registry_user_create` omits a generated password; env reads are always masked (`reveal` is never passed); `deploy` does not accept an `env` payload (use `yanto_project_env_write`). Log and env tools prefix the text block with an untrusted-data note.
- **Resources not implemented** (`yanto://...`, 03 d): tools only in M8.
- **Tools not built because the service operation does not exist yet:** `system_logs`, `settings_read|update`, `project_databases_list|set`, `backup_restore`, `daemons_status`, `daemon_control`, `traefik_status`.

## Pending for M7 (add as `mcp/tools/<area>.rs` plus one register line)

`yanto_hostnames_list`, `yanto_hostname_create|retry|enable|disable|delete`, `yanto_hostnames_diagnostics`, `yanto_cloudflare_clients_list`, `yanto_cloudflare_client_validate|create|update|delete`, `yanto_cloudflare_zones_list`, `yanto_cloudflare_tunnels_list`, `yanto_cloudflare_tunnel_create|activate|status|delete`, `yanto_cloudflare_dns_list|create|update|delete`, `yanto_frps_overview`, `yanto_frps_settings_update`, `yanto_traefik_status`, `yanto_daemons_status`, `yanto_daemon_control` (confirm), and the resources `yanto://hostnames`. Extend `sample_args()` in `tests/mcp.rs` (a test fails until every tool has sample arguments).
