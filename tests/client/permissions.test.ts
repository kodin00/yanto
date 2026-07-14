import { describe, expect, it } from "vitest";
import type { SessionUser } from "../../src/shared/types";
import { canView } from "../../src/client/permissions";

function session(role: SessionUser["role"], allowedViews: SessionUser["allowedViews"]): SessionUser {
  return {
    id: "usr_test",
    username: role,
    role,
    status: "active",
    projectAccess: [],
    allowedViews,
    appBaseUrl: "https://yanto.test",
    localNodeId: "node_master_local"
  };
}

describe("permission-aware client views", () => {
  it("keeps AI Tasks owner-only even when a legacy member session lists the view", () => {
    expect(canView(session("member", ["dashboard", "tasks"]), "tasks")).toBe(false);
  });

  it("allows the owner to open AI Tasks", () => {
    expect(canView(session("owner", []), "tasks")).toBe(true);
  });
});
