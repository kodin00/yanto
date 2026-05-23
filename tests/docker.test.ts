import { beforeEach, describe, expect, it, vi } from "vitest";

const runCommand = vi.hoisted(() => vi.fn());

vi.mock("../src/server/services/commands.js", () => ({
  runCommand
}));

import { cleanupDocker, listContainers, normalizeDockerCreatedAt, previewDockerCleanup, restartContainer, stopContainer } from "../src/server/services/docker.js";

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

  it("keeps cleanup preview usable when optional docker detail commands are unavailable", async () => {
    runCommand.mockImplementation(async (_command: string, args: string[]) => ({
      exitCode: args[0] === "builder" ? 1 : 0,
      output: args[0] === "builder" ? "Usage: docker builder COMMAND" : "ok"
    }));

    const output = await previewDockerCleanup();

    expect(output).toContain("$ docker system df\nok\n");
    expect(output).toContain("$ docker builder du\nUsage: docker builder COMMAND\n");
    expect(output).toContain("Preview detail command failed; continuing");
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

  it("marks likely Postgres containers from image and compose service labels", async () => {
    runCommand
      .mockResolvedValueOnce({
        exitCode: 0,
        output: [
          JSON.stringify({
            ID: "pg-id",
            Names: "shop-db-1",
            Image: "postgres:16",
            Status: "Up 2 minutes",
            State: "running",
            Ports: "5432/tcp",
            CreatedAt: "2026-05-23 10:00:00 +0000 UTC",
            Labels: "com.docker.compose.project=shop,com.docker.compose.service=db"
          }),
          JSON.stringify({
            ID: "web-id",
            Names: "shop-web-1",
            Image: "node:22",
            Status: "Up 2 minutes",
            State: "running",
            Ports: "3000/tcp",
            CreatedAt: "2026-05-23 10:00:00 +0000 UTC",
            Labels: "com.docker.compose.project=shop,com.docker.compose.service=web"
          })
        ].join("\n")
      })
      .mockResolvedValueOnce({
        exitCode: 0,
        output: [
          JSON.stringify({ ID: "pg-id", CPUPerc: "0.10%", MemUsage: "64MiB / 1GiB", MemPerc: "6.25%" }),
          JSON.stringify({ ID: "web-id", CPUPerc: "0.20%", MemUsage: "32MiB / 1GiB", MemPerc: "3.13%" })
        ].join("\n")
      });

    await expect(listContainers()).resolves.toMatchObject([
      {
        id: "pg-id",
        createdAt: "2026-05-23T10:00:00.000Z",
        composeProject: "shop",
        composeService: "db",
        isPostgresCandidate: true
      },
      {
        id: "web-id",
        composeProject: "shop",
        composeService: "web",
        isPostgresCandidate: false
      }
    ]);
  });

  it("normalizes Docker created timestamps with timezone names", () => {
    expect(normalizeDockerCreatedAt("2026-05-23 10:00:00 +0700 WIB")).toBe("2026-05-23T03:00:00.000Z");
    expect(normalizeDockerCreatedAt("2026-05-23 10:00:00 +0000 UTC")).toBe("2026-05-23T10:00:00.000Z");
    expect(normalizeDockerCreatedAt(undefined)).toBeNull();
  });
});
