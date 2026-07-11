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
  inheritEnv?: boolean;
  signal?: AbortSignal;
  killProcessGroup?: boolean;
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
      env: options.inheritEnv === false ? options.env : { ...process.env, ...options.env },
      detached: options.killProcessGroup === true && process.platform !== "win32",
      shell: false
    });

    let output = "";
    let outputBytes = 0;
    let truncated = false;
    let settled = false;
    let timedOut = false;
    let abortKillTimer: NodeJS.Timeout | undefined;
    const timeout =
      timeoutMs > 0
        ? setTimeout(() => {
            timedOut = true;
            kill("SIGTERM");
            setTimeout(() => {
              if (!settled) {
                kill("SIGKILL");
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
      options.signal?.removeEventListener("abort", abort);
      if (abortKillTimer) clearTimeout(abortKillTimer);
      if (timeout) {
        clearTimeout(timeout);
      }
      if (extraOutput) {
        output += extraOutput;
      }
      resolve({ exitCode, output, timedOut: timedOut || undefined, truncated: truncated || undefined });
    };

    const kill = (signal: NodeJS.Signals) => {
      if (options.killProcessGroup && process.platform !== "win32" && child.pid) {
        try { process.kill(-child.pid, signal); return; } catch { /* Fall back to the direct child. */ }
      }
      child.kill(signal);
    };

    const abort = () => {
      kill("SIGTERM");
      abortKillTimer = setTimeout(() => {
        if (!settled) kill("SIGKILL");
      }, 5_000);
      abortKillTimer.unref();
    };

    options.signal?.addEventListener("abort", abort, { once: true });
    if (options.signal?.aborted) abort();

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

/**
 * Runs a command and throws if it exits with a non-zero code.
 * Returns the command output on success.
 */
export async function runCommandChecked(command: string, args: string[], options?: RunCommandOptions): Promise<string> {
  const result = await runCommand(command, args, options);
  if (result.exitCode !== 0) {
    throw new Error(`Command "${command} ${args.join(" ")}" failed with exit code ${result.exitCode}: ${result.output.slice(-500)}`);
  }
  return result.output;
}
