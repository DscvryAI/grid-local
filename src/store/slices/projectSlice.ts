/**
 * Project Slice
 *
 * Handles project/folder scanning and session listing.
 */

import { api } from "@/services/api";
import { storageAdapter } from "@/services/storage";
import {
  syncGridIndex,
  runFirstIndex as runFirstIndexCommand,
  cancelFirstIndex as cancelFirstIndexCommand,
  getArchiveDbStatus,
  type BackfillSummary,
  type FirstIndexProgressEvent,
} from "@/services/archiveSyncApi";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";
import type { ClaudeProject, ClaudeSession, SessionPage, AppError, ProviderId, UserSettings } from "../../types";
import { AppErrorType } from "../../types";
import type { StateCreator } from "zustand";
import { toast } from "sonner";
import i18n from "../../i18n";
import type { FullAppStore } from "./types";
import {
  detectWorktreeGroupsHybrid,
  groupProjectsByDirectory,
  type WorktreeGroupingResult,
  type DirectoryGroupingResult,
} from "../../utils/worktreeUtils";
import type { GroupingMode } from "../../types/metadata.types";
import {
  DEFAULT_PROVIDER_ID,
  getProviderId,
  normalizeProviderIds,
  PROVIDER_IDS,
} from "../../utils/providers";
import { INITIAL_PAGINATION } from "./messageSlice";
import { nextRequestId, getRequestId } from "../../utils/requestId";

// ============================================================================
// State Interface
// ============================================================================

export interface ProjectSliceState {
  claudePath: string;
  projects: ClaudeProject[];
  selectedProject: ClaudeProject | null;
  sessions: ClaudeSession[];
  sessionsTotal: number;
  sessionsOffset: number;
  hasMoreSessions: boolean;
  selectedSession: ClaudeSession | null;
  isLoading: boolean;
  isLoadingProjects: boolean;
  isLoadingSessions: boolean;
  isLoadingMoreSessions: boolean;
  isRefreshingAllConversations: boolean;
  error: AppError | null;
  /** First-run gate: show the one-tap "scan for other AI tools" prompt */
  showProviderDiscoveryOnboarding: boolean;
  /** One-time, non-blocking gate: show the "keep your archive up to date?"
   * preference prompt (unlike provider discovery, this never blocks the
   * rest of the UI -- History/Search work fine without archive_db). */
  showArchiveSyncPrompt: boolean;
  /** Non-null exactly while the mandatory first-run index (`runFirstIndex`)
   * is in flight -- the first index is unconditional and blocks
   * `initializeApp` until it settles (completes, fails, or is cancelled),
   * regardless of the `archiveSyncMode` preference, which governs only
   * later automatic refreshes. */
  firstIndexProgress: FirstIndexProgressEvent | null;
  /** Message from the most recent failed `runFirstIndex`/`syncArchiveIndex`
   * attempt -- `null` once a later
   * attempt succeeds. Distinguishes "index build failed" from "index has
   * never been built" for `useArchiveIndexHealth`'s empty-state taxonomy;
   * a runtime-only signal, not persisted across restarts. */
  archiveIndexError: string | null;
}

export interface ProjectSliceActions {
  /** Re-entrancy-safe entry point: a call while one is already in flight
   * awaits that SAME run instead of starting a second, racing one (see
   * `initializeAppPromise`'s own doc comment). Always call this, never
   * `runInitializeApp` directly. */
  initializeApp: () => Promise<void>;
  /** The actual initialization body. Not re-entrancy-safe on its own --
   * only `initializeApp` should ever call this. */
  runInitializeApp: () => Promise<void>;
  /** Resolve the Claude folder (saved/default path) and scan — the original
   * startup path, run once the provider-discovery prompt has been resolved. */
  runStartupScan: () => Promise<void>;
  /** Screen 1's single first-run action (spec §5/§6): always runs full
   * discovery across every supported provider. */
  completeProviderDiscoveryOnboarding: () => Promise<void>;
  /** Persists the user's archive-sync choice and closes the prompt.
   * Dismissing the dialog (Escape/backdrop) should call this with
   * `"manual"` -- the agreed default when the user doesn't pick either
   * option explicitly. */
  completeArchiveSyncPrompt: (mode: "auto" | "manual") => Promise<void>;
  /** Fire-and-forget: runs `sync_grid_index` (idempotent, read-only to
   * Claude's own files). Errors are logged, not thrown -- callers that
   * want to surface a result (e.g. a "Sync now" Settings button) read the
   * returned summary/null themselves. */
  syncArchiveIndex: () => Promise<BackfillSummary | null>;
  /** The mandatory first-run index -- see `firstIndexProgress`'s doc
   * comment. Subscribes to `"first-index-progress"` for the duration of
   * the call and always clears `firstIndexProgress` back to `null` on
   * settle (success, error, or cancellation). Never throws -- errors are
   * logged and `null` is returned, matching `syncArchiveIndex`'s own
   * fire-and-forget-safe convention, since a failed first index must not
   * block the rest of app startup. */
  runFirstIndex: () => Promise<BackfillSummary | null>;
  /** Signals an in-progress `runFirstIndex` call to stop before its next
   * provider phase. No-op if no first index is running. */
  cancelFirstIndex: () => void;
  /** Runs the mandatory first index if `archive_db` has never been
   * populated, otherwise respects `archiveSyncMode` for a background
   * refresh. Called from both `initializeApp` (returning launches) and
   * `completeProviderDiscoveryOnboarding` (so a fresh install's first
   * index happens in the SAME session as onboarding, not only on a
   * hypothetical next launch). */
  ensureFirstIndexIfNeeded: () => Promise<void>;
  /** Empty-state taxonomy's "index build failed" retry action.
   * Re-checks `archive_db`'s real state and re-runs
   * whichever of `runFirstIndex`/`syncArchiveIndex` is appropriate --
   * never assumes which one failed last, since the failure could be stale
   * relative to the archive's current state. */
  retryArchiveIndex: () => Promise<void>;
  discoverProviders: () => Promise<void>;
  scanProjects: () => Promise<void>;
  refreshAllConversations: () => Promise<void>;
  selectProject: (project: ClaudeProject) => Promise<void>;
  loadMoreSessions: () => Promise<void>;
  clearProjectSelection: () => void;
  setClaudePath: (path: string) => Promise<void>;
  setError: (error: AppError | null) => void;
  setSelectedSession: (session: ClaudeSession | null) => void;
  setSessions: (sessions: ClaudeSession[]) => void;
  getGroupedProjects: () => WorktreeGroupingResult;
  getDirectoryGroupedProjects: () => DirectoryGroupingResult;
  getEffectiveGroupingMode: () => GroupingMode;
}

