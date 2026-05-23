import { beforeEach, describe, expect, it, vi } from "vitest";

const runCommand = vi.hoisted(() => vi.fn());

vi.mock("../src/server/services/commands.js", () => ({
  runCommand
}));

import { cleanupDocker, restartContainer, stopContainer } from "../src/server/services/docker.js";

describe("docker helpers", () => {
  beforeEach(() => {
    runCommand.mockReset();
  });

  it("runs cleanup commands in the protected order and returns a command transcript", async () => {
    runCommand.mockResolvedValue({ exitCode: 0, output: "done" });

    const output = await cleanupDocker();

    expect(runCommand.mock.calls).toEqual([
      ["docker", ["builder", "prune", "-f"]],
      ["docker", ["image", "prune", "-f"]],
      ["docker", ["container", "prune", "-f"]],
      ["docker", ["network", "prune", "-f"]],
      ["sh", ["-lc", "command -v apt-get >/dev/null 2>&1 && apt-get clean || true"]]
    ]);
    expect(output).toContain("$ docker builder prune -f\ndone\n");
    expect(output).toContain("$ sh -lc command -v apt-get >/dev/null 2>&1 && apt-get clean || true\ndone\n");
  });

  it("stops cleanup after a failing command and preserves the preview transcript", async () => {
    runCommand.mockImplementation(async (_command: string, args: string[]) => ({
      exitCode: args[0] === "image" ? 7 : 0,
      output: args[0] === "image" ? "image prune failed" : "ok"
    }));

    await expect(cleanupDocker()).rejects.toThrow(
      "$ docker builder prune -f\nok\n$ docker image prune -f\nimage prune failed\n"
    );
    expect(runCommand).toHaveBeenCalledTimes(2);
  });

  it("protects Yanto app containers from stop actions", async () => {
    runCommand.mockResolvedValueOnce({ exitCode: 0, output: "/yanto-app-1\n" });

    await expect(stopContainer("container-id")).rejects.toThrow("protected");

    expect(runCommand.mock.calls).toEqual([["docker", ["inspect", "--format", "{{.Name}}", "container-id"]]]);
  });

  it("inspects before restarting unprotected containers", async () => {
    runCommand
      .mockResolvedValueOnce({ exitCode: 0, output: "/customer-web-1\n" })
      .mockResolvedValueOnce({ exitCode: 0, output: "customer-web-1\n" });

    await restartContainer("container-id");

    expect(runCommand.mock.calls).toEqual([
      ["docker", ["inspect", "--format", "{{.Name}}", "container-id"]],
      ["docker", ["restart", "container-id"]]
    ]);
  });
});
