# Project Operating Memory

At the start of every session:
1. Read `.claude/handoff.md`
2. Read `.claude/tasks.md`
3. Read `.claude/decisions.md`
4. Continue from the latest handoff unless instructed otherwise.

During work:
- Keep `.claude/tasks.md` updated.
- Record major decisions in `.claude/decisions.md`.

Before stopping, switching Claude accounts, compacting context, or when usage/limit seems close:
- Update `.claude/handoff.md`
- Update `.claude/tasks.md`
- Update `.claude/decisions.md`

The handoff must allow another Claude account to continue without asking the user to re-explain.

## Working Standard

- Preserve context.
- Be explicit about files changed.
- Be precise about commands run.
- Do not hide blockers.
- Always leave the next account with a clear next action.

## Commit conventions

- Do not add a `Co-Authored-By: Claude` (or similar AI-attribution)
  trailer to commits in this repository.
- Do not reference internal review documents, session dates, or
  ticket-style item numbers in code comments or commit messages.
  Comments should explain non-obvious technical rationale on its own
  terms, not narrate the development process that produced them.
