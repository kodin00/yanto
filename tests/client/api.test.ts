import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock document.cookie for CSRF token
Object.defineProperty(globalThis, "document", {
  value: { cookie: "" },
  writable: true,
  configurable: true
});

// Import after document mock is set up
const { api, ApiError, setApiUnauthorizedHandler } = await import("../../src/client/lib/api");

function mockFetch(body: unknown, options: { status?: number; contentType?: string } = {}) {
  const { status = 200, contentType = "application/json" } = options;
  const response = {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? "OK" : "Error",
    headers: new Headers({ "content-type": contentType }),
    json: vi.fn().mockResolvedValue(body),
    text: vi.fn().mockResolvedValue(typeof body === "string" ? body : JSON.stringify(body))
  };
  globalThis.fetch = vi.fn().mockResolvedValue(response);
  return globalThis.fetch as ReturnType<typeof vi.fn>;
}

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status >= 200 && status < 300 ? "OK" : "Error",
    headers: new Headers({ "content-type": "application/json" }),
    json: vi.fn().mockResolvedValue(body),
    text: vi.fn().mockResolvedValue(JSON.stringify(body))
  };
}

describe("api client", () => {
  beforeEach(() => {
    (globalThis as { document: { cookie: string } }).document.cookie = "";
    setApiUnauthorizedHandler(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("CSRF token handling", () => {
    it("includes X-CSRF-Token header when cookie is present", async () => {
      (globalThis as { document: { cookie: string } }).document.cookie = "yanto_csrf=my-token-123";
      const fetchMock = mockFetch({ username: "admin" });

      await api.me();

      const headers = new Headers((fetchMock.mock.calls[0][1] as RequestInit).headers);
      expect(headers.get("X-CSRF-Token")).toBe("my-token-123");
    });

    it("omits X-CSRF-Token header when no cookie", async () => {
      const fetchMock = mockFetch({ username: "admin" });

      await api.me();

      const headers = new Headers((fetchMock.mock.calls[0][1] as RequestInit).headers);
      expect(headers.has("X-CSRF-Token")).toBe(false);
    });

    it("ignores a malformed encoded CSRF cookie instead of blocking the request", async () => {
      (globalThis as { document: { cookie: string } }).document.cookie = "yanto_csrf=%E0%A4%A";
      const fetchMock = mockFetch({ username: "admin" });

      await api.me();

      const headers = new Headers((fetchMock.mock.calls[0][1] as RequestInit).headers);
      expect(headers.has("X-CSRF-Token")).toBe(false);
    });
  });

  describe("request method", () => {
    it("coalesces concurrent reads for the same resource", async () => {
      const fetchMock = mockFetch([{ id: "p1" }]);

      const [first, second] = await Promise.all([api.projects(), api.projects()]);

      expect(first).toEqual(second);
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });

    it("does not reuse a read that started before a mutation", async () => {
      let resolveStaleRead!: (response: ReturnType<typeof jsonResponse>) => void;
      const staleResponse = new Promise<ReturnType<typeof jsonResponse>>((resolve) => { resolveStaleRead = resolve; });
      const fetchMock = vi.fn()
        .mockImplementationOnce(() => staleResponse)
        .mockResolvedValueOnce(jsonResponse({ ok: true }))
        .mockResolvedValueOnce(jsonResponse([{ id: "fresh" }]));
      globalThis.fetch = fetchMock;

      const staleRead = api.projects();
      await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
      await api.logout();
      const freshRead = api.projects();
      resolveStaleRead(jsonResponse([{ id: "stale" }]));

      await expect(freshRead).resolves.toEqual([{ id: "fresh" }]);
      await expect(staleRead).resolves.toEqual([{ id: "stale" }]);
      expect(fetchMock.mock.calls.map(([path]) => path)).toEqual([
        "/api/projects",
        "/api/auth/logout",
        "/api/projects"
      ]);
    });

    it("includes credentials: include", async () => {
      const fetchMock = mockFetch([]);

      await api.projects();

      expect(fetchMock).toHaveBeenCalledWith("/api/projects", expect.objectContaining({
        credentials: "include"
      }));
    });

    it("sets Content-Type to application/json", async () => {
      const fetchMock = mockFetch([]);

      await api.projects();

      const headers = new Headers((fetchMock.mock.calls[0][1] as RequestInit).headers);
      expect(headers.get("Content-Type")).toBe("application/json");
    });
  });

  describe("error handling", () => {
    it("throws error with message from response body", async () => {
      mockFetch({ message: "Unauthorized" }, { status: 401 });

      await expect(api.me()).rejects.toThrow("Unauthorized");
    });

    it("throws generic message when response body has no message", async () => {
      const response = {
        ok: false,
        status: 500,
        statusText: "Internal Server Error",
        headers: new Headers({ "content-type": "application/json" }),
        json: vi.fn().mockResolvedValue({}),
        text: vi.fn().mockResolvedValue("")
      };
      globalThis.fetch = vi.fn().mockResolvedValue(response);

      await expect(api.me()).rejects.toThrow("Internal Server Error");
    });

    it("handles JSON parse errors gracefully", async () => {
      const response = {
        ok: false,
        status: 500,
        statusText: "Server Error",
        headers: new Headers({ "content-type": "application/json" }),
        json: vi.fn().mockRejectedValue(new Error("parse error")),
        text: vi.fn().mockResolvedValue("")
      };
      globalThis.fetch = vi.fn().mockResolvedValue(response);

      await expect(api.me()).rejects.toThrow("Server Error");
    });

    it("notifies the app and exposes the status when authentication expires", async () => {
      const onUnauthorized = vi.fn();
      setApiUnauthorizedHandler(onUnauthorized);
      mockFetch({ message: "Authentication required." }, { status: 401 });

      const rejection = api.me();

      await expect(rejection).rejects.toBeInstanceOf(ApiError);
      await expect(rejection).rejects.toMatchObject({ status: 401, message: "Authentication required." });
      expect(onUnauthorized).toHaveBeenCalledTimes(1);
    });
  });

  describe("response parsing", () => {
    it("returns undefined for 204 responses", async () => {
      const response = {
        ok: true,
        status: 204,
        statusText: "No Content",
        headers: new Headers({ "content-type": "application/json" }),
        json: vi.fn(),
        text: vi.fn()
      };
      globalThis.fetch = vi.fn().mockResolvedValue(response);

      const result = await api.deleteProject("123");
      expect(result).toBeUndefined();
    });

    it("returns text for text/plain content type", async () => {
      mockFetch("log output here", { contentType: "text/plain" });

      const result = await api.deploymentLogs("dep-1");
      expect(result).toBe("log output here");
    });

    it("returns parsed JSON for JSON content type", async () => {
      mockFetch([{ id: "p1", name: "Project 1" }]);

      const result = await api.projects();
      expect(result).toEqual([{ id: "p1", name: "Project 1" }]);
    });
  });

  describe("API endpoints", () => {
    it("login sends POST to /api/auth/login", async () => {
      const fetchMock = mockFetch({ username: "admin" });

      await api.login("admin", "pass123");

      expect(fetchMock).toHaveBeenCalledWith("/api/auth/login", expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ username: "admin", password: "pass123" })
      }));
    });

    it("previews an account link without putting its token in the URL", async () => {
      const fetchMock = mockFetch({ username: "deploy-operator" });

      await api.accountSetupDetails("secret-token");

      expect(fetchMock).toHaveBeenCalledWith("/api/auth/account/setup/preview", expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ token: "secret-token" })
      }));
    });

    it("sends both password entries when completing account setup", async () => {
      const fetchMock = mockFetch({ username: "deploy-operator" });

      await api.completeAccountSetup("secret-token", "new-password", "new-password");

      expect(fetchMock).toHaveBeenCalledWith("/api/auth/account/setup", expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ token: "secret-token", password: "new-password", passwordConfirmation: "new-password" })
      }));
    });

    it("deletes a managed user", async () => {
      const fetchMock = mockFetch(undefined, { status: 204 });

      await api.deleteUser("usr_member");

      expect(fetchMock).toHaveBeenCalledWith("/api/users/usr_member", expect.objectContaining({ method: "DELETE" }));
    });

    it("logout sends POST to /api/auth/logout", async () => {
      const fetchMock = mockFetch({ ok: true });

      await api.logout();

      expect(fetchMock).toHaveBeenCalledWith("/api/auth/logout", expect.objectContaining({
        method: "POST"
      }));
    });

    it("me sends GET to /api/auth/me", async () => {
      const fetchMock = mockFetch({ username: "admin" });

      await api.me();

      expect(fetchMock).toHaveBeenCalledWith("/api/auth/me", expect.objectContaining({
        credentials: "include"
      }));
    });

    it("createProject sends POST with payload", async () => {
      const fetchMock = mockFetch({ id: "new-1", name: "New" });
      const payload = { name: "New", branch: "main", folderName: "new", composeFile: "docker-compose.yml", autoStart: true, manualDeployEnabled: true, githubWebhookEnabled: false, targetNodeId: null };

      await api.createProject(payload);

      expect(fetchMock).toHaveBeenCalledWith("/api/projects", expect.objectContaining({
        method: "POST",
        body: JSON.stringify(payload)
      }));
    });

    it("updateProject sends PATCH to /api/projects/:id", async () => {
      const fetchMock = mockFetch({ id: "p1", name: "Updated" });

      await api.updateProject("p1", { name: "Updated" });

      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1", expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({ name: "Updated" })
      }));
    });

    it("deleteProject sends DELETE to /api/projects/:id", async () => {
      mockFetch(undefined, { status: 204 });
      const response = {
        ok: true,
        status: 204,
        statusText: "No Content",
        headers: new Headers({ "content-type": "application/json" }),
        json: vi.fn(),
        text: vi.fn()
      };
      globalThis.fetch = vi.fn().mockResolvedValue(response);

      await api.deleteProject("p1");

      expect(globalThis.fetch).toHaveBeenCalledWith("/api/projects/p1", expect.objectContaining({
        method: "DELETE"
      }));
    });

    it("projectDeployToken sends GET to /api/projects/:id/deploy-token", async () => {
      const fetchMock = mockFetch({ deployToken: "secret-token" });

      const result = await api.projectDeployToken("p1");

      expect(result.deployToken).toBe("secret-token");
      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1/deploy-token", expect.objectContaining({
        credentials: "include"
      }));
    });

    it("deployProject sends POST to /api/projects/:id/deploy", async () => {
      const fetchMock = mockFetch({ deployment: {}, reused: false });

      await api.deployProject("p1");

      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1/deploy", expect.objectContaining({
        method: "POST",
        body: "{}"
      }));
    });

    it("deployProject can include pending env variables", async () => {
      const fetchMock = mockFetch({ deployment: {}, reused: false });
      const payload = { envVariables: [{ key: "APP_PORT", value: "3000" }] };

      await api.deployProject("p1", payload);

      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1/deploy", expect.objectContaining({
        method: "POST",
        body: JSON.stringify(payload)
      }));
    });

    it("deployProject can retry the exact target ref", async () => {
      const fetchMock = mockFetch({ deployment: {}, reused: false });

      await api.deployProject("p1", { targetRef: "abc123def" });

      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1/deploy", expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ targetRef: "abc123def" })
      }));
    });

    it("rollbackPreview sends targetRef to preview endpoint", async () => {
      const fetchMock = mockFetch({ requestedRef: "v1.2.3" });

      await api.rollbackPreview("p1", "v1.2.3");

      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1/rollback/preview", expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ targetRef: "v1.2.3" })
      }));
    });

    it("rollbackProject sends targetRef instead of deploymentId", async () => {
      const fetchMock = mockFetch({ deployment: {} });

      await api.rollbackProject("p1", "v1.2.3");

      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1/rollback", expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ targetRef: "v1.2.3" })
      }));
    });

    it("stopProject sends POST to /api/projects/:id/stop", async () => {
      const fetchMock = mockFetch({ ok: true });

      await api.stopProject("p1");

      expect(fetchMock).toHaveBeenCalledWith("/api/projects/p1/stop", expect.objectContaining({
        method: "POST"
      }));
    });

    it("generateSshKey sends POST to settings endpoint", async () => {
      const fetchMock = mockFetch({ ok: true, sshKey: { hasManagedKey: true, publicKey: "ssh-ed25519 test" } });

      await api.generateSshKey();

      expect(fetchMock).toHaveBeenCalledWith("/api/settings/ssh-key/generate", expect.objectContaining({
        method: "POST"
      }));
    });

    it("settings includes multi-node settings in response shape", async () => {
      mockFetch({
        projectsRoot: "/projects",
        hostProjectsRoot: "~/projects",
        sshKeysDir: "/data/ssh",
        appBaseUrl: "http://localhost:8080",
        projectCount: 0,
        r2: {},
        cf: {},
        setupWizard: {},
        sshKey: {},
        multiNode: { enabled: false, releaseStage: "beta" }
      });

      const result = await api.settings();

      expect(result.multiNode).toEqual({ enabled: false, releaseStage: "beta" });
    });

    it("saveMultiNodeSettings sends POST to settings endpoint", async () => {
      const fetchMock = mockFetch({ ok: true, multiNode: { enabled: true, releaseStage: "beta" } });

      await api.saveMultiNodeSettings({ enabled: true });

      expect(fetchMock).toHaveBeenCalledWith("/api/settings/multi-node", expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ enabled: true })
      }));
    });

    it("containers sends GET to /api/containers", async () => {
      const fetchMock = mockFetch([]);

      await api.containers();

      expect(fetchMock).toHaveBeenCalledWith("/api/containers", expect.objectContaining({
        credentials: "include"
      }));
    });

    it("uploads restore files as a raw byte stream", async () => {
      const fetchMock = mockFetch({ ok: true, target: {} });
      const file = new File(["{}"], "dump.json", { type: "application/json" });

      await api.restorePostgresTarget("postgres-1", file);

      const request = fetchMock.mock.calls[0][1] as RequestInit;
      expect(new Headers(request.headers).get("Content-Type")).toBe("application/octet-stream");
      expect(request.body).toBe(file);
    });

    it("deployments requests the expanded history limit", async () => {
      const fetchMock = mockFetch([]);

      await api.deployments();

      expect(fetchMock).toHaveBeenCalledWith("/api/deployments?limit=500", expect.objectContaining({
        credentials: "include"
      }));
    });

    it("startContainer sends POST to /api/containers/:id/start", async () => {
      const fetchMock = mockFetch({ ok: true });

      await api.startContainer("ct-1");

      expect(fetchMock).toHaveBeenCalledWith("/api/containers/ct-1/start", expect.objectContaining({
        method: "POST"
      }));
    });

    it("deploymentLogStream returns stream URL", () => {
      expect(api.deploymentLogStream("dep-1")).toBe("/api/deployments/dep-1/logs/stream");
    });

    it("backupDownloadUrl returns download URL", () => {
      expect(api.backupDownloadUrl("bk-1")).toBe("/api/backups/bk-1/download");
    });

    it("containerLogStream returns stream URL", () => {
      expect(api.containerLogStream("ct-1")).toBe("/api/containers/ct-1/logs/stream");
    });

    it("cloudflareRouteDiagnostics sends GET to diagnostics endpoint", async () => {
      const fetchMock = mockFetch([]);

      await api.cloudflareRouteDiagnostics();

      expect(fetchMock).toHaveBeenCalledWith("/api/cloudflare/routes/diagnostics", expect.objectContaining({
        credentials: "include"
      }));
    });

    it("creates a client without changing the credential payload", async () => {
      const fetchMock = mockFetch({ id: "cfc_1" });
      const payload = { name: "Acme", accountId: "account", zoneId: "zone", apiToken: "secret" };
      await api.createCloudflareClient(payload);
      expect(fetchMock).toHaveBeenCalledWith("/api/cloudflare/clients", expect.objectContaining({ method: "POST", body: JSON.stringify(payload) }));
    });

    it("creates a tunnel network assignment", async () => {
      const fetchMock = mockFetch({ id: "cfa_1" });
      const payload = { tunnelId: "cft_1", projectId: "prj_1", composeProject: "shop", composeService: "web" };
      await api.createCloudflareAssignment(payload);
      expect(fetchMock).toHaveBeenCalledWith("/api/cloudflare/assignments", expect.objectContaining({ method: "POST", body: JSON.stringify(payload) }));
    });

    it("force deletes a managed tunnel using its tunnel id", async () => {
      const fetchMock = mockFetch(undefined, { status: 204 });
      await api.deleteCloudflareTunnel("cft_1", true);
      expect(fetchMock).toHaveBeenCalledWith("/api/cloudflare/tunnels/cft_1?force=true", expect.objectContaining({ method: "DELETE" }));
    });

    it("controls cloudflared using the deployment node id", async () => {
      const fetchMock = mockFetch({ ok: true });
      await api.startCloudflared("node_master_local");
      expect(fetchMock).toHaveBeenLastCalledWith("/api/cloudflare/tunnels/node/node_master_local/start", expect.objectContaining({ method: "POST" }));
      await api.stopCloudflared("node_master_local");
      expect(fetchMock).toHaveBeenLastCalledWith("/api/cloudflare/tunnels/node/node_master_local/stop", expect.objectContaining({ method: "POST" }));
      await api.restartCloudflared("node_master_local");
      expect(fetchMock).toHaveBeenLastCalledWith("/api/cloudflare/tunnels/node/node_master_local/restart", expect.objectContaining({ method: "POST" }));
    });

    it("preserves managed-hostname deletion warnings", async () => {
      mockFetch({ ok: true, warnings: ["DNS record was already missing."] });
      await expect(api.deleteCloudflareHostname("route_1")).resolves.toEqual({ ok: true, warnings: ["DNS record was already missing."] });
    });

    it("saves the FRP public endpoint", async () => {
      const fetchMock = mockFetch({ publicHost: "203.0.113.10" });
      await api.saveFrpSettings("203.0.113.10");
      expect(fetchMock).toHaveBeenCalledWith("/api/frp/settings", expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ publicHost: "203.0.113.10" })
      }));
    });

    it("creates and updates FRP tunnels", async () => {
      const fetchMock = mockFetch({ id: "frp_1" });
      const payload = { name: "Minecraft", protocol: "tcp" as const, localHost: "127.0.0.1", localPort: 25565, remotePort: 25565, enabled: true };
      await api.createFrpTunnel(payload);
      expect(fetchMock).toHaveBeenLastCalledWith("/api/frp/tunnels", expect.objectContaining({ method: "POST", body: JSON.stringify(payload) }));
      await api.updateFrpTunnel("frp_1", { enabled: false });
      expect(fetchMock).toHaveBeenLastCalledWith("/api/frp/tunnels/frp_1", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ enabled: false }) }));
    });

    it("loads manual FRPC client setup", async () => {
      const fetchMock = mockFetch({ frpcToml: "serverAddr = \"x.x.x.x\"\n", installScript: "#!/usr/bin/env bash\n" });
      await api.frpClientSetup();
      expect(fetchMock).toHaveBeenCalledWith("/api/frp/client-setup", expect.objectContaining({ credentials: "include" }));
    });

    it("controls the FRP server lifecycle", async () => {
      const fetchMock = mockFetch({ running: true });
      await api.controlFrpServer("restart");
      expect(fetchMock).toHaveBeenCalledWith("/api/frp/server/restart", expect.objectContaining({ method: "POST" }));
    });
  });

  describe("projectEnv normalization", () => {
    it("normalizes array response", async () => {
      mockFetch([
        { key: "DB_HOST", value: "localhost", masked: false },
        { key: "SECRET", value: null, masked: true }
      ]);

      const result = await api.projectEnv("p1");

      expect(result).toEqual([
        { key: "DB_HOST", value: "localhost" },
        { key: "SECRET", value: "" }
      ]);
    });

    it("normalizes object response", async () => {
      mockFetch({ API_KEY: "abc123", DB_URL: "postgres://..." });

      const result = await api.projectEnv("p1");

      expect(result).toEqual([
        { key: "API_KEY", value: "abc123" },
        { key: "DB_URL", value: "postgres://..." }
      ]);
    });

    it("returns empty array for non-object response", async () => {
      mockFetch(null);

      const result = await api.projectEnv("p1");

      expect(result).toEqual([]);
    });
  });
});
