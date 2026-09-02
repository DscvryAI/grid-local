/**
 * Settings API Service
 *
 * Thin wrappers for the Settings rework (spec §31) that don't belong to
 * a more specific existing service file.
 */

import { api } from "@/services/api";

/** `~/.grid-local` -- Grid's own metadata/archive folder. */
export async function getMetadataFolderPath(): Promise<string> {
  return api<string>("get_metadata_folder_path");
}

/** Third-party attribution notices for About > Licences. */
export async function getThirdPartyNotices(): Promise<string> {
  return api<string>("get_third_party_notices");
}
