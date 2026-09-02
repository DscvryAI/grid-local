import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { ExportDiagnosticsDialog } from "@/layouts/Header/SettingDropdown/ExportDiagnosticsDialog";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string) => fallback ?? key,
  }),
}));

vi.mock("@/components/ui", () => ({
  Dialog: ({ open, children }: { open?: boolean; children?: ReactNode }) =>
    open ? <div>{children}</div> : null,
  DialogContent: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  DialogHeader: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  DialogTitle: ({ children }: { children?: ReactNode }) => <h2>{children}</h2>,
  DialogDescription: ({ children }: { children?: ReactNode }) => <p>{children}</p>,
  DialogFooter: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Button: ({ children, ...props }: { children?: ReactNode }) => (
    <button {...props}>{children}</button>
  ),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

const { mockGetSnapshot, mockRecordEvent, mockSaveFileDialog } = vi.hoisted(() => ({
  mockGetSnapshot: vi.fn(),
  mockRecordEvent: vi.fn().mockResolvedValue(undefined),
  mockSaveFileDialog: vi.fn(),
}));

vi.mock("@/services/diagnosticsApi", () => ({
  getDiagnosticsSnapshot: mockGetSnapshot,
  recordDiagnosticsEvent: mockRecordEvent,
}));

vi.mock("@/utils/fileDialog", () => ({
  saveFileDialog: mockSaveFileDialog,
}));

const SAMPLE_LOG = {
  version: 1,
  installedAt: "2026-08-31T00:00:00Z",
  firstIndexCompletedAt: "2026-08-31T00:01:00Z",
  firstPopulatedHomeAt: "2026-08-31T00:01:05Z",
  firstEvidenceDrilldownAt: null,
  launchCount: 4,
  activeDays: ["2026-08-31"],
  searchCount: 2,
  searchZeroResultCount: 1,
  searchResultOpenCount: 1,
  surfaceVisits: { home: 4, insights: 2 },
  problemOpens: 0,
  agentRunOpens: 0,
  indexRuns: [],
  crashCount: 0,
  exportCounts: {},
};

describe("ExportDiagnosticsDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetSnapshot.mockResolvedValue(SAMPLE_LOG);
    mockSaveFileDialog.mockResolvedValue(true);
  });

  it("renders nothing when closed, and never fetches the snapshot", () => {
    render(<ExportDiagnosticsDialog open={false} onOpenChange={vi.fn()} />);
    expect(mockGetSnapshot).not.toHaveBeenCalled();
  });

  it("fetches and displays the real diagnostics payload before any export action", async () => {
    render(<ExportDiagnosticsDialog open onOpenChange={vi.fn()} />);

    await waitFor(() => {
      expect(screen.getByText(/"launchCount": 4/)).toBeInTheDocument();
    });
    // The exact content the user would export is shown, not a summary.
    expect(screen.getByText(/"installedAt": "2026-08-31T00:00:00Z"/)).toBeInTheDocument();
  });

  it("shows the load error inline instead of the payload when the fetch fails", async () => {
    mockGetSnapshot.mockRejectedValue(new Error("read failed"));
    render(<ExportDiagnosticsDialog open onOpenChange={vi.fn()} />);

    await waitFor(() => {
      expect(screen.getByText("read failed")).toBeInTheDocument();
    });
  });

  it("exports the exact fetched payload and records a diagnostics-export event on success", async () => {
    const onOpenChange = vi.fn();
    render(<ExportDiagnosticsDialog open onOpenChange={onOpenChange} />);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Export" })).not.toBeDisabled();
    });

    fireEvent.click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => {
      expect(mockSaveFileDialog).toHaveBeenCalledWith(
        JSON.stringify(SAMPLE_LOG, null, 2),
        expect.objectContaining({ defaultPath: "grid-local-diagnostics.json" })
      );
    });
    expect(mockRecordEvent).toHaveBeenCalledWith({
      kind: "exported",
      artifactType: "diagnostics",
    });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("does not record an export event when the save dialog is cancelled", async () => {
    mockSaveFileDialog.mockResolvedValue(false);
    render(<ExportDiagnosticsDialog open onOpenChange={vi.fn()} />);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Export" })).not.toBeDisabled();
    });
    fireEvent.click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => {
      expect(mockSaveFileDialog).toHaveBeenCalled();
    });
    expect(mockRecordEvent).not.toHaveBeenCalled();
  });
});
