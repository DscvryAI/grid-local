import { describe, it, expect, vi, beforeAll, afterAll, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useUpdater } from "../hooks/useUpdater";
import { SimpleUpdateManager } from "../components/SimpleUpdateManager";
import { SettingDropdown } from "../layouts/Header/SettingDropdown/index";
import { PlatformProvider } from "../contexts/platform";
import type { UpdateSettings } from "../types/updateSettings";

// Simulate Tauri environment so isTauri() returns true
beforeAll(() => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).__TAURI_INTERNALS__ = {};
});
afterAll(() => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  delete (window as any).__TAURI_INTERNALS__;
});

const {
  mockCheck,
  mockRelaunch,
  mockGetVersion,
  mockOpenModal,
} = vi.hoisted(() => ({
  mockCheck: vi.fn(),
  mockRelaunch: vi.fn(),
  mockGetVersion: vi.fn(),
  mockOpenModal: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: mockCheck,
}));

vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: mockRelaunch,
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: mockGetVersion,
}));

vi.mock("react-i18next", async () => {
  const actual = await vi.importActual<typeof import("react-i18next")>(
    "react-i18next"
  );

  return {
    ...actual,
    useTranslation: () => ({
      t: (key: string) => key,
    }),
  };
});

vi.mock("@/contexts/modal", () => ({
  useModal: () => ({
    openModal: mockOpenModal,
  }),
}));

vi.mock("@/components/ui/dropdown-menu", () => ({
  DropdownMenu: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="dropdown-menu">{children}</div>
  ),
  DropdownMenuTrigger: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  DropdownMenuContent: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="dropdown-content">{children}</div>
  ),
  DropdownMenuItem: ({
    children,
    onClick,
    disabled,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
    disabled?: boolean;
  }) => (
    <button onClick={onClick} disabled={disabled}>
      {children}
    </button>
  ),
  DropdownMenuLabel: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DropdownMenuSeparator: () => <hr />,
  DropdownMenuSub: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DropdownMenuSubTrigger: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DropdownMenuSubContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

// AboutMenuGroup is NOT mocked -- rendered for realism, though it no
// longer exposes a "Check for Updates" UI trigger itself (hidden since the
// updater plugin has no real signing infra yet and the button could only
// ever error -- see the component's own doc comment). This test instead
// dispatches the underlying `manual-update-check` window event directly,
// exercising `SimpleUpdateManager`'s still-fully-intact response to it --
// the exact mechanism a future re-added button would use. The other 3
// groups are unrelated to this test's purpose and would each need their
// own store/service mocking to render without crashing.
vi.mock("../layouts/Header/SettingDropdown/DataMenuGroup", () => ({
  DataMenuGroup: () => <div data-testid="data-group" />,
}));

vi.mock("../layouts/Header/SettingDropdown/ProvidersMenuGroup", () => ({
  ProvidersMenuGroup: () => <div data-testid="providers-group" />,
}));

vi.mock("../layouts/Header/SettingDropdown/AppearanceMenuGroup", () => ({
  AppearanceMenuGroup: () => <div data-testid="appearance-group" />,
}));

const defaultSettings: UpdateSettings = {
  autoCheck: true,
  checkInterval: "startup",
  skippedVersions: [],
  postponeInterval: 24 * 60 * 60 * 1000,
  hasSeenIntroduction: false,
  respectOfflineStatus: true,
  allowCriticalUpdates: true,
};

const mockStore = {
  updateSettings: { ...defaultSettings },
  loadUpdateSettings: vi.fn(async () => {}),
  setUpdateSetting: vi.fn(async () => {}),
  postponeUpdate: vi.fn(async () => {}),
  skipVersion: vi.fn(async () => {}),
};

vi.mock("@/store/useAppStore", () => ({
  useAppStore: (selector: (state: typeof mockStore) => unknown) => selector(mockStore),
}));

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function UpdateFlowHarness() {
  const updater = useUpdater();

  return (
    <PlatformProvider>
      <SettingDropdown updater={updater} />
      <SimpleUpdateManager updater={updater} />
    </PlatformProvider>
  );
}

describe("Update flow integration", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetVersion.mockResolvedValue("1.0.0");
    mockCheck.mockResolvedValue(null);
    mockRelaunch.mockResolvedValue(undefined);
    mockStore.updateSettings = { ...defaultSettings };
  });

  it("runs a manual check and shows checking -> up-to-date flow", async () => {
    const deferred = createDeferred<null>();
    mockCheck.mockReturnValueOnce(deferred.promise);

    render(<UpdateFlowHarness />);

    await waitFor(() => {
      expect(mockStore.loadUpdateSettings).toHaveBeenCalledTimes(1);
    });

    fireEvent(window, new Event("manual-update-check"));

    await waitFor(() => {
      expect(mockCheck).toHaveBeenCalledTimes(1);
    });

    // No "Check for Updates" button exists to assert a disabled state on
    // (hidden -- see this file's own top-of-file comment) -- the checking
    // notification itself is the real, remaining signal.
    expect(screen.getByText("updateSettingsModal.checking")).toBeInTheDocument();

    await act(async () => {
      deferred.resolve(null);
      await deferred.promise;
    });

    await waitFor(() => {
      expect(screen.getByText("upToDateNotification.upToDate")).toBeInTheDocument();
      expect(mockStore.setUpdateSetting).toHaveBeenCalledWith(
        "lastCheckedAt",
        expect.any(Number)
      );
    });
  });

  it("supports skip version action from update modal after manual check", async () => {
    const mockDownloadAndInstall = vi.fn();
    mockCheck.mockResolvedValueOnce({
      version: "2.0.0",
      downloadAndInstall: mockDownloadAndInstall,
    });

    render(<UpdateFlowHarness />);

    await waitFor(() => {
      expect(mockStore.loadUpdateSettings).toHaveBeenCalledTimes(1);
    });

    fireEvent(window, new Event("manual-update-check"));

    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: "simpleUpdateModal.newUpdateAvailable" })
      ).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText("simpleUpdateModal.skipVersion"));

    await waitFor(() => {
      expect(mockStore.skipVersion).toHaveBeenCalledWith("2.0.0");
    });
  });
});
