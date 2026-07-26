import { beforeEach, describe, expect, it, vi } from "vitest";

const runCommand = vi.hoisted(() => vi.fn());
const poolQuery = vi.hoisted(() => vi.fn());
const osMock = vi.hoisted(() => ({
  totalmem: vi.fn(),
  freemem: vi.fn(),
  loadavg: vi.fn(),
  cpus: vi.fn()
}));

vi.mock("../src/server/services/commands.js", () => ({
  runCommand
}));

vi.mock("../src/server/db/index.js", () => ({
  pool: { query: poolQuery }
}));

vi.mock("node:os", () => ({
  default: osMock
}));

import { healthStatus, systemUsage } from "../src/server/services/system.js";

describe("system usage aggregation", () => {
  beforeEach(() => {
    runCommand.mockReset();
    poolQuery.mockReset();
    osMock.totalmem.mockReturnValue(1_000);
    osMock.freemem.mockReturnValue(250);
    osMock.loadavg.mockReturnValue([2, 1, 0.5]);
    osMock.cpus.mockReturnValue([{}, {}, {}, {}]);
  });

  it("runs database and Docker health checks concurrently with independent short deadlines", async () => {
    let resolveDatabase!: () => void;
    poolQuery.mockImplementationOnce(() => new Promise<void>((resolve) => { resolveDatabase = resolve; }));
    runCommand.mockResolvedValueOnce({ exitCode: 0, output: "27.5.1\n" });

    const health = healthStatus();
    await vi.waitFor(() => expect(runCommand).toHaveBeenCalledOnce());
    expect(poolQuery).toHaveBeenCalledWith({ text: "SELECT 1", query_timeout: 3_000 });
    expect(runCommand).toHaveBeenCalledWith(
      "docker",
      ["version", "--format", "{{.Server.Version}}"],
      expect.objectContaining({ signal: expect.any(AbortSignal), timeoutMs: 3_000, killProcessGroup: true })
    );

    resolveDatabase();
    await expect(health).resolves.toMatchObject({
      ok: true,
      checks: { database: { ok: true }, docker: { ok: true, message: "27.5.1" } }
    });
  });

  it("reports check failures without preventing the other health check", async () => {
    poolQuery.mockRejectedValueOnce(new Error("database deadline exceeded"));
    runCommand.mockResolvedValueOnce({ exitCode: 124, output: "Docker check timed out" });

    await expect(healthStatus()).resolves.toMatchObject({
      ok: false,
      checks: {
        database: { ok: false, message: "database deadline exceeded" },
        docker: { ok: false, message: "Docker check timed out" }
      }
    });
  });

  it("returns after the health deadline and aborts a stuck Docker process", async () => {
    vi.useFakeTimers();
    let dockerSignal: AbortSignal | undefined;
    poolQuery.mockImplementationOnce(() => new Promise(() => undefined));
    runCommand.mockImplementationOnce((_command, _args, options: { signal?: AbortSignal }) => {
      dockerSignal = options.signal;
      return new Promise(() => undefined);
    });
    try {
      const health = healthStatus();
      await vi.advanceTimersByTimeAsync(3_000);

      await expect(health).resolves.toMatchObject({
        ok: false,
        checks: {
          database: { ok: false, message: "Database health check timed out after 3000ms." },
          docker: { ok: false, message: "Docker health check timed out after 3000ms." }
        }
      });
      expect(dockerSignal?.aborted).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("aggregates cpu, memory, and parsed storage usage", async () => {
    runCommand.mockResolvedValue({
      exitCode: 0,
      output: [
        "Filesystem 1024-blocks Used Available Capacity Mounted on",
        "/dev/root 100 25 75 25% /",
        "/dev/projects 200 150 50 75% /projects"
      ].join("\n")
    });

    await expect(systemUsage()).resolves.toEqual({
      cpuLoadPercent: 50,
      memory: {
        total: 1_000,
        used: 750,
        free: 250,
        usedPercent: 75
      },
      storage: [
        {
          filesystem: "/dev/root",
          size: 102_400,
          used: 25_600,
          available: 76_800,
          usedPercent: 25,
          mount: "/"
        },
        {
          filesystem: "/dev/projects",
          size: 204_800,
          used: 153_600,
          available: 51_200,
          usedPercent: 75,
          mount: "/projects"
        }
      ]
    });
    expect(runCommand).toHaveBeenCalledWith("df", ["-kP", "/", "/projects"]);
  });

  it("caps cpu load percentage and ignores malformed df rows", async () => {
    osMock.loadavg.mockReturnValue([12, 0, 0]);
    osMock.cpus.mockReturnValue([{}, {}]);
    runCommand.mockResolvedValue({
      exitCode: 0,
      output: "Filesystem 1024-blocks Used Available Capacity Mounted on\nnot-enough-columns\n/dev/root 10 5 5 50% /"
    });

    const usage = await systemUsage();

    expect(usage.cpuLoadPercent).toBe(100);
    expect(usage.storage).toHaveLength(1);
    expect(usage.storage[0]?.mount).toBe("/");
  });
});
