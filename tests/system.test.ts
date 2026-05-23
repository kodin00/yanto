import { beforeEach, describe, expect, it, vi } from "vitest";

const runCommand = vi.hoisted(() => vi.fn());
const osMock = vi.hoisted(() => ({
  totalmem: vi.fn(),
  freemem: vi.fn(),
  loadavg: vi.fn(),
  cpus: vi.fn()
}));

vi.mock("../src/server/services/commands.js", () => ({
  runCommand
}));

vi.mock("node:os", () => ({
  default: osMock
}));

import { systemUsage } from "../src/server/services/system.js";

describe("system usage aggregation", () => {
  beforeEach(() => {
    runCommand.mockReset();
    osMock.totalmem.mockReturnValue(1_000);
    osMock.freemem.mockReturnValue(250);
    osMock.loadavg.mockReturnValue([2, 1, 0.5]);
    osMock.cpus.mockReturnValue([{}, {}, {}, {}]);
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
