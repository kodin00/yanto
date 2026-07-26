import { act, renderHook } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useOperationalHistory } from "../../src/client/hooks/useOperationalHistory";

const api = vi.hoisted(() => ({
  deploymentsPage: vi.fn(),
  backupsPage: vi.fn(),
  postgresBackupTargets: vi.fn(),
  auditLogPage: vi.fn()
}));

vi.mock("../../src/client/lib/api", () => ({ api }));

function useHarness(onError = vi.fn(), onPageLoaded = vi.fn()) {
  const [loading, setViewLoading] = useState<Record<string, boolean>>({});
  const [errors, setViewErrors] = useState<Record<string, string | undefined>>({});
  const history = useOperationalHistory({ setViewLoading, setViewErrors, onError, onPageLoaded });
  return { history, loading, errors };
}

describe("useOperationalHistory", () => {
  beforeEach(() => {
    api.deploymentsPage.mockReset();
    api.backupsPage.mockReset();
    api.postgresBackupTargets.mockReset();
    api.auditLogPage.mockReset();
  });

  it("commits a new deployment page only after its rows load", async () => {
    const onPageLoaded = vi.fn();
    api.deploymentsPage.mockResolvedValue({
      items: [{ id: "dep_2", projectId: "prj_1", status: "success" }],
      total: 24,
      page: 2,
      pageSize: 10
    });
    const { result } = renderHook(() => useHarness(vi.fn(), onPageLoaded));

    await act(() => result.current.history.selectDeploymentPage(2));

    expect(api.deploymentsPage).toHaveBeenCalledWith(2, 10);
    expect(result.current.history.deploymentPage).toBe(2);
    expect(result.current.history.deploymentRows).toEqual([expect.objectContaining({ id: "dep_2" })]);
    expect(result.current.history.deploymentTotal).toBe(24);
    expect(result.current.loading.deployments).toBe(false);
    expect(onPageLoaded).toHaveBeenCalledWith("deployments");
  });

  it("retains the current page and exposes an error when a page request fails", async () => {
    const onError = vi.fn();
    api.auditLogPage.mockRejectedValue(new Error("Audit service unavailable"));
    const { result } = renderHook(() => useHarness(onError));

    await act(() => result.current.history.selectAuditPage(3));

    expect(result.current.history.auditPage).toBe(1);
    expect(result.current.errors.audit).toBe("Audit service unavailable");
    expect(result.current.loading.audit).toBe(false);
    expect(onError).toHaveBeenCalledWith("Audit service unavailable");
  });
});
