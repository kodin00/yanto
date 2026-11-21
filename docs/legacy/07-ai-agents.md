# AI Agents

Yanto is a self-hosted "coding agent" server. An **agent** is not a long-lived daemon; it is a *task run* — a bounded execution of an AI model against a dedicated Git worktree, with a fixed set of filesystem/shell tools, producing a stream of events that are persisted and pushed live to clients. The agent works autonomously to modify code in the worktree; Yanto (not the agent) owns all Git operations (commit/push/cleanup) and Docker is explicitly disabled.

Two execution backends exist:
- **Provider runner** (Anthropic, OpenAI Chat, OpenAI Responses): Yanto runs the model-call/tool-call loop itself, with a fixed in-process tool registry scoped to the worktree.
- **Codex account runner**: Yanto delegates the entire agentic loop to the OpenAI Codex SDK, streaming its thread events into Yanto's event bus.

---

## 1. Agent Lifecycle

There is no persistent "agent" entity. The lifecycle is really the lifecycle of a **task** and its **runs**.

### Concurrency / ownership model

- A single in-process map `activeRuns: Map<taskId, ActiveAgentRun>` tracks the one live run per task. A DB partial unique index `agent_runs_one_running_per_task_idx WHERE status='running'` enforces "at most one `running` run per task" at the database level too — so ownership is double-gated (in-memory + Postgres).
- `config.agentMaxConcurrentRuns` caps total concurrent runs across the server (HTTP 429 when full).
- `withAgentLifecycleLock(keys, op)` serializes destructive operations against run admission. Keys are `project:<id>` and `task:<id>`. The lock is a per-key promise-chain: operations acquire all needed keys (sorted, deduped) before proceeding, then release in reverse order. This prevents, e.g., deleting a task's worktree while a run is being admitted for it.

### Task states

`backlog` → `running` → (`review` | `done`).

- **Create** (`createAgentTask`): inserts task with `status: "backlog"`, plus an initial `user` message containing the prompt. Validates: project exists, project has a `gitUrl`, model resolves, title/prompt non-empty, and the consistency rules `autoPush ⇒ autoCommit` and `autoCleanup ⇒ autoPush`. A unique constraint on `(projectId, taskBranch)` prevents two tasks from claiming the same branch.
- **Start** (`startAgentTask(id, followUp?)`): under the lifecycle lock, re-checks status, checks `activeRuns` capacity, creates an `AbortController` + `ActiveAgentRun` handle, then in a single DB transaction: atomically flips `status` to `running` only if it wasn't already running (conditional `UPDATE ... WHERE status != 'running'`; if 0 rows → 409), inserts an `agent_runs` row (`status: "running"`, records `providerProtocol` + `modelName`), and optionally inserts the `followUp` text as a new `user` message. The actual work runs detached; the function returns the run row immediately.
- **Stop** (`stopAgentTask`): aborts the `AbortController` with "Stopped by user." and awaits the runner's registered stop callback. Cancellation is treated as terminal — tool calls check `signal.throwIfAborted()` and a canceled run is never allowed to continue modifying files.
- **Terminal reconciliation** (`reconcileTerminalRun`): writes the final run/task state in a single transaction (run status, assistant text, error, timestamps; task status, lastError, pushedAt, optionally nulls `worktreePath` if cleaned; inserts the assistant summary as an `agent_messages` row). This is retried with exponential backoff **indefinitely** — a transient DB outage must not release ownership and let a second owner claim the run, so the run stays `running` until the write succeeds.
- **Archive** (`archiveCompletedAgentTasks`): a background job auto-archives `done` tasks whose `updatedAt` is older than 3 days. Manual archive only allowed for `done` tasks.
- **Delete** (`deleteAgentTask`): under lock, refuses if `running`, cleans up the worktree (with `force` option), then deletes the row.
- **Recovery on startup** (`recoverInterruptedAgentRuns`): finds `agent_runs` left `running` (from a crash/upgrade), removes any legacy Docker containers labeled `com.yanto.agent=true` / `com.yanto.agent.run-id=<id>`, then marks those runs `failed` and tasks `done` with "Yanto restarted while the agent run was active."
- **Shutdown** (`drainActiveAgentRuns`): on graceful shutdown, stops all active runs and awaits their completion, racing a `config.agentShutdownTimeoutMs` timeout.

### Where agents run

