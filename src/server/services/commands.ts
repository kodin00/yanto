import { spawn } from "node:child_process";
import { config } from "../config.js";

export type CommandResult = {
  exitCode: number;
  output: string;
  timedOut?: boolean;
  truncated?: boolean;
};

export type RunCommandOptions = {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs?: number;
  maxOutputBytes?: number;
  onData?: (chunk: string) => void;
};

export function runCommand(command: string, args: string[], options: RunCommandOptions = {}) {
  return new Promise<CommandResult>((resolve) => {
    const timeoutMs = options.timeoutMs ?? config.commandTimeoutMs;
    const maxOutputBytes = options.maxOutputBytes ?? config.commandOutputMaxBytes;
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: {
        ...process.env,
        ...options.env
      },
      shell: false
    });

    let output = "";
    let outputBytes = 0;
    let truncated = false;
    let settled = false;
    let timedOut = false;
    const timeout =
      timeoutMs > 0
        ? setTimeout(() => {
            timedOut = true;
            child.kill("SIGTERM");
            setTimeout(() => {
              if (!settled) {
                child.kill("SIGKILL");
              }
            }, 5000).unref();
          }, timeoutMs)
        : null;
    timeout?.unref();

    const collect = (buffer: Buffer) => {
      const chunk = buffer.toString();
      if (outputBytes < maxOutputBytes) {
        const remaining = maxOutputBytes - outputBytes;
        const next = buffer.byteLength <= remaining ? chunk : buffer.subarray(0, remaining).toString();
        output += next;
        outputBytes += Buffer.byteLength(next);
      }
      if (outputBytes >= maxOutputBytes && buffer.byteLength > 0) {
        truncated = true;
      }
      options.onData?.(chunk);
    };

    const finish = (exitCode: number, extraOutput = "") => {
      if (settled) return;
      settled = true;
      if (timeout) {
        clearTimeout(timeout);
      }
      if (extraOutput) {
        output += extraOutput;
      }
      resolve({ exitCode, output, timedOut: timedOut || undefined, truncated: truncated || undefined });
    };

    child.stdout.on("data", collect);
    child.stderr.on("data", collect);
    child.on("error", (error) => {
      const chunk = `${error.message}\n`;
      options.onData?.(chunk);
      finish(1, chunk);
    });
    child.on("close", (exitCode) => {
      if (timedOut) {
        finish(124, `\nCommand timed out after ${timeoutMs}ms.\n`);
        return;
      }
      finish(exitCode ?? 1);
    });
  });
}
