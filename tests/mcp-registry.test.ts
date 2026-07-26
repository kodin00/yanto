import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { afterEach, describe, expect, it } from "vitest";
import type { McpAccessLevel } from "../src/shared/types.js";
import { createYantoMcpServer } from "../src/server/mcp/registry.js";

const connected: Array<{ client: Client; closeServer: () => Promise<void> }> = [];

async function connect(accessLevel: McpAccessLevel) {
  const server = createYantoMcpServer({
    tokenId: `test-${accessLevel}`,
    tokenName: `${accessLevel} test token`,
    accessLevel,
    actor: `mcp:test:${accessLevel}`
  });
  const client = new Client({ name: "yanto-registry-test", version: "1.0.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([client.connect(clientTransport), server.server.connect(serverTransport)]);
  connected.push({ client, closeServer: () => server.close() });
  return client;
}

afterEach(async () => {
  await Promise.all(connected.splice(0).flatMap(({ client, closeServer }) => [client.close(), closeServer()]));
});

describe("MCP registry contracts", () => {
  it("exposes only read tools to read tokens and publishes core resources", async () => {
    const client = await connect("read");
    const [{ tools }, { resources }] = await Promise.all([client.listTools(), client.listResources()]);
    const names = new Set(tools.map((tool) => tool.name));

    expect(names).toContain("yanto_health");
    expect(names).toContain("yanto_projects_list");
    expect(names).not.toContain("yanto_project_create");
    expect(names).not.toContain("yanto_cleanup");
    expect(resources.map((resource) => resource.uri)).toContain("yanto://overview");
  });

  it("adds write tools without exposing admin-only tools", async () => {
    const client = await connect("write");
    const names = new Set((await client.listTools()).tools.map((tool) => tool.name));

    expect(names).toContain("yanto_project_create");
    expect(names).toContain("yanto_project_deploy");
    expect(names).not.toContain("yanto_project_delete");
    expect(names).not.toContain("yanto_cleanup");
  });

  it("exposes the complete administrative surface to admin tokens", async () => {
    const client = await connect("admin");
    const tools = (await client.listTools()).tools;
    const names = new Set(tools.map((tool) => tool.name));

    expect(tools.length).toBeGreaterThanOrEqual(80);
    expect(names).toContain("yanto_project_delete");
    expect(names).toContain("yanto_cleanup");
    expect(names).toContain("yanto_cloudflare_tunnel_delete");
  });
});
