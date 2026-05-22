import { describe, expect, it } from "vitest";
import { normalizeFolderName, projectPath } from "../src/server/services/paths.js";

describe("project paths", () => {
  it("accepts safe folder names", () => {
    expect(normalizeFolderName("my-app_1.2")).toBe("my-app_1.2");
  });

  it("rejects path traversal and absolute paths", () => {
    expect(() => normalizeFolderName("../app")).toThrow();
    expect(() => normalizeFolderName("/srv/app")).toThrow();
  });

  it("resolves projects below the configured root", () => {
    expect(projectPath("demo-app")).toMatch(/\/projects\/demo-app$/);
  });
});
