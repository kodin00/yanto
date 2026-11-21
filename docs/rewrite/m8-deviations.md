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
- **Tools not built:** `settings_read|update` (no service operation yet), `daemon_control` (host-level start/stop/restart of a daemon is left to the dashboard, REST and CLI; 03 d lists it, deliberately not offered over MCP), Hostnames of the Hosted Registry (Owner-only `hostname_registry_manage`), `hostnames_resync|options|public_ip_set`, `frps_config_restore`, `system_logs_stream`.
- **`frps_settings_update` needs `confirm: true`** although 03 d marks it write without confirm: a saved change takes effect on the next `frps` restart, which drops every live tunnel (D28). The tool only saves (the service never restarts); the result reports `pending_restart`.
- **Cloudflare secrets.** API tokens are accepted as arguments (write-only, never echoed); the tunnel connector token is never returned (`tunnel_create` omits it, `tunnel_token_reveal` is session-only). `traefik_status` lives in the Hostname service and is registered with the daemon tools (`tools/daemons.rs`).
- **`backup_restore`** is `admin`, confirm, Owner only; in-place and database restores still need `confirm_name` (service guard, not satisfied by `confirm`).

## Tools added after M8

`tools/hostnames.rs` (7), `cloudflare.rs` (15), `frps.rs` (2), `daemons.rs` (`daemons_status`, `traefik_status`), `app_logs.rs` (`system_logs`), `project_databases.rs` (2), `backup_restore.rs` (1). The resource `yanto://hostnames` is still not implemented (resources are not implemented at all).
