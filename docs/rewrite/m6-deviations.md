> Classification: INTERNAL

# M6 deviations and open items (Hosted Registry)

Recorded by M6 in the style of [m2-deviations](m2-deviations.md). Behavior and data decisions stay in [00-scope](00-scope.md) and 04 section 4.

## What was added

Migration `0600_hosted_registry` (`hosted_registry_settings` singleton, `registry_users`, prefix `rus`); service `registry` (`Services::registry`); ports `RegistryApiPort` (reqwest client `HttpRegistryApi`, fake `FakeRegistryApi`) bundled with two tunables as `RegistryDeps` in `ServiceDeps::registry`; pure helpers in `hosted_registry.rs`; dashboard `/registry`; REST `/api/v1/registry`; `bcrypt` and `reqwest` (no TLS stack, loopback only). The container is driven through the existing `CommandPort` (`docker run/rm/restart/exec/pull`), like M2.

Operations (all Owner-only): `registry_status|users_list|repos_list|tags_list|tag_delete_preview|gc_status` (read), `registry_user_create|user_password_set` (write), `registry_enable|disable|settings_update|restart|rerender|user_delete|tag_delete|gc_preview|gc` (admin). Audit actions: `registry.enable|disable|settings_update|restart|rerender|tag_delete|gc`, `registry_user.create|password_set|delete`; metadata never holds a password.

## Deviations from 00-07

- **Config lives in a directory mount.** `config.yml` is at `<data_dir>/registry/conf/config.yml`, mounted as the directory `/etc/yanto` and passed as `registry serve /etc/yanto/config.yml` (resolves OQ-21's config-path question). A single-file bind mount pins the inode, so the atomic rename Yanto uses would never reach the container; `auth/` is mounted as a directory for the same reason.
- **Pinned image.** `registry:2` is pulled on first enable and run as the repo digest recorded in `image_digest` (`registry@sha256:...`); later enables do not pull again.
- **GC runs in the background** (spawned task, in-memory status, 256 KiB output tail) and the dashboard polls `GET /registry/gc` every 2 s with htmx instead of an SSE stream. REST: `POST /registry/gc` is 202, poll `GET /registry/gc`. The persisted `gc_in_progress` flag is cleared once normal mode is restored (also after a failed collection); `RegistryService::recover` runs at `yanto serve` start.
- **Reclaimed bytes** come from walking `<data_dir>/registry/data` before and after (no helper container).
- **Tag listing** uses one `GET` per tag (digest from `Docker-Content-Digest`, size from config + layers), pages of 100, sorted by name within a page (the registry lists in storage order). `HEAD` is used to resolve digests for delete.
- **Tag delete** is split into `registry_tag_delete_preview` (digest and sibling tags) and `registry_tag_delete`; the sibling scan stops after 5000 tags.
- **Dashboard paths.** Repository names with `/` are one percent-encoded path segment (`/registry/repos/team%2Fapp/tags`). The user password change is `PUT /registry/users/{id}` on the dashboard and `PUT /registry/users/{id}/password` on REST.
- **Internal credential for pulls.** An image Project whose `image_ref` host is `127.0.0.1:<port>` gets `yanto-internal` (keys `127.0.0.1:<port>` and `localhost:<port>`) in the per-command `DOCKER_CONFIG`. A Git Project gets it after the clone when its compose file names the host (a second auth directory with the same scope). A Project credential for the same key wins. A failed pull of such an image while the registry is disabled logs a hint; the pull itself decides success, so an unrelated local registry on the same port still works.
- **Remote-push guidance** is a static page (`/registry/remote-push`) plus the display-only `public_host` setting. The "refuse to show instructions unless a TLS Hostname exists" rule needs M7's `hostnames` table; until then the push commands appear only once `public_host` is set, and Hostname creation (`target_kind='registry'`) and its Cloudflare caveat banner are M7's.
- **Port rules.** `listen_port` is chosen at enable (1024-65535) and refused when it equals Yanto's own port, a declared Project port or a TCP listener on the Host. Changing it later means disable, enable, and updating image references.
- **Nav.** The active-link test is now `path == href || path starts_with href + "/"`, so `/registry` and `/registry-credentials` do not highlight together.

## Not done in M6

- The M7-owned Hostname for the registry and its UI banner; checks that a Project port differs from the Hosted Registry port beyond the live listener scan (M2 follow-up).
- CLI commands for the registry; MCP tools (M8).
- Stop-the-world GC fallback (stop container, run in a one-off container) is documented in 04 4.3 but not implemented.

## Unverified

- Cloudflare request-body limits for layer uploads (OQ-5): the page says "not verified" and gives no number.
- Behavior under a real remote client over HTTPS (no Traefik/Cloudflare was involved); a large-image push was not tested.
- Pushes are rejected while GC runs in read-only mode: the config is rendered with `storage.maintenance.readonly.enabled`, but the smoke test does not race a push against it.
- `registry:2` as shipped on 2026-11 (Distribution 2.x per the pulled image in the smoke run on OrbStack); other daemons and a later major were not exercised.
- htpasswd `$2y$` hashes from the `bcrypt` crate are accepted by the registry (verified in the smoke test: login succeeds, wrong password and a removed user are refused).
