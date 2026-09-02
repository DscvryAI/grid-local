import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import { toast } from "sonner";
import { api } from "../services/api";
import {
  createProjectSlice,
  type ProjectSlice,
} from "../store/slices/projectSlice";
import {
  AppErrorType,
  DEFAULT_USER_METADATA,
  type ClaudeProject,
  type ClaudeSession,
  type ProviderInfo,
  type UserMetadata,
} from "../types";

vi.mock("../services/api", () => ({
  api: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: {
    error: vi.fn(),
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(vi.fn()),
}));

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
};

const createDeferred = <T,>(): Deferred<T> => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
};

const flushMicrotasks = async () => {
  await Promise.resolve();
  await Promise.resolve();
};

type TestStore = ProjectSlice & {
  providers: ProviderInfo[];
  userMetadata: UserMetadata;
  updateUserSettings: ReturnType<typeof vi.fn>;
  excludeSidechain: boolean;
  analytics: { currentView: "messages" | "analytics" | "board" | "archive" };
  messages: unknown[];
  activeProviders: ProviderInfo["id"][];
  detectProviders: ReturnType<typeof vi.fn>;
  setActiveProviders: ReturnType<typeof vi.fn>;
  loadMetadata: ReturnType<typeof vi.fn>;
  loadServerConfig: ReturnType<typeof vi.fn>;
  exitSessionSelectionMode: ReturnType<typeof vi.fn>;
  selectSession: ReturnType<typeof vi.fn>;
  loadGlobalStats: ReturnType<typeof vi.fn>;
  loadProjectTokenStats: ReturnType<typeof vi.fn>;
  loadSessionTokenStats: ReturnType<typeof vi.fn>;
  loadProjectStatsSummary: ReturnType<typeof vi.fn>;
  setAnalyticsProjectSummary: ReturnType<typeof vi.fn>;
  loadSessionComparison: ReturnType<typeof vi.fn>;
  setAnalyticsSessionComparison: ReturnType<typeof vi.fn>;
  loadBoardSessions: ReturnType<typeof vi.fn>;
  loadArchives: ReturnType<typeof vi.fn>;
  clearSessionSearch: ReturnType<typeof vi.fn>;
  clearTokenStats: ReturnType<typeof vi.fn>;
  clearTargetMessage: ReturnType<typeof vi.fn>;
  resetAnalytics: ReturnType<typeof vi.fn>;
  clearBoard: ReturnType<typeof vi.fn>;
  setDateFilter: ReturnType<typeof vi.fn>;
};

const createMockProject = (
  name: string,
  provider?: ClaudeProject["provider"],
  lastModified = "2026-01-01T00:00:00.000Z",
): ClaudeProject => ({
  name,
  path: `/sessions/${name}`,
  actual_path: `/workspace/${name}`,
  session_count: 1,
  message_count: 1,
  last_modified: lastModified,
  git_info: null,
  ...(provider ? { provider } : {}),
});

const createMockSession = (
  id: string,
  project: ClaudeProject,
): ClaudeSession => ({
  session_id: id,
  actual_session_id: `actual-${id}`,
  file_path: `${project.path}/${id}.jsonl`,
  project_name: project.name,
  message_count: 1,
  first_message_time: "2026-01-01T00:00:00.000Z",
  last_message_time: "2026-01-01T00:00:00.000Z",
  last_modified: "2026-01-01T00:00:00.000Z",
  has_tool_use: false,
  has_errors: false,
  provider: project.provider,
});

