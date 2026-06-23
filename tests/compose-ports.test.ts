import { beforeEach, describe, expect, it, vi } from "vitest";

const listContainers = vi.hoisted(() => vi.fn());
const runCommand = vi.hoisted(() => vi.fn());

vi.mock("../src/server/services/docker.js", () => ({
  listContainers
}));

vi.mock("../src/server/services/commands.js", () => ({
  runCommand
}));

import { extractPublishedComposePorts, findComposePortConflicts, formatComposePortConflictMessage } from "../src/server/services/compose.js";

describe("compose port preflight", () => {
  beforeEach(() => {
    listContainers.mockReset();
    runCommand.mockReset();
    listContainers.mockResolvedValue([]);
    runCommand.mockResolvedValue({ exitCode: 0, output: "" });
  });

  it("extracts numeric published ports from short and long compose syntax", () => {
    expect(
      extractPublishedComposePorts(`
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - "127.0.0.1:3000:3000/tcp"
      - "[::1]:9443:443"
      - "\${APP_PORT:-5173}:5173"
      - target: 443
        published: "8443"
        protocol: tcp
`)
    ).toMatchObject([
      { service: "web", port: 8080, protocol: "tcp" },
      { service: "web", port: 3000, protocol: "tcp" },
      { service: "web", port: 9443, protocol: "tcp" },
      { service: "web", port: 8443, protocol: "tcp" }
    ]);
  });

  it("reports conflicts with compose duplicates, running containers, and listening host processes", async () => {
    listContainers.mockResolvedValue([
      {
        name: "existing-web-1",
        state: "running",
        ports: "0.0.0.0:8080->80/tcp, [::]:8443->443/tcp",
        composeProject: "other"
      },
      {
        name: "stopped-api-1",
        state: "exited",
        ports: "0.0.0.0:3000->3000/tcp",
        composeProject: "old"
      }
    ]);
    runCommand.mockResolvedValue({
      exitCode: 0,
      output: "node 123 user 12u IPv4 0x123 0t0 TCP *:3000 (LISTEN)\n"
    });

    const conflicts = await findComposePortConflicts(`
services:
  web:
    ports:
      - "8080:80"
      - "3000:3000"
  api:
    ports:
      - "8080:8080"
      - target: 443
        published: 8443
`);

    expect(conflicts).toEqual(expect.arrayContaining([
      expect.objectContaining({ service: "api", port: 8080, conflictWith: "service web" }),
      expect.objectContaining({ service: "web", port: 8080, conflictWith: expect.stringContaining("existing-web-1") }),
      expect.objectContaining({ service: "api", port: 8443, conflictWith: expect.stringContaining("existing-web-1") }),
      expect.objectContaining({ service: "web", port: 3000, conflictWith: "node process" })
    ]));
    expect(formatComposePortConflictMessage(conflicts)).toContain("Docker compose port conflict detected");
  });

  it("ignores ports already published by the same compose project", async () => {
    listContainers.mockResolvedValue([
      {
        name: "shop-web-1",
        state: "running",
        ports: "0.0.0.0:8080->80/tcp",
        composeProject: "shop"
      }
    ]);
    runCommand.mockResolvedValue({
      exitCode: 0,
      output: "com.docke 123 user 12u IPv4 0x123 0t0 TCP *:8080 (LISTEN)\n"
    });

    await expect(
      findComposePortConflicts(
        `
services:
  web:
    ports:
      - "8080:80"
`,
        { ignoreComposeProject: "shop" }
      )
    ).resolves.toEqual([]);
  });
});
