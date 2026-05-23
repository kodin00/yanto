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

  it("merges command-specific environment variables", async () => {
    const result = await runCommand("node", ["-e", "process.stdout.write(process.env.YANTO_COMMAND_TEST ?? '')"], {
      env: { YANTO_COMMAND_TEST: "merged" }
    });

    expect(result).toEqual({ exitCode: 0, output: "merged" });
  });

  it("streams stdout and stderr chunks to onData", async () => {
    const chunks: string[] = [];
    const result = await runCommand(
      "node",
      ["-e", "process.stdout.write('out'); process.stderr.write('err')"],
      { onData: (chunk) => chunks.push(chunk) }
    );

    expect(result.exitCode).toBe(0);
    expect(result.output).toContain("out");
    expect(result.output).toContain("err");
    expect(chunks.join("")).toBe(result.output);
  });
});
