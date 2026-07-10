# AI Task Reliability and Security Improvements

## Objective

Harden the current AI-task implementation without removing existing capabilities or changing the public task workflow:

1. Create a task from a Git-backed project.
2. Run it with an API-key provider or a signed-in Codex account.
3. Stop, continue, review, commit, push, and clean the task worktree.
4. Preserve existing task, provider, model, branch, and conversation data during upgrades.

The safest approach is to land these improvements as small phases. Each phase should keep the existing API response shapes and UI flow unless explicitly noted.

## Compatibility rules

- Do not rename or remove the current task statuses or provider protocols.
- Keep existing task and Codex thread IDs valid.
- Make database migrations additive and idempotent.
- Do not automatically delete a remote branch.
- Never silently discard worktree changes.
- A failed start must leave the task in its previous usable state.
- A stop response must mean the execution container has actually stopped, or clearly report that cleanup is still pending.

## Phase 1: Add orchestration tests before changing behavior

The existing worktree and tool tests are useful, but the service orchestration and Codex runner need focused coverage. Introduce injectable runner, Docker, event-bus, and database boundaries where necessary so these cases can be tested without a real model account.

Add tests for:

- Two simultaneous start requests for the same task.
- Concurrent starts when only one global capacity slot remains.
- A provider-resolution failure with a follow-up message.
- Stop before worktree preparation completes.
- Stop immediately after the Codex Docker command is spawned.
- A runner event callback throwing while the container is active.
- Graceful shutdown with API-key and Codex runs active.
- Startup recovery with labeled orphan containers present.
- A run finishing between an SSE snapshot query and live subscription.
- Deleting a running task or its project.
- Deleting and then explicitly resuming a pushed task branch.

Acceptance criteria:

- All existing tests remain unchanged and passing.
- New tests reproduce the current race conditions before their corresponding fixes are applied.
- No test requires real OpenAI, Anthropic, Git-hosting, or Codex credentials.

## Phase 2: Make run admission atomic

### Problem

`startAgentTask` checks task state and in-memory capacity before several awaited operations. Concurrent requests can both pass those checks, create separate runs, and operate on one worktree. A follow-up message is also written before provider resolution and run creation, so a rejected start can leave an orphan or duplicated instruction.

### Improvement

1. Add a partial unique index that permits only one `running` row per task:

   ```sql
   CREATE UNIQUE INDEX IF NOT EXISTS agent_runs_one_running_per_task_idx
   ON agent_runs (task_id)
   WHERE status = 'running';
   ```

2. Resolve and validate the selected model before modifying conversation state.
3. In one database transaction:

   - Lock or conditionally transition the task from a non-running status to `running`.
   - Insert the run.
   - Insert the optional follow-up with that `run_id`.
   - Update task timestamps and clear `last_error`.

4. Reserve the in-memory capacity slot before launching asynchronous preparation. If launch setup fails, release the slot and roll back or terminally finish the created run.
5. Translate the unique-index conflict into the existing `409 Task already has an active run` response.

For the current single-master deployment, the in-memory map can continue enforcing global capacity. If multiple app replicas are supported later, move global capacity admission to a PostgreSQL advisory lock or lease table.

Acceptance criteria:

- Exactly one of two concurrent starts succeeds.
- Only the successful request stores its follow-up message.
- Failed provider validation leaves task status, messages, and timestamps unchanged.
- Existing clients still receive `202` for an accepted run and `409` or `429` for rejection.

## Phase 3: Give every runner deterministic ownership and cleanup

### Problem

Codex cancellation is registered after Docker spawning and checks an already-aborted signal only after the child exits. Cleanup is not protected by `finally`. Application shutdown and startup recovery change database state without guaranteeing that old containers have stopped.

### Improvement

Create one internal runner handle for both provider types:

```ts
type ActiveAgentRun = {
  runId: string;
  taskId: string;
  controller: AbortController;
  stop: () => Promise<void>;
  completion: Promise<void>;
};
```

For Docker-backed runners:

1. Add labels when creating containers, for example:

   - `com.yanto.agent=true`
   - `com.yanto.agent.run-id=<runId>`
   - `com.yanto.agent.task-id=<taskId>`

2. Check `signal.throwIfAborted()` before any Docker operation.
3. Register cancellation before starting the container and check the signal again immediately after registration.
4. Prefer an explicit `docker create` followed by `docker start -a` over a single `docker run`. This provides a stable container identity that can always be removed, including cancellation during startup.
5. Put signal-listener removal, stream closure, child termination, and `docker rm -f` in an idempotent `finally` block.
6. Await confirmed container removal before resolving `stopAgentTask`.
7. Do not mark a run terminal until its runner cleanup has completed or a cleanup failure has been recorded.

For application lifecycle:

- On `SIGINT` or `SIGTERM`, stop accepting new HTTP requests, abort all active runs, await their cleanup with a bounded timeout, then close the database pool.
- At startup, list only containers carrying `com.yanto.agent=true`, remove containers whose run rows are still `running`, and then mark those runs interrupted.
- Never remove containers based only on a broad name prefix.

Acceptance criteria:

- Stopping during Git preparation prevents a Codex container from starting.
- Stopping after container creation removes that exact container.
- Restart recovery leaves no labeled orphan runner containers.
- Once a task becomes `review`, no previous runner can continue changing its worktree.
- Repeated cleanup calls are safe.

## Phase 4: Protect Codex account data inside the runner

### Problem

The Codex container currently receives the persistent account and session directory as a read-write mount while also executing model-selected commands with network access. This relies entirely on the inner Codex sandbox to prevent task commands or repository code from reading account credentials and other task sessions.

### Improvement

Treat this as a sandbox-verification task before changing authentication behavior:

