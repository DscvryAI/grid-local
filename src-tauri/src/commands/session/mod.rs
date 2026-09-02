//! Session commands module
//!
//! This module contains all session-related Tauri commands organized into submodules:
//! - `load`: Session and message loading functions
//! - `search`: Message search functions
//!
//! Grid Local never writes into a provider's own session storage or invokes
//! external processes on its behalf (read-only guarantee, spec §22/§23/§46) —
//! native session rename/delete, file-restore, and terminal-spawn ("Resume
//! in terminal") commands used to live here and have all been removed. The
//! only resume-related affordance left is "copy resume command" to the
//! clipboard (see `useSessionEditing.ts`/`SessionCopyMenu.tsx`), which has
//! no backend command at all.

mod chain;
mod load;
mod search;

// Re-export all commands
pub use chain::{resolve_session_chain, superseded_chain_paths};
// Crate-internal only (`pub(crate)` in `chain.rs`) -- shared-snapshot
// plumbing for `archive_db::ingest`'s hot loop, not part of this module's
// public command surface.
pub(crate) use chain::{project_snapshot, ProjectSnapshot};
pub use load::*;
pub use search::*;