Agents run **host-native** (no containers). Each task gets:
- A **Git worktree** at `config.agentWorktreesRoot/<projectFolderName>/<taskId>`. The worktree is created on run start and removed on cleanup.
- A **per-task HOME directory** at `os.tmpdir()/yanto-agent-homes/<sha256(worktreePath)[:32]>` (mode 0700), created on start and removed on stop. This isolates agent-run shell state (no shared `~`).
- The tool environment sets `DOCKER_HOST=unix:///dev/null` (Docker disabled), `CI=true`, `YANTO_AGENT_WORKSPACE=<worktree>`, and only forwards a whitelist of env vars (`PATH`, `LANG`, `LC_ALL`, `TERM`, `TMPDIR`, `SSL_CERT_*`). The system prompt forbids Docker, branch switching, commit, push, and accessing paths outside the workspace.

---

## 2. Agent Tasks

The **task** is the unit of work (`agent_tasks` table). Fields of note: `projectId`, `modelId`, `title`, `prompt`, `status`, `sourceBranch`, `taskBranch`, `sourceSha`, `worktreePath`, `codexThreadId`, `resumeExistingBranch`, `autoCommit`, `autoPush`, `autoCleanup`, `lastError`, and timestamps.

- A task binds a **project** (must be Git-backed) to a **model** (must resolve to an enabled provider+model) and a **branching strategy**: a `sourceBranch` to start from and a `taskBranch` to work on. `(projectId, taskBranch)` is unique.
- `resumeExistingBranch` allows reusing a `taskBranch` that already exists on origin (with safety checks that local is an ancestor of remote).
- **Messages** (`agent_messages`): a conversation log per task, roles `user`/`assistant`. The initial prompt is stored as the first `user` message. Follow-up prompts on re-start add more `user` messages. The assistant's final summary is stored as an `assistant` message tied to the run. On a provider run, only the last 50 messages are fed to the model.
- **Runs** (`agent_runs`): each start creates a run row capturing `providerProtocol`, `modelName`, `status`, `assistantText`, `error`, `startedAt`/`finishedAt`. Multiple runs per task are kept historically; the "latest run" is the most recent by `startedAt`.
- **Task results / reporting:** the task ends in `review` (default after a successful provider run, awaiting human Git review) or `done` (when auto-push succeeds and the worktree is clean — or on failure/cancellation). `lastError` carries failure reasons. The assistant's textual summary is stored both on the run (`assistantText`) and as a message. `gitPreviewForTask` exposes a structured diff/preview; `commitAgentTask`/`pushAgentTask`/`cleanupAgentTask` let a human drive Git manually post-run (all refused while `running`).
- **Listing/views:** `listAgentTasks` (active vs archived), `listAgentWorktrees` (maps internal worktree paths to host-visible paths and reports `exists`/`removable`), `getAgentTask` (full detail: task + messages + runs + events of the latest run).

---

## 3. Agent Tools

The **tool registry** is a static array of six tools, each with a JSON-Schema `input_schema`. These are the only tools a provider-run agent can invoke; Codex runs use the Codex SDK's own tool surface instead.

| Tool | Behavior |
|---|---|
| `list_files` | Recursive directory listing up to `depth` (1–6, default 3), capped at 500 entries; skips `.git` and `node_modules`; marks directories with trailing `/`. |
| `search_files` | Substring line search across workspace files (skips `.git`/`node_modules`, skips files >1 MiB), up to 200 matches, each `path:line:truncated-line(500)`. |
| `read_file` | Read UTF-8 file with optional 1-based `start_line`/`end_line` (default 400-line window); output is `lineno: content`. |
| `write_file` | Create/replace a UTF-8 file (creates parent dirs). |
| `replace_text` | Replace exactly one occurrence of `old_text`; fails if absent or if it occurs more than once (forces unambiguous context). |
| `run_command` | Non-interactive `sh -lc <command>` with cwd = worktree, the sandboxed env, a process-group kill, a timeout (clamped to `config.agentCommandTimeoutMs`, 1s–600s), and output capped at `config.agentCommandOutputMaxBytes`. Returns `exit_code=... [timed_out=...] [truncated=...]` plus output. |

### Path safety (`safePath`)

All filesystem tools resolve paths against the worktree root and reject anything escaping it (including via symlinks — it `realpath`s and checks containment). Direct access to any path component named `.git` is forbidden. For writes to nonexistent paths, it walks up to find an existing ancestor and `realpath`s that for the symlink check.

### Dispatch

