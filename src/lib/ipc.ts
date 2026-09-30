// Typed wrappers around Tauri commands. Types come from Rust via ts-rs
// (src/bindings); never hand-write a type that crosses the IPC boundary.
import { invoke } from "@tauri-apps/api/core";
import type { AppInfo } from "../bindings/AppInfo";
import type { Drive } from "../bindings/Drive";
import type { ProtectedLocation } from "../bindings/ProtectedLocation";

// Fallible commands reject with a `CommandError` ({ kind, message }).
export const ipc = {
  appInfo: () => invoke<AppInfo>("app_info"),
  protectedLocations: () => invoke<ProtectedLocation[]>("protected_locations"),
  listDrives: () => invoke<Drive[]>("list_drives"),
};

/** Turn whatever a rejected command produced into text a person can read. */
export function describeError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (err && typeof err === "object" && "message" in err) return String(err.message);
  return "An unexpected error occurred.";
}
