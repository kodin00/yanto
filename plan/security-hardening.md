# Yanto Security Hardening Plan

## Context

Yanto Deploy is a self-hosted VPS deployment platform with a React/Express/PostgreSQL stack. A security audit identified findings across authentication, credential handling, transport security, and input validation. This plan covers the actionable items that can be fixed without breaking existing production deployments or requiring Docker configuration changes.

**What was done well:** Zod input validation, Drizzle ORM (no SQL injection), `shell: false` in spawn, constant-time token comparison, httpOnly cookies, audit logging, path traversal protections in `paths.ts`, raw body capture for webhook signature verification.

---

## Findings & Remediation Plan

### HIGH -- Must Fix

#### 1. Remove plaintext password fallback
**File:** `src/server/auth.ts` (line 20)
**Current:** If `ADMIN_PASSWORD` is not bcrypt-hashed, it is compared as plaintext.
**Fix:** Auto-hash the configured password on startup if it is not already bcrypt-formatted, store the hash in memory, and always use `bcrypt.compare()`. This is backwards-compatible -- existing bcrypt hashes still work, and plaintext env vars get hashed transparently.

#### 2. Add rate limiting on login endpoint
**File:** `src/server/index.ts`
**Current:** No rate limiting anywhere. Login endpoint is brute-forceable.
**Fix:** Install `express-rate-limit`. Apply a strict limiter (e.g., 5 attempts per 15 minutes per IP) on `POST /api/auth/login`.

#### 3. Stop exposing deploy tokens to the client
**Files:** `src/server/services/projects.ts`, `src/server/index.ts`, `src/shared/types.ts`, `src/client/lib/api.ts`
**Current:** `listProjectsWithContainerCounts()` returns full project rows including `deployToken`. The `Project` type includes `deployToken`.
**Fix:** Create a `publicProject()` helper that omits `deployToken` from list/get API responses. Only return the full token once at project creation time. The deploy/webhook endpoints continue to work since they read from the DB directly.

---

### MEDIUM -- Should Fix

#### 4. Add security headers (helmet)
**File:** `src/server/index.ts`
**Current:** No CSP, X-Frame-Options, X-Content-Type-Options, or HSTS headers.
**Fix:** Install `helmet`. Apply `helmet()` middleware before routes. Configure CSP to allow inline scripts (required for Vite SPA). Set `X-Frame-Options: DENY` and `X-Content-Type-Options: nosniff`.

#### 5. Sanitize error responses
**File:** `src/server/index.ts` (lines 1110-1115)
**Current:** Global error handler returns raw `error.message` to clients, potentially leaking file paths, DB errors, or internal details.
**Fix:** In production, return a generic message ("Internal server error") and log the full error server-side. Only return detailed messages in development.

#### 6. Enforce HTTPS cookie flag in production
**File:** `src/server/config.ts`
**Current:** `COOKIE_SECURE` defaults to `false` even in production unless `APP_BASE_URL` starts with `https://`.
**Fix:** Default `cookieSecure` to `true` when `nodeEnv === "production"`. Only allow override to `false` via explicit `COOKIE_SECURE=false` env var.

#### 7. Validate Cloudflare hostname and serviceTarget
**File:** `src/server/route-schemas.ts` (lines 55-59)
**Current:** `hostname` and `serviceTarget` are `z.string().min(1)` with no format validation.
**Fix:** Add regex validation: `hostname` must match a valid domain pattern (`/^[a-zA-Z0-9]([a-zA-Z0-9.-]*[a-zA-Z0-9])?$/`), `serviceTarget` must match `hostname:port` format.

#### 8. Encrypt Cloudflare tunnel tokens at rest
**File:** `src/server/services/cloudflare.ts`
**Current:** `cloudflare_tunnels.tunnel_token` is stored in plaintext in the database.
**Fix:** Encrypt tunnel tokens at rest using AES-256-GCM with a key derived from `JWT_SECRET`. Decrypt only when writing to the env file for cloudflared.

---

### LOW -- Nice to Have

#### 9. Add CSRF protection
**File:** `src/server/index.ts`
**Current:** Relies only on `sameSite: "lax"` cookies.
**Fix:** Implement double-submit cookie pattern or use `csrf-csrf` package for state-changing POST/PATCH/DELETE routes.

#### 10. Remove hardcoded Cloudflare account ID from client
**File:** `src/client/App.tsx` (line ~1363)
**Current:** Hardcoded Cloudflare dashboard URL with account hash and domain.
**Fix:** Remove or make configurable via settings.

#### 11. Mask R2 accessKeyId in API responses
**File:** `src/server/services/settings.ts`
**Current:** `publicR2Settings()` returns the actual `accessKeyId`.
**Fix:** Mask it similarly to `secretAccessKey` (return `hasAccessKeyId: boolean` or a truncated version).

#### 12. Pin SSH host keys for CI/CD
**File:** `.github/workflows/deploy.yml`
**Current:** Uses `ssh-keyscan` to populate known_hosts at deploy time.
**Fix:** Pin the VPS host key as a GitHub secret and write it to known_hosts directly.

---

## Implementation Order

1. **Phase 1 (Critical):** Items 1, 2, 3 -- Auth hardening and token exposure
2. **Phase 2 (Important):** Items 4, 5, 6 -- Headers, errors, cookies
3. **Phase 3 (Hardening):** Items 7, 8 -- Input validation and encryption at rest
4. **Phase 4 (Polish):** Items 9-12 -- Defense in depth

## Verification

- After each phase, run `make typecheck lint test` to verify no regressions
- Test login rate limiting manually with rapid requests
- Verify deploy tokens are no longer in API responses by checking `/api/projects` response
- Verify security headers are present using `curl -I http://localhost:8080`
- Run `npm audit` to check for dependency vulnerabilities
