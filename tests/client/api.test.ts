import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock document.cookie for CSRF token
Object.defineProperty(globalThis, "document", {
  value: { cookie: "" },
  writable: true,
  configurable: true
});

// Import after document mock is set up
const { api } = await import("../../src/client/lib/api");

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

describe("api client", () => {
  beforeEach(() => {
    (globalThis as { document: { cookie: string } }).document.cookie = "";
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("CSRF token handling", () => {
    it("includes X-CSRF-Token header when cookie is present", async () => {
      (globalThis as { document: { cookie: string } }).document.cookie = "yanto_csrf=my-token-123";
      const fetchMock = mockFetch({ username: "admin" });

      await api.me();

      expect(fetchMock).toHaveBeenCalledWith("/api/auth/me", expect.objectContaining({
        headers: expect.objectContaining({ "X-CSRF-Token": "my-token-123" })
      }));
    });

    it("omits X-CSRF-Token header when no cookie", async () => {
      const fetchMock = mockFetch({ username: "admin" });

      await api.me();

      const headers = (fetchMock.mock.calls[0][1] as RequestInit).headers as Record<string, string>;
      expect(headers["X-CSRF-Token"]).toBeUndefined();
    });
  });

  describe("request method", () => {
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

      expect(fetchMock).toHaveBeenCalledWith("/api/projects", expect.objectContaining({
        headers: expect.objectContaining({ "Content-Type": "application/json" })
      }));
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

      await expect(api.me()).rejects.toThrow("Request failed.");
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
  });

  describe("projectEnv normalization", () => {
    it("normalizes array response", async () => {
      mockFetch([
        { key: "DB_HOST", value: "localhost", masked: false },
        { key: "SECRET", value: null, masked: true }
      ]);

      const result = await api.projectEnv("p1");

      expect(result).toEqual([
        { key: "DB_HOST", value: "localhost", masked: false },
        { key: "SECRET", value: "", masked: true }
      ]);
    });

    it("normalizes object response", async () => {
      mockFetch({ API_KEY: "abc123", DB_URL: "postgres://..." });

      const result = await api.projectEnv("p1");

      expect(result).toEqual([
        { key: "API_KEY", value: "abc123", masked: true },
        { key: "DB_URL", value: "postgres://...", masked: true }
      ]);
    });

    it("returns empty array for non-object response", async () => {
      mockFetch(null);

      const result = await api.projectEnv("p1");

      expect(result).toEqual([]);
    });
  });
});