const createTestStore = () =>
  create<TestStore>()((set, get) => ({
    providers: [],
    userMetadata: DEFAULT_USER_METADATA,
    updateUserSettings: vi.fn().mockResolvedValue(undefined),
    excludeSidechain: true,
    analytics: { currentView: "messages" },
    messages: [],
    activeProviders: ["claude"],
    detectProviders: vi.fn().mockResolvedValue(true),
    setActiveProviders: vi.fn().mockImplementation((ids: ProviderInfo["id"][]) => {
      set({ activeProviders: ids });
    }),
    loadMetadata: vi.fn().mockResolvedValue(undefined),
    loadServerConfig: vi.fn().mockResolvedValue(undefined),
    // Cross-slice dep added by the multi-select feature: selectProject /
    // clearProjectSelection abandon any in-progress session selection.
    exitSessionSelectionMode: vi.fn(),
    selectSession: vi.fn().mockImplementation(async (session: ClaudeSession) => {
      set({ selectedSession: session });
    }),
    loadGlobalStats: vi.fn().mockResolvedValue(undefined),
    loadProjectTokenStats: vi.fn().mockResolvedValue(undefined),
    loadSessionTokenStats: vi.fn().mockResolvedValue(undefined),
    loadProjectStatsSummary: vi.fn().mockResolvedValue({}),
    setAnalyticsProjectSummary: vi.fn(),
    loadSessionComparison: vi.fn().mockResolvedValue({}),
    setAnalyticsSessionComparison: vi.fn(),
    loadBoardSessions: vi.fn().mockResolvedValue(undefined),
    loadArchives: vi.fn().mockResolvedValue(undefined),
    clearSessionSearch: vi.fn(),
    clearTokenStats: vi.fn(),
    clearTargetMessage: vi.fn(),
    resetAnalytics: vi.fn(),
    clearBoard: vi.fn(),
    setDateFilter: vi.fn(),
    ...createProjectSlice(
      set as Parameters<typeof createProjectSlice>[0],
      get as Parameters<typeof createProjectSlice>[1],
      undefined as never,
    ),
  }));