`AgentWorkspace.execute(name, input)` is the single dispatcher. It is constructed with the worktree path and an `AbortSignal`. `start()` must be called before `run_command` (it verifies the worktree is a directory and creates the HOME dir). `stop()` awaits any in-flight command, then recursively removes the HOME dir. The active command is tracked so `stop()` can drain it. Every tool checks `signal.throwIfAborted()` so cancellation is prompt.

The provider runner calls `workspace.execute(call.name, call.input)` and bounds the output to `config.agentCommandOutputMaxBytes` before returning it to the model and emitting it as a `tool_result` event.

---

## 4. Agent Events

The event system has two channels: **persistence** (DB) and **live streaming** (in-process bus).

### Event shape

`AgentLiveEvent = { taskId, runId, sequence, kind, payload, done? }`. `sequence` is a per-run monotonic int.

### Flow (`recordEvent`)

1. Increment the per-run sequence.
2. **Persist** (best-effort): for all kinds *except* `assistant_delta`, insert into `agent_events` (runId+sequence unique). The payload is first compacted via `compactPersistedAgentEvent` to keep review history small — for the "verbose" kinds (`tool_call`, `tool_update`, `tool_result`, `reasoning`, `file_change`), if the JSON exceeds `config.agentPersistedToolPayloadMaxBytes`, it keeps a small set of scalar keys plus one "detail" key truncated to fit the byte budget. `assistant_delta` is never persisted (streaming-only) to avoid huge DB rows.
3. **Publish live** regardless of persistence outcome: emit on an EventEmitter channel `task:<taskId>`. A live subscriber (the SSE endpoint) attaches and gets every event in order, including the final `done: true` event.

So: persistence failure does not drop a live event, and a live subscriber always sees the event even if the DB write fails.

### Event kinds emitted

- `run_started` `{ model, protocol }`
- `worktree_ready` `{ path, sourceSha, branch }`
- `workspace_started` `{ runtime: "codex"|"provider", mode: "host" }`
- `assistant_delta` `{ delta }` (streaming text deltas; not persisted)
- `tool_call` `{ id, name, input, phase }`
- `tool_update` `{ ...payload, phase }` (Codex only; throttled to one per 250 ms per item, 16 KiB tail)
- `tool_result` `{ id, name, output, isError, phase }`
- `reasoning` `{ id, phase, text }` (Codex reasoning items)
- `file_change` `{ id, phase, changes, status }` (Codex file-change items)
- `codex_thread` `{ threadId }` (Codex thread id; persisted onto `agent_tasks.codexThreadId`)
- `git_committed` `{ sha, message }`, `git_pushed` `{ sha, branch }`, `worktree_cleaned` `{}`, `git_automation_failed` `{ error }`
- `run_finished` `{ status, error, assistantText }` (the events API strips `assistantText` from `run_finished` payloads when serving the events API, to keep responses small)
- `task_finished` `{ status, error? }` — emitted with `done: true` as the terminal live event.

### Retrieval

`agentTaskEvents(taskId, runId?, limit)` returns persisted events for a run (default latest run), newest-first fetch then reversed to chronological, capped 1–200.

---

## 5. AI Providers

Providers and models are user-managed DB rows (`ai_providers`, `ai_models`).

### Protocols (enum `AiProviderProtocol`)

`openai_responses` | `openai_chat` | `anthropic_messages` | `codex_account`. The first three are "bring-your-own-key"; `codex_account` is special (managed via Codex sign-in, not API key).

### API key storage

- For non-Codex providers, the API key is **encrypted at rest** (AES-256-GCM envelope `enc:v1:...`) and stored in `ai_providers.api_key`. The public API exposes only `hasApiKey: boolean`, never the key.
- `resolveProviderModel(modelId)` joins `ai_models`→`ai_providers`, requires both enabled, and returns the decrypted key (empty string for `codex_account`). This is the single entry point used before every run.
- Codex provider (`CODEX_PROVIDER_ID = "aip_codex_account"`) has `apiKey: ""`, `baseUrl: ""`, and cannot be created/edited/deleted through the normal provider CRUD — it's managed by `syncCodexProvider`/`setCodexProviderEnabled` and the Codex account controls. Its core fields are immutable via `updateAiProvider`.

### Base URL handling

`normalizeBaseUrl` validates HTTP/HTTPS, strips trailing slash, defaults to `https://api.anthropic.com` (Anthropic) or `https://api.openai.com/v1` (OpenAI). Codex forces `""`.

