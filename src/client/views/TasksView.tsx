import { Archive, ArchiveRestore, Bot, Check, CheckCircle2, ChevronDown, FileCode2, GitBranch, Play, Plus, RefreshCw, Settings2, Square, Trash2, Upload, Wrench, X, XCircle } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { AgentEvent, AgentGitPreview, AgentTask, AgentTaskDetail, AiProvider, AiProviderProtocol, CodexAccountStatus, Project, ProjectBranch } from "../../shared/types";
import { Button, CustomSelect, Modal, StatusBadge, TextAreaField, TextField, ToggleField } from "../components/ui";
import { api } from "../lib/api";

type Props = { projects: Project[]; refreshKey: number; toast: (message: string, kind?: "ok" | "error" | "loading") => void };
type Tab = "board" | "archive" | "providers";

const columns: Array<{ status: AgentTask["status"]; label: string; help: string }> = [
  { status: "backlog", label: "Backlog", help: "Ready to start" },
  { status: "running", label: "Running", help: "Agent is working" },
  { status: "review", label: "Review", help: "Inspect, continue, or push" },
  { status: "done", label: "Done", help: "Branch pushed" }
];

const protocolOptions: Array<{ value: AiProviderProtocol; label: string }> = [
  { value: "openai_responses", label: "OpenAI Responses" },
  { value: "openai_chat", label: "OpenAI-compatible Chat" },
  { value: "anthropic_messages", label: "Anthropic Messages" }
];

function slug(value: string) {
  return value.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 48);
}

function TaskCard({ task, onSelect, onStart, onRestore }: { task: AgentTask; onSelect: () => void; onStart?: () => void; onRestore?: () => void }) {
  return (
    <article className={`agent-task-card ${task.archivedAt ? "archived" : ""}`} onClick={onSelect}>
      <div className="agent-task-card-head"><StatusBadge status={task.latestRun?.status ?? task.status} /><small>{task.providerName}</small></div>
      <h3>{task.title}</h3>
      <p>{task.projectName}</p>
      <div className="branch-line"><GitBranch size={13} /><span>{task.sourceBranch}</span><span>→</span><strong>{task.taskBranch}</strong></div>
      {task.lastError ? <p className="agent-error">{task.lastError}</p> : null}
      <footer>
        <span>{task.archivedAt ? `Archived ${new Date(task.archivedAt).toLocaleDateString()}` : task.modelName}</span>
        {onStart ? <button type="button" onClick={(event) => { event.stopPropagation(); onStart(); }}><Play size={13} /> Start</button> : null}
        {onRestore ? <button type="button" onClick={(event) => { event.stopPropagation(); onRestore(); }}><ArchiveRestore size={13} /> Restore</button> : null}
      </footer>
    </article>
  );
}

