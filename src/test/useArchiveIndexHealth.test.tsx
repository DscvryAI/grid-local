import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/services/api";
import { useArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";

vi.mock("@/services/api", () => ({
  api: vi.fn(),
}));

const { mockStore } = vi.hoisted(() => ({
  mockStore: {
    firstIndexProgress: null as { providerKey: string; phasesDone: number; phasesTotal: number } | null,
    archiveIndexError: null as string | null,
  },
}));

vi.mock("@/store/useAppStore", () => ({
  useAppStore: (selector: (state: typeof mockStore) => unknown) => selector(mockStore),
}));

describe("useArchiveIndexHealth", () => {
  beforeEach(() => {
    vi.mocked(api).mockReset();
    mockStore.firstIndexProgress = null;
    mockStore.archiveIndexError = null;
  });

  it("reports 'building' while a first index is in flight, regardless of archive_db's real status", async () => {
    mockStore.firstIndexProgress = { providerKey: "claude", phasesDone: 1, phasesTotal: 18 };
    vi.mocked(api).mockResolvedValue({
      providerCount: 1,
      projectCount: 1,
      sessionCount: 5,
      messageCount: 20,
    });

    const { result } = renderHook(() => useArchiveIndexHealth());

    expect(result.current.state).toBe("building");
    if (result.current.state === "building") {
      expect(result.current.progress.phasesDone).toBe(1);
    }
  });

  it("reports 'failed' with the recorded error when the last index/sync attempt threw", async () => {
    mockStore.archiveIndexError = "disk is full";
    vi.mocked(api).mockResolvedValue({
      providerCount: 0,
      projectCount: 0,
      sessionCount: 0,
      messageCount: 0,
    });

    const { result } = renderHook(() => useArchiveIndexHealth());

    expect(result.current.state).toBe("failed");
    if (result.current.state === "failed") {
      expect(result.current.error).toBe("disk is full");
    }
  });

  it("reports 'never-built' when archive_db genuinely has zero sessions and there's no error", async () => {
    vi.mocked(api).mockResolvedValue({
      providerCount: 0,
      projectCount: 0,
      sessionCount: 0,
      messageCount: 0,
    });

    const { result } = renderHook(() => useArchiveIndexHealth());

    await waitFor(() => expect(result.current.state).toBe("never-built"));
  });

  it("reports 'ready' with the real status once archive_db has sessions", async () => {
    vi.mocked(api).mockResolvedValue({
      providerCount: 2,
      projectCount: 3,
      sessionCount: 42,
      messageCount: 500,
    });

    const { result } = renderHook(() => useArchiveIndexHealth());

    await waitFor(() => expect(result.current.state).toBe("ready"));
    if (result.current.state === "ready") {
      expect(result.current.status.sessionCount).toBe(42);
    }
  });

  it("re-checks archive_db once an in-flight first index finishes (transitions building -> ready)", async () => {
    vi.mocked(api).mockResolvedValue({
      providerCount: 0,
      projectCount: 0,
      sessionCount: 0,
      messageCount: 0,
    });
    mockStore.firstIndexProgress = { providerKey: "claude", phasesDone: 0, phasesTotal: 18 };

    const { result, rerender } = renderHook(() => useArchiveIndexHealth());
    expect(result.current.state).toBe("building");

    vi.mocked(api).mockResolvedValue({
      providerCount: 1,
      projectCount: 1,
      sessionCount: 10,
      messageCount: 50,
    });
    act(() => {
      mockStore.firstIndexProgress = null;
    });
    rerender();

    await waitFor(() => expect(result.current.state).toBe("ready"));
  });
});
