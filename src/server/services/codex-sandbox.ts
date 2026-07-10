export const CODEX_TASK_PERMISSION_PROFILE = "yanto-task";
export const CODEX_CONTAINER_HOME = "/data/codex";
export const CODEX_CONTAINER_WORKSPACE = "/workspace";

export const codexTaskConfig = {
  approval_policy: "never",
  default_permissions: CODEX_TASK_PERMISSION_PROFILE,
  permissions: {
    [CODEX_TASK_PERMISSION_PROFILE]: {
      filesystem: {
        ":minimal": "read",
        ":workspace_roots": "write",
        [CODEX_CONTAINER_HOME]: "deny"
      },
      network: { enabled: false }
    }
  }
} as const;

export function codexTaskConfigArgs() {
  return [
    "--config", 'approval_policy="never"',
    "--config", `default_permissions="${CODEX_TASK_PERMISSION_PROFILE}"`,
    "--config", `permissions.${CODEX_TASK_PERMISSION_PROFILE}.filesystem.:minimal="read"`,
    "--config", `permissions.${CODEX_TASK_PERMISSION_PROFILE}.filesystem.:workspace_roots="write"`,
    "--config", `permissions.${CODEX_TASK_PERMISSION_PROFILE}.filesystem.${CODEX_CONTAINER_HOME}="deny"`,
    "--config", `permissions.${CODEX_TASK_PERMISSION_PROFILE}.network.enabled=false`
  ];
}

type DockerCreateInput = {
  name: string;
  workspaceHost: string;
  codexHomeHost: string;
  labels: string[];
  entrypoint: string;
  command: string[];
};

/** Shared outer isolation for both the production runner and its image probe. */
export function codexDockerCreateArgs(image: string, input: DockerCreateInput) {
  return [
    "create", "-i", "--name", input.name,
    ...input.labels.flatMap((label) => ["--label", label]),
    "--workdir", CODEX_CONTAINER_WORKSPACE,
    // The Codex parent needs the API; its task commands are network-disabled by the inner profile.
    "--network", "bridge",
    "--memory", "4g", "--cpus", "2", "--pids-limit", "512",
    "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
    "-e", `CODEX_HOME=${CODEX_CONTAINER_HOME}`,
    "-v", `${input.workspaceHost}:${CODEX_CONTAINER_WORKSPACE}`,
    "-v", `${input.codexHomeHost}:${CODEX_CONTAINER_HOME}`,
    "--entrypoint", input.entrypoint,
    image,
    ...input.command
  ];
}
