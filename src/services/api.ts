/**
 * Backend command adapter — delegates to `@tauri-apps/api/core` invoke().
 *
 * Usage:
 *   import { api } from "@/services/api";
 *   const result = await api<MyType>("command_name", { key: "value" });
 */

/**
 * Call a Tauri backend command.
 *
 * @param command Tauri command name
 * @param args    Optional arguments object
 * @returns       The deserialised response from the backend
 */
export async function api<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return args != null ? invoke<T>(command, args) : invoke<T>(command);
}