describe("projectSlice scanProjects", () => {
  beforeEach(() => {
    vi.mocked(api).mockReset();
    vi.mocked(toast.error).mockReset();
  });

  it("publishes each provider as soon as that provider scan completes", async () => {
    const store = createTestStore();
    const claudeProject = createMockProject(
      "claude-only",
      undefined,
      "2026-01-03T00:00:00.000Z",
    );
    const geminiProject = createMockProject(
      "gemini-project",
      "gemini",
      "2026-01-02T00:00:00.000Z",
    );
    const codexProject = createMockProject(
      "codex-project",
      "codex",
      "2026-01-01T00:00:00.000Z",
    );
    const codexScan = createDeferred<ClaudeProject[]>();
    const geminiScan = createDeferred<ClaudeProject[]>();

    store.setState({
      claudePath: "/root/.claude",
      providers: [
        {
          id: "claude",
          display_name: "Claude Code",
          base_path: "/root/.claude",
          is_available: true,
        },
        {
          id: "codex",
          display_name: "Codex",
          base_path: "/root/.codex",
          is_available: true,
        },
        {
          id: "gemini",
          display_name: "Gemini CLI",
          base_path: "/root/.gemini",
          is_available: true,
        },
      ],
    });

    vi.mocked(api).mockImplementation((command, args) => {
      if (command === "scan_projects") {
        return Promise.resolve([claudeProject]);
      }
      if (command === "scan_all_projects") {
        const provider = (args?.activeProviders as string[] | undefined)?.[0];
        if (provider === "codex") {
          return codexScan.promise;
        }
        if (provider === "gemini") {
          return geminiScan.promise;
        }
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    const scanPromise = store.getState().scanProjects();
    await flushMicrotasks();

    expect(store.getState().isLoadingProjects).toBe(true);
    expect(store.getState().projects).toEqual([
      { ...claudeProject, provider: "claude" },
    ]);

    geminiScan.resolve([geminiProject]);
    await flushMicrotasks();

    expect(store.getState().isLoadingProjects).toBe(true);
    expect(store.getState().projects).toEqual([
      { ...claudeProject, provider: "claude" },
      geminiProject,
    ]);

    codexScan.resolve([codexProject]);
    await scanPromise;

    expect(store.getState().isLoadingProjects).toBe(false);
    expect(store.getState().projects).toEqual([
      { ...claudeProject, provider: "claude" },
      geminiProject,
      codexProject,
    ]);
  });

  it("limits the initial scan to Claude before provider discovery is requested", async () => {
    const store = createTestStore();
    const claudeProject = createMockProject("initial-claude");

    store.setState({
      claudePath: "/root/.claude",
      providers: [],
      activeProviders: ["claude"],
    });

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "scan_projects") {
        return Promise.resolve([claudeProject]);
      }
      if (command === "scan_all_projects") {
        return Promise.reject(
          new Error("initial startup must not scan non-Claude providers")
        );
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().scanProjects();

    expect(store.getState().projects).toEqual([
      { ...claudeProject, provider: "claude" },
    ]);
    expect(vi.mocked(api)).toHaveBeenCalledWith("scan_projects", {
      claudePath: "/root/.claude",
    });
    expect(vi.mocked(api)).not.toHaveBeenCalledWith(
      "scan_all_projects",
      expect.anything()
    );
  });

  it("restores explicitly discovered providers when Claude is not installed", async () => {
    const store = createTestStore();
    const codexProject = createMockProject("persisted-codex", "codex");

    store.setState({
      claudePath: "",
      providers: [],
      activeProviders: ["claude"],
      userMetadata: {
        ...DEFAULT_USER_METADATA,
        settings: {
          discoveredProviderIds: ["codex"],
          hasSeenProviderDiscoveryPrompt: true,
        },
      },
    });

    vi.mocked(api).mockImplementation((command, args) => {
      if (command === "get_claude_folder_path") {
        return Promise.reject(new Error("CLAUDE_FOLDER_NOT_FOUND:missing"));
      }
      if (command === "scan_all_projects") {
        expect(args).toEqual({ activeProviders: ["codex"] });
        return Promise.resolve([codexProject]);
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().initializeApp();

    expect(store.getState().error).toBeNull();
    expect(store.getState().projects).toEqual([codexProject]);
    expect(store.getState().detectProviders).not.toHaveBeenCalled();
    expect(store.getState().setActiveProviders).toHaveBeenCalledWith(["codex"]);
  });

  it("initializes WSL-only sources when native Claude is unavailable", async () => {
    const store = createTestStore();
    const wslProject = createMockProject("wsl-only");

    store.setState({
      claudePath: "",
      providers: [],
      activeProviders: ["claude"],
      userMetadata: {
        ...DEFAULT_USER_METADATA,
        settings: {
          wsl: { enabled: true, excludedDistros: [] },
          hasSeenProviderDiscoveryPrompt: true,
        },
      },
    });

    vi.mocked(api).mockImplementation((command, args) => {
      if (command === "get_claude_folder_path") {
        return Promise.reject(new Error("CLAUDE_FOLDER_NOT_FOUND:missing"));
      }
      if (command === "scan_all_projects") {
        expect(args).toEqual({
          activeProviders: ["claude"],
          wslEnabled: true,
          wslExcludedDistros: [],
        });
        return Promise.resolve([wslProject]);
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().initializeApp();

    expect(store.getState().error).toBeNull();
    expect(store.getState().projects).toEqual([
      { ...wslProject, provider: "claude" },
    ]);
    expect(store.getState().detectProviders).not.toHaveBeenCalled();
    expect(
      vi.mocked(api).mock.calls.some(([command]) => command === "scan_projects"),
    ).toBe(false);
  });

  it("persists the provider IDs returned by explicit discovery", async () => {
    const store = createTestStore();
    const codexProject = createMockProject("discovered-codex", "codex");
    const updateUserSettings = vi.fn().mockImplementation(
      async (update: UserMetadata["settings"]) => {
        store.setState({
          userMetadata: {
            ...store.getState().userMetadata,
            settings: {
              ...store.getState().userMetadata.settings,
              ...update,
            },
          },
        });
      }
    );

    store.setState({
      claudePath: "",
      providers: [
        {
          id: "codex",
          display_name: "Codex",
          base_path: "/root/.codex",
          is_available: true,
        },
      ],
      activeProviders: ["codex"],
      updateUserSettings,
    });

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "detect_claude_config_dir") {
        return Promise.resolve(null);
      }
      if (command === "scan_all_projects") {
        return Promise.resolve([codexProject]);
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().discoverProviders();

    expect(updateUserSettings).toHaveBeenCalledWith({
      discoveredProviderIds: ["codex"],
    });
    expect(store.getState().userMetadata.settings.discoveredProviderIds).toEqual([
      "codex",
    ]);
    expect(store.getState().projects).toEqual([codexProject]);
  });

  it("preserves persisted provider IDs when explicit discovery fails", async () => {
    const store = createTestStore();
    const codexProject = createMockProject("saved-codex", "codex");
    const updateUserSettings = vi.fn();

    store.setState({
      claudePath: "",
      providers: [],
      activeProviders: ["codex"],
      userMetadata: {
        ...DEFAULT_USER_METADATA,
        settings: { discoveredProviderIds: ["codex"] },
      },
      detectProviders: vi.fn().mockResolvedValue(false),
      updateUserSettings,
    });

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "scan_all_projects") {
        return Promise.resolve([codexProject]);
      }
      if (command === "detect_claude_config_dir") {
        return Promise.resolve(null);
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().discoverProviders();

    expect(updateUserSettings).not.toHaveBeenCalled();
    expect(store.getState().userMetadata.settings.discoveredProviderIds).toEqual([
      "codex",
    ]);
    expect(store.getState().projects).toEqual([codexProject]);
  });

  it("surfaces provider settings persistence failures", async () => {
    const store = createTestStore();
    const updateUserSettings = vi.fn().mockRejectedValue(new Error("save failed"));

    store.setState({
      claudePath: "",
      providers: [
        {
          id: "codex",
          display_name: "Codex",
          base_path: "/root/.codex",
          is_available: true,
        },
      ],
      activeProviders: ["codex"],
      updateUserSettings,
    });

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "detect_claude_config_dir") {
        return Promise.resolve(null);
      }
      if (command === "scan_all_projects") {
        return Promise.resolve([]);
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().discoverProviders();

    expect(toast.error).toHaveBeenCalledWith(expect.any(String));
  });

  it("reports provider errors when successful scans return no projects", async () => {
    const store = createTestStore();

    store.setState({
      providers: [
        {
          id: "codex",
          display_name: "Codex",
          base_path: "/root/.codex",
          is_available: true,
        },
        {
          id: "gemini",
          display_name: "Gemini CLI",
          base_path: "/root/.gemini",
          is_available: true,
        },
      ],
    });

    vi.mocked(api).mockImplementation((command, args) => {
      if (command === "scan_all_projects") {
        const provider = (args?.activeProviders as string[] | undefined)?.[0];
        if (provider === "codex") {
          return Promise.resolve([]);
        }
        if (provider === "gemini") {
          return Promise.reject(new Error("scan failed"));
        }
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().scanProjects();

    expect(store.getState().projects).toEqual([]);
    expect(store.getState().error).toEqual({
      type: AppErrorType.UNKNOWN,
      message: "gemini: scan failed",
    });
  });

  it("refreshes all conversations and reopens the selected session", async () => {
    const store = createTestStore();
    const project = createMockProject("current", "claude");
    const refreshedProject = {
      ...project,
      session_count: 2,
      last_modified: "2026-01-02T00:00:00.000Z",
    };
    const selectedSession = createMockSession("session-1", project);
    const refreshedSession = {
      ...selectedSession,
      message_count: 3,
      summary: "fresh session",
    };

    store.setState({
      claudePath: "/root/.claude",
      providers: [
        {
          id: "claude",
          display_name: "Claude Code",
          base_path: "/root/.claude",
          is_available: true,
        },
      ],
      selectedProject: project,
      selectedSession,
      activeProviders: ["claude"],
    });

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "scan_projects") {
        return Promise.resolve([refreshedProject]);
      }
      if (command === "load_provider_sessions_page") {
        return Promise.resolve({
          sessions: [refreshedSession],
          total: 1,
          offset: 0,
          limit: 250,
          nextOffset: 1,
          hasMore: false,
        });
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().refreshAllConversations();

    expect(store.getState().detectProviders).not.toHaveBeenCalled();
    expect(store.getState().activeProviders).toEqual(["claude"]);
    expect(store.getState().selectedProject).toEqual(refreshedProject);
    expect(store.getState().sessions).toEqual([refreshedSession]);
    expect(store.getState().selectSession).toHaveBeenCalledWith(refreshedSession);
    expect(store.getState().isRefreshingAllConversations).toBe(false);
  });

  it("clears stale selection when the selected project no longer exists", async () => {
    const store = createTestStore();
    const project = createMockProject("deleted", "claude");
    const selectedSession = createMockSession("session-1", project);

    store.setState({
      claudePath: "/root/.claude",
      providers: [
        {
          id: "claude",
          display_name: "Claude Code",
          base_path: "/root/.claude",
          is_available: true,
        },
      ],
      selectedProject: project,
      selectedSession,
      sessions: [selectedSession],
      messages: [{ uuid: "stale" }],
    });

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "scan_projects") {
        return Promise.resolve([]);
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().refreshAllConversations();

    expect(store.getState().selectedProject).toBeNull();
    expect(store.getState().selectedSession).toBeNull();
    expect(store.getState().sessions).toEqual([]);
    expect(store.getState().messages).toEqual([]);
    expect(store.getState().isRefreshingAllConversations).toBe(false);
  });

  it("clears stale session when the selected session no longer exists", async () => {
    const store = createTestStore();
    const project = createMockProject("current", "claude");
    const selectedSession = createMockSession("session-1", project);

    store.setState({
      claudePath: "/root/.claude",
      providers: [
        {
          id: "claude",
          display_name: "Claude Code",
          base_path: "/root/.claude",
          is_available: true,
        },
      ],
      selectedProject: project,
      selectedSession,
      sessions: [selectedSession],
      messages: [{ uuid: "stale" }],
    });

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "scan_projects") {
        return Promise.resolve([project]);
      }
      if (command === "load_provider_sessions_page") {
        return Promise.resolve({
          sessions: [],
          total: 0,
          offset: 0,
          limit: 250,
          nextOffset: 0,
          hasMore: false,
        });
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().refreshAllConversations();

    expect(store.getState().selectedProject).toEqual(project);
    expect(store.getState().selectedSession).toBeNull();
    expect(store.getState().messages).toEqual([]);
    expect(store.getState().clearSessionSearch).toHaveBeenCalled();
    expect(store.getState().clearTokenStats).toHaveBeenCalled();
  });

  it("refreshes project-level analytics when no session is selected", async () => {
    const store = createTestStore();
    const project = createMockProject("analytics", "claude");
    const projectSummary = { total_tokens: 123 };

    store.setState({
      claudePath: "/root/.claude",
      analytics: { currentView: "analytics" },
      providers: [
        {
          id: "claude",
          display_name: "Claude Code",
          base_path: "/root/.claude",
          is_available: true,
        },
      ],
      selectedProject: project,
      selectedSession: null,
    });
    store.getState().loadProjectStatsSummary.mockResolvedValue(projectSummary);

    vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
      if (command === "scan_projects") {
        return Promise.resolve([project]);
      }
      if (command === "load_provider_sessions_page") {
        return Promise.resolve({
          sessions: [],
          total: 0,
          offset: 0,
          limit: 250,
          nextOffset: 0,
          hasMore: false,
        });
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    await store.getState().refreshAllConversations();

    expect(store.getState().loadProjectStatsSummary).toHaveBeenCalledWith(
      project.path
    );
    expect(store.getState().setAnalyticsProjectSummary).toHaveBeenCalledWith(
      projectSummary
    );
    expect(store.getState().setAnalyticsSessionComparison).toHaveBeenCalledWith(
      null
    );
  });

  describe("provider discovery onboarding gate", () => {
    it("shows the onboarding gate on first run without scanning anything", async () => {
      const store = createTestStore();
      store.setState({
        claudePath: "",
        userMetadata: { ...DEFAULT_USER_METADATA, settings: {} },
      });
      vi.mocked(api).mockImplementation((command) =>
        Promise.reject(new Error(`Unexpected command: ${command}`))
      );

      await store.getState().initializeApp();

      expect(store.getState().showProviderDiscoveryOnboarding).toBe(true);
      expect(store.getState().error).toBeNull();
      expect(vi.mocked(api)).not.toHaveBeenCalled();
    });

    it("the onboarding action runs full provider discovery and persists the flag", async () => {
      const store = createTestStore();
      const codexProject = createMockProject("accepted-codex", "codex");
      const updateUserSettings = vi.fn().mockResolvedValue(undefined);

      store.setState({
        claudePath: "",
        userMetadata: { ...DEFAULT_USER_METADATA, settings: {} },
        showProviderDiscoveryOnboarding: true,
        providers: [
          {
            id: "codex",
            display_name: "Codex",
            base_path: "/root/.codex",
            is_available: true,
          },
        ],
        updateUserSettings,
      });

      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        if (command === "detect_claude_config_dir") {
          return Promise.resolve(null);
        }
        if (command === "scan_all_projects") {
          return Promise.resolve([codexProject]);
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      await store.getState().completeProviderDiscoveryOnboarding();

      expect(store.getState().showProviderDiscoveryOnboarding).toBe(false);
      expect(store.getState().error).toBeNull();
      expect(store.getState().detectProviders).toHaveBeenCalled();
      expect(updateUserSettings).toHaveBeenCalledWith({
        hasSeenProviderDiscoveryPrompt: true,
      });
      expect(store.getState().projects).toEqual([codexProject]);
    });
  });

  describe("mandatory first-run index", () => {
    it("runs the first index unconditionally when archive_db has never been populated, even with archiveSyncMode=manual", async () => {
      const store = createTestStore();
      const claudeProject = createMockProject("first-index-claude");

      store.setState({
        claudePath: "/root/.claude",
        userMetadata: {
          ...DEFAULT_USER_METADATA,
          settings: {
            hasSeenProviderDiscoveryPrompt: true,
            hasSeenArchiveSyncPrompt: true,
            archiveSyncMode: "manual",
          },
        },
      });

      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        if (command === "get_archive_db_status") {
          return Promise.resolve({
            providerCount: 0,
            projectCount: 0,
            sessionCount: 0,
            messageCount: 0,
          });
        }
        if (command === "run_first_index") {
          return Promise.resolve({
            projectsScanned: 1,
            sessionsIngested: 1,
            sessionsSkippedUnchanged: 0,
            messagesIngested: 2,
            cancelled: false,
          });
        }
        if (command === "get_claude_folder_path") {
          return Promise.resolve("/root/.claude");
        }
        if (command === "scan_projects") {
          return Promise.resolve([claudeProject]);
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      await store.getState().initializeApp();

      expect(store.getState().error).toBeNull();
      expect(vi.mocked(api)).toHaveBeenCalledWith("run_first_index");
      expect(vi.mocked(api)).not.toHaveBeenCalledWith("sync_grid_index");
      // The manual-mode preference must not have been overridden by this --
      // it still governs later refreshes, only the FIRST index is forced.
      expect(
        store.getState().userMetadata?.settings?.archiveSyncMode
      ).toBe("manual");
    });

    it("does not run the mandatory first index when archive_db already has sessions, and respects archiveSyncMode=auto for a background sync instead", async () => {
      const store = createTestStore();
      const claudeProject = createMockProject("already-indexed-claude");

      store.setState({
        claudePath: "/root/.claude",
        userMetadata: {
          ...DEFAULT_USER_METADATA,
          settings: {
            hasSeenProviderDiscoveryPrompt: true,
            hasSeenArchiveSyncPrompt: true,
            archiveSyncMode: "auto",
          },
        },
      });

      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        if (command === "get_archive_db_status") {
          return Promise.resolve({
            providerCount: 1,
            projectCount: 1,
            sessionCount: 5,
            messageCount: 20,
          });
        }
        if (command === "sync_grid_index") {
          return Promise.resolve({
            projectsScanned: 1,
            sessionsIngested: 0,
            sessionsSkippedUnchanged: 5,
            messagesIngested: 0,
            cancelled: false,
          });
        }
        if (command === "get_claude_folder_path") {
          return Promise.resolve("/root/.claude");
        }
        if (command === "scan_projects") {
          return Promise.resolve([claudeProject]);
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      await store.getState().initializeApp();

      expect(store.getState().error).toBeNull();
      expect(vi.mocked(api)).not.toHaveBeenCalledWith("run_first_index");
      // syncArchiveIndex is fire-and-forget (`void`) -- flush microtasks so
      // its underlying api() call has actually happened before asserting.
      await flushMicrotasks();
      expect(vi.mocked(api)).toHaveBeenCalledWith("sync_grid_index");
    });

    it("a second concurrent initializeApp() call awaits the same in-flight run instead of racing its own (regression: React StrictMode double-invoke)", async () => {
      const store = createTestStore();
      const claudeProject = createMockProject("reentrant-claude");
      const firstIndexCall = createDeferred<{
        projectsScanned: number;
        sessionsIngested: number;
        sessionsSkippedUnchanged: number;
        messagesIngested: number;
        cancelled: boolean;
      }>();

      store.setState({
        userMetadata: {
          ...DEFAULT_USER_METADATA,
          settings: {
            hasSeenProviderDiscoveryPrompt: true,
            hasSeenArchiveSyncPrompt: true,
            archiveSyncMode: "manual",
          },
        },
      });

      let archiveDbStatusCalls = 0;
      let runFirstIndexCalls = 0;
      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        if (command === "get_archive_db_status") {
          archiveDbStatusCalls += 1;
          return Promise.resolve({
            providerCount: 0,
            projectCount: 0,
            sessionCount: 0,
            messageCount: 0,
          });
        }
        if (command === "run_first_index") {
          runFirstIndexCalls += 1;
          return firstIndexCall.promise;
        }
        if (command === "get_claude_folder_path") {
          return Promise.resolve("/root/.claude");
        }
        if (command === "scan_projects") {
          return Promise.resolve([claudeProject]);
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      // Mimic StrictMode's mount -> cleanup -> remount double-invoke: call
      // initializeApp() twice without awaiting the first in between.
      const firstCall = store.getState().initializeApp();
      const secondCall = store.getState().initializeApp();

      // get_archive_db_status resolving, then runFirstIndex's own dynamic
      // `import("@tauri-apps/api/event")` and `listen(...)` awaits (which
      // can cross a real macrotask boundary, not just microtasks) before
      // it finally calls run_first_index -- poll rather than guess a tick
      // count.
      await vi.waitFor(() => {
        expect(runFirstIndexCalls).toBe(1);
      });
      expect(archiveDbStatusCalls).toBe(1);
      // Still indexing -- neither call may have released isLoading yet.
      expect(store.getState().isLoading).toBe(true);

      firstIndexCall.resolve({
        projectsScanned: 1,
        sessionsIngested: 1,
        sessionsSkippedUnchanged: 0,
        messagesIngested: 2,
        cancelled: false,
      });
      await firstCall;
      await secondCall;

      expect(store.getState().isLoading).toBe(false);
      expect(store.getState().error).toBeNull();
      // 2, not 1: `runFirstIndex`'s own diagnostics recording reads
      // `get_archive_db_status` again after a successful, non-cancelled
      // run to get the real provider-coverage count -- still exactly one
      // extra call total, not one per StrictMode double-invoke (that race
      // is still guarded by `initializeApp`'s promise memoization,
      // unrelated to this).
      expect(archiveDbStatusCalls).toBe(2);
      expect(runFirstIndexCalls).toBe(1);
    });
  });

  describe("archive index error surfacing", () => {
    it("records archiveIndexError when the mandatory first index throws, for the empty-state taxonomy's 'failed' reason", async () => {
      const store = createTestStore();
      store.setState({
        userMetadata: {
          ...DEFAULT_USER_METADATA,
          settings: {
            hasSeenProviderDiscoveryPrompt: true,
            hasSeenArchiveSyncPrompt: true,
            archiveSyncMode: "manual",
          },
        },
      });

      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        if (command === "get_archive_db_status") {
          return Promise.resolve({
            providerCount: 0,
            projectCount: 0,
            sessionCount: 0,
            messageCount: 0,
          });
        }
        if (command === "run_first_index") {
          return Promise.reject(new Error("disk is full"));
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      await store.getState().runFirstIndex();

      expect(store.getState().archiveIndexError).toBe("disk is full");
      // The mandatory-index caller (ensureFirstIndexIfNeeded) never throws
      // on a failed first index -- app startup must not be blocked by it.
    });

    it("clears a stale archiveIndexError once a later syncArchiveIndex call succeeds", async () => {
      const store = createTestStore();
      store.setState({ archiveIndexError: "previous failure" });

      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        if (command === "sync_grid_index") {
          return Promise.resolve({
            projectsScanned: 1,
            sessionsIngested: 0,
            sessionsSkippedUnchanged: 5,
            messagesIngested: 0,
            cancelled: false,
          });
        }
        if (command === "get_archive_db_status") {
          return Promise.resolve({
            providerCount: 1,
            projectCount: 1,
            sessionCount: 5,
            messageCount: 20,
          });
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      await store.getState().syncArchiveIndex();

      expect(store.getState().archiveIndexError).toBeNull();
    });

    it("retryArchiveIndex runs the first index when archive_db is still empty", async () => {
      const store = createTestStore();
      const calls: string[] = [];
      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        calls.push(command);
        if (command === "get_archive_db_status") {
          return Promise.resolve({
            providerCount: 0,
            projectCount: 0,
            sessionCount: 0,
            messageCount: 0,
          });
        }
        if (command === "run_first_index") {
          return Promise.resolve({
            projectsScanned: 1,
            sessionsIngested: 1,
            sessionsSkippedUnchanged: 0,
            messagesIngested: 2,
            cancelled: false,
          });
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      await store.getState().retryArchiveIndex();

      expect(calls).toContain("run_first_index");
      expect(calls).not.toContain("sync_grid_index");
    });

    it("retryArchiveIndex runs a background sync instead when archive_db already has sessions", async () => {
      const store = createTestStore();
      const calls: string[] = [];
      vi.mocked(api).mockImplementation((command) => {
      // Diagnostics events are fire-and-forget side effects unrelated to
      // what most of these tests exercise -- resolve them quietly rather
      // than making every pre-existing mock enumerate a command it
      // doesn't otherwise care about.
      if (command === "record_diagnostics_event") {
        return Promise.resolve(undefined);
      }
        calls.push(command);
        if (command === "get_archive_db_status") {
          return Promise.resolve({
            providerCount: 1,
            projectCount: 1,
            sessionCount: 5,
            messageCount: 20,
          });
        }
        if (command === "sync_grid_index") {
          return Promise.resolve({
            projectsScanned: 1,
            sessionsIngested: 0,
            sessionsSkippedUnchanged: 5,
            messagesIngested: 0,
            cancelled: false,
          });
        }
        return Promise.reject(new Error(`Unexpected command: ${command}`));
      });

      await store.getState().retryArchiveIndex();

      expect(calls).toContain("sync_grid_index");
      expect(calls).not.toContain("run_first_index");
    });
  });
});
