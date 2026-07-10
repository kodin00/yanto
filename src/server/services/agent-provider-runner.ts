import Anthropic from "@anthropic-ai/sdk";
import OpenAI from "openai";
import { config } from "../config.js";
import type { AiProviderProtocol } from "./ai-providers.js";
import { agentToolDefinitions, type AgentSandbox } from "./agent-tools.js";

export type ConversationMessage = { role: "user" | "assistant"; content: string };
export type AgentRunnerEvent = (kind: string, payload: Record<string, unknown>) => Promise<void>;

export type AgentProviderRunInput = {
  protocol: AiProviderProtocol;
  baseUrl: string;
  apiKey: string;
  model: string;
  messages: ConversationMessage[];
  sandbox: AgentSandbox;
  signal: AbortSignal;
  event: AgentRunnerEvent;
};

const systemPrompt = `You are Yanto's coding agent. Work autonomously on the user's task inside the isolated /workspace directory.
Inspect the repository before editing, make focused changes, and run relevant tests or checks. Use only the supplied tools. Do not attempt Git operations: Yanto owns branch, commit, push, and cleanup. Never access paths outside the workspace. If a tool fails, diagnose it and continue when safe. End with a concise summary of changes and verification.`;

async function executeTools(calls: Array<{ id: string; name: string; input: unknown }>, sandbox: AgentSandbox, event: AgentRunnerEvent) {
  const results: Array<{ id: string; name: string; output: string; isError: boolean }> = [];
  for (const call of calls) {
    await event("tool_call", { id: call.id, name: call.name, input: call.input });
    try {
      const output = await sandbox.execute(call.name, call.input);
      const bounded = output.slice(0, config.agentCommandOutputMaxBytes);
      await event("tool_result", { id: call.id, name: call.name, output: bounded, isError: false });
      results.push({ id: call.id, name: call.name, output: bounded, isError: false });
    } catch (error) {
      const output = error instanceof Error ? error.message : String(error);
      await event("tool_result", { id: call.id, name: call.name, output, isError: true });
      results.push({ id: call.id, name: call.name, output, isError: true });
    }
  }
  return results;
}

async function runOpenAiResponses(input: AgentProviderRunInput) {
  const client = new OpenAI({ apiKey: input.apiKey, baseURL: input.baseUrl, timeout: config.agentRunTimeoutMs, maxRetries: 2 });
  let responseId: string | undefined;
  let nextInput: unknown = input.messages.map((message) => ({ role: message.role, content: message.content }));
  let finalText = "";
  for (let turn = 0; turn < config.agentMaxTurns; turn += 1) {
    input.signal.throwIfAborted();
    const stream = await client.responses.create({
      model: input.model,
      instructions: systemPrompt,
      input: nextInput as never,
      previous_response_id: responseId,
      tools: agentToolDefinitions.map((tool) => ({ type: "function", name: tool.name, description: tool.description, parameters: tool.input_schema })) as never,
      stream: true
    }, { signal: input.signal });
    const calls: Array<{ id: string; name: string; input: unknown }> = [];
    let turnText = "";
    for await (const rawEvent of stream) {
      const event = rawEvent as unknown as Record<string, unknown>;
      if (event.type === "response.output_text.delta" && typeof event.delta === "string") {
        turnText += event.delta;
        await input.event("assistant_delta", { delta: event.delta });
      }
      if (event.type === "response.output_item.done") {
        const item = event.item as Record<string, unknown> | undefined;
        if (item?.type === "function_call") {
          let parsed: unknown = {};
          try { parsed = JSON.parse(String(item.arguments ?? "{}")); } catch { parsed = {}; }
          calls.push({ id: String(item.call_id ?? item.id), name: String(item.name), input: parsed });
        }
      }
      if (event.type === "response.completed") responseId = String((event.response as Record<string, unknown>)?.id ?? responseId);
    }
    finalText += turnText;
    if (!calls.length) return finalText.trim() || "Task completed.";
    const results = await executeTools(calls, input.sandbox, input.event);
    nextInput = results.map((result) => ({ type: "function_call_output", call_id: result.id, output: result.output }));
  }
  throw new Error(`Agent exceeded the ${config.agentMaxTurns}-turn limit.`);
}

