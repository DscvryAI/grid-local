import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/services/api";
import { getDiagnosticsSnapshot, recordDiagnosticsEvent } from "@/services/diagnosticsApi";

vi.mock("@/services/api", () => ({
  api: vi.fn(),
}));

describe("diagnosticsApi", () => {
  beforeEach(() => {
    vi.mocked(api).mockReset();
  });

  it("recordDiagnosticsEvent calls the backend command with the event as an arg", async () => {
    vi.mocked(api).mockResolvedValue(undefined);

    await recordDiagnosticsEvent({ kind: "surfaceVisited", surface: "home" });

    expect(api).toHaveBeenCalledWith("record_diagnostics_event", {
      event: { kind: "surfaceVisited", surface: "home" },
    });
  });

  it("recordDiagnosticsEvent never throws -- a failed write is swallowed and logged", async () => {
    vi.mocked(api).mockRejectedValue(new Error("disk full"));
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    await expect(recordDiagnosticsEvent({ kind: "crashed" })).resolves.toBeUndefined();
    expect(consoleSpy).toHaveBeenCalled();

    consoleSpy.mockRestore();
  });

  it("getDiagnosticsSnapshot returns the backend's response as-is", async () => {
    const snapshot = {
      version: 1,
      installedAt: "2026-08-31T00:00:00Z",
      firstIndexCompletedAt: null,
      firstPopulatedHomeAt: null,
      firstEvidenceDrilldownAt: null,
      launchCount: 3,
      activeDays: ["2026-08-31"],
      searchCount: 0,
      searchZeroResultCount: 0,
      searchResultOpenCount: 0,
      surfaceVisits: {},
      problemOpens: 0,
      agentRunOpens: 0,
      indexRuns: [],
      crashCount: 0,
      exportCounts: {},
    };
    vi.mocked(api).mockResolvedValue(snapshot);

    const result = await getDiagnosticsSnapshot();

    expect(api).toHaveBeenCalledWith("get_diagnostics_snapshot");
    expect(result).toEqual(snapshot);
  });
});
