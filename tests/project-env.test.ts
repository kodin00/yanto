import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ProjectRow } from "../src/server/db/schema.js";
import { readProjectEnvVariables, writeProjectEnvVariables } from "../src/server/services/project-env.js";

let tempDir: string;

function project(overrides: Partial<ProjectRow> = {}): ProjectRow {
  return {
    id: "prj_test",
    name: "Test project",
    gitUrl: null,
    branch: "master",
    folderName: "test-project",
    localPath: tempDir,
    composeFile: "docker-compose.yml",
    composeContent: null,
    envFile: ".env.production",
    autoStart: false,
    deployToken: "token",
    sshPrivateKeyPath: null,
    sshPublicKey: null,
    createdAt: new Date(),
    updatedAt: new Date(),
    ...overrides
  };
}

describe("project env files", () => {
  beforeEach(async () => {
    tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-env-"));
  });

  afterEach(async () => {
    await fs.rm(tempDir, { recursive: true, force: true });
  });

  it("reads .env in the project directory by default", async () => {
    await fs.writeFile(path.join(tempDir, ".env"), "APP_PORT=3000\nJWT_SECRET=super-secret-value\n");
    await fs.writeFile(path.join(tempDir, ".env.production"), "APP_PORT=9000\n");

    const rows = await readProjectEnvVariables(project());

    expect(rows).toEqual([
      { key: "APP_PORT", value: "3000", masked: false },
      { key: "JWT_SECRET", value: "********", masked: true }
    ]);
  });

  it("writes .env in the project directory by default", async () => {
    await writeProjectEnvVariables(project(), [{ key: "APP_PORT", value: "8080" }]);

    await expect(fs.readFile(path.join(tempDir, ".env"), "utf8")).resolves.toBe("APP_PORT=8080\n");
    await expect(fs.access(path.join(tempDir, ".env.production"))).rejects.toThrow();
  });
});
