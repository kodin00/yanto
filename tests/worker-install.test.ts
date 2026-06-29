import fs from "node:fs";
import { describe, expect, it } from "vitest";
import YAML from "yaml";

describe("worker install/runtime files", () => {
  it("keeps the worker compose service headless", () => {
    const compose = YAML.parse(fs.readFileSync("compose.worker.yml", "utf8"));

    expect(compose.services.worker.ports).toBeUndefined();
    expect(compose.services.worker.environment.YANTO_NODE_ROLE).toBe("worker");
    expect(compose.services.worker.extra_hosts).toContain("host.docker.internal:host-gateway");
  });

  it("routes worker containers to the worker process through the image command", () => {
    const dockerfile = fs.readFileSync("Dockerfile", "utf8");

    expect(dockerfile).toContain("YANTO_NODE_ROLE");
    expect(dockerfile).toContain("dist/server/server/worker.js");
    expect(dockerfile).toContain("dist/server/server/index.js");
    expect(dockerfile).toContain("FRP_VERSION=0.69.0");
    expect(dockerfile).toContain("sha256sum -c");
  });

  it("documents master and worker one-liner entrypoints in the installer", () => {
    const installer = fs.readFileSync("scripts/install.sh", "utf8");

    expect(installer).toContain("master|worker");
    expect(installer).toContain("compose.worker.yml");
    expect(installer).toContain("--join-token");
  });
});
