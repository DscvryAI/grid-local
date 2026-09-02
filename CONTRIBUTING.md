# Contributing to Grid Local

Thanks for considering a contribution.

## Development setup

Requires `pnpm` and a Rust toolchain (stable).

```bash
pnpm install
pnpm exec tauri dev      # run the desktop app in dev mode
```

## Before opening a pull request

Run the full check suite locally:

```bash
pnpm lint                                  # ESLint
pnpm tsc --build                           # TypeScript
pnpm exec vitest run                       # frontend tests
cd src-tauri
cargo clippy --all-targets -- -D warnings  # Rust lints
cargo test --lib                           # backend tests
```

All of the above should pass cleanly. If a change touches the Rust
backend, also run a real build to confirm packaging isn't broken:

```bash
pnpm tauri:build
```

## Guidelines

- Keep pull requests focused — one change per PR is easier to review
  and easier to revert if something goes wrong.
- Add tests for new behavior and bug fixes.
- Comments should explain non-obvious *why*, not restate what the code
  already says. Don't reference issue numbers, review rounds, or dates
  in code comments — that context belongs in the PR description and
  commit history, not in the source.
- Grid Local is local-first and read-only with respect to other tools'
  data by design (see `README.md`). Changes that would have it write
  to, modify, or transmit data from other tools' storage need a strong
  justification and explicit discussion first.

## Reporting bugs

Open a GitHub issue with steps to reproduce, your OS, and the relevant
log output (see `SECURITY.md` if the bug involves a security or
privacy concern instead).
