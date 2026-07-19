import { CornerDownLeft } from "lucide-react";
import type { KeyboardEvent } from "react";
import { useRef, useState } from "react";
import type { ContainerInfo } from "../../shared/types";
import { api } from "../lib/api";
import { Button, LogViewer, Modal, StatusBadge } from "./ui";

export function ContainerTerminal({ container, onClose }: { container: ContainerInfo; onClose: () => void }) {
  const [output, setOutput] = useState(
    `Connected to ${container.name}.\nEach command runs in a fresh shell via \`docker exec\` — there is no persistent state between commands.\n`
  );
  const [command, setCommand] = useState("");
  const [running, setRunning] = useState(false);
  const [history, setHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const inputRef = useRef<HTMLInputElement | null>(null);

  const runCommand = async () => {
    const trimmed = command.trim();
    if (!trimmed || running) return;
    setOutput((current) => `${current}\n$ ${trimmed}\n`);
    setHistory((current) => (current.at(-1) === trimmed ? current : [...current, trimmed]));
    setHistoryIndex(-1);
    setCommand("");
    setRunning(true);
    try {
      const result = await api.execContainer(container.id, trimmed);
      const notices = [
        result.exitCode !== 0 ? `[exited with code ${result.exitCode}]` : "",
        result.truncated ? "[output truncated]" : "",
        result.timedOut ? "[command timed out]" : ""
      ]
        .filter(Boolean)
        .join(" ");
      const trailingNewline = !result.output || result.output.endsWith("\n") ? "" : "\n";
      setOutput((current) => `${current}${result.output}${trailingNewline}${notices ? `${notices}\n` : ""}`);
    } catch (error) {
      setOutput((current) => `${current}${error instanceof Error ? error.message : "Command failed."}\n`);
    } finally {
      setRunning(false);
      window.requestAnimationFrame(() => inputRef.current?.focus());
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void runCommand();
      return;
    }
    if (event.key === "ArrowUp") {
      if (!history.length) return;
      event.preventDefault();
      const nextIndex = historyIndex < 0 ? history.length - 1 : Math.max(0, historyIndex - 1);
      setHistoryIndex(nextIndex);
      setCommand(history[nextIndex]);
      return;
    }
    if (event.key === "ArrowDown") {
      if (historyIndex < 0) return;
      event.preventDefault();
      const nextIndex = historyIndex + 1;
      if (nextIndex >= history.length) {
        setHistoryIndex(-1);
        setCommand("");
      } else {
        setHistoryIndex(nextIndex);
        setCommand(history[nextIndex]);
      }
    }
  };

  return (
    <Modal title={`Terminal — ${container.name}`} size="wide" onClose={onClose}>
      <div className="log-status-line">
        <StatusBadge status={running ? "live" : "ready"} />
        <span>{running ? "Running command..." : "Ready"}</span>
      </div>
      <LogViewer logs={output} />
      <div className="terminal-input-row">
        <span className="terminal-prompt" aria-hidden="true">$</span>
        <input
          ref={inputRef}
          className="terminal-input"
          value={command}
          onChange={(event) => setCommand(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Type a command, e.g. ls -la"
          aria-label={`Command to run in ${container.name}`}
          spellCheck={false}
          autoComplete="off"
          disabled={running}
        />
        <Button onClick={() => void runCommand()} loading={running} disabled={!command.trim()} icon={<CornerDownLeft size={15} />}>
          Run
        </Button>
      </div>
    </Modal>
  );
}
