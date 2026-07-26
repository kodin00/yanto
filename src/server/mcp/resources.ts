import { ResourceTemplate, type McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { ProjectRow } from "../db/schema.js";
import { HttpError } from "../http-utils.js";
import { logger } from "../logger.js";
import { readProjectCompose } from "../services/compose.js";
import { containerLogs } from "../services/docker.js";
import { findDeployment } from "../services/deployments.js";
import { hasMcpAccess } from "../services/mcp-tokens.js";
import { readProjectEnv } from "../services/project-env.js";
import type { YantoMcpContext } from "./context.js";

type ResourceDependencies = {
  settingsSnapshot: () => Promise<unknown>;
  requireProject: (id: string) => Promise<ProjectRow>;
};

export function registerYantoResources(server: McpServer, ctx: YantoMcpContext, dependencies: ResourceDependencies) {
  if (!hasMcpAccess(ctx.accessLevel, "read")) return;

  server.registerResource("yanto_overview", "yanto://overview", { title: "Yanto overview", mimeType: "application/json" }, async (uri) => ({
    contents: [{ uri: uri.href, mimeType: "application/json", text: JSON.stringify(await dependencies.settingsSnapshot(), null, 2) }]
  }));
  server.registerResource("yanto_project_compose", new ResourceTemplate("yanto://projects/{id}/compose", { list: undefined }), { title: "Project compose", mimeType: "text/yaml" }, async (uri, variables) => {
    const project = await dependencies.requireProject(String(variables.id));
    const compose = await readProjectCompose(project);
    return { contents: [{ uri: uri.href, mimeType: "text/yaml", text: compose.content }] };
  });
  server.registerResource("yanto_project_env", new ResourceTemplate("yanto://projects/{id}/env", { list: undefined }), { title: "Project masked environment", mimeType: "text/plain" }, async (uri, variables) => {
    const project = await dependencies.requireProject(String(variables.id));
    const env = await readProjectEnv(project);
    return { contents: [{ uri: uri.href, mimeType: "text/plain", text: env.content }] };
  });
  server.registerResource("yanto_deployment_logs", new ResourceTemplate("yanto://deployments/{id}/logs", { list: undefined }), { title: "Deployment logs", mimeType: "text/plain" }, async (uri, variables) => {
    const deployment = await findDeployment(String(variables.id));
    if (!deployment) throw new HttpError(404, "Deployment not found.");
    return { contents: [{ uri: uri.href, mimeType: "text/plain", text: deployment.logs }] };
  });
  server.registerResource("yanto_container_logs", new ResourceTemplate("yanto://containers/{id}/logs", { list: undefined }), { title: "Container logs", mimeType: "text/plain" }, async (uri, variables) => ({
    contents: [{ uri: uri.href, mimeType: "text/plain", text: await containerLogs(String(variables.id)) }]
  }));
  server.registerResource("yanto_system_logs", "yanto://system/logs", { title: "System logs", mimeType: "text/plain" }, async (uri) => ({
    contents: [{ uri: uri.href, mimeType: "text/plain", text: logger.history() || "No system log entries recorded yet." }]
  }));
}
