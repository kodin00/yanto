import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult, ToolAnnotations } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import { hasMcpAccess } from "../services/mcp-tokens.js";
import type { YantoMcpContext } from "./context.js";
import { asRecord, safeTool, toolResult } from "./tool-utils.js";

export type ToolLevel = "read" | "write" | "admin";
type ToolHandler<T extends z.ZodTypeAny> = (input: z.infer<T>) => Promise<CallToolResult> | CallToolResult;

export const readOnly: ToolAnnotations = { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false };
export const externalRead: ToolAnnotations = { ...readOnly, openWorldHint: true };
export const write: ToolAnnotations = { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false };
export const externalWrite: ToolAnnotations = { ...write, openWorldHint: true };
export const destructive: ToolAnnotations = { readOnlyHint: false, destructiveHint: true, idempotentHint: false, openWorldHint: false };
export const externalDestructive: ToolAnnotations = { ...destructive, openWorldHint: true };

export function summarize(name: string, value: unknown, key = "result") {
  const count = Array.isArray(value) ? ` (${value.length})` : "";
  return toolResult(`${name}${count}`, asRecord(value, key));
}

export function registerTool<T extends z.ZodTypeAny>(
  server: McpServer,
  ctx: YantoMcpContext,
  level: ToolLevel,
  name: string,
  description: string,
  inputSchema: T,
  annotations: ToolAnnotations,
  handler: ToolHandler<T>
) {
  if (!hasMcpAccess(ctx.accessLevel, level)) return;
  const register = server.registerTool.bind(server) as any;
  register(
    name,
    { title: name, description, inputSchema, annotations },
    (args: any) => safeTool(() => handler(inputSchema.parse(args)))
  );
}
