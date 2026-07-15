import express from "express";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { HttpError } from "../src/server/http-utils.js";

const accountMocks = vi.hoisted(() => ({
  accountSetupDetails: vi.fn(),
  authenticateUser: vi.fn(),
  completeAccountSetup: vi.fn(),
  createInitialOwner: vi.fn(),
  setupStatus: vi.fn(),
  toSessionUser: vi.fn((principal: unknown) => principal)
}));

vi.mock("../src/server/services/accounts.js", () => accountMocks);
vi.mock("../src/server/services/audit.js", () => ({ recordAuditLog: vi.fn() }));

const { default: authRouter } = await import("../src/server/routes/auth.js");

function createApp() {
  const app = express();
  app.use(express.json());
  app.use(authRouter);
  app.use((error: unknown, _req: express.Request, res: express.Response, next: express.NextFunction) => {
    void next;
    res.status(error instanceof HttpError ? error.status : 500).json({ message: error instanceof Error ? error.message : "Unexpected error." });
  });
  return app;
}

async function post(path: string, body: unknown) {
  const server = createApp().listen(0);
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Server did not start.");
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body)
    });
    return { status: response.status, body: await response.json() as { message?: string; username?: string } };
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

describe("account setup routes", () => {
  beforeEach(() => vi.clearAllMocks());

  it("resolves the username for a valid setup link", async () => {
    accountMocks.accountSetupDetails.mockResolvedValue({ username: "deploy-operator" });

    const response = await post("/api/auth/account/setup/preview", { token: "one-time-token" });

    expect(response).toEqual({ status: 200, body: { username: "deploy-operator" } });
    expect(accountMocks.accountSetupDetails).toHaveBeenCalledWith("one-time-token");
  });

  it("rejects mismatched owner passwords before creating an account", async () => {
    const response = await post("/api/setup/owner", {
      username: "owner",
      password: "first",
      passwordConfirmation: "second",
      setupCode: "setup-code"
    });

    expect(response).toEqual({ status: 400, body: { message: "Passwords do not match." } });
    expect(accountMocks.createInitialOwner).not.toHaveBeenCalled();
  });

  it("rejects mismatched member passwords before consuming the link", async () => {
    const response = await post("/api/auth/account/setup", {
      token: "one-time-token",
      password: "first",
      passwordConfirmation: "second"
    });

    expect(response).toEqual({ status: 400, body: { message: "Passwords do not match." } });
    expect(accountMocks.completeAccountSetup).not.toHaveBeenCalled();
  });
});
