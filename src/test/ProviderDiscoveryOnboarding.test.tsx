import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderDiscoveryOnboarding } from "../components/ProviderDiscoveryOnboarding";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, fallback?: string) => fallback ?? _key,
  }),
}));

const { mockStore } = vi.hoisted(() => ({
  mockStore: {
    completeProviderDiscoveryOnboarding: vi.fn().mockResolvedValue(undefined),
    detectProviders: vi.fn().mockResolvedValue(true),
    providers: [] as { id: string; display_name: string; base_path: string; is_available: boolean }[],
    isDetectingProviders: false,
    isLoadingProjects: false,
  },
}));

vi.mock("@/store/useAppStore", () => {
  const useAppStoreMock = (selector: (state: typeof mockStore) => unknown) =>
    selector(mockStore);
  useAppStoreMock.getState = () => mockStore;
  return { useAppStore: useAppStoreMock };
});

describe("ProviderDiscoveryOnboarding", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockStore.detectProviders = vi.fn().mockResolvedValue(true);
    mockStore.providers = [];
    mockStore.isDetectingProviders = false;
    mockStore.isLoadingProjects = false;
  });

  it("renders the single scan action and the privacy link", () => {
    render(<ProviderDiscoveryOnboarding />);

    expect(
      screen.getByRole("heading", { name: "Grid Local" })
    ).toBeInTheDocument();
    expect(
      screen.getByText("Your AI coding history, finally useful.")
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Scan my coding history" })
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "How Grid handles my data" })
    ).toBeInTheDocument();
  });

  it("detects providers first, then requires a second confirm tap to actually scan", async () => {
    mockStore.providers = [
      { id: "claude", display_name: "Claude Code", base_path: "C:\\Users\\me\\.claude\\projects", is_available: true },
    ];
    render(<ProviderDiscoveryOnboarding />);

    fireEvent.click(
      screen.getByRole("button", { name: "Scan my coding history" })
    );
    expect(mockStore.detectProviders).toHaveBeenCalledWith();
    expect(mockStore.completeProviderDiscoveryOnboarding).not.toHaveBeenCalled();

    // Detection resolved -- the manifest screen should now be showing.
    expect(await screen.findByText("Ready to scan")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("C:\\Users\\me\\.claude\\projects")).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Scan coding history" })
    );
    expect(mockStore.completeProviderDiscoveryOnboarding).toHaveBeenCalledWith();
  });

  it("falls straight through to the full flow when detection finds nothing", async () => {
    mockStore.providers = [];
    render(<ProviderDiscoveryOnboarding />);

    fireEvent.click(
      screen.getByRole("button", { name: "Scan my coding history" })
    );

    expect(mockStore.detectProviders).toHaveBeenCalledWith();
    await waitFor(() =>
      expect(mockStore.completeProviderDiscoveryOnboarding).toHaveBeenCalledWith()
    );
  });

  it("toggles the privacy detail panel without triggering a scan", () => {
    render(<ProviderDiscoveryOnboarding />);

    expect(
      screen.queryByText(/Grid only reads session files/)
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "How Grid handles my data" })
    );

    expect(
      screen.getByText(/Grid only reads session files/)
    ).toBeInTheDocument();
    expect(mockStore.completeProviderDiscoveryOnboarding).not.toHaveBeenCalled();
  });

  it("disables the scan action while detection is in progress", () => {
    mockStore.isDetectingProviders = true;
    render(<ProviderDiscoveryOnboarding />);

    expect(
      screen.getByRole("button", { name: "Searching for providers..." })
    ).toBeDisabled();
  });
});
