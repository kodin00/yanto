import { Codex, type ThreadItem, type ThreadOptions } from "@openai/codex-sdk";
import { codexTaskConfig } from "./codex-sandbox.js";

type Input = { prompt: string; model: string; threadId?: string | null };
type Output = Record<string, unknown> & { type: string };

function write(output: Output) { process.stdout.write(`${JSON.stringify(output)}\n`); }

function itemOutput(item: ThreadItem): Output | null {
  switch (item.type) {
    case "agent_message": return { type: "assistant", text: item.text };
    case "reasoning": return { type: "reasoning", text: item.text };
    case "command_execution": return { type: "command", command: item.command, output: item.aggregated_output, exitCode: item.exit_code ?? null, status: item.status };
    case "file_change": return { type: "file_change", changes: item.changes, status: item.status };
    case "mcp_tool_call": return { type: "tool", server: item.server, tool: item.tool, status: item.status, error: item.error?.message ?? null };
    case "web_search": return { type: "web_search", query: item.query };
    case "todo_list": return { type: "todo", items: item.items };
    case "error": return { type: "error", message: item.message };
  }
}

async function readInput() {
  let body = "";
  for await (const chunk of process.stdin) body += chunk;
  return JSON.parse(body) as Input;
}

async function main() {
  const input = await readInput();
  const env = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
  delete env.OPENAI_API_KEY;
  delete env.CODEX_API_KEY;
  const codex = new Codex({ env, config: codexTaskConfig });
  const options: ThreadOptions = {
    workingDirectory: "/workspace", skipGitRepoCheck: true,
    approvalPolicy: "never",
    ...(input.model && input.model !== "default" ? { model: input.model } : {})
  };
  const thread = input.threadId ? codex.resumeThread(input.threadId, options) : codex.startThread(options);
  const streamed = await thread.runStreamed(input.prompt);
  const messages: string[] = [];
  let threadId = input.threadId ?? null;
  for await (const event of streamed.events) {
    if (event.type === "thread.started") { threadId = event.thread_id; write({ type: "thread", threadId }); }
    else if (event.type === "item.completed") {
      const output = itemOutput(event.item);
      if (output) write(output);
      if (event.item.type === "agent_message") messages.push(event.item.text);
    } else if (event.type === "turn.failed") throw new Error(event.error.message);
    else if (event.type === "error") throw new Error(event.message);
  }
  write({ type: "result", success: true, threadId, assistantText: messages.join("\n\n") });
}

void main().catch((error) => {
  write({ type: "result", success: false, error: error instanceof Error ? error.message : String(error) });
  process.exitCode = 1;
});
