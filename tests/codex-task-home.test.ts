import fs from "node:fs/promises";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ codexHome: `/tmp/yanto-codex-home-${process.pid}` }));

vi.mock("../src/server/config.js", () => ({
  config: { codexHome: mocks.codexHome }
}));

import { clearCodexTaskAuthentication, prepareCodexTaskHome } from "../src/server/services/codex-account-runner.js";

describe("per-task Codex homes", () => {
  beforeEach(async () => {
    await fs.rm(mocks.codexHome, { recursive: true, force: true });
    await fs.mkdir(path.join(mocks.codexHome, "sessions", "2026", "07", "10"), { recursive: true });
    await fs.writeFile(path.join(mocks.codexHome, "auth.json"), "account-auth", { mode: 0o600 });
  });

  afterEach(async () => {
    await fs.rm(mocks.codexHome, { recursive: true, force: true });
  });

  it("seeds authentication and imports only the selected legacy thread", async () => {
    const selected = "019f-selected-thread";
    const other = "019f-other-thread";
    const legacy = path.join(mocks.codexHome, "sessions", "2026", "07", "10");
    await fs.writeFile(path.join(legacy, `rollout-${selected}.jsonl`), "selected-session");
    await fs.writeFile(path.join(legacy, `rollout-${other}.jsonl`), "other-session");

    const taskHome = await prepareCodexTaskHome("agt_one", selected);
    const secondHome = await prepareCodexTaskHome("agt_two", null);

    expect(taskHome).not.toBe(secondHome);
    expect(await fs.readFile(path.join(taskHome, "auth.json"), "utf8")).toBe("account-auth");
    expect(await fs.readFile(path.join(taskHome, "sessions", "2026", "07", "10", `rollout-${selected}.jsonl`), "utf8")).toBe("selected-session");
    await expect(fs.stat(path.join(taskHome, "sessions", "2026", "07", "10", `rollout-${other}.jsonl`))).rejects.toMatchObject({ code: "ENOENT" });
    await expect(fs.stat(path.join(secondHome, "sessions"))).rejects.toMatchObject({ code: "ENOENT" });
  });

  it("fails closed when the persistent account credential is missing", async () => {
    await fs.rm(path.join(mocks.codexHome, "auth.json"));
    await expect(prepareCodexTaskHome("agt_missing", null)).rejects.toThrow("Sign in again");
  });

  it("removes task-local credential copies on logout without deleting sessions", async () => {
    const taskHome = await prepareCodexTaskHome("agt_logout", null);
    await fs.mkdir(path.join(taskHome, "sessions"));
    await fs.writeFile(path.join(taskHome, "sessions", "thread.jsonl"), "conversation");

    await clearCodexTaskAuthentication();

    await expect(fs.stat(path.join(taskHome, "auth.json"))).rejects.toMatchObject({ code: "ENOENT" });
    expect(await fs.readFile(path.join(taskHome, "sessions", "thread.jsonl"), "utf8")).toBe("conversation");
  });
});
