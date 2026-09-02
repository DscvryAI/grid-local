/**
 * Platform detection utilities.
 *
 * Uses the presence of `__TAURI_INTERNALS__` on the global window to
 * detect the Tauri desktop shell.
 */

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

/** True when the current platform is macOS. */
export const isMacOS = (): boolean =>
  typeof navigator !== "undefined" && /mac/i.test(navigator.userAgent);

/** True when the current platform is Windows. */
export const isWindows = (): boolean =>
  typeof navigator !== "undefined" && /windows/i.test(navigator.userAgent);

/** True when the action modifier key is held (Cmd on macOS, Ctrl elsewhere). */
export const isActionModifier = (e: { metaKey: boolean; ctrlKey: boolean }): boolean =>
  isMacOS() ? e.metaKey : e.ctrlKey;

/** True when running inside the Tauri desktop shell. */
export const isTauri = (): boolean =>
  typeof window !== "undefined" && window.__TAURI_INTERNALS__ != null;

export const EXTERNAL_OPEN_HELPER_ATTRIBUTE = "data-external-open-helper";

/**
 * Open a URL in the system default browser.
 *
 * In Tauri mode, uses `@tauri-apps/plugin-opener` to open links externally.
 * Otherwise falls back to a secure anchor click (e.g. dev/test harnesses
 * running the SPA in a bare browser tab).
 */
export async function openExternalUrl(url: string): Promise<void> {
  const normalized = url.trim();
  if (!/^https?:\/\//i.test(normalized) && !/^mailto:/i.test(normalized)) {
    throw new Error(`Unsupported URL scheme: ${normalized}`);
  }

  if (isTauri()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(normalized);
  } else {
    const root = document.body ?? document.documentElement;
    if (!root) {
      throw new Error("Document root unavailable");
    }

    const link = document.createElement("a");
    link.href = normalized;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.style.display = "none";
    link.setAttribute(EXTERNAL_OPEN_HELPER_ATTRIBUTE, "true");

    root.appendChild(link);
    try {
      link.click();
    } finally {
      root.removeChild(link);
    }
  }
}
