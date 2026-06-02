import { afterAll, describe, expect, it, vi } from "vitest";
import fs from "node:fs/promises";
import path from "node:path";

const testPaths = vi.hoisted(() => {
  const projectsRoot = `/tmp/yanto-paths-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  process.env.PROJECTS_ROOT = projectsRoot;
  return { projectsRoot };
});

import { normalizeFolderName, projectPath, removeProjectDirectory } from "../src/server/services/paths.js";

describe("project paths", () => {
  afterAll(async () => {
    await fs.rm(testPaths.projectsRoot, { recursive: true, force: true });
  });

  it("accepts safe folder names", () => {
    expect(normalizeFolderName("my-app_1.2")).toBe("my-app_1.2");
  });

  it("rejects path traversal and absolute paths", () => {
    expect(() => normalizeFolderName("../app")).toThrow();
    expect(() => normalizeFolderName("/srv/app")).toThrow();
  });

  it("resolves projects below the configured root", () => {
    expect(projectPath("demo-app")).toBe(path.join(testPaths.projectsRoot, "demo-app"));
  });

  it("removes a project directory inside the configured root", async () => {
    const target = projectPath("delete-me");
    await fs.mkdir(target, { recursive: true });
    await fs.writeFile(path.join(target, "compose.yml"), "services: {}\n", "utf8");

    await removeProjectDirectory("delete-me");

    await expect(fs.access(target)).rejects.toThrow();
  });
});
