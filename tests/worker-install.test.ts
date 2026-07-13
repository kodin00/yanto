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
    expect(dockerfile).toContain("then exec node dist/server/server/worker.js");
    expect(dockerfile).toContain("else exec node dist/server/server/index.js");
    expect(dockerfile).toContain("FRP_VERSION=0.69.0");
    expect(dockerfile).toContain("ARG TARGETARCH\n");
    expect(dockerfile).not.toContain("ARG TARGETARCH=amd64");
    expect(dockerfile).toContain("24a4fc82b4c041835103419685ea124c4d6a7dbf83d0425481c5831b4ce4b3a4");
    expect(dockerfile).toContain("sha256sum -c");
  });

  it("documents master and worker one-liner entrypoints in the installer", () => {
    const installer = fs.readFileSync("scripts/install.sh", "utf8");

    expect(installer).toContain("master|worker");
    expect(installer).toContain("compose.worker.yml");
    expect(installer).toContain("--join-token");
    expect(installer).toContain("JWT_SECRET=change-this-to-a-long-random-secret");
    expect(installer).toContain("ADMIN_PASSWORD=change-this-admin-password");
    expect(installer).toContain("POSTGRES_PASSWORD=$(random_secret)");
    expect(installer).toContain("chmod 600 .env");
  });
});
