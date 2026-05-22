import fs from "node:fs/promises";
import path from "node:path";
import { config } from "../config.js";
import { runCommand } from "./commands.js";

async function fileExists(target: string) {
  try {
    await fs.access(target);
    return true;
  } catch {
    return false;
  }
}

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

export async function resolveGitPrivateKeyPath() {
  if (await fileExists(config.managedSshPrivateKeyPath)) {
    return config.managedSshPrivateKeyPath;
  }
  if (await fileExists(config.sshPrivateKeyPath)) {
    return config.sshPrivateKeyPath;
  }
  return null;
}

export async function saveManagedSshPrivateKey(privateKey: string) {
  const normalized = privateKey.trimEnd();
  if (!normalized.includes("-----BEGIN") || !normalized.includes("PRIVATE KEY-----")) {
    throw new Error("SSH private key must be in OpenSSH private key format.");
  }

  await fs.mkdir(path.dirname(config.managedSshPrivateKeyPath), { recursive: true, mode: 0o700 });
  await fs.writeFile(config.managedSshPrivateKeyPath, `${normalized}\n`, { mode: 0o600 });
  await fs.chmod(config.managedSshPrivateKeyPath, 0o600);

  const publicKeyPath = `${config.managedSshPrivateKeyPath}.pub`;
  const result = await runCommand("ssh-keygen", ["-y", "-f", config.managedSshPrivateKeyPath]);
  if (result.exitCode !== 0) {
    await fs.rm(config.managedSshPrivateKeyPath, { force: true });
    throw new Error(`Unable to read SSH private key: ${result.output}`);
  }
  await fs.writeFile(publicKeyPath, result.output.trim() ? `${result.output.trim()}\n` : "", "utf8");

  return {
    privateKeyPath: config.managedSshPrivateKeyPath,
    publicKey: result.output.trim()
  };
}

export async function managedSshKeyStatus() {
  const hasManagedKey = await fileExists(config.managedSshPrivateKeyPath);
  const hasMountedKey = await fileExists(config.sshPrivateKeyPath);
  let publicKey: string | null = null;
  if (hasManagedKey) {
    try {
      publicKey = (await fs.readFile(`${config.managedSshPrivateKeyPath}.pub`, "utf8")).trim() || null;
    } catch {
      publicKey = null;
    }
  }

  return {
    hasManagedKey,
    hasMountedKey,
    managedPrivateKeyPath: config.managedSshPrivateKeyPath,
    mountedPrivateKeyPath: config.sshPrivateKeyPath,
    activePrivateKeyPath: await resolveGitPrivateKeyPath(),
    publicKey
  };
}
