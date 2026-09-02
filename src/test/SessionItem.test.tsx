/**
 * @fileoverview Tests for SessionItem component
 * Tests for session display and read-only user interactions
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SessionItem } from "../components/SessionItem";
import type { ClaudeSession } from "../types";

// Mock react-i18next
vi.mock("react-i18next", async () => {
  const actual = await vi.importActual<typeof import("react-i18next")>(
    "react-i18next"
  );

  return {
    ...actual,
    useTranslation: () => ({
      t: (key: string, fallback?: string) => fallback || key,
    }),
  };
});

// Mock dropdown menu
vi.mock("@/components/ui/dropdown-menu", () => ({
  DropdownMenu: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="dropdown-menu">{children}</div>
  ),
  DropdownMenuTrigger: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="dropdown-trigger">{children}</div>
  ),
  DropdownMenuContent: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="dropdown-content">{children}</div>
  ),
  DropdownMenuItem: ({
    children,
    onClick,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
  }) => (
    <button data-testid="dropdown-item" onClick={onClick}>
      {children}
    </button>
  ),
  DropdownMenuSeparator: () => <hr data-testid="dropdown-separator" />,
}));

// Helper to create mock session
function createMockSession(overrides: Partial<ClaudeSession> = {}): ClaudeSession {
  return {
    session_id: overrides.session_id ?? "test-session-id",
    actual_session_id: overrides.actual_session_id ?? "actual-session-id",
    file_path: overrides.file_path ?? "/path/to/session.jsonl",
    project_name: overrides.project_name ?? "test-project",
    message_count: overrides.message_count ?? 10,
    first_message_time: overrides.first_message_time ?? "2024-01-01T00:00:00Z",
    last_message_time: overrides.last_message_time ?? "2024-01-01T12:00:00Z",
    last_modified: overrides.last_modified ?? "2024-01-01T12:00:00Z",
    has_tool_use: overrides.has_tool_use ?? false,
    has_errors: overrides.has_errors ?? false,
    summary: overrides.summary,
    provider: overrides.provider,
  };
}

describe("SessionItem", () => {
  const mockFormatTimeAgo = vi.fn(() => "1 hour ago");
  const mockOnSelect = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe("Rendering", () => {
    it("should render session with summary", () => {
      const session = createMockSession({ summary: "Test Session Summary" });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      expect(screen.getByText("Test Session Summary")).toBeInTheDocument();
    });

    it("should render 'No summary' when summary is undefined", () => {
      const session = createMockSession({ summary: undefined });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      expect(screen.getByText("No summary")).toBeInTheDocument();
    });

    it("should display message count", () => {
      const session = createMockSession({ message_count: 42 });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      expect(screen.getByText("42")).toBeInTheDocument();
    });

    it("should display formatted time", () => {
      const session = createMockSession();

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      expect(screen.getByText("1 hour ago")).toBeInTheDocument();
      expect(mockFormatTimeAgo).toHaveBeenCalledWith(session.last_modified);
    });

    it("should show archived icon for codex archived sessions", () => {
      const session = createMockSession({
        provider: "codex",
        file_path: "/Users/test/.codex/archived_sessions/rollout-2026.jsonl",
      });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      expect(screen.getByLabelText("Archived session")).toBeInTheDocument();
    });

    it("should show archived badge for codex archived sessions on Windows-style paths", () => {
      const session = createMockSession({
        provider: "codex",
        file_path:
          "C:\\\\Users\\\\test\\\\.codex\\\\archived_sessions\\\\rollout-2026.jsonl",
      });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      expect(screen.getByLabelText("Archived session")).toBeInTheDocument();
    });
    it("should not show archived icon for non-archived sessions", () => {
      const session = createMockSession({
        provider: "codex",
        file_path: "/Users/test/.codex/sessions/2026/02/21/rollout-2026.jsonl",
      });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      expect(screen.queryByLabelText("Archived session")).not.toBeInTheDocument();
    });

    it("should apply selected styles when isSelected is true", () => {
      const session = createMockSession();

      const { container } = render(
        <SessionItem
          session={session}
          isSelected={true}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      // Check for selected class (bg-accent/15)
      const sessionDiv = container.firstChild as HTMLElement;
      expect(sessionDiv.className).toContain("bg-accent/15");
    });
  });

  describe("Click behavior", () => {
    it("should call onSelect when clicked and not selected", async () => {
      const session = createMockSession();

      const { container } = render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      const sessionDiv = container.firstChild as HTMLElement;
      fireEvent.click(sessionDiv);

      expect(mockOnSelect).toHaveBeenCalledTimes(1);
    });

    it("should not call onSelect when already selected", () => {
      const session = createMockSession();

      const { container } = render(
        <SessionItem
          session={session}
          isSelected={true}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      const sessionDiv = container.firstChild as HTMLElement;
      fireEvent.click(sessionDiv);

      expect(mockOnSelect).not.toHaveBeenCalled();
    });
  });

  describe("Tool use and error indicators", () => {
    it("should show tool use indicator when has_tool_use is true", () => {
      const session = createMockSession({ has_tool_use: true });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      // Wrench icon should be present (lucide renders as svg)
      const container = screen.getByText(session.message_count.toString())
        .closest("div")?.parentElement;
      expect(container?.innerHTML).toContain("svg");
    });

    it("should show error indicator when has_errors is true", () => {
      const session = createMockSession({ has_errors: true });

      render(
        <SessionItem
          session={session}
          isSelected={false}
          onSelect={mockOnSelect}
          formatTimeAgo={mockFormatTimeAgo}
        />
      );

      // AlertTriangle icon should be present
      const container = screen.getByText(session.message_count.toString())
        .closest("div")?.parentElement;
      expect(container?.innerHTML).toContain("svg");
    });
  });
});
