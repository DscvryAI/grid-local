//! DTO for `archive_db::list_provider_tiers` (provider coverage UX).

use serde::{Deserialize, Serialize};

/// One ingested provider's coverage tier, as stored at ingest time (see
/// `archive_db::ingest::upsert_provider`'s own `tier` argument). Plain
/// `"A"`/`"B"`/(reserved) `"C"` -- the frontend is responsible for
/// translating this into user-facing wording, never surfacing the raw
/// letter itself (spec's own "no Tier A/B/C jargon exposed" ask).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderTier {
    pub provider_key: String,
    pub tier: String,
}
