# Grid Local

**Your AI coding history, finally useful.**

Grid Local reads the AI coding sessions already stored on this computer
(Claude Code, Codex CLI, and other supported tools) and turns them into
searchable history, activity insights, and traceable evidence — locally,
read-only, and private.

Nothing leaves this machine. Grid never modifies your coding tools' data.

## Development

This app is built with Tauri 2 (Rust backend) + React 19 + TypeScript +
Vite. Requires `pnpm`.

```bash
pnpm install
pnpm exec tauri dev      # run the desktop app in dev mode
pnpm exec tauri build    # production build
pnpm lint                # ESLint
pnpm test                # vitest
```

Backend tests (from `src-tauri/`):

```bash
cargo test -- --test-threads=1
```

## Contributing

See `CONTRIBUTING.md` for development setup and pull request guidelines.

## Security

See `SECURITY.md` to report a vulnerability.

## Attribution

Grid Local began as a rebrand of the open-source
[Claude Code History Viewer](https://github.com/jhlee0409/claude-code-history-viewer).
See `THIRD_PARTY_NOTICES.md` for the required MIT notice.
