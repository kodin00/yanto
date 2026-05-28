import { describe, expect, it } from "vitest";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import YAML from "yaml";
import { autoStartOverrideFile, buildAutoStartOverride, readProjectCompose } from "../src/server/services/compose.js";

describe("compose helpers", () => {
  it("builds a restart override for each service", () => {
    const override = YAML.parse(
      buildAutoStartOverride(`
services:
  web:
    build: .
  worker:
    image: node:22
`)
    );

    expect(autoStartOverrideFile()).toBe(".yanto.restart.override.yml");
    expect(override).toEqual({
      services: {
        web: { restart: "unless-stopped" },
        worker: { restart: "unless-stopped" }
      }
    });
  });

  it("rejects compose files without services", () => {
    expect(() => buildAutoStartOverride("volumes:\n  data:\n")).toThrow("services object");
  });

  it("reads a real project compose file for overrides", async () => {
    const tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-compose-"));
    await fs.writeFile(path.join(tempDir, "compose.yml"), "services:\n  web:\n    image: nginx\n", "utf8");

    await expect(
      readProjectCompose({
        localPath: tempDir,
        composeFile: "compose.yml"
      } as Parameters<typeof readProjectCompose>[0])
    ).resolves.toEqual({
      composeFile: "compose.yml",
      content: "services:\n  web:\n    image: nginx\n",
      exists: true
    });
  });
});