export type ProjectSlice = ProjectSliceState & ProjectSliceActions;

// ============================================================================
// Initial State
// ============================================================================

const initialProjectState: ProjectSliceState = {
  claudePath: "",
  projects: [],
  selectedProject: null,
  sessions: [],
  sessionsTotal: 0,
  sessionsOffset: 0,
  hasMoreSessions: false,
  selectedSession: null,
  isLoading: false,
  isLoadingProjects: false,
  isLoadingSessions: false,
  isLoadingMoreSessions: false,
  isRefreshingAllConversations: false,
  error: null,
  showProviderDiscoveryOnboarding: false,
  showArchiveSyncPrompt: false,
  firstIndexProgress: null,
  archiveIndexError: null,
};

const SESSION_PAGE_LIMIT = 250;

function classifyStartupError(error: unknown): AppError {
  const errorMessage = error instanceof Error ? error.message : String(error);
  let errorType = AppErrorType.UNKNOWN;
  let message = errorMessage;

  if (errorMessage.includes("CLAUDE_FOLDER_NOT_FOUND:")) {
    errorType = AppErrorType.CLAUDE_FOLDER_NOT_FOUND;
    message = errorMessage.split(":")[1] || errorMessage;
  } else if (errorMessage.includes("PERMISSION_DENIED:")) {
    errorType = AppErrorType.PERMISSION_DENIED;
    message = errorMessage.split(":")[1] || errorMessage;
  } else if (errorMessage.includes("Tauri API")) {
    errorType = AppErrorType.TAURI_NOT_AVAILABLE;
  }

  return { type: errorType, message };
}

// Fire-and-forget: records a completed (non-cancelled) index/sync run for
// the diagnostics export's "index duration, provider coverage, parser
// failures" measure. A cancelled run
// isn't a completion, so callers skip this when `summary.cancelled`.
// `providerCount` comes from a fresh `getArchiveDbStatus()` read rather
// than the summary itself (which has no such field) -- the real,
// already-computed count of providers with any ingested data.
const recordIndexCompletedDiagnostics = async (summary: BackfillSummary): Promise<void> => {
  let providerCount = 0;
  try {
    providerCount = (await getArchiveDbStatus()).providerCount;
  } catch (error) {
    console.error("Failed to read archive_db status for diagnostics:", error);
  }
  await recordDiagnosticsEvent({
    kind: "indexCompleted",
    durationMs: summary.durationMs ?? 0,
    providerCount,
    sessionCount: summary.sessionsIngested + summary.sessionsSkippedUnchanged,
    parserFailures: summary.parserFailures ?? 0,
  });
};

const dedupeSessionsById = (sessions: ClaudeSession[]): ClaudeSession[] => {
  const seen = new Set<string>();
  const deduped: ClaudeSession[] = [];
  for (const session of sessions) {
    if (seen.has(session.session_id)) {
      continue;
    }
    seen.add(session.session_id);
    deduped.push(session);
  }
  return deduped;
};

// ============================================================================
// Helper
// ============================================================================

const isTauriAvailable = () => {
  try {
    return typeof window !== "undefined" && typeof api === "function";
  } catch {
    return false;
  }
};

