import { describe, expect, it } from "vitest";
import {
  CODEX_TASK_PERMISSION_PROFILE,
  codexDockerCreateArgs,
  codexTaskConfig,
  codexTaskConfigArgs
} from "../src/server/services/codex-sandbox.js";

describe("Codex account sandbox profile", () => {
  it("allows only the workspace and minimal toolchain while denying CODEX_HOME and network", () => {
    const profile = codexTaskConfig.permissions[CODEX_TASK_PERMISSION_PROFILE];

    expect(codexTaskConfig.default_permissions).toBe(CODEX_TASK_PERMISSION_PROFILE);
    expect(profile.filesystem).toEqual({
      ":minimal": "read",
      ":workspace_roots": "write",
      "/data/codex": "deny"
    });
    expect(profile.network.enabled).toBe(false);
    expect(codexTaskConfigArgs()).toEqual(expect.arrayContaining([
      "--config", `permissions.${CODEX_TASK_PERMISSION_PROFILE}.filesystem./data/codex="deny"`,
      "--config", `permissions.${CODEX_TASK_PERMISSION_PROFILE}.network.enabled=false`
    ]));
  });

  it("uses identical outer isolation without mounting host control-plane data", () => {
    const args = codexDockerCreateArgs("agent:image", {
      name: "probe",
      workspaceHost: "/host/workspace",
      codexHomeHost: "/host/task-home",
      labels: ["probe=true"],
      entrypoint: "node",
      command: ["runner.js"]
    });
    const joined = args.join(" ");

    expect(args).toEqual(expect.arrayContaining([
      "--network", "bridge",
      "--cap-drop", "ALL",
      "--security-opt", "no-new-privileges",
      "-v", "/host/workspace:/workspace",
      "-v", "/host/task-home:/data/codex"
    ]));
    expect(joined).not.toContain("/var/run/docker.sock");
    expect(joined).not.toContain("/.ssh");
    expect(joined).not.toContain("/host/.git");
  });
});
