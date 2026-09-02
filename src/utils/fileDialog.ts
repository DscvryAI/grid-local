/**
 * Cross-platform file dialog utilities.
 *
 * In Tauri mode, delegates to `@tauri-apps/plugin-dialog`.
 * In web mode, uses browser-native Blob download and <input type="file">.
 *
 * `@/services/api` is imported statically, not dynamically: it has 27
 * other static importers across the app, so it can never actually be
 * code-split -- the previous `await import("@/services/api")` calls here
 * were pure indirection with zero bundling benefit.
 * `@tauri-apps/plugin-dialog` stays a genuine dynamic import below; it has
 * no other static importer.
 */

import { isTauri } from "@/utils/platform";
import { api } from "@/services/api";

interface SaveDialogOptions {
  filters?: { name: string; extensions: string[] }[];
  defaultPath?: string;
  mimeType?: string;
}

function uint8ArrayToBase64(data: Uint8Array): string {
  const CHUNK_SIZE = 0x8000;
  const chunks: string[] = [];
  for (let i = 0; i < data.length; i += CHUNK_SIZE) {
    chunks.push(String.fromCharCode(...data.subarray(i, i + CHUNK_SIZE)));
  }
  return btoa(chunks.join(""));
}

/**
 * Show a "Save file" dialog and write content.
 *
 * - Tauri: shows native save dialog, then writes the file via IPC (`write_text_file`).
 * - Web: triggers a browser download with the given content.
 *
 * Returns `true` if the save completed (web always returns true).
 */
export async function saveFileDialog(
  content: string,
  options?: SaveDialogOptions,
): Promise<boolean> {
  if (isTauri()) {
    const dialogModule = await import("@tauri-apps/plugin-dialog");
    const filePath = await dialogModule.save(options);
    if (!filePath) return false;

    await api("write_text_file", { path: filePath, content });
    return true;
  }

  // Web fallback: Blob download
  const filename = options?.defaultPath ?? "download.json";
  const blob = new Blob([content], { type: options?.mimeType ?? "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
  return true;
}

/**
 * Show a "Save file" dialog and write binary content (e.g., PNG image).
 *
 * - Tauri: shows native save dialog, then writes binary via IPC (`save_screenshot`).
 * - Web: triggers a browser download with the given blob.
 *
 * Returns `true` if the save completed (web always returns true).
 */
export async function saveBinaryFileDialog(
  data: Uint8Array,
  options?: SaveDialogOptions & { mimeType?: string },
): Promise<boolean> {
  try {
    if (isTauri()) {
      const dialogModule = await import("@tauri-apps/plugin-dialog");
      const filePath = await dialogModule.save(options);
      if (!filePath) return false;

      const base64Data = uint8ArrayToBase64(data);

      await api("save_screenshot", { path: filePath, data: base64Data });
      return true;
    }

    // Web fallback: Blob download
    const filename = options?.defaultPath ?? "download.png";
    const mimeType = options?.mimeType ?? "image/png";
    const blob = new Blob([data], { type: mimeType });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    return true;
  } catch {
    return false;
  }
}