// Module-scoped (not store state) re-entrancy guard for `initializeApp`.
// Found via live testing: React 18
// StrictMode double-invokes the mount effect that calls `initializeApp`
// in dev, and nothing previously stopped two concurrent calls from
// racing. Before the mandatory first index, that race was harmless
// (`scanProjects`/`syncArchiveIndex` are idempotent to call twice). It
// stopped being harmless once one call can be genuinely long-running:
// the second call's own `get_archive_db_status` check could see rows the
// first call had already written and wrongly conclude "already indexed,"
// then immediately flip `isLoading` back to `false` in its own `finally`
// while the first call's real indexing kept running underneath the
// now-interactive UI. A second call now awaits the SAME in-flight
// promise instead of starting its own independent run.
let initializeAppPromise: Promise<void> | null = null;

const projectTimestamp = (project: ClaudeProject): number | null => {
  const timestamp = Date.parse(project.last_modified);
  return Number.isNaN(timestamp) ? null : timestamp;
};

const sortProjectsByLastModified = (projects: ClaudeProject[]): ClaudeProject[] =>
  [...projects].sort((a, b) => {
    const aTimestamp = projectTimestamp(a);
    const bTimestamp = projectTimestamp(b);
    if (aTimestamp != null && bTimestamp != null) {
      return bTimestamp - aTimestamp;
    }
    if (aTimestamp != null) {
      return -1;
    }
    if (bTimestamp != null) {
      return 1;
    }
    return b.last_modified.localeCompare(a.last_modified);
  });

const withProvider = (
  projects: ClaudeProject[],
  provider: ProviderId,
): ClaudeProject[] =>
  projects.map((project) => ({
    ...project,
    provider: project.provider ?? provider,
  }));

const isSameProject = (
  project: ClaudeProject,
  selectedProject: ClaudeProject,
): boolean =>
  project.path === selectedProject.path &&
  getProviderId(project.provider) === getProviderId(selectedProject.provider);

const isSameSession = (
  session: ClaudeSession,
  selectedSession: ClaudeSession,
): boolean =>
  session.file_path === selectedSession.file_path ||
  session.session_id === selectedSession.session_id ||
  session.actual_session_id === selectedSession.actual_session_id;

const scanProviderProjects = async ({
  provider,
  claudePath,
  customClaudePaths,
  settings,
}: {
  provider: ProviderId;
  claudePath: string;
  customClaudePaths: UserSettings["customClaudePaths"];
  settings: UserSettings | undefined;
}): Promise<ClaudeProject[]> => {
  const hasCustomPaths = customClaudePaths != null && customClaudePaths.length > 0;
  const wslEnabled = settings?.wsl?.enabled ?? false;

  if (provider === DEFAULT_PROVIDER_ID && !hasCustomPaths && !wslEnabled) {
    if (!claudePath) {
      return [];
    }
    const projects = await api<ClaudeProject[]>("scan_projects", {
      claudePath,
    });
    return withProvider(projects, provider);
  }

  const projects = await api<ClaudeProject[]>("scan_all_projects", {
    ...(claudePath && { claudePath }),
    activeProviders: [provider],
    ...(provider === DEFAULT_PROVIDER_ID && hasCustomPaths
      ? { customClaudePaths }
      : {}),
    ...(provider === DEFAULT_PROVIDER_ID
      ? {
          wslEnabled,
          wslExcludedDistros: settings?.wsl?.excludedDistros ?? [],
        }
      : {}),
  });
  return withProvider(projects, provider);
};

// ============================================================================
// CLAUDE_CONFIG_DIR Auto-detection
// ============================================================================

/** Auto-register CLAUDE_CONFIG_DIR as a custom directory if not already present. */
async function autoRegisterConfigDir(get: () => FullAppStore): Promise<void> {
  try {
    if (get().isServerReadOnly) return;

    const detected = await api<string | null>("detect_claude_config_dir");
    if (!detected) return;

    const normalize = (p: string) => p.replace(/[\\/]+$/, "");
    const normalizedDetected = normalize(detected);
    const existing = get().userMetadata?.settings?.customClaudePaths ?? [];
    const alreadyRegistered = existing.some((cp) => normalize(cp.path) === normalizedDetected);
    if (alreadyRegistered) return;

    await get().addCustomClaudePath(detected, "CLAUDE_CONFIG_DIR");
  } catch {
    if (import.meta.env.DEV) {
      console.warn("[autoRegisterConfigDir] Failed to detect CLAUDE_CONFIG_DIR");
    }
  }
}

// ============================================================================
// Slice Creator
// ============================================================================

export const createProjectSlice: StateCreator<
  FullAppStore,
  [],
  [],
  ProjectSlice
