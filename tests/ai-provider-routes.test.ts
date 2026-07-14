import express, { type NextFunction, type Request, type Response } from "express";
import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ listAiProviders: vi.fn() }));

vi.mock("../src/server/auth.js", () => ({
  currentUser: () => ({ username: "admin" }),
  requireOwner: (req: Request, res: Response, next: NextFunction) => {
    if (req.header("authorization") === "member") {
      res.status(403).json({ message: "Owner access required." });
      return;
    }
    next();
  }
}));
vi.mock("../src/server/services/audit.js", () => ({ recordAuditLog: vi.fn() }));
vi.mock("../src/server/services/codex-auth.js", () => ({
  cancelCodexLogin: vi.fn(), getCodexAccountStatus: vi.fn(), listCodexModels: vi.fn(), logoutCodexAccount: vi.fn(), startCodexLogin: vi.fn()
}));
vi.mock("../src/server/services/ai-providers.js", () => ({
  addAiModel: vi.fn(), createAiProvider: vi.fn(), deleteAiModel: vi.fn(), deleteAiProvider: vi.fn(), discoverAiModels: vi.fn(),
  listAiProviders: mocks.listAiProviders, setCodexProviderEnabled: vi.fn(), syncCodexProvider: vi.fn(), updateAiModel: vi.fn(), updateAiProvider: vi.fn()
}));

const { default: router } = await import("../src/server/routes/ai-providers.js");

describe("AI provider route authorization", () => {
  it("keeps available model metadata owner-only", async () => {
    const app = express();
    app.use(router);
    const server = app.listen(0);
    try {
      const address = server.address();
      if (!address || typeof address === "string") throw new Error("Server did not start.");
      const response = await fetch(`http://127.0.0.1:${address.port}/api/ai/models/available`, { headers: { authorization: "member" } });
      expect(response.status).toBe(403);
      expect(mocks.listAiProviders).not.toHaveBeenCalled();
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});