export function TasksView({ projects, refreshKey, toast }: Props) {
  const [tab, setTab] = useState<Tab>("board");
  const [tasks, setTasks] = useState<AgentTask[]>([]);
  const [archivedTasks, setArchivedTasks] = useState<AgentTask[]>([]);
  const [providers, setProviders] = useState<AiProvider[]>([]);
  const [loading, setLoading] = useState(true);
  const [createOpen, setCreateOpen] = useState(false);
  const [detailId, setDetailId] = useState<string | null>(null);
  const toastRef = useRef(toast);

  useEffect(() => { toastRef.current = toast; }, [toast]);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [taskRows, archivedRows, providerRows] = await Promise.all([api.agentTasks(), api.agentTasks(true), api.aiProviders()]);
      setTasks(taskRows);
      setArchivedTasks(archivedRows);
      setProviders(providerRows);
    } catch (error) {
      toastRef.current(error instanceof Error ? error.message : "Unable to load AI tasks.", "error");
    } finally { setLoading(false); }
  }, []);

  useEffect(() => { void refresh(); }, [refresh, refreshKey]);
  useEffect(() => {
    const timer = window.setInterval(() => { void refresh(); }, 5 * 60 * 1_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  return (
    <div className="agent-page">
      <div className="agent-tabs">
        <button type="button" className={tab === "board" ? "active" : ""} onClick={() => setTab("board")}><Bot size={15} /> Board</button>
        <button type="button" className={tab === "archive" ? "active" : ""} onClick={() => setTab("archive")}><Archive size={15} /> Archive {archivedTasks.length ? <span className="tab-count">{archivedTasks.length}</span> : null}</button>
        <button type="button" className={tab === "providers" ? "active" : ""} onClick={() => setTab("providers")}><Settings2 size={15} /> Providers</button>
        <div className="agent-tabs-actions">
          {tab === "board" ? <Button onClick={() => setCreateOpen(true)} icon={<Plus size={15} />}>New task</Button> : null}
        </div>
      </div>

      {tab === "board" ? (
        <div className="kanban-board">
          {columns.map((column) => {
            const rows = tasks.filter((task) => task.status === column.status);
            return (
              <section className={`kanban-column ${column.status}`} key={column.status}>
                <header><div><h2>{column.label}</h2><p>{column.help}</p></div><span>{rows.length}</span></header>
                <div className="kanban-stack">
                  {rows.map((task) => <TaskCard task={task} key={task.id} onSelect={() => setDetailId(task.id)} onStart={task.status === "backlog" ? () => { void api.runAgentTask(task.id).then(() => refresh()).catch((error: Error) => toast(error.message, "error")); } : undefined} />)}
                  {!rows.length ? <div className="kanban-empty">{loading ? "Loading…" : "No tasks"}</div> : null}
                </div>
              </section>
            );
          })}
        </div>
      ) : tab === "archive" ? (
        <section className="task-archive">
          <header><div><h2>Archived tasks</h2><p>Done tasks move here automatically after three days.</p></div><span>{archivedTasks.length}</span></header>
          {archivedTasks.length ? <div className="task-archive-grid">{archivedTasks.map((task) => <TaskCard task={task} key={task.id} onSelect={() => setDetailId(task.id)} onRestore={() => { void api.setAgentTaskArchived(task.id, false).then(() => refresh()).catch((error: Error) => toast(error.message, "error")); }} />)}</div> : <div className="kanban-empty">{loading ? "Loading…" : "No archived tasks"}</div>}
        </section>
      ) : <ProviderPanel providers={providers} refresh={refresh} toast={toast} />}

      {createOpen ? <CreateTaskModal projects={projects.filter((project) => Boolean(project.gitUrl))} providers={providers} onClose={() => setCreateOpen(false)} onCreated={async (task) => { setCreateOpen(false); await refresh(); setDetailId(task.id); }} toast={toast} /> : null}
      {detailId ? <TaskDetailModal taskId={detailId} providers={providers} onClose={() => { setDetailId(null); void refresh(); }} refreshBoard={refresh} toast={toast} /> : null}
    </div>
  );
}

function CreateTaskModal({ projects, providers, onClose, onCreated, toast }: { projects: Project[]; providers: AiProvider[]; onClose: () => void; onCreated: (task: AgentTaskDetail) => void; toast: Props["toast"] }) {
  const models = providers.flatMap((provider) => provider.enabled ? provider.models.filter((model) => model.enabled).map((model) => ({ ...model, providerName: provider.name })) : []);
  const [form, setForm] = useState({ projectId: projects[0]?.id ?? "", modelId: models[0]?.id ?? "", title: "", prompt: "", sourceBranch: projects[0]?.branch ?? "master", taskBranch: "", resumeExistingBranch: false, autoCommit: false, autoPush: false, autoCleanup: false });
  const [branches, setBranches] = useState<ProjectBranch[]>([]);
  const [branchesLoading, setBranchesLoading] = useState(Boolean(projects[0]?.id));
  const [branchTouched, setBranchTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const project = projects.find((row) => row.id === form.projectId);
  const toastRef = useRef(toast);

  useEffect(() => { toastRef.current = toast; }, [toast]);

  useEffect(() => {
    if (!form.projectId) return;
    let active = true;
    setBranchesLoading(true);
    void api.projectAgentBranches(form.projectId).then((rows) => {
      if (!active) return;
      setBranches(rows);
      const preferred = rows.find((row) => row.name === project?.branch)?.name ?? rows[0]?.name ?? project?.branch ?? "master";
      setForm((current) => ({
        ...current,
        sourceBranch: rows.some((row) => row.name === current.sourceBranch) ? current.sourceBranch : preferred
      }));
    }).catch((error: Error) => {
      if (active) toastRef.current(error.message, "error");
    }).finally(() => {
      if (active) setBranchesLoading(false);
    });
    return () => { active = false; };
  }, [form.projectId, project?.branch]);

  const writesToSource = form.taskBranch.trim() === form.sourceBranch;
  const remoteTaskBranchExists = !writesToSource && branches.some((branch) => branch.remote && branch.name === form.taskBranch.trim());
  async function create() {
    setBusy(true);
    try {
      const task = await api.createAgentTask(form);
      onCreated(task);
    } catch (error) { toast(error instanceof Error ? error.message : "Unable to create task.", "error"); }
    finally { setBusy(false); }
  }
  return (
    <Modal title="Create AI task" size="wide" onClose={onClose}>
      <div className="agent-create-grid">
        <div className="agent-create-copy">
          <TextField label="Task title" value={form.title} onChange={(title) => setForm({ ...form, title, taskBranch: branchTouched ? form.taskBranch : `task/${slug(title)}` })} placeholder="Add deployment health checks" />
          <TextAreaField label="Instructions" value={form.prompt} onChange={(prompt) => setForm({ ...form, prompt })} placeholder="Describe the outcome, constraints, and checks to run." />
        </div>
        <div className="agent-create-config">
          <CustomSelect label="Project" value={form.projectId} options={projects.map((row) => ({ label: row.name, value: row.id }))} onChange={(projectId) => setForm({ ...form, projectId })} />
          <CustomSelect label="Model" value={form.modelId} options={models.map((model) => ({ label: `${model.providerName} · ${model.displayName}`, value: model.id }))} onChange={(modelId) => setForm({ ...form, modelId })} />
          <CustomSelect label="Source branch" value={form.sourceBranch} options={branches.map((branch) => ({ label: `${branch.name}${branch.remote ? " · origin" : ""}`, value: branch.name }))} onChange={(sourceBranch) => setForm({ ...form, sourceBranch })} disabled={branchesLoading} placeholder={branchesLoading ? "Loading branches…" : "No branches found"} />
          <TextField label="Push branch" value={form.taskBranch} onChange={(taskBranch) => { setBranchTouched(true); setForm({ ...form, taskBranch }); }} placeholder="task/health-checks or main" />
          <p className="branch-target-note">Set this to the source branch to let the task commit and push directly to it.</p>
          {remoteTaskBranchExists ? <ToggleField label="Resume existing branch" description="The branch already exists on origin." value={form.resumeExistingBranch} onChange={(resumeExistingBranch) => setForm({ ...form, resumeExistingBranch })} /> : null}
          <ToggleField label="Auto-commit on success" value={form.autoCommit} onChange={(autoCommit) => setForm({ ...form, autoCommit, autoPush: autoCommit ? form.autoPush : false, autoCleanup: autoCommit ? form.autoCleanup : false })} />
          <ToggleField label="Auto-push after commit" value={form.autoPush} disabled={!form.autoCommit} onChange={(autoPush) => setForm({ ...form, autoPush, autoCleanup: autoPush ? form.autoCleanup : false })} />
          <ToggleField label="Auto-clean after push" value={form.autoCleanup} disabled={!form.autoPush} onChange={(autoCleanup) => setForm({ ...form, autoCleanup })} />
        </div>
      </div>
      {!projects.length ? <p className="agent-error">Register a project with a Git URL before creating an AI task.</p> : null}
      {!models.length ? <p className="agent-error">Add an enabled provider model first.</p> : null}
      <div className="actions"><Button variant="secondary" onClick={onClose}>Cancel</Button><Button loading={busy} disabled={branchesLoading || !form.projectId || !form.modelId || !form.title.trim() || !form.prompt.trim() || !form.sourceBranch || !form.taskBranch.trim() || (remoteTaskBranchExists && !form.resumeExistingBranch)} onClick={() => void create()}>Create task</Button></div>
    </Modal>
  );
}

type ActivityEvent = Pick<AgentEvent, "runId" | "sequence" | "kind" | "payload"> & { createdAt?: string };
type ToolEntry = { key: string; label: string; detail: string; status: "running" | "completed" | "failed"; kind: "tool" | "file" };

function compactDetail(value: unknown) {
  if (value === undefined || value === null || value === "") return "";
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.replace(/\s+/g, " ").trim().slice(0, 240);
}

function toolActivityEntries(events: ActivityEvent[], running: boolean): ToolEntry[] {
  const results = new Map(events.filter((event) => event.kind === "tool_result" && typeof event.payload.id === "string").map((event) => [String(event.payload.id), event]));
  const pairedResults = new Set<string>();
  const entries: ToolEntry[] = [];

  for (const event of events) {
    if (event.kind === "tool_call") {
      const id = typeof event.payload.id === "string" ? event.payload.id : undefined;
      const result = id ? results.get(id) : undefined;
      if (id && result) pairedResults.add(id);
      const payload = result?.payload ?? event.payload;
      const failed = payload.isError === true || Boolean(payload.error) || ["failed", "error"].includes(String(payload.status ?? "").toLowerCase());
      const label = String(event.payload.name ?? event.payload.tool ?? (event.payload.type === "web_search" ? "Web search" : event.payload.type === "todo" ? "Task plan" : "Tool"));
      const detail = compactDetail(event.payload.command ?? event.payload.query ?? event.payload.input ?? event.payload.server ?? event.payload.path ?? event.payload.items);
      entries.push({ key: `${event.runId}:${event.sequence}`, label, detail, status: failed ? "failed" : result || event.payload.status === "completed" ? "completed" : running ? "running" : "completed", kind: "tool" });
    }
    if (event.kind === "tool_result") {
      const id = typeof event.payload.id === "string" ? event.payload.id : undefined;
      if (id && pairedResults.has(id)) continue;
      const exitCode = typeof event.payload.exitCode === "number" ? event.payload.exitCode : null;
      const failed = event.payload.isError === true || Boolean(event.payload.error) || (exitCode !== null && exitCode !== 0) || ["failed", "error"].includes(String(event.payload.status ?? "").toLowerCase());
      const label = String(event.payload.name ?? (event.payload.type === "command" ? "Shell command" : "Tool result"));
      const detail = compactDetail(event.payload.command ?? event.payload.input ?? event.payload.path);
      entries.push({ key: `${event.runId}:${event.sequence}`, label, detail, status: failed ? "failed" : "completed", kind: "tool" });
    }
    if (event.kind === "file_change") {
      entries.push({ key: `${event.runId}:${event.sequence}`, label: "File changes", detail: compactDetail(event.payload.path ?? event.payload.changes), status: "completed", kind: "file" });
    }
  }
  return entries;
}

function ToolActivity({ events, running }: { events: ActivityEvent[]; running: boolean }) {
  const entries = toolActivityEntries(events, running);
  const [open, setOpen] = useState(running);
  useEffect(() => { if (running) setOpen(true); }, [running]);
  if (!entries.length) return null;
  const failed = entries.filter((entry) => entry.status === "failed").length;
  return (
    <details className="task-events" open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>
        <span className="task-events-icon"><Wrench size={14} /></span>
        <span><strong>Tools used</strong><small>{running ? "Activity updates live" : `${entries.length} operation${entries.length === 1 ? "" : "s"}`}</small></span>
        {failed ? <span className="task-events-failed">{failed} failed</span> : <span className="task-events-count">{entries.length}</span>}
        <ChevronDown className="task-events-chevron" size={15} />
      </summary>
      <div className="task-event-list">
        {entries.map((entry) => (
          <div className={`task-event-row ${entry.status}`} key={entry.key}>
            <span className="task-event-state">{entry.status === "failed" ? <XCircle size={14} /> : entry.status === "running" ? <span className="task-event-pulse" /> : <CheckCircle2 size={14} />}</span>
            <span className="task-event-kind">{entry.kind === "file" ? <FileCode2 size={13} /> : <Wrench size={13} />}</span>
            <div><strong>{entry.label}</strong>{entry.detail ? <code title={entry.detail}>{entry.detail}</code> : null}</div>
            <small>{entry.status}</small>
          </div>
        ))}
      </div>
    </details>
  );
}

function TaskDetailModal({ taskId, providers, onClose, refreshBoard, toast }: { taskId: string; providers: AiProvider[]; onClose: () => void; refreshBoard: () => Promise<void>; toast: Props["toast"] }) {
  const [task, setTask] = useState<AgentTaskDetail | null>(null);
  const [liveText, setLiveText] = useState("");
  const [eventLog, setEventLog] = useState<ActivityEvent[]>([]);
  const [followUp, setFollowUp] = useState("");
  const [git, setGit] = useState<AgentGitPreview | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [commitMessage, setCommitMessage] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const eventDedup = useRef<{ runId: string | null; keys: Set<string>; order: string[] }>({ runId: null, keys: new Set(), order: [] });

  const load = useCallback(async () => {
    const detail = await api.agentTask(taskId);
    setTask(detail);
    const persistedEvents = detail.events.map((event) => ({ runId: event.runId, sequence: event.sequence, kind: event.kind, payload: event.payload, createdAt: event.createdAt }));
    setEventLog(persistedEvents);
    eventDedup.current = {
      runId: detail.latestRun?.id ?? null,
      keys: new Set(persistedEvents.map((event) => `${event.runId}:${event.sequence}`)),
      order: persistedEvents.map((event) => `${event.runId}:${event.sequence}`)
    };
    if (detail.worktreePath && detail.status !== "running") {
      const preview = await api.agentTaskGit(taskId).catch(() => null);
      setGit(preview);
      setSelected(preview?.files.map((file) => file.path) ?? []);
    }
  }, [taskId]);
  useEffect(() => { void load().catch((error: Error) => toast(error.message, "error")); }, [load, toast]);
  useEffect(() => {
    if (task?.status !== "running") return;
    type StreamEvent = { runId?: string; sequence?: number; kind?: string; payload?: Record<string, unknown>; createdAt?: string; done?: boolean };
    let source: EventSource | null = null;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;
    let reconciling = false;
    let finished = false;

    const remember = (event: StreamEvent) => {
      if (!event.runId || typeof event.sequence !== "number") return true;
      const key = `${event.runId}:${event.sequence}`;
      if (eventDedup.current.keys.has(key)) return false;
      eventDedup.current.keys.add(key);
      eventDedup.current.order.push(key);
      if (eventDedup.current.order.length > 1_000) {
        const oldest = eventDedup.current.order.shift();
        if (oldest) eventDedup.current.keys.delete(oldest);
      }
      return true;
    };
    const finish = () => {
      if (finished) return;
      finished = true;
      source?.close();
      source = null;
      setLiveText("");
      void load();
      void refreshBoard();
    };
    const applyEvent = (event: StreamEvent) => {
      if (!remember(event)) return;
      if (event.kind === "assistant_delta" && typeof event.payload?.delta === "string") {
        setLiveText((current) => current + event.payload!.delta);
      } else if (event.runId && typeof event.sequence === "number" && event.kind && event.kind !== "snapshot" && event.kind !== "codex_thread") {
        setEventLog((current) => [...current.slice(-199), { runId: event.runId!, sequence: event.sequence!, kind: event.kind!, payload: event.payload ?? {}, createdAt: event.createdAt }]);
      }
      if (event.done) finish();
    };
    const connect = () => {
      if (disposed) return;
      source = new EventSource(api.agentTaskEventStream(taskId));
      source.onmessage = (message) => {
        let data: StreamEvent;
        try { data = JSON.parse(message.data) as StreamEvent; } catch { return; }
        if (data.kind === "snapshot") {
          const runId = typeof data.payload?.runId === "string" ? data.payload.runId : null;
          if (eventDedup.current.runId !== runId) {
            eventDedup.current = { runId, keys: new Set(), order: [] };
            setEventLog([]);
            setLiveText("");
          }
          const events = Array.isArray(data.payload?.events) ? data.payload.events as StreamEvent[] : [];
          events.forEach(applyEvent);
          if (data.done) finish();
          return;
        }
        applyEvent(data);
      };
      source.onerror = () => {
        source?.close();
        source = null;
        if (disposed || finished || reconciling) return;
        reconciling = true;
        void api.agentTask(taskId).then(async (detail) => {
          if (disposed) return;
          if (detail.status === "running") {
            setTask(detail);
            retryTimer = setTimeout(connect, 750);
          } else {
            setLiveText("");
            await load();
            await refreshBoard();
          }
        }).catch(() => {
          if (!disposed) retryTimer = setTimeout(connect, 1_500);
        }).finally(() => { reconciling = false; });
      };
    };
    connect();
    return () => {
      disposed = true;
      if (retryTimer) clearTimeout(retryTimer);
      source?.close();
    };
  }, [task?.status, taskId, load, refreshBoard]);

  async function action(name: string, fn: () => Promise<unknown>) {
    setBusy(name);
    try { await fn(); await load(); await refreshBoard(); }
    catch (error) { toast(error instanceof Error ? error.message : "Action failed.", "error"); }
    finally { setBusy(null); }
  }
  if (!task) return <Modal title="AI task" onClose={onClose}><p>Loading…</p></Modal>;
  const allModels = providers.flatMap((provider) => provider.models.map((model) => ({ ...model, label: `${provider.name} · ${model.displayName}` })));
  return (
    <Modal title={task.title} size="wide" onClose={onClose} closeOnEscape={task.status !== "running"}>
      <div className="task-pipeline"><span>{task.projectName}</span><code>origin/{task.sourceBranch}</code><span>→</span><code>{task.taskBranch}</code><StatusBadge status={task.latestRun?.status ?? task.status} /></div>
      <div className="task-detail-grid">
        <section className="task-conversation">
          <div className="task-messages">
            {task.messages.map((message) => <article className={`task-message ${message.role}`} key={message.id}><strong>{message.role === "user" ? "You" : "Agent"}</strong><pre>{message.content}</pre></article>)}
            {liveText ? <article className="task-message assistant live"><strong>Agent · live</strong><pre>{liveText}</pre></article> : null}
          </div>
          <ToolActivity events={eventLog} running={task.status === "running"} />
          <TextAreaField label={task.status === "backlog" ? "Instructions" : "Follow-up instruction"} value={followUp} onChange={setFollowUp} placeholder="Ask the agent to adjust or continue…" />
          <div className="actions">
            {task.status === "running" ? <Button variant="danger" loading={busy === "stop"} onClick={() => void action("stop", () => api.stopAgentTask(task.id))} icon={<Square size={14} />}>Stop run</Button> : <Button loading={busy === "run"} onClick={() => void action("run", async () => { await api.runAgentTask(task.id, followUp.trim() || undefined); setFollowUp(""); })} icon={<Play size={14} />}>{task.status === "backlog" ? "Start" : "Run follow-up"}</Button>}
          </div>
        </section>
        <aside className="task-review-panel">
          <h3>Run settings</h3>
          <CustomSelect label="Model" value={task.modelId} options={allModels.map((model) => ({ label: model.label, value: model.id }))} disabled={task.status === "running"} onChange={(modelId) => void action("settings", () => api.updateAgentTask(task.id, { modelId }))} />
          <ToggleField label="Auto-commit" value={task.autoCommit} disabled={task.status === "running"} onChange={(autoCommit) => void action("settings", () => api.updateAgentTask(task.id, { autoCommit, ...(autoCommit ? {} : { autoPush: false, autoCleanup: false }) }))} />
          <ToggleField label="Auto-push" value={task.autoPush} disabled={task.status === "running" || !task.autoCommit} onChange={(autoPush) => void action("settings", () => api.updateAgentTask(task.id, { autoPush, ...(autoPush ? {} : { autoCleanup: false }) }))} />
          <ToggleField label="Auto-clean" value={task.autoCleanup} disabled={task.status === "running" || !task.autoPush} onChange={(autoCleanup) => void action("settings", () => api.updateAgentTask(task.id, { autoCleanup }))} />
          <div className="task-review-head"><h3>Changes</h3>{task.worktreePath && task.status !== "running" ? <button type="button" onClick={() => void load()}><RefreshCw size={14} /></button> : null}</div>
          {!task.worktreePath ? <p className="muted">The worktree is created when the first run starts.</p> : task.status === "running" ? <p className="muted">Git controls unlock when the run finishes.</p> : git ? (
            <>
              <div className="git-summary"><span>{git.files.length} files</span><span>ahead {git.ahead}</span><span>behind {git.behind}</span></div>
              <div className="git-file-list">{git.files.map((file) => <label key={file.path}><input type="checkbox" checked={selected.includes(file.path)} onChange={() => setSelected((current) => current.includes(file.path) ? current.filter((path) => path !== file.path) : [...current, file.path])} /><code>{file.status}</code><span>{file.path}</span></label>)}</div>
              {git.diff ? <details className="task-diff"><summary>Diff preview</summary><pre>{git.diff}</pre></details> : null}
              <TextField label="Commit message" value={commitMessage} onChange={setCommitMessage} placeholder={`task: ${task.title}`} />
              <div className="actions stacked"><Button loading={busy === "commit"} disabled={git.isClean || !selected.length} onClick={() => void action("commit", () => api.commitAgentTask(task.id, commitMessage.trim() || `task: ${task.title}`, selected))} icon={<Check size={14} />}>Commit selected</Button><Button loading={busy === "push"} variant="secondary" onClick={() => void action("push", () => api.pushAgentTask(task.id))} icon={<Upload size={14} />}>Push branch</Button></div>
              <Button variant="ghost" loading={busy === "cleanup"} onClick={() => void action("cleanup", () => api.cleanupAgentTask(task.id))}>Clean worktree</Button>
            </>
          ) : <p className="muted">Loading Git state…</p>}
          {task.archivedAt ? <Button variant="secondary" loading={busy === "restore"} onClick={() => void action("restore", () => api.setAgentTaskArchived(task.id, false))} icon={<ArchiveRestore size={14} />}>Restore to board</Button> : null}
          {task.status !== "running" ? <Button variant="danger" onClick={() => void action("delete", async () => { await api.deleteAgentTask(task.id); onClose(); })} icon={<Trash2 size={14} />}>Delete task</Button> : null}
        </aside>
      </div>
    </Modal>
  );
}

function ProviderPanel({ providers, refresh, toast }: { providers: AiProvider[]; refresh: () => Promise<void>; toast: Props["toast"] }) {
  const [form, setForm] = useState<{ name: string; protocol: AiProviderProtocol; baseUrl: string; apiKey: string }>({ name: "", protocol: "openai_responses", baseUrl: "https://api.openai.com/v1", apiKey: "" });
  const [modelDrafts, setModelDrafts] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [codex, setCodex] = useState<CodexAccountStatus | null>(null);
  const codexProvider = providers.find((provider) => provider.protocol === "codex_account");

  useEffect(() => {
    let active = true;
    void api.codexStatus().then(async (status) => { if (active) setCodex(status); if (status.connected) await refresh(); }).catch(() => undefined);
    return () => { active = false; };
  }, [refresh]);

  useEffect(() => {
    if (!codex?.login || codex.connected) return;
    const timer = window.setInterval(() => {
      void api.codexStatus().then(async (status) => {
        setCodex(status);
        if (status.connected) {
          window.clearInterval(timer);
          await api.refreshCodexModels();
          await refresh();
          toast("Codex account connected.", "ok");
        }
      }).catch(() => undefined);
    }, 2000);
    return () => window.clearInterval(timer);
  }, [codex?.connected, codex?.login, refresh, toast]);
  async function action(name: string, fn: () => Promise<unknown>) {
    setBusy(name);
    try { await fn(); await refresh(); }
    catch (error) { toast(error instanceof Error ? error.message : "Provider action failed.", "error"); }
    finally { setBusy(null); }
  }
  return (
    <div className="provider-layout">
      <section className="panel provider-create">
        <div className="codex-connect">
          <div><h2>Codex account</h2><p className="muted">Use your ChatGPT/Codex subscription. No API key needed.</p></div>
          {codex?.connected ? (
            <>
              <StatusBadge status="success" label="Connected" />
              <p className="muted">{codex.email || "Codex account"}{codex.planType ? ` · ${codex.planType}` : ""}</p>
              <div className="actions"><Button loading={busy === "codex-models"} onClick={() => void action("codex-models", () => api.refreshCodexModels())} icon={<RefreshCw size={14} />}>Refresh models</Button><Button variant="ghost" loading={busy === "codex-logout"} onClick={() => void action("codex-logout", async () => { await api.logoutCodex(); setCodex(await api.codexStatus()); })}>Sign out</Button></div>
            </>
          ) : codex?.login ? (
            <div className="codex-device-code">
              <p>Open the verification page and enter:</p><code>{codex.login.userCode}</code>
              <a className="button" href={codex.login.verificationUrl} target="_blank" rel="noreferrer">Open Codex sign-in</a>
              <Button variant="ghost" onClick={() => void action("codex-cancel", async () => { await api.cancelCodexLogin(); setCodex(await api.codexStatus()); })}>Cancel</Button>
            </div>
          ) : <Button loading={busy === "codex-login"} onClick={() => void action("codex-login", async () => { const login = await api.startCodexLogin(); setCodex({ connected: false, email: null, planType: null, login }); })}>Sign in with Codex</Button>}
          {codexProvider?.models.length ? <p className="muted">{codexProvider.models.length} models available</p> : null}
        </div>
        <hr />
        <h2>Register provider</h2><p className="muted">Keys are encrypted at rest and never returned to the browser.</p>
        <TextField label="Name" value={form.name} onChange={(name) => setForm({ ...form, name })} placeholder="OpenAI production" />
        <CustomSelect<AiProviderProtocol> label="Protocol" value={form.protocol} options={protocolOptions} onChange={(protocol) => setForm({ ...form, protocol, baseUrl: protocol === "anthropic_messages" ? "https://api.anthropic.com" : "https://api.openai.com/v1" })} />
        <TextField label="Base URL" value={form.baseUrl} onChange={(baseUrl) => setForm({ ...form, baseUrl })} />
        <TextField label="API key" type="password" value={form.apiKey} onChange={(apiKey) => setForm({ ...form, apiKey })} autoComplete="off" />
        <Button loading={busy === "create"} disabled={!form.name.trim() || !form.apiKey.trim()} onClick={() => void action("create", async () => { await api.createAiProvider(form); setForm({ ...form, name: "", apiKey: "" }); })}>Register provider</Button>
      </section>
      <div className="provider-list">
        {providers.filter((provider) => provider.protocol !== "codex_account").map((provider) => (
          <section className="panel provider-card" key={provider.id}>
            <header><div><h2>{provider.name}</h2><p>{protocolOptions.find((option) => option.value === provider.protocol)?.label} · {provider.baseUrl}</p></div><StatusBadge status={provider.enabled ? "success" : "disabled"} label={provider.enabled ? "Enabled" : "Disabled"} /></header>
            <div className="actions"><Button variant="secondary" loading={busy === `discover:${provider.id}`} onClick={() => void action(`discover:${provider.id}`, () => api.discoverAiModels(provider.id))} icon={<RefreshCw size={14} />}>Fetch models</Button><Button variant="ghost" onClick={() => void action(`toggle:${provider.id}`, () => api.updateAiProvider(provider.id, { enabled: !provider.enabled }))}>{provider.enabled ? "Disable" : "Enable"}</Button><Button variant="danger" onClick={() => void action(`delete:${provider.id}`, () => api.deleteAiProvider(provider.id))} icon={<Trash2 size={14} />}>Delete</Button></div>
            <div className="provider-models">{provider.models.map((model) => <div key={model.id}><span><strong>{model.displayName}</strong><code>{model.modelId}</code></span><button type="button" onClick={() => void action(`model:${model.id}`, () => api.deleteAiModel(model.id))}><X size={14} /></button></div>)}</div>
            <div className="provider-model-add"><input value={modelDrafts[provider.id] ?? ""} onChange={(event) => setModelDrafts({ ...modelDrafts, [provider.id]: event.target.value })} placeholder="Manual model ID" /><Button variant="secondary" disabled={!modelDrafts[provider.id]?.trim()} onClick={() => void action(`add:${provider.id}`, async () => { await api.addAiModel(provider.id, { modelId: modelDrafts[provider.id] }); setModelDrafts({ ...modelDrafts, [provider.id]: "" }); })} icon={<Plus size={14} />}>Add</Button></div>
          </section>
        ))}
        {!providers.some((provider) => provider.protocol !== "codex_account") ? <section className="panel"><p className="muted">No API-key providers registered.</p></section> : null}
      </div>
    </div>
  );
}
