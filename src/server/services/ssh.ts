import fs from "node:fs/promises";
import path from "node:path";
import { config } from "../config.js";
import { runCommand } from "./commands.js";

export async function ensureSshKey(projectId: string) {
  await fs.mkdir(config.sshKeysDir, { recursive: true, mode: 0o700 });
  const keyPath = path.join(config.sshKeysDir, projectId);
  const publicPath = `${keyPath}.pub`;

  try {
    const publicKey = await fs.readFile(publicPath, "utf8");
    return { privateKeyPath: keyPath, publicKey: publicKey.trim() };
  } catch {
    const result = await runCommand("ssh-keygen", ["-t", "ed25519", "-N", "", "-C", `yanto-${projectId}`, "-f", keyPath]);
    if (result.exitCode !== 0) {
      throw new Error(`Failed to generate SSH key: ${result.output}`);
    }
    await fs.chmod(keyPath, 0o600);
    const publicKey = await fs.readFile(publicPath, "utf8");
    return { privateKeyPath: keyPath, publicKey: publicKey.trim() };
  }
}

export function gitSshEnv(privateKeyPath: string | null) {
  const baseCommand = "ssh -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/tmp/yanto_known_hosts";
  return {
    GIT_SSH_COMMAND: privateKeyPath ? `${baseCommand} -i ${privateKeyPath}` : baseCommand
  };
}
