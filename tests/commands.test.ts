import { describe, expect, it } from "vitest";
import { runCommand } from "../src/server/services/commands.js";

describe("command runner", () => {
  it("captures stdout and exit code", async () => {
    const result = await runCommand("node", ["-e", "process.stdout.write('ok')"]);
    expect(result).toEqual({ exitCode: 0, output: "ok" });
  });

  it("captures failing commands", async () => {
    const result = await runCommand("node", ["-e", "process.stderr.write('bad'); process.exit(3)"]);
    expect(result.exitCode).toBe(3);
    expect(result.output).toBe("bad");
  });
});
