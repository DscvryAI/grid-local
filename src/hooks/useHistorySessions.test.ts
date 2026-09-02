import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act, waitFor } from "@testing-library/react";
import { useHistorySessions } from "./useHistorySessions";
import type { HistorySessionsPage } from "../types";

const { mockFetchHistorySessions } = vi.hoisted(() => ({
  mockFetchHistorySessions: vi.fn(),
}));

vi.mock("../services/historyApi", () => ({
  fetchHistorySessions: mockFetchHistorySessions,
}));

function makePage(overrides: Partial<HistorySessionsPage> = {}): HistorySessionsPage {
  return {
    items: [],
    total_count: 0,
    has_more: false,
    available_projects: [],
    available_providers: [],
    available_models: [],
    custom_claude_dirs_omitted: false,
    ...overrides,
  };
}

describe("useHistorySessions", () => {
  beforeEach(() => {
    mockFetchHistorySessions.mockReset();
  });

  it("fetches the first page on mount", async () => {
    mockFetchHistorySessions.mockResolvedValue(
      makePage({
        items: [
          {
            session_id: "s1",
            actual_session_id: "s1",
            provider_id: "claude",
            project_key: "/proj",
            project_name: "proj",
            file_path: "/proj/s1.jsonl",
            recency_time: "2026-01-01T00:00:00Z",
            message_count: 3,
            has_tool_use: false,
            has_errors: false,
          },
        ],
        total_count: 1,
      })
    );

    const { result } = renderHook(() => useHistorySessions());

    expect(result.current.isLoading).toBe(true);

    await waitFor(() => expect(result.current.isLoading).toBe(false));

    expect(result.current.sessions).toHaveLength(1);
    expect(result.current.totalCount).toBe(1);
    expect(mockFetchHistorySessions).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 0, limit: 50 })
    );
  });

  it("surfaces an error message instead of throwing when the fetch fails", async () => {
    mockFetchHistorySessions.mockRejectedValue(new Error("boom"));

    const { result } = renderHook(() => useHistorySessions());

    await waitFor(() => expect(result.current.isLoading).toBe(false));

    expect(result.current.error).toBe("boom");
    expect(result.current.sessions).toEqual([]);
  });

  it("appends to the existing list on loadMore instead of replacing it", async () => {
    mockFetchHistorySessions
      .mockResolvedValueOnce(
        makePage({
          items: [
            {
              session_id: "s1",
              actual_session_id: "s1",
              provider_id: "claude",
              project_key: "/proj",
              project_name: "proj",
              file_path: "/proj/s1.jsonl",
              recency_time: "2026-01-02T00:00:00Z",
              message_count: 1,
              has_tool_use: false,
              has_errors: false,
            },
          ],
          total_count: 2,
          has_more: true,
        })
      )
      .mockResolvedValueOnce(
        makePage({
          items: [
            {
              session_id: "s2",
              actual_session_id: "s2",
              provider_id: "claude",
              project_key: "/proj",
              project_name: "proj",
              file_path: "/proj/s2.jsonl",
              recency_time: "2026-01-01T00:00:00Z",
              message_count: 1,
              has_tool_use: false,
              has_errors: false,
            },
          ],
          total_count: 2,
          has_more: false,
        })
      );

    const { result } = renderHook(() => useHistorySessions());
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current.sessions).toHaveLength(1);
    expect(result.current.hasMore).toBe(true);

    act(() => {
      result.current.loadMore();
    });

    await waitFor(() => expect(result.current.isLoadingMore).toBe(false));
    expect(result.current.sessions.map((s) => s.session_id)).toEqual(["s1", "s2"]);
    expect(mockFetchHistorySessions).toHaveBeenLastCalledWith(
      expect.objectContaining({ offset: 1, limit: 50 })
    );
  });

  it("refetches from offset 0 when a filter changes", async () => {
    mockFetchHistorySessions.mockResolvedValue(makePage());

    const { result } = renderHook(() => useHistorySessions());
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    mockFetchHistorySessions.mockClear();

    act(() => {
      result.current.setDateFilter({ startDate: "2026-01-01" });
    });

    await waitFor(() => expect(mockFetchHistorySessions).toHaveBeenCalled());
    expect(mockFetchHistorySessions).toHaveBeenCalledWith(
      expect.objectContaining({
        offset: 0,
        filters: expect.objectContaining({ start_date: "2026-01-01" }),
      })
    );
  });
});