### Model discovery

`discoverAiModels(providerId)` lists models from the upstream API (Anthropic `models.list` or OpenAI `models.list`) and upserts them into `ai_models` (unique on `(providerId, modelId)`). Refused for `codex_account` (use the Codex card instead). `listCodexModels` paginates the Codex app-server's `model/list` RPC.

### Model selection at run time

A task stores a specific `modelId` (an `ai_models.id`). `startAgentTask` resolves it once before the lock, then re-resolves inside the lock if it changed (model swap between read and admission). The resolved `{ provider, model, apiKey }` is passed into the runner. The actual upstream model string used is `ai_models.modelId` (e.g. `claude-...` or `gpt-...`), not the internal row id.

---

## 6. Codex Account Runner & Auth

### Auth flow (`codex-auth`)

- A long-lived **Codex app-server** child process is spawned lazily: `node node_modules/@openai/codex/bin/codex.js app-server --stdio`, with `CODEX_HOME=config.codexHome` and `OPENAI_API_KEY`/`CODEX_API_KEY` removed from env (Codex uses ChatGPT account auth, not API keys). Communication is newline-delimited JSON-RPC over stdio. It's `initialize`d with client info `{name:"yanto", title:"Yanto", version:"..."}`.
- **Login** (`startCodexLogin`): cancels any pending login, calls `account/login/start` with `type: "chatgptDeviceCode"`, returns `{loginId, verificationUrl, userCode}` (OAuth device-code flow). The user completes it in a browser. The app-server pushes `account/login/completed` and `account/updated` notifications, which clear the in-memory `login` and invalidate the account-status cache. `cancelCodexLogin` calls `account/login/cancel`.
- **Account status** (`getCodexAccountStatus`): cached for 30s normally, 1s while a login is pending. A single-flight `accountReadRequest` dedups concurrent `account/read` RPCs. Returns `{connected, email, planType, login}`. `invalidateCodexAccountStatusCache` bumps a revision so stale in-flight reads are re-read.
- **Logout** (`logoutCodexAccount`): `account/logout`, then `clearCodexTaskAuthentication()` (removes `auth.json` from every per-task home so no task can keep using stale creds), clears `login`.
- **Models**: `listCodexModels` paginates `model/list` (limit 100, non-hidden), returning `{id, name}`.

### Per-task execution (`codex-account-runner`)

`runCodexAccount(input)`:
1. **Prepare an isolated Codex home** (`prepareCodexTaskHome`): creates `config.codexHome/task-sessions/<sha256(taskId)[:32]>` (mode 0700). Copies the account `auth.json` into the task home *if the account auth is newer* than any task-local refresh. If the task has a `threadId`, it copies that thread's session files from the legacy `sessions`/`archived_sessions` dirs into the task home (so a resumed Codex thread has its history). Throws if account creds are missing.
2. Builds a combined abort signal (`AbortSignal.any` of the run signal + a local controller) and registers a `stop` callback that aborts the local controller.
3. Constructs a Codex client where the env sets `CODEX_HOME=taskHome`, `HOME=taskHome`, `DOCKER_HOST=unix:///dev/null`, plus proxy/SSL/PATH passthrough.
4. Thread options: `workingDirectory = worktreePath`, `sandboxMode: "danger-full-access"`, `approvalPolicy: "never"`, optional `model`. **Resumes** an existing thread if the task has a `codexThreadId`, else **starts** a new one.
5. Sends a wrapped prompt: a Yanto preamble ("work only in the current Git worktree; do not run Docker, switch branches, commit, push, or access unrelated host paths; Yanto owns those operations") + the user's last user-message (or the original prompt).
6. `thread.runStreamed(prompt, { signal })` and iterates `ThreadEvent`s:
   - `thread.started` → emit `codex_thread { threadId }` (the orchestrator persists this onto `agent_tasks.codexThreadId` so future runs resume).
   - `item.started`/`item.updated`/`item.completed` → translate Codex `ThreadItem`s into Yanto events:
     - `agent_message` → `assistant_delta` (only on completed; the full text as one delta).
     - `reasoning` → `reasoning { id, phase, text }`.
     - `file_change` → `file_change { id, phase, changes, status }`.
     - `error` → `tool_result { id, name:"Codex", status:"failed", error, isError }`.
     - `command_execution` → `tool_call`/`tool_update`/`tool_result` with `{id, type:"command", name:"Shell command", command, output (bounded-tail), outputTruncated, exitCode, status}`. `item.updated` is throttled to one `tool_update` per 250ms per item, with a 16 KiB tail.
     - `mcp_tool_call` → tool events with `{id, type:"mcp_tool_call", name:tool, server, input:arguments, status, error}`.
     - `web_search` → tool events with `{id, type:"web_search", query}`.
     - `todo_list` → tool events with `{id, type:"todo", items}`.
   - `turn.failed` / `error` → throw with the message.