1. Add a runner-image integration probe using the exact production Codex sandbox configuration. Place fake credential and session sentinels under the mounted `CODEX_HOME`, then verify that a model-executed shell command cannot read or modify them.
2. Configure a Codex permission profile or sandbox state whose readable and writable task roots are limited to `/workspace` and required system/toolchain paths, explicitly excluding `/data/codex` from command execution. The Codex parent process may access its home; its spawned task commands must not.
3. Repeat the probe for symlinks, subprocesses, test scripts, Python/Node file reads, and direct network exfiltration attempts.
4. Separate persistent account state from per-task conversation storage where supported. A task should not be able to read another task's saved session.
5. Default task-command network access to disabled if the Codex parent can still reach its API. If dependency installation genuinely requires direct command networking, make that an explicit project setting with a warning rather than an unconditional default.
6. Refuse to start Codex-account tasks when the sandbox probe or required permission profile is unavailable. API-key providers can remain available because their keys are not mounted in task containers.

Do not replace this with a read-only credential mount alone: read-only credentials can still be copied. The important guarantee is that task-controlled processes cannot read the credential path.

Acceptance criteria:

- The Codex CLI can authenticate and resume existing task threads.
- Task commands can read and write the worktree normally.
- Task commands cannot read account credentials or another task's session.
- A malicious repository test cannot exfiltrate the fake sentinel.
- The container still has no Docker socket, SSH key, or parent Git metadata.

## Phase 5: Make SSE delivery race-free and bounded

### Problem

The stream route queries and sends a snapshot before subscribing to live events. A terminal event can occur in that gap. The browser closes on stream error without reconciling task state. The snapshot also replays unbounded event history that the current client does not consume.

### Improvement

1. Subscribe before querying the task snapshot and temporarily buffer incoming events.
2. Include a run ID and sequence watermark in the snapshot.
3. After sending the snapshot, flush buffered events whose sequence is newer than the watermark and then switch to direct delivery.
4. Deduplicate client events by `(runId, sequence)`.
5. On EventSource error or unexpected close, reload the task and reconnect only if it is still running.
6. Send only the current/latest run's bounded recent activity in the snapshot. Add a paginated history endpoint later if historical tool logs are needed.
7. Cap persisted tool payloads separately from command output and retain only the event detail needed for review.

Acceptance criteria:

- The client always observes a terminal state, including completion during connection setup.
- Reconnection does not duplicate visible activity.
- Opening a long-lived task does not serialize every historical tool result.
- Existing final assistant messages and Git state still load normally.

## Phase 6: Make task and project deletion lifecycle-aware

### Project deletion

Before deleting a project:

1. Reject with `409` when any task is running, unless a future explicit force workflow first stops and confirms cleanup of every runner.
2. Load all project tasks before the database cascade.
3. Remove their worktrees through Git while the parent repository still exists.
4. Prune worktree metadata.
5. Delete the database project and then its deployment checkout and empty `.yanto-worktrees/<folder>` directory.

This ordering avoids deleting the Git metadata required to clean worktrees.

### Task deletion and branch retention

Keep remote branches untouched. The least surprising compatibility behavior is:

- Deleting a task removes its worktree and database history but retains its local and remote branches.
- A later task with `resumeExistingBranch=true` may reuse an existing local branch only when the matching remote branch exists and the local branch can be safely fast-forwarded to it.
- A local-only branch with no owning task remains rejected, because automatically claiming or deleting it could lose user work.
- Optionally add a separate explicit `Delete local task branch` control after verifying it is not checked out and has no unpushed commits.

Acceptance criteria:

- A running project cannot be deleted accidentally.
- Project deletion leaves no worktree directories or live runner containers.
- A pushed task branch can be explicitly resumed after its old task is deleted.
- No remote branch is deleted implicitly.
- Local-only unpushed work is never silently removed.

## Phase 7: Tighten status and failure semantics

Keep the current statuses, but make their meaning precise:

- `backlog`: no active or accepted run.
- `running`: exactly one accepted run owns the task worktree.
- `review`: no runner owns the worktree; inspectable output or an actionable error exists.
- `done`: the branch was pushed and the worktree was clean at that pushed commit.

Additional rules:

- Store agent execution success separately from Git automation success, as the current run/task split already allows.
- When Git automation fails, retain the successful assistant message and run result, set the task to `review`, and preserve the worktree.
- If final database writes fail, retry terminal reconciliation rather than unlocking the task immediately while ownership is uncertain.
- Associate every follow-up message with the run that accepted it.
- Clear `lastError` only after a new run has been accepted, not before validation.

## Recommended delivery order

1. Orchestration tests.
2. Atomic run admission and transactional follow-ups.
3. Deterministic cancellation, shutdown, and orphan recovery.
4. Codex credential-isolation probe and permission profile.
5. Race-free, bounded SSE delivery.
6. Lifecycle-aware task and project deletion.
7. Status reconciliation and operational polish.

Phases 2, 3, and 4 are release blockers because they prevent concurrent worktree ownership, hidden post-cancellation edits, and credential exposure. Phases 5 through 7 can follow independently without changing the core feature.

## Release verification checklist

Before publishing the hardened release, verify:

- `npm run typecheck`
- `npm run lint`
- `npm run test:run`
- `npm run build`
- Double-clicking Start produces one run and one message.
- Stop works during fetch, container startup, model execution, and tool execution.
- Restarting Yanto during a run leaves no active agent container.
- Codex sign-in, model refresh, first run, and follow-up thread resume still work.
- Fake Codex credential/session sentinels are inaccessible to task commands.
- API-key provider runs still receive no provider key inside their sandbox.
- SSE reconnects and reaches a terminal UI state.
- Project and task deletion preserve remote branches and never discard uncommitted work.
