import { setTimeout as delay } from "node:timers/promises";

const DEFAULT_WORKER_REQUEST_TIMEOUT_MS = 30_000;
const MIN_WORKER_REQUEST_TIMEOUT_MS = 1_000;
const MAX_WORKER_REQUEST_TIMEOUT_MS = 5 * 60_000;

export function workerRequestTimeoutMs(value = process.env.YANTO_WORKER_REQUEST_TIMEOUT_MS) {
  if (value === undefined || value.trim() === "") return DEFAULT_WORKER_REQUEST_TIMEOUT_MS;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < MIN_WORKER_REQUEST_TIMEOUT_MS || parsed > MAX_WORKER_REQUEST_TIMEOUT_MS) {
    throw new Error(
      `YANTO_WORKER_REQUEST_TIMEOUT_MS must be an integer between ${MIN_WORKER_REQUEST_TIMEOUT_MS} and ${MAX_WORKER_REQUEST_TIMEOUT_MS}.`
    );
  }
  return parsed;
}

export async function requestWorkerJson<T>(url: string, options: RequestInit = {}, timeoutMs = workerRequestTimeoutMs()): Promise<T> {
  const timeoutSignal = AbortSignal.timeout(timeoutMs);
  const signal = options.signal ? AbortSignal.any([options.signal, timeoutSignal]) : timeoutSignal;

  try {
    const response = await fetch(url, { ...options, signal });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({ message: response.statusText }))) as { message?: string };
      throw new Error(body.message ?? `Worker request failed with ${response.status}.`);
    }
    if (response.status === 204) return undefined as T;
    return response.json() as Promise<T>;
  } catch (error) {
    if (timeoutSignal.aborted && !options.signal?.aborted) {
      throw new Error(`Worker request timed out after ${timeoutMs}ms.`, { cause: error });
    }
    throw error;
  }
}

export async function waitForWorkerPoll(delayMs: number, signal: AbortSignal) {
  try {
    await delay(delayMs, undefined, { signal });
    return true;
  } catch (error) {
    if (signal.aborted) return false;
    throw error;
  }
}