7. Returns `{ assistantText: <all agent_message texts joined by "\n\n">, threadId }`.

Cancellation maps to a "Codex run canceled." error. The orchestrator then treats this like any provider failure (run `canceled`, task `done`).

---

## 7. Provider Runner — the orchestration loop

`runAgentProvider(input)` dispatches on `input.protocol`:

- `openai_responses` → `runOpenAiResponses`
- `openai_chat` → `runOpenAiChat`
- otherwise (Anthropic) → `runAnthropic`

All three share the same shape: a fixed **system prompt** (Yanto's coding agent; work in the worktree; inspect before editing; focused changes; run tests; use only supplied tools; no Docker/branch-switch/commit/push; stay in workspace; end with a summary), streaming responses, a turn loop bounded by `config.agentMaxTurns`, and delegation of tool execution to `executeAgentTools`.

### Common loop

For up to `agentMaxTurns` turns:
1. `signal.throwIfAborted()`.
2. Start a streaming model request with the tool definitions attached (mapped to each provider's tool schema: OpenAI Responses uses `{type:"function", name, description, parameters}`; OpenAI Chat uses `{type:"function", function:{...}}`; Anthropic uses `{name, description, input_schema}`). Anthropic uses `max_tokens: 8192`.
3. Consume the stream:
   - Text deltas → append to turn text and emit `assistant_delta { delta }`.
   - Tool calls → collect (OpenAI Responses: from `response.output_item.done` `function_call` items, parsing `arguments` JSON; OpenAI Chat: accumulate `tool_calls` deltas by index, parse `arguments` at end; Anthropic: `tool_use` content blocks with `id`/`name`/`input` already parsed).
4. If **no tool calls** this turn → return `finalText.trim() || "Task completed."` (the agent is "done").
5. Otherwise execute the tool calls via `executeAgentTools`, then feed results back into the next turn:
   - OpenAI Responses: `nextInput = results.map(r => ({type:"function_call_output", call_id, output}))`, and uses `previous_response_id` for server-side conversation continuity.
   - OpenAI Chat: push an `assistant` message with `tool_calls`, then a `tool` message per result (`tool_call_id`, `content: output`).
   - Anthropic: push the `assistant` content blocks, then a `user` message with `tool_result` blocks (`tool_use_id`, `content`, `is_error`).
6. If the loop exhausts `agentMaxTurns` without the model stopping → throw `Agent exceeded the N-turn limit.`

### `executeAgentTools(calls, workspace, signal, event)`

Runs calls **sequentially** (not in parallel). For each: emit `tool_call { id, name, input }`, then `workspace.execute(name, input)`, bound the output to `agentCommandOutputMaxBytes`, emit `tool_result { id, name, output, isError:false }`. On error: re-check `signal.throwIfAborted()` (cancellation is terminal — a canceled call does not become a benign tool failure that lets later calls proceed), then emit `tool_result` with `isError:true` and the error message. Results are returned to the loop to feed back to the model.

### Timeout & retries

Each provider client is constructed with `timeout: config.agentRunTimeoutMs` and `maxRetries: 2`. A separate outer `setTimeout` in `executeAgentRun` aborts the controller after `agentRunTimeoutMs`.

---

## 8. How agents relate to projects / worktrees / deployments

### Projects → tasks → worktrees

- A **project** (`projects` table) must have a `gitUrl` to host AI tasks. It has a `localPath` (the primary clone) and a `folderName`.
- `ensureAgentRepository` lazily clones the repo to `project.localPath` if missing and verifies it's a git repo. `fetchProjectBranches` fetches origin and lists branches (deduping remote/local, hiding `HEAD`).
- On run start, `prepareTaskWorktree(project, task)`:
  - Validates `sourceBranch` and `taskBranch` against a strict ref regex.
  - Fetches origin, resolves `sourceSha` from `origin/<sourceBranch>` (falling back to the local branch).
  - If the worktree path already exists, verifies it's on `taskBranch` (else 409) and reuses it.
  - Otherwise creates the worktree with `git worktree add`, handling many cases: source==task branch (with uncommitted-changes guard on the primary checkout), local branch exists (with `resumeExistingBranch`/ancestor checks), remote branch exists (resume or error), or brand-new branch from `sourceRef`.
- The worktree path and `sourceSha` are persisted onto the task (`worktreePath`, `sourceSha`).
- `taskGitPreview` produces a structured review view: porcelain status, numstat, full diff (capped 2 MiB), HEAD sha, ahead/behind counts vs source, plus inline content for untracked small files.
- `commitTaskWorktree` (commits as `Yanto Agent <yanto@localhost>`), `pushTaskWorktree` (`push -u origin <taskBranch>`), `cleanupTaskWorktree` (`worktree remove` + `worktree prune`) — all SSH-key aware via `gitSshEnv`.

### Post-run Git automation (inside `executeAgentRun`)

After the provider/Codex run returns assistant text:
1. If `autoCommit`: compute a git preview; if not clean, commit with message `task: <title>` and emit `git_committed`.
2. If `autoPush`: push, set `pushedAt`, emit `git_pushed`, re-check preview; if still not clean → fail with "Branch pushed, but uncommitted changes remain." If clean → `finalTaskStatus = "done"`.
3. If `autoCleanup` (implies autoPush implies autoCommit): remove the worktree, null out `worktreePath`, emit `worktree_cleaned`.
4. Git failures are **contained**: they do not undo the model's work. The run is marked `failed` with `runError`/`taskError` = `"Git automation failed: <msg>"`, but the task still goes to `done` (the work is committed/pushed as far as it got) and the error is surfaced via `git_automation_failed` and `lastError`. The model's own success is not overwritten by a Git plumbing failure.
5. Manual post-run Git ops (`commitAgentTask`, `pushAgentTask`, `cleanupAgentTask`) mirror this for the human-review (`review`) path, all gated by the lifecycle lock and refused while `running`. `pushAgentTask` will auto-cleanup if `autoCleanup` is set and the push left the tree clean, transitioning `review → done`.

### Deployments

The `deployments` table is a **separate concern** from agents — it references `projects` and `deployment_nodes` but is not touched by the agent services. Agents produce branch pushes (reviewable diffs); deployments are an independent project lifecycle. There is no agent→deployment coupling.

---

## Precise data flow summary (one provider run)

```
startAgentTask(id, followUp?)
  └─ lifecycle lock {project, task}
       └─ DB tx: task backlog→running (conditional), insert agent_runs(running), maybe insert followUp message
            └─ register ActiveAgentRun {controller, completion} in activeRuns[taskId]
                 └─ (return run row to caller; run continues detached)

executeAgentRun(active, resolved)  [background]
  ├─ setTimeout(abort, agentRunTimeoutMs)
  ├─ recordEvent run_started {model, protocol}
  ├─ prepareTaskWorktree → persist worktreePath/sourceSha → recordEvent worktree_ready
  ├─ if codex_account:
  │     recordEvent workspace_started{codex}
  │     runCodexAccount → (Codex SDK thread; events → recordEvent; codexThreadId persisted)
  │     assistantText = result.assistantText
  ├─ else:
  │     workspace = AgentWorkspace(worktree, signal); registerRunnerStop(stop)
  │     workspace.start() → recordEvent workspace_started{provider}
  │     runAgentProvider({protocol, baseUrl, apiKey, model, messages(last 50), workspace, signal, event})
  │        └─ turn loop: stream model → assistant_delta events → collect tool calls
  │              → executeAgentTools → tool_call/tool_result events → feed results back
  │              → stop when no tool calls (return finalText) or throw on maxTurns
  │     workspace.stop()
  ├─ Git automation (autoCommit/autoPush/autoCleanup) → git_committed/git_pushed/worktree_cleaned/git_automation_failed
  ├─ reconcileTerminalRun (retry until DB write succeeds)
  │     run: succeeded|failed|canceled ; task: review|done ; assistant message persisted
  ├─ recordEvent run_finished {status, error, assistantText}
  └─ recordEvent task_finished {status} done=true   [final live event]
  finally: clearTimeout; activeRuns.delete(taskId); eventSequences.delete(runId); resolveCompletion
```

Live event path: every `recordEvent` → event bus → SSE subscriber on `task:<taskId>`. Persisted path (skipping `assistant_delta`): `agent_events` table, retrievable via `agentTaskEvents`.
