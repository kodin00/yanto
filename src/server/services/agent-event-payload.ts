const persistedToolEventKinds = new Set(["tool_call", "tool_result", "reasoning", "file_change"]);
const retainedScalarKeys = ["id", "name", "type", "command", "status", "exitCode", "isError", "server", "tool", "query", "error", "path"];
const detailKeys = ["output", "input", "text", "changes", "items", "message"];

function byteLength(value: string) {
  return Buffer.byteLength(value, "utf8");
}

function truncateUtf8(value: string, maxBytes: number) {
  if (byteLength(value) <= maxBytes) return value;
  let low = 0;
  let high = value.length;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (byteLength(value.slice(0, middle)) <= maxBytes) low = middle;
    else high = middle - 1;
  }
  return value.slice(0, low);
}

/** Keeps persisted review activity small without reducing the output returned to the model or live stream. */
export function compactPersistedAgentEvent(kind: string, payload: Record<string, unknown>, maxBytes: number) {
  if (!persistedToolEventKinds.has(kind) || byteLength(JSON.stringify(payload)) <= maxBytes) return payload;

  const compact: Record<string, unknown> = { truncated: true };
  for (const key of retainedScalarKeys) {
    const value = payload[key];
    if (typeof value === "string") compact[key] = truncateUtf8(value, 1_024);
    else if (typeof value === "number" || typeof value === "boolean" || value === null) compact[key] = value;
  }

  const detailKey = detailKeys.find((key) => payload[key] !== undefined);
  if (detailKey) {
    const raw = typeof payload[detailKey] === "string" ? payload[detailKey] as string : JSON.stringify(payload[detailKey]);
    const overhead = byteLength(JSON.stringify({ ...compact, [detailKey]: "" }));
    compact[detailKey] = truncateUtf8(raw, Math.max(0, maxBytes - overhead));
  }

  // Extremely long retained metadata can still exceed a deliberately tiny test/config limit.
  while (byteLength(JSON.stringify(compact)) > maxBytes) {
    const stringKey = Object.keys(compact).find((key) => typeof compact[key] === "string" && (compact[key] as string).length > 0);
    if (!stringKey) break;
    compact[stringKey] = truncateUtf8(compact[stringKey] as string, Math.floor(byteLength(compact[stringKey] as string) / 2));
  }
  return compact;
}
