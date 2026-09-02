import { describe, expect, it } from "vitest";
import * as fs from "fs";
import * as path from "path";

/**
 * Grid Local is a read-only viewer over provider session data: it must
 * never write into a provider's own session storage and must never
 * spawn a process on the user's behalf as a batch/background action.
 * This is a regression test for a real, confirmed incident: a "Files"
 * tab shipped a `restore_file` command that wrote into project files,
 * and a batch "Resume in terminal" action shipped an
 * `open_resume_in_terminal` command that spawned shell processes —
 * both were reverted.
 *
 * Static source-text checks like this only catch a *reintroduction* of the
 * exact removed names; they are not a substitute for reviewing new commands
 * on their own merits. This test ties the read-only promise to the two
 * concrete commands that violated it, rather than only asserting config
 * presence the way most of this project's other security tests do.
 */

const libRsPath = path.join(__dirname, "../src/lib.rs");
const sessionModPath = path.join(__dirname, "../src/commands/session");

const BANNED_COMMANDS = [
  "restore_file",
  "open_resume_in_terminal",
  "get_recent_edits",
] as const;

describe("write surface: no native write/execute commands are registered", () => {
  const libRs = fs.readFileSync(libRsPath, "utf-8");

  it("does not register the removed restore/resume/recent-edits commands", () => {
    for (const command of BANNED_COMMANDS) {
      expect(libRs).not.toMatch(new RegExp(`\\b${command}\\b`));
    }
  });

  it("no longer has session command submodules dedicated to file writes or process spawning", () => {
    expect(fs.existsSync(path.join(sessionModPath, "edits.rs"))).toBe(false);
    expect(fs.existsSync(path.join(sessionModPath, "resume.rs"))).toBe(false);
  });
});
