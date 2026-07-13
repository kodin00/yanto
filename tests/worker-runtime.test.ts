import { afterEach, describe, expect, it, vi } from "vitest";
import { requestWorkerJson, waitForWorkerPoll, workerRequestTimeoutMs } from "../src/server/services/worker-runtime.js";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("worker runtime", () => {
  it("validates the worker request deadline", () => {
    expect(workerRequestTimeoutMs(undefined)).toBe(30_000);
    expect(workerRequestTimeoutMs("15000")).toBe(15_000);
    expect(() => workerRequestTimeoutMs("0")).toThrow("YANTO_WORKER_REQUEST_TIMEOUT_MS");
    expect(() => workerRequestTimeoutMs("not-a-number")).toThrow("YANTO_WORKER_REQUEST_TIMEOUT_MS");
  });

  it("aborts a master request when its deadline expires", async () => {
    vi.stubGlobal("fetch", vi.fn((_url: string, options: RequestInit) => new Promise((_resolve, reject) => {
      options.signal?.addEventListener("abort", () => reject(options.signal?.reason), { once: true });
    })));

    await expect(requestWorkerJson("http://master.test/heartbeat", {}, 10)).rejects.toThrow(
      "Worker request timed out after 10ms."
    );
  });

  it("preserves caller cancellation instead of reporting a timeout", async () => {
    const controller = new AbortController();
    const reason = new Error("worker stopping");
    controller.abort(reason);
    vi.stubGlobal("fetch", vi.fn((_url: string, options: RequestInit) => Promise.reject(options.signal?.reason)));

    await expect(requestWorkerJson("http://master.test/jobs", { signal: controller.signal }, 60_000)).rejects.toBe(reason);
  });

  it("wakes a polling delay immediately during shutdown", async () => {
    const controller = new AbortController();
    const waiting = waitForWorkerPoll(60_000, controller.signal);
    controller.abort();

    await expect(waiting).resolves.toBe(false);
  });
});
