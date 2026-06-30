import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { ZodError } from "zod";
import { HttpError } from "../http-utils.js";

export function toolResult(text: string, structuredContent: Record<string, unknown> = {}): CallToolResult {
  return {
    content: [{ type: "text", text }],
    structuredContent
  };
}

export function toolError(message: string, structuredContent: Record<string, unknown> = {}): CallToolResult {
  return {
    isError: true,
    content: [{ type: "text", text: message }],
    structuredContent
  };
}

export async function safeTool(handler: () => Promise<CallToolResult> | CallToolResult): Promise<CallToolResult> {
  try {
    return await handler();
  } catch (error) {
    if (error instanceof ZodError) {
      return toolError("Validation failed.", { issues: error.issues });
    }
    if (error instanceof HttpError) {
      return toolError(error.message, { status: error.status });
    }
    return toolError(error instanceof Error ? error.message : "Unexpected MCP tool failure.");
  }
}

export function requireConfirm(confirm: boolean | undefined, action: string) {
  if (confirm !== true) {
    throw new HttpError(400, `${action} requires confirm: true.`);
  }
}

export function limitText(value: string, maxChars = 80_000) {
  if (value.length <= maxChars) {
    return value;
  }
  return `${value.slice(0, maxChars)}\n[... truncated ${value.length - maxChars} chars ...]`;
}

export function asRecord(value: unknown, key = "value"): Record<string, unknown> {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return { [key]: value };
}
