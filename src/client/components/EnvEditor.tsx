import { FileText, List, Plus, Trash2 } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import { normalizeEnvRows } from "../app-utils";
import { Button, IconButton, TextAreaField, TextField } from "./ui";
import type { ProjectEnvVariable } from "../lib/api";

export type EnvEditMode = "pairs" | "text";

export type ProjectEnvState = {
  rows: ProjectEnvVariable[];
  baseline: ProjectEnvVariable[];
  draftKey: string;
  draftValue: string;
  content: string;
  mode: EnvEditMode;
  loading: boolean;
  available: boolean;
  opened: boolean;
};

function serializeEnvRows(rows: ProjectEnvVariable[]) {
  const content = normalizeEnvRows(rows)
    .map((row) => `${row.key}=${row.value ?? ""}`)
    .join("\n");
  return content ? `${content}\n` : "";
}

function parseEnvContentRows(content: string) {
  const rows: ProjectEnvVariable[] = [];
  for (const line of content.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = line.indexOf("=");
    if (separator <= 0) continue;
    const value = line.slice(separator + 1);
    rows.push({ key: line.slice(0, separator).trim(), value });
  }
  return normalizeEnvRows(rows);
}

export function EnvEditor({ modal, onChange }: { modal: ProjectEnvState; onChange: Dispatch<SetStateAction<ProjectEnvState>> }) {
  const baselineKeys = new Set(modal.baseline.map((row) => row.key));
  const currentKeys = new Set(modal.rows.map((row) => row.key));
  const changedRows = modal.rows.filter((row) => {
    const original = modal.baseline.find((item) => item.key === row.key);
    return !original || original.value !== row.value;
  });
  const removedRows = modal.baseline.filter((row) => !currentKeys.has(row.key));
  const setRows = (rows: ProjectEnvVariable[], patch: Partial<ProjectEnvState> = {}) => onChange({ ...modal, ...patch, rows, content: serializeEnvRows(rows) });
  const setMode = (mode: EnvEditMode) => {
    onChange((current) => {
      if (mode === current.mode) return current;
      return mode === "text" ? { ...current, mode } : { ...current, mode, rows: parseEnvContentRows(current.content) };
    });
  };

  return (
    <div className={`env-editor ${modal.mode === "text" ? "text-mode" : ""}`}>
      <div className="env-mode-toggle" role="group" aria-label="Environment input mode">
        <button type="button" aria-pressed={modal.mode === "pairs"} className={modal.mode === "pairs" ? "active" : ""} onClick={() => setMode("pairs")}>
          <List size={15} />
          <span>Key/value</span>
        </button>
        <button type="button" aria-pressed={modal.mode === "text"} className={modal.mode === "text" ? "active" : ""} onClick={() => setMode("text")}>
          <FileText size={15} />
          <span>Text</span>
        </button>
      </div>
      {modal.mode === "text" ? (
        <TextAreaField label="Environment text" value={modal.content} onChange={(content) => onChange((current) => ({ ...current, content }))} />
      ) : (
        <>
          <div className="env-rows">
            {modal.rows.map((row, index) => (
              <div className="env-row" key={`${row.key}:${index}`}>
                <TextField label="Key" value={row.key} onChange={(key) => setRows(modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, key } : item)))} />
                <TextField
                  label="Value"
                  value={row.value ?? ""}
                  onChange={(value) => setRows(modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, value } : item)))}
                />
                <IconButton label="Remove variable" variant="danger" onClick={() => setRows(modal.rows.filter((_, rowIndex) => rowIndex !== index))}>
                  <Trash2 size={15} />
                </IconButton>
              </div>
            ))}
          </div>
          <div className="env-add-row">
            <TextField label="New key" value={modal.draftKey} onChange={(draftKey) => onChange({ ...modal, draftKey })} />
            <TextField label="New value" value={modal.draftValue} onChange={(draftValue) => onChange({ ...modal, draftValue })} />
            <Button
              variant="secondary"
              onClick={() => {
                const key = modal.draftKey.trim();
                if (!key) return;
                setRows(normalizeEnvRows([...modal.rows, { key, value: modal.draftValue }]), { draftKey: "", draftValue: "" });
              }}
              icon={<Plus size={15} />}
            >
              Add
            </Button>
          </div>
          <div className="env-diff">
            <div className="section-kicker">Environment diff</div>
            {[...changedRows, ...removedRows].map((row) => {
              const removed = !currentKeys.has(row.key);
              const created = !baselineKeys.has(row.key);
              return (
                <div key={`${row.key}:diff`}>
                  <span>{row.key}</span>
                  <strong>{removed ? "removed" : created ? "added" : "updated"}</strong>
                </div>
              );
            })}
            {!changedRows.length && !removedRows.length ? <p className="muted">No pending environment changes.</p> : null}
          </div>
        </>
      )}
    </div>
  );
}
