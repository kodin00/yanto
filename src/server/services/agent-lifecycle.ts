const lifecycleTails = new Map<string, Promise<void>>();

async function acquireLifecycleLock(key: string) {
  const previous = lifecycleTails.get(key) ?? Promise.resolve();
  let release!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  const tail = previous.then(() => held);
  lifecycleTails.set(key, tail);
  await previous;
  return () => {
    release();
    if (lifecycleTails.get(key) === tail) lifecycleTails.delete(key);
  };
}

export function agentProjectLifecycleKey(projectId: string) {
  return `project:${projectId}`;
}

export function agentTaskLifecycleKey(taskId: string) {
  return `task:${taskId}`;
}

/** Serializes start admission with destructive lifecycle operations in the current single-master process. */
export async function withAgentLifecycleLock<T>(keys: string[], operation: () => Promise<T>) {
  const releases: Array<() => void> = [];
  try {
    for (const key of [...new Set(keys)].sort()) releases.push(await acquireLifecycleLock(key));
    return await operation();
  } finally {
    for (const release of releases.reverse()) release();
  }
}