async function runOpenAiChat(input: AgentProviderRunInput) {
  const client = new OpenAI({ apiKey: input.apiKey, baseURL: input.baseUrl, timeout: config.agentRunTimeoutMs, maxRetries: 2 });
  const messages: Array<Record<string, unknown>> = [{ role: "system", content: systemPrompt }, ...input.messages];
  let finalText = "";
  for (let turn = 0; turn < config.agentMaxTurns; turn += 1) {
    input.signal.throwIfAborted();
    const stream = await client.chat.completions.create({
      model: input.model,
      messages: messages as never,
      tools: agentToolDefinitions.map((tool) => ({ type: "function", function: { name: tool.name, description: tool.description, parameters: tool.input_schema } })) as never,
      tool_choice: "auto",
      stream: true
    }, { signal: input.signal });
    let turnText = "";
    const partialCalls = new Map<number, { id: string; name: string; arguments: string }>();
    for await (const chunk of stream) {
      const delta = chunk.choices[0]?.delta;
      if (delta?.content) {
        turnText += delta.content;
        await input.event("assistant_delta", { delta: delta.content });
      }
      for (const toolCall of delta?.tool_calls ?? []) {
        const current = partialCalls.get(toolCall.index) ?? { id: "", name: "", arguments: "" };
        current.id = toolCall.id ?? current.id;
        current.name += toolCall.function?.name ?? "";
        current.arguments += toolCall.function?.arguments ?? "";
        partialCalls.set(toolCall.index, current);
      }
    }
    finalText += turnText;
    const calls = [...partialCalls.values()].map((call) => {
      let parsed: unknown = {};
      try { parsed = JSON.parse(call.arguments || "{}"); } catch { parsed = {}; }
      return { id: call.id, name: call.name, input: parsed };
    });
    if (!calls.length) return finalText.trim() || "Task completed.";
    messages.push({
      role: "assistant",
      content: turnText || null,
      tool_calls: [...partialCalls.values()].map((call) => ({ id: call.id, type: "function", function: { name: call.name, arguments: call.arguments } }))
    });
    const results = await executeTools(calls, input.sandbox, input.event);
    for (const result of results) messages.push({ role: "tool", tool_call_id: result.id, content: result.output });
  }
  throw new Error(`Agent exceeded the ${config.agentMaxTurns}-turn limit.`);
}

async function runAnthropic(input: AgentProviderRunInput) {
  const client = new Anthropic({ apiKey: input.apiKey, baseURL: input.baseUrl, timeout: config.agentRunTimeoutMs, maxRetries: 2 });
  const messages: Array<Record<string, unknown>> = input.messages.map((message) => ({ role: message.role, content: message.content }));
  let finalText = "";
  for (let turn = 0; turn < config.agentMaxTurns; turn += 1) {
    input.signal.throwIfAborted();
    const stream = client.messages.stream({
      model: input.model,
      max_tokens: 8192,
      system: systemPrompt,
      messages: messages as never,
      tools: agentToolDefinitions.map((tool) => ({ name: tool.name, description: tool.description, input_schema: tool.input_schema })) as never
    }, { signal: input.signal });
    stream.on("text", (delta) => { void input.event("assistant_delta", { delta }); });
    const response = await stream.finalMessage();
    const text = response.content.filter((block) => block.type === "text").map((block) => block.text).join("");
    finalText += text;
    const calls = response.content.filter((block) => block.type === "tool_use").map((block) => ({ id: block.id, name: block.name, input: block.input }));
    messages.push({ role: "assistant", content: response.content });
    if (!calls.length) return finalText.trim() || "Task completed.";
    const results = await executeTools(calls, input.sandbox, input.event);
    messages.push({ role: "user", content: results.map((result) => ({ type: "tool_result", tool_use_id: result.id, content: result.output, is_error: result.isError })) });
  }
  throw new Error(`Agent exceeded the ${config.agentMaxTurns}-turn limit.`);
}

export async function runAgentProvider(input: AgentProviderRunInput) {
  if (input.protocol === "openai_responses") return runOpenAiResponses(input);
  if (input.protocol === "openai_chat") return runOpenAiChat(input);
  return runAnthropic(input);
}
