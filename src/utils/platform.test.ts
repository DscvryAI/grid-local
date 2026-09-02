import { beforeEach, describe, expect, it, vi } from "vitest";
import { EXTERNAL_OPEN_HELPER_ATTRIBUTE, openExternalUrl } from "./platform";

describe("openExternalUrl", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it("rejects unsupported URL schemes", async () => {
    await expect(openExternalUrl("javascript:alert(1)")).rejects.toThrow("Unsupported URL scheme");
  });

  it("opens web URLs through a helper anchor in web mode", async () => {
    const openSpy = vi.spyOn(window, "open");
    const clickSpy = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});

    await expect(openExternalUrl("https://example.com")).resolves.toBeUndefined();

    expect(openSpy).not.toHaveBeenCalled();
    expect(clickSpy).toHaveBeenCalledTimes(1);

    const helperLink = clickSpy.mock.instances[0] as HTMLAnchorElement;
    expect(helperLink.getAttribute("href")).toBe("https://example.com");
    expect(helperLink.target).toBe("_blank");
    expect(helperLink.rel).toBe("noopener noreferrer");
    expect(helperLink.getAttribute(EXTERNAL_OPEN_HELPER_ATTRIBUTE)).toBe("true");
    expect(document.querySelector(`[${EXTERNAL_OPEN_HELPER_ATTRIBUTE}]`)).toBeNull();
  });
});