> = (set, get) => ({
  ...initialProjectState,

  initializeApp: async () => {
    if (initializeAppPromise) {
      return initializeAppPromise;
    }
    initializeAppPromise = get()
      .runInitializeApp()
      .finally(() => {
        initializeAppPromise = null;
      });
    return initializeAppPromise;
  },

  runInitializeApp: async () => {
    set({ isLoading: true, error: null });
    try {
      await get().loadServerConfig();

      if (!isTauriAvailable()) {
        throw new Error(
          "Tauri API를 사용할 수 없습니다. 데스크톱 앱에서 실행해주세요."
        );
      }

      // Load metadata before resolving the Claude path so an explicit provider
      // discovery choice can restore non-Claude projects on startup without
      // running the broad provider detector again.
      await get().loadMetadata();
      const savedProviderIds = normalizeProviderIds(
        get().userMetadata?.settings?.discoveredProviderIds ?? []
      );
      if (savedProviderIds.length > 0) {
        get().setActiveProviders(savedProviderIds);
      }

      // First launch: gate on a one-tap prompt instead of silently deciding
      // for the user whether every supported provider gets scanned. Kept
      // separate from the Claude-folder resolution below so a user who
      // skips still gets the exact prior single-provider behavior.
      const hasSeenProviderDiscoveryPrompt =
        get().userMetadata?.settings?.hasSeenProviderDiscoveryPrompt ?? false;
      if (!hasSeenProviderDiscoveryPrompt) {
        set({ showProviderDiscoveryOnboarding: true });
        return;
      }

      // Non-blocking archive-sync preference (found via live testing: no
      // frontend path ever called `sync_grid_index`, so `archive_db` was
      // never populated for a real install). Asked once, separately from
      // provider discovery, since History/Search work fine without it --
      // this must never gate the rest of the app the way discovery does.
      // This preference governs only LATER automatic refreshes -- see the
      // mandatory-first-index block below for why.
      const archiveSyncSettings = get().userMetadata?.settings;
      if (!(archiveSyncSettings?.hasSeenArchiveSyncPrompt ?? false)) {
        set({ showArchiveSyncPrompt: true });
      }

      await get().ensureFirstIndexIfNeeded();

      await get().runStartupScan();
    } catch (error) {
      console.error("Failed to initialize app:", error);
      set({ error: classifyStartupError(error) });
    } finally {
      set({ isLoading: false });
    }
  },

  runStartupScan: async () => {
    const savedProviderIds = normalizeProviderIds(
      get().userMetadata?.settings?.discoveredProviderIds ?? []
    );
    const hasSavedNonClaudeProviders = savedProviderIds.some(
      (provider) => provider !== DEFAULT_PROVIDER_ID
    );
    const savedSettings = get().userMetadata?.settings;
    const hasCustomClaudePaths =
      (savedSettings?.customClaudePaths?.length ?? 0) > 0;
    const hasWslSource = savedSettings?.wsl?.enabled ?? false;
    const hasConfiguredScanSource =
      hasSavedNonClaudeProviders || hasCustomClaudePaths || hasWslSource;

    // Try to load saved settings first
    try {
      const store = await storageAdapter.load("settings.json", {
        autoSave: false,
        defaults: {},
      });
      const savedPath = await store.get<string>("claudePath");

      if (savedPath) {
        const isValid = await api<boolean>("validate_claude_folder", {
          path: savedPath,
        });
        if (isValid) {
          set({ claudePath: savedPath });
          await get().scanProjects();
          return;
        }
      }
    } catch {
      console.log("No saved settings found");
    }

    // Try the default Claude path. Provider discovery is intentionally not
    // part of this fallback: scanning every supported provider can touch
    // protected user folders before the user has asked to browse them.
    try {
      const claudePath = await api<string>("get_claude_folder_path");
      set({ claudePath });
      await get().scanProjects();
      return;
    } catch (claudeFolderError) {
      const claudeErrorMessage =
        claudeFolderError instanceof Error
          ? claudeFolderError.message
          : String(claudeFolderError);
      if (!claudeErrorMessage.includes("CLAUDE_FOLDER_NOT_FOUND:")) {
        throw claudeFolderError;
      }

      // A user who previously opted in to another provider (or configured a
      // custom Claude/WSL source) should not be forced through the Claude
      // folder picker on every launch.
      if (hasConfiguredScanSource) {
        await get().scanProjects();
        return;
      }

      throw claudeFolderError;
    }
  },

  // Screen 1's single action (spec §5/§6): always runs full discovery
  // across every supported provider — there is no separate "Claude only"
  // choice. If discovery finds nothing, discoverProviders' own fallback
  // (scanProjects) surfaces the existing Claude-folder-not-found path.
  completeProviderDiscoveryOnboarding: async () => {
    set({ showProviderDiscoveryOnboarding: false, isLoading: true, error: null });
    try {
      await get().updateUserSettings({ hasSeenProviderDiscoveryPrompt: true });
      await get().discoverProviders();
      // Same-session mandatory first index -- a fresh install must not
      // have to relaunch the app before Home/Insights have real data.
      await get().ensureFirstIndexIfNeeded();
    } catch (error) {
      console.error("Failed to complete provider discovery onboarding:", error);
      set({ error: classifyStartupError(error) });
    } finally {
      set({ isLoading: false });
    }
  },

  // Non-blocking, separate from provider discovery: persists the user's
  // archive-sync choice (or the agreed "manual" default if they dismissed
  // the dialog without picking) and closes the prompt. Doesn't touch
  // `isLoading`/`error` -- unlike onboarding, this never gates the rest
  // of the UI, so there's nothing here for a loading/error state to guard.
  completeArchiveSyncPrompt: async (mode) => {
    set({ showArchiveSyncPrompt: false });
    try {
      await get().updateUserSettings({
        archiveSyncMode: mode,
        hasSeenArchiveSyncPrompt: true,
      });
    } catch (error) {
      console.error("Failed to save archive sync preference:", error);
    }
    if (mode === "auto") {
      void get().syncArchiveIndex();
    }
  },

  syncArchiveIndex: async () => {
    try {
      const summary = await syncGridIndex();
      set({ archiveIndexError: null });
      if (!summary.cancelled) {
        void recordIndexCompletedDiagnostics(summary);
      }
      return summary;
    } catch (error) {
      console.error("Failed to sync Grid's local archive:", error);
      set({
        archiveIndexError: error instanceof Error ? error.message : String(error),
      });
      return null;
    }
  },

  runFirstIndex: async () => {
    let unlisten: (() => void) | null = null;
    try {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<FirstIndexProgressEvent>(
        "first-index-progress",
        (event) => {
          set({ firstIndexProgress: event.payload });
        }
      );
      const summary = await runFirstIndexCommand();
      set({ archiveIndexError: null });
      if (!summary.cancelled) {
        void recordIndexCompletedDiagnostics(summary);
      }
      return summary;
    } catch (error) {
      console.error("Failed to run the first-run index:", error);
      set({
        archiveIndexError: error instanceof Error ? error.message : String(error),
      });
      return null;
    } finally {
      unlisten?.();
      set({ firstIndexProgress: null });
    }
  },

  cancelFirstIndex: () => {
    void cancelFirstIndexCommand().catch((error) => {
      console.error("Failed to cancel the first-run index:", error);
    });
  },

  // The very first index is unconditional: without this, a user can
  // complete first-run exactly as asked -- scan -- and land on
  // a Home/Insights that quietly renders nothing because `archive_db`
  // was never populated, since the auto/manual preference previously
  // gated it. Detected from `archive_db`'s own real state (never-zero
  // session count), not a metadata flag, so this self-heals for any
  // earlier install that dismissed the archive-sync prompt with
  // "manual" and therefore never got a first index either.
  ensureFirstIndexIfNeeded: async () => {
    // Diagnostics "launch count"/"active days" fired here, not earlier
    // in `runInitializeApp`, because
    // this is the one choke point BOTH real entry paths share -- a normal
    // launch (`runInitializeApp`, after the onboarding gate has already
    // been answered) and a first-run launch (`completeProviderDiscoveryOnboarding`,
    // after the user answers the one-tap prompt). The onboarding GATE
    // screen itself must still do nothing at all until the user acts on
    // it, matching every other startup side effect gated the same way.
    void recordDiagnosticsEvent({ kind: "appLaunched" });

    let archiveDbNeverIndexed = false;
    try {
      const status = await getArchiveDbStatus();
      archiveDbNeverIndexed = status.sessionCount === 0;
    } catch (error) {
      console.error("Failed to read archive_db status:", error);
    }

    if (archiveDbNeverIndexed) {
      await get().runFirstIndex();
    } else if (get().userMetadata?.settings?.archiveSyncMode === "auto") {
      void get().syncArchiveIndex();
    }
  },

  retryArchiveIndex: async () => {
    let sessionCount = 0;
    try {
      sessionCount = (await getArchiveDbStatus()).sessionCount;
    } catch (error) {
      console.error("Failed to read archive_db status before retrying:", error);
    }
    if (sessionCount === 0) {
      await get().runFirstIndex();
    } else {
      await get().syncArchiveIndex();
    }
  },

  // Explicitly opt in to discovery of other providers and custom Claude
  // locations. This is the only path that calls the broad provider detector.
  discoverProviders: async () => {
    set({ error: null });
    const detected = await get().detectProviders();
    if (!detected) {
      // Detection failures are already surfaced by providerSlice. Continue
      // with the last persisted/detected provider state without overwriting
      // it with an empty discovery result.
      await get().scanProjects();
      return;
    }
    await autoRegisterConfigDir(get);
    const discoveredProviderIds = normalizeProviderIds(
      get().providers
        .filter((provider) => provider.is_available)
        .map((provider) => provider.id as ProviderId)
    );
    try {
      await get().updateUserSettings({ discoveredProviderIds });
    } catch (error) {
      // Provider discovery should still show the current result if metadata
      // persistence is unavailable; the next explicit discovery can retry it.
      console.error("Failed to persist discovered providers:", error);
      toast.error(i18n.t("common.provider.saveError"));
    }
    await get().scanProjects();
  },

  // Provider discovery is empty during first startup, so the initial scan is
  // limited to the default provider. Once the user explicitly discovers
  // providers, the detected IDs (or their persisted IDs after restart) become
  // the scan candidate list while `activeProviders` remains a client-side
  // filter. Provider scans are launched independently so a slow provider does
  // not block fast providers from appearing in the sidebar.
  scanProjects: async () => {
    const requestId = nextRequestId("scanProjects");
    const { claudePath, providers, activeProviders } = get();
    const customClaudePaths = get().userMetadata?.settings?.customClaudePaths;
    const hasCustomPaths = customClaudePaths != null && customClaudePaths.length > 0;
    const settings = get().userMetadata?.settings;
    const wslEnabled = settings?.wsl?.enabled ?? false;
    const detectedProviderIds = normalizeProviderIds(
      providers
        .filter((provider) => provider.is_available)
        .map((provider) => provider.id as ProviderId)
    );
    const persistedProviderIds = normalizeProviderIds(
      settings?.discoveredProviderIds ?? []
    );
    const requestedProviderIds = normalizeProviderIds(activeProviders);
    const providerSet = new Set<ProviderId>(
      detectedProviderIds.length > 0
        ? detectedProviderIds
        : persistedProviderIds.length > 0
          ? persistedProviderIds
        : requestedProviderIds.length > 0
          ? requestedProviderIds
          : [DEFAULT_PROVIDER_ID]
    );
    if (claudePath || hasCustomPaths || wslEnabled) {
      providerSet.add(DEFAULT_PROVIDER_ID);
    }
    const scanProviders = PROVIDER_IDS.filter((provider) => providerSet.has(provider));
    const hasNonClaudeProviders = scanProviders.some((provider) => provider !== DEFAULT_PROVIDER_ID);
    // Allow scanning when at least one source is available: a saved Claude path,
    // a custom Claude path, WSL, or any non-Claude provider detected on disk (#222).
    if (!claudePath && !hasCustomPaths && !wslEnabled && !hasNonClaudeProviders) return;

    set({ isLoadingProjects: true, error: null });
    try {
      const start = performance.now();
      const settings = get().userMetadata?.settings;
      const previouslyLoadedProjects = get().projects.filter((project) =>
        scanProviders.includes(getProviderId(project.provider))
      );
      const loadedProviders = new Set<ProviderId>();
      const projectsByProvider = new Map<ProviderId, ClaudeProject[]>();
      const providerErrors: string[] = [];

      const publishPartialResults = () => {
        const pendingPreviousProjects = previouslyLoadedProjects.filter(
          (project) => !loadedProviders.has(getProviderId(project.provider))
        );
        const loadedProjects = Array.from(projectsByProvider.values()).flat();
        set({
          projects: sortProjectsByLastModified([
            ...pendingPreviousProjects,
            ...loadedProjects,
          ]),
        });
      };

      await Promise.all(
        scanProviders.map(async (provider) => {
          try {
            const providerProjects = await scanProviderProjects({
              provider,
              claudePath,
              customClaudePaths,
              settings,
            });
            if (requestId !== getRequestId("scanProjects")) {
              return;
            }
            loadedProviders.add(provider);
            projectsByProvider.set(provider, providerProjects);
            publishPartialResults();
          } catch (scanError) {
            const message = scanError instanceof Error
              ? scanError.message
              : String(scanError);
            providerErrors.push(`${provider}: ${message}`);
            if (import.meta.env.DEV) {
              console.warn(`[Frontend] ${provider} project scan failed:`, scanError);
            }
          }
        })
      );

      const duration = performance.now() - start;
      const projects = sortProjectsByLastModified(
        Array.from(projectsByProvider.values()).flat()
      );
      if (import.meta.env.DEV) {
        console.log(
          `[Frontend] scanProjects: ${projects.length}개 프로젝트, ${duration.toFixed(1)}ms`
        );
      }
      if (requestId !== getRequestId("scanProjects")) {
        return;
      }
      set({ projects });
      if (projects.length === 0 && providerErrors.length > 0) {
        set({
          error: {
            type: AppErrorType.UNKNOWN,
            message: providerErrors.join("; "),
          },
        });
      }

      // Auto-enable worktree grouping if worktrees are detected
      // Only auto-enable if user has never explicitly set the preference
      const { userMetadata, updateUserSettings } = get();
      const worktreeGrouping = userMetadata?.settings?.worktreeGrouping ?? false;
      const userHasSet = userMetadata?.settings?.worktreeGroupingUserSet ?? false;
      if (!get().isServerReadOnly && !worktreeGrouping && !userHasSet && projects.length > 0) {
        const { groups } = detectWorktreeGroupsHybrid(projects);
        if (groups.length > 0) {
          if (requestId !== getRequestId("scanProjects")) {
            return;
          }
          // Worktrees detected - auto-enable grouping
          await updateUserSettings({ worktreeGrouping: true });
          if (requestId !== getRequestId("scanProjects")) {
            return;
          }
          if (import.meta.env.DEV) {
            console.log(
              `[Worktree] Auto-enabled grouping: ${groups.length} groups detected`
            );
          }
        }
      }
    } catch (error) {
      if (requestId !== getRequestId("scanProjects")) {
        return;
      }
      console.error("Failed to scan projects:", error);
      set({ error: { type: AppErrorType.UNKNOWN, message: String(error) } });
    } finally {
      if (requestId === getRequestId("scanProjects")) {
        set({ isLoadingProjects: false });
      }
    }
  },

  refreshAllConversations: async () => {
    if (get().isRefreshingAllConversations) {
      return;
    }

    const previouslySelectedProject = get().selectedProject;
    const previouslySelectedSession = get().selectedSession;

    set({ isRefreshingAllConversations: true, error: null });

    try {
      await get().scanProjects();

      const stateAfterScan = get();
      if (!previouslySelectedProject) {
        if (stateAfterScan.analytics.currentView === "analytics") {
          await stateAfterScan.loadGlobalStats();
        }
        return;
      }

      const refreshedProject = stateAfterScan.projects.find((project) =>
        isSameProject(project, previouslySelectedProject)
      );

      if (!refreshedProject) {
        get().clearProjectSelection();
        return;
      }

      await get().selectProject(refreshedProject);

      let refreshedSession: ClaudeSession | null = null;
      if (previouslySelectedSession) {
        refreshedSession = get().sessions.find((session) =>
          isSameSession(session, previouslySelectedSession)
        ) ?? null;

        if (refreshedSession) {
          await get().selectSession(refreshedSession);
        } else {
          set({
            selectedSession: null,
            messages: [],
            pagination: { ...INITIAL_PAGINATION },
            isLoadingMessages: false,
            subagentSessions: [],
            parentSessionStack: [],
          });
          get().clearSessionSearch();
          get().clearTokenStats();
          get().clearTargetMessage();
        }
      }

      const refreshedState = get();
      if (refreshedState.analytics.currentView === "analytics") {
        const projectSummary = await refreshedState.loadProjectStatsSummary(
          refreshedProject.path
        );
        refreshedState.setAnalyticsProjectSummary(projectSummary);
        if (refreshedSession) {
          const sessionComparison = await refreshedState.loadSessionComparison(
            refreshedSession.actual_session_id,
            refreshedProject.path
          );
          refreshedState.setAnalyticsSessionComparison(sessionComparison);
        } else {
          refreshedState.setAnalyticsSessionComparison(null);
        }
      }
    } catch (error) {
      console.error("Failed to refresh all conversations:", error);
      const message = error instanceof Error ? error.message : String(error);
      toast.error(`Failed to refresh conversations: ${message}`);
      get().setError({
        type: AppErrorType.UNKNOWN,
        message,
      });
    } finally {
      set({ isRefreshingAllConversations: false });
    }
  },

  selectProject: async (project: ClaudeProject) => {
    const requestId = nextRequestId("selectProject");
    // Selection is scoped to a single project's session list; switching
    // projects abandons any in-progress multi-selection.
    get().exitSessionSelectionMode();
    set({
      selectedProject: project,
      sessions: [],
      sessionsTotal: project.session_count,
      sessionsOffset: 0,
      hasMoreSessions: false,
      selectedSession: null,
      isLoadingSessions: true,
      isLoadingMoreSessions: false,
    });
    try {
      const provider = project.provider ?? "claude";
      const page = await api<SessionPage>("load_provider_sessions_page", {
        provider,
        projectPath: project.path,
        excludeSidechain: get().excludeSidechain,
        offset: 0,
        limit: SESSION_PAGE_LIMIT,
      });

      if (requestId !== getRequestId("selectProject")) {
        return;
      }

      set({
        sessions: page.sessions,
        sessionsTotal: page.total,
        sessionsOffset: page.nextOffset,
        hasMoreSessions: page.hasMore,
      });

      // Update project's session_count to match actual loaded sessions
      // (scan_projects counts files, but load_sessions filters invalid ones)
      if (page.total !== project.session_count) {
        const projects = get().projects.map((p) =>
          p.path === project.path
            ? { ...p, session_count: page.total }
            : p
        );
        set({ projects });
      }
    } catch (error) {
      if (requestId !== getRequestId("selectProject")) {
        return;
      }
      console.error("Failed to load project sessions:", error);
      set({ error: { type: AppErrorType.UNKNOWN, message: String(error) } });
    } finally {
      if (requestId === getRequestId("selectProject")) {
        set({ isLoadingSessions: false });
      }
    }
  },

  loadMoreSessions: async () => {
    const {
      selectedProject,
      sessionsOffset,
      hasMoreSessions,
      isLoadingSessions,
      isLoadingMoreSessions,
    } = get();

    if (
      selectedProject == null ||
      !hasMoreSessions ||
      isLoadingSessions ||
      isLoadingMoreSessions
    ) {
      return;
    }

    const requestId = getRequestId("selectProject");
    set({ isLoadingMoreSessions: true });

    try {
      const page = await api<SessionPage>("load_provider_sessions_page", {
        provider: selectedProject.provider ?? "claude",
        projectPath: selectedProject.path,
        excludeSidechain: get().excludeSidechain,
        offset: sessionsOffset,
        limit: SESSION_PAGE_LIMIT,
      });

      if (requestId !== getRequestId("selectProject")) {
        return;
      }

      set({
        sessions: dedupeSessionsById([...get().sessions, ...page.sessions]),
        sessionsTotal: page.total,
        sessionsOffset: page.nextOffset,
        hasMoreSessions: page.hasMore,
      });
    } catch (error) {
      if (requestId !== getRequestId("selectProject")) {
        return;
      }
      console.error("Failed to load more project sessions:", error);
      set({ error: { type: AppErrorType.UNKNOWN, message: String(error) } });
    } finally {
      if (requestId === getRequestId("selectProject")) {
        set({ isLoadingMoreSessions: false });
      }
    }
  },

  clearProjectSelection: () => {
    nextRequestId("selectProject");

    set({
      selectedProject: null,
      selectedSession: null,
      sessions: [],
      sessionsTotal: 0,
      sessionsOffset: 0,
      hasMoreSessions: false,
      messages: [],
      pagination: { ...INITIAL_PAGINATION },
      isLoadingMessages: false,
      isLoadingSessions: false,
      isLoadingMoreSessions: false,
      subagentSessions: [],
      parentSessionStack: [],
    });

    get().clearSessionSearch();
    get().clearTokenStats();
    get().resetAnalytics();
    get().setDateFilter({ start: null, end: null });
    get().clearTargetMessage();
    get().exitSessionSelectionMode();
  },

  setClaudePath: async (path: string) => {
    set({ claudePath: path });

    try {
      const store = await storageAdapter.load("settings.json", {
        autoSave: false,
        defaults: {},
      });
      await store.set("claudePath", path);
      await store.save();
    } catch (error) {
      console.error("Failed to save claude path:", error);
    }
  },

  setError: (error: AppError | null) => {
    set({ error });
  },

  setSelectedSession: (session: ClaudeSession | null) => {
    set({ selectedSession: session });
  },

  setSessions: (sessions: ClaudeSession[]) => {
    set({
      sessions,
      sessionsTotal: sessions.length,
      sessionsOffset: sessions.length,
      hasMoreSessions: false,
    });
  },

  getGroupedProjects: () => {
    const { projects, userMetadata, isProjectHidden } = get();
    const settings = userMetadata?.settings;

    // Determine effective grouping mode (same logic as getEffectiveGroupingMode)
    const effectiveMode = settings?.groupingMode ?? (settings?.worktreeGrouping ? "worktree" : "none");

    // Filter out hidden projects first (use actual_path for pattern matching)
    const visibleProjects = projects.filter((p) => !isProjectHidden(p.actual_path));

    // Only group when worktree mode is active
    if (effectiveMode !== "worktree") {
      // When worktree grouping is disabled, return all visible projects as ungrouped
      return { groups: [], ungrouped: visibleProjects };
    }

    // Use hybrid detection: git-based (100% accurate) + heuristic fallback
    const result = detectWorktreeGroupsHybrid(visibleProjects);

    // Filter hidden children from worktree groups
    const filtered = result.groups.map((group) => ({
      ...group,
      children: group.children.filter((child) => !isProjectHidden(child.actual_path)),
    }));

    // Keep groups with visible children; rescue orphaned parents to ungrouped
    // (only if the parent itself is not hidden)
    result.groups = filtered.filter((group) => group.children.length > 0);
    const orphanedParents = filtered
      .filter((group) => group.children.length === 0)
      .map((group) => group.parent)
      .filter((parent) => !isProjectHidden(parent.actual_path));
    result.ungrouped = [...result.ungrouped, ...orphanedParents];

    return result;
  },

  getDirectoryGroupedProjects: () => {
    const { projects, isProjectHidden } = get();

    // Filter out hidden projects first (use actual_path for pattern matching)
    const visibleProjects = projects.filter((p) => !isProjectHidden(p.actual_path));

    return groupProjectsByDirectory(visibleProjects);
  },

  getEffectiveGroupingMode: (): GroupingMode => {
    const { userMetadata } = get();
    const settings = userMetadata?.settings;

    // If explicit groupingMode is set, use it
    if (settings?.groupingMode) {
      return settings.groupingMode;
    }

    // Legacy: if worktreeGrouping is true, use "worktree" mode
    if (settings?.worktreeGrouping) {
      return "worktree";
    }

    return "none";
  },
});
