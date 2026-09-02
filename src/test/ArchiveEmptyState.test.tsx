import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ArchiveEmptyState,
  ArchiveHealthEmptyState,
} from "@/components/common/ArchiveEmptyState";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, fallback?: string, vars?: Record<string, unknown>) => {
      if (typeof fallback !== "string") return _key;
      if (!vars) return fallback;
      return Object.entries(vars).reduce(
        (acc, [k, v]) => acc.replace(`{{${k}}}`, String(v)),
        fallback
      );
    },
  }),
}));

const { mockStore } = vi.hoisted(() => ({
  mockStore: {
    retryArchiveIndex: vi.fn().mockResolvedValue(undefined),
  },
}));

vi.mock("@/store/useAppStore", () => ({
  useAppStore: (selector: (state: typeof mockStore) => unknown) => selector(mockStore),
}));

describe("ArchiveEmptyState", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders never-built copy distinctly from a genuine no-data message", () => {
    render(<ArchiveEmptyState reason="never-built" />);
    expect(
      screen.getByText("Your local index hasn't been built yet")
    ).toBeInTheDocument();
  });

  it("renders building copy with progress when provided", () => {
    render(
      <ArchiveEmptyState
        reason="building"
        progress={{ providerKey: "claude", phasesDone: 3, phasesTotal: 18 }}
      />
    );
    expect(screen.getByText("Grid is still building your index")).toBeInTheDocument();
    expect(screen.getByText("4 of 18 sources scanned")).toBeInTheDocument();
  });

  it("renders failed copy with a working retry button", () => {
    const onRetry = vi.fn();
    render(<ArchiveEmptyState reason="failed" onRetry={onRetry} />);

    expect(screen.getByText("Grid's local index couldn't be built")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("renders unsupported-provider copy naming the provider", () => {
    render(<ArchiveEmptyState reason="unsupported-provider" provider="zed" />);
    expect(screen.getByText(/isn't fully supported yet/)).toBeInTheDocument();
  });

  it("no-data reason uses the caller-supplied title, not a generic default", () => {
    render(<ArchiveEmptyState reason="no-data" title="Nothing since your last visit" />);
    expect(screen.getByText("Nothing since your last visit")).toBeInTheDocument();
  });
});

describe("ArchiveHealthEmptyState", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("wires 'Try again' to the store's retryArchiveIndex action for a failed health", () => {
    render(<ArchiveHealthEmptyState health={{ state: "failed", error: "boom" }} />);

    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(mockStore.retryArchiveIndex).toHaveBeenCalledTimes(1);
  });

  it("renders never-built with no retry action", () => {
    render(<ArchiveHealthEmptyState health={{ state: "never-built" }} />);

    expect(
      screen.getByText("Your local index hasn't been built yet")
    ).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
