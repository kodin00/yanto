export function normalizeString(value: unknown) {
  return typeof value === "string" ? value.trim() : "";
}
