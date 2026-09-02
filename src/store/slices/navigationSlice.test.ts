import { describe, expect, it, vi } from "vitest";
import { createNavigationSlice } from "./navigationSlice";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";

vi.mock("@/services/diagnosticsApi", () => ({
  recordDiagnosticsEvent: vi.fn().mockResolvedValue(undefined),
}));

describe("navigationSlice", () => {
  it("sets the target message and highlight flag", () => {
    const set = vi.fn();
    const get = () => ({ selectedSession: null });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    slice.navigateToMessage("message-123");

    expect(set).toHaveBeenCalledWith({
      targetMessageUuid: "message-123",
      shouldHighlightTarget: true,
    });
  });

  it("clears the target message and highlight flag", () => {
    const set = vi.fn();
    const get = () => ({ selectedSession: null });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    slice.clearTargetMessage();

    expect(set).toHaveBeenCalledWith({
      targetMessageUuid: null,
      shouldHighlightTarget: false,
    });
  });

  it("defaults to the home surface", () => {
    const set = vi.fn();
    const get = () => ({ selectedSession: null });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    expect(slice.primarySurface).toBe("home");
  });

  it("sets the primary surface and records a diagnostics visit", () => {
    vi.mocked(recordDiagnosticsEvent).mockClear();
    const set = vi.fn();
    const get = () => ({ selectedSession: null, primarySurface: "home" });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    slice.setPrimarySurface("insights");

    expect(set).toHaveBeenCalledWith({ primarySurface: "insights" });
    expect(recordDiagnosticsEvent).toHaveBeenCalledWith({
      kind: "surfaceVisited",
      surface: "insights",
    });
  });

  it("does not record a diagnostics visit when re-selecting the already-active surface", () => {
    vi.mocked(recordDiagnosticsEvent).mockClear();
    const set = vi.fn();
    const get = () => ({ selectedSession: null, primarySurface: "insights" });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    slice.setPrimarySurface("insights");

    expect(set).toHaveBeenCalledWith({ primarySurface: "insights" });
    expect(recordDiagnosticsEvent).not.toHaveBeenCalled();
  });

  it("defaults to the conversation session tab", () => {
    const set = vi.fn();
    const get = () => ({ selectedSession: null });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    expect(slice.sessionTab).toBe("conversation");
  });

  it("sets the session tab", () => {
    const set = vi.fn();
    const get = () => ({ selectedSession: null });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    slice.setSessionTab("tools");

    expect(set).toHaveBeenCalledWith({ sessionTab: "tools" });
  });

  it("defaults to the questions insights tab", () => {
    const set = vi.fn();
    const get = () => ({ selectedSession: null });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    expect(slice.insightsTab).toBe("questions");
  });

  it("sets the insights tab", () => {
    const set = vi.fn();
    const get = () => ({ selectedSession: null });
    const slice = createNavigationSlice(set as never, get as never, {} as never);

    slice.setInsightsTab("problems");

    expect(set).toHaveBeenCalledWith({ insightsTab: "problems" });
  });
});
