import { describe, expect, it } from "vitest";
import { constantTimeEqual, createDeployToken, createId, createWorkerJoinToken } from "../src/server/services/tokens.js";

describe("tokens", () => {
  it("creates identifiable ids and deploy tokens", () => {
    expect(createId("prj")).toMatch(/^prj_[a-f0-9]{24}$/);
    expect(createDeployToken()).toMatch(/^ydp_/);
    expect(createWorkerJoinToken()).toMatch(/^ywj_/);
  });

  it("compares tokens without accepting different lengths", () => {
    expect(constantTimeEqual("same", "same")).toBe(true);
    expect(constantTimeEqual("same", "diff")).toBe(false);
    expect(constantTimeEqual("same", "same-longer")).toBe(false);
  });
});
