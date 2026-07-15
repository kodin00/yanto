import express, { type NextFunction, type Request, type Response } from "express";
import { beforeEach, describe, expect, it, vi } from "vitest";

const accountMocks = vi.hoisted(() => ({
  PROJECT_PERMISSIONS: ["deploy", "runtime", "config", "secrets", "backups", "hostnames"],
  createMember: vi.fn(),
  createResetLink: vi.fn(),
  deleteMember: vi.fn(),
  listManagedUsers: vi.fn(),
  replaceProjectAccess: vi.fn(),
  setUserStatus: vi.fn()
}));

vi.mock("../src/server/auth.js", () => ({
  currentUser: () => ({ username: "owner" }),
  requireOwner: (req: Request, res: Response, next: NextFunction) => {
    if (req.header("authorization") === "ok") {
      req.yantoAuth = { id: "usr_owner", username: "owner", role: "owner", status: "active", sessionVersion: 1, projectAccess: [] };
      next();
      return;
    }
    res.status(401).json({ message: "Authentication required." });
  }
}));
vi.mock("../src/server/services/accounts.js", () => accountMocks);
vi.mock("../src/server/services/audit.js", () => ({ recordAuditLog: vi.fn() }));

const { default: usersRouter } = await import("../src/server/routes/users.js");

function createApp() {
  const app = express();
  app.use(express.json());
  app.use(usersRouter);
  return app;
}

async function removeMember(authorized: boolean) {
  const server = createApp().listen(0);
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Server did not start.");
    return await fetch(`http://127.0.0.1:${address.port}/api/users/usr_member`, {
      method: "DELETE",
      headers: authorized ? { authorization: "ok" } : {}
    });
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

describe("managed user routes", () => {
  beforeEach(() => vi.clearAllMocks());

  it("lets the owner permanently remove a member", async () => {
    accountMocks.deleteMember.mockResolvedValue({ id: "usr_member", username: "operator" });

    const response = await removeMember(true);

    expect(response.status).toBe(204);
    expect(accountMocks.deleteMember).toHaveBeenCalledWith("usr_member");
  });

  it("rejects unauthenticated member deletion", async () => {
    const response = await removeMember(false);

    expect(response.status).toBe(401);
    expect(accountMocks.deleteMember).not.toHaveBeenCalled();
  });
});
