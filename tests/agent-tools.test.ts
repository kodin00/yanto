import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AgentWorkspace } from "../src/server/services/agent-tools.js";

describe("agent workspace tools", () => {
  let root: string;
  let workspace: AgentWorkspace;

  beforeEach(async () => {
    root = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-agent-tools-"));
    workspace = new AgentWorkspace(root);
  });

  afterEach(async () => { await fs.rm(root, { recursive: true, force: true }); });

  it("supports scoped write, read, replace, list, and search operations", async () => {
    await expect(workspace.execute("write_file", { path: "src/example.ts", content: "const value = 1;\n" })).resolves.toContain("Wrote");
    await expect(workspace.execute("read_file", { path: "src/example.ts" })).resolves.toContain("1: const value = 1;");
    await expect(workspace.execute("replace_text", { path: "src/example.ts", old_text: "1", new_text: "2" })).resolves.toContain("Updated");
    await expect(workspace.execute("search_files", { path: "src", query: "value = 2" })).resolves.toContain("src/example.ts:1");
    await expect(workspace.execute("list_files", { path: ".", depth: 3 })).resolves.toContain("src/example.ts");
  });

  it("blocks traversal, Git metadata, and symlink escapes", async () => {
    const outside = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-agent-outside-"));
    await fs.writeFile(path.join(outside, "secret.txt"), "secret");
    await fs.symlink(outside, path.join(root, "escape"));
    await expect(workspace.execute("read_file", { path: "../secret.txt" })).rejects.toThrow("outside");
    await expect(workspace.execute("read_file", { path: ".git" })).rejects.toThrow("Git metadata");
    await expect(workspace.execute("write_file", { path: "escape/secret.txt", content: "changed" })).rejects.toThrow("Symlink");
    await fs.rm(outside, { recursive: true, force: true });
  });

  it("runs commands directly with the worktree as cwd and without inheriting Yanto secrets", async () => {
    process.env.YANTO_TEST_SECRET = "must-not-leak";
    await workspace.start();
    const output = await workspace.execute("run_command", {
      command: "printf '%s\\n%s\\n%s' \"$PWD\" \"${YANTO_TEST_SECRET-unset}\" \"$HOME\""
    });
    delete process.env.YANTO_TEST_SECRET;

    expect(output).toContain(root);
    expect(output).toContain("unset");
    const home = output.trim().split("\n").at(-1)!;
    expect(home).not.toBe(root);
    expect(home.startsWith(root)).toBe(false);
  });

  it("cancels the host command process group through the run signal", async () => {
    const controller = new AbortController();
    const cancelable = new AgentWorkspace(root, { signal: controller.signal });
    const ready = path.join(root, "descendant-ready");
    const survived = path.join(root, "descendant-survived");
    await cancelable.start();
    const command = cancelable.execute("run_command", {
      command: `(sleep 0.25; printf survived > ${JSON.stringify(survived)}) & printf ready > ${JSON.stringify(ready)}; wait`
    });
    for (let attempt = 0; attempt < 50; attempt += 1) {
      if (await fs.access(ready).then(() => true).catch(() => false)) break;
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    expect(await fs.access(ready).then(() => true).catch(() => false)).toBe(true);
    controller.abort(new Error("Stopped by test."));

    await expect(command).rejects.toThrow("Stopped by test.");
    await cancelable.stop();
    await new Promise((resolve) => setTimeout(resolve, 350));
    await expect(fs.access(survived)).rejects.toMatchObject({ code: "ENOENT" });
  });
});
