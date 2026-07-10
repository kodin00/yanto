import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AgentSandbox } from "../src/server/services/agent-tools.js";

describe("agent workspace tools", () => {
  let root: string;
  let sandbox: AgentSandbox;

  beforeEach(async () => {
    root = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-agent-tools-"));
    sandbox = new AgentSandbox("run_test", root, "unused");
  });

  afterEach(async () => { await fs.rm(root, { recursive: true, force: true }); });

  it("supports scoped write, read, replace, list, and search operations", async () => {
    await expect(sandbox.execute("write_file", { path: "src/example.ts", content: "const value = 1;\n" })).resolves.toContain("Wrote");
    await expect(sandbox.execute("read_file", { path: "src/example.ts" })).resolves.toContain("1: const value = 1;");
    await expect(sandbox.execute("replace_text", { path: "src/example.ts", old_text: "1", new_text: "2" })).resolves.toContain("Updated");
    await expect(sandbox.execute("search_files", { path: "src", query: "value = 2" })).resolves.toContain("src/example.ts:1");
    await expect(sandbox.execute("list_files", { path: ".", depth: 3 })).resolves.toContain("src/example.ts");
  });

  it("blocks traversal, Git metadata, and symlink escapes", async () => {
    const outside = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-agent-outside-"));
    await fs.writeFile(path.join(outside, "secret.txt"), "secret");
    await fs.symlink(outside, path.join(root, "escape"));
    await expect(sandbox.execute("read_file", { path: "../secret.txt" })).rejects.toThrow("outside");
    await expect(sandbox.execute("read_file", { path: ".git" })).rejects.toThrow("Git metadata");
    await expect(sandbox.execute("write_file", { path: "escape/secret.txt", content: "changed" })).rejects.toThrow("Symlink");
    await fs.rm(outside, { recursive: true, force: true });
  });
});
