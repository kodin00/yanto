import { spawn } from "node:child_process";

export type CommandResult = {
  exitCode: number;
  output: string;
};

export type RunCommandOptions = {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  onData?: (chunk: string) => void;
};

export function runCommand(command: string, args: string[], options: RunCommandOptions = {}) {
  return new Promise<CommandResult>((resolve) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: {
        ...process.env,
        ...options.env
      },
      shell: false
    });

    let output = "";
    const collect = (buffer: Buffer) => {
      const chunk = buffer.toString();
      output += chunk;
      options.onData?.(chunk);
    };

    child.stdout.on("data", collect);
    child.stderr.on("data", collect);
    child.on("error", (error) => {
      const chunk = `${error.message}\n`;
      output += chunk;
      options.onData?.(chunk);
      resolve({ exitCode: 1, output });
    });
    child.on("close", (exitCode) => {
      resolve({ exitCode: exitCode ?? 1, output });
    });
  });
}
