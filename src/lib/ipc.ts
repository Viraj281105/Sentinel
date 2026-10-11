// Typed wrappers around Tauri commands. Types come from Rust via ts-rs
// (src/bindings); never hand-write a type that crosses the IPC boundary.
import { invoke } from "@tauri-apps/api/core";
import type { AiSettings } from "../bindings/AiSettings";
import type { AnomalyReport } from "../bindings/AnomalyReport";
import type { AskResponse } from "../bindings/AskResponse";
import type { AppInfo } from "../bindings/AppInfo";
import type { AuditEntry } from "../bindings/AuditEntry";
import type { AuditIntegrity } from "../bindings/AuditIntegrity";
import type { CleanupRunResponse } from "../bindings/CleanupRunResponse";
import type { DirListing } from "../bindings/DirListing";
import type { DependencyGraph } from "../bindings/DependencyGraph";
import type { DockerOverview } from "../bindings/DockerOverview";
import type { Drive } from "../bindings/Drive";
import type { DriveTrend } from "../bindings/DriveTrend";
import type { FolderHistory } from "../bindings/FolderHistory";
import type { GrowthHistory } from "../bindings/GrowthHistory";
import type { JvmAnalysis } from "../bindings/JvmAnalysis";
import type { LargeFileView } from "../bindings/LargeFileView";
import type { ManifestEntry } from "../bindings/ManifestEntry";
import type { PlanRunResponse } from "../bindings/PlanRunResponse";
import type { Preferences } from "../bindings/Preferences";
import type { PreviewResponse } from "../bindings/PreviewResponse";
import type { ProjectSearch } from "../bindings/ProjectSearch";
import type { ProtectedLocation } from "../bindings/ProtectedLocation";
import type { ProviderInfo } from "../bindings/ProviderInfo";
import type { QuarantineContents } from "../bindings/QuarantineContents";
import type { QueryResult } from "../bindings/QueryResult";
import type { RecoveryPlan } from "../bindings/RecoveryPlan";
import type { Risk } from "../bindings/Risk";
import type { RuntimeInventory } from "../bindings/RuntimeInventory";
import type { SoftwareInventory } from "../bindings/SoftwareInventory";
import type { WatchStatus } from "../bindings/WatchStatus";
import type { WslInventory } from "../bindings/WslInventory";
import type { ScanStatus } from "../bindings/ScanStatus";

// Fallible commands reject with a `CommandError` ({ kind, message }).
export const ipc = {
  appInfo: () => invoke<AppInfo>("app_info"),
  protectedLocations: () => invoke<ProtectedLocation[]>("protected_locations"),
  listDrives: () => invoke<Drive[]>("list_drives"),
  driveTrends: () => invoke<DriveTrend[]>("drive_trends"),
  startScan: (root: string) => invoke<number>("start_scan", { root }),
  anomalyReport: (scanId?: number, baselineId?: number) =>
    invoke<AnomalyReport>("anomaly_report", { scanId, baselineId }),
  growthHistory: (scanId?: number) => invoke<GrowthHistory>("growth_history", { scanId }),
  folderHistory: (scanId: number, node: number) => invoke<FolderHistory>("folder_history", { scanId, node }),
  updateScan: () => invoke<number>("update_scan"),
  watchStatus: () => invoke<WatchStatus>("watch_status"),
  querySavedAnalysis: (scanId: number, queryJson: string) =>
    invoke<QueryResult>("query_saved_analysis", { scanId, queryJson }),
  planRecovery: (targetBytes: number, maxRisk: Risk) =>
    invoke<RecoveryPlan>("plan_recovery", { targetBytes, maxRisk }),
  runRecoveryPlan: (approved: { source: string; path: string }[]) =>
    invoke<PlanRunResponse>("run_recovery_plan", { approved }),
  preferences: () => invoke<Preferences>("preferences"),
  setPreferences: (preferences: Preferences) => invoke<Preferences>("set_preferences", { preferences }),
  aiSettings: () => invoke<AiSettings>("ai_settings"),
  setAiSettings: (settings: AiSettings) => invoke<AiSettings>("set_ai_settings", { settings }),
  askAssistant: (question: string, scanId?: number) => invoke<AskResponse>("ask_assistant", { question, scanId }),
  cancelScan: (id: number) => invoke<boolean>("cancel_scan", { id }),
  scanStatus: () => invoke<ScanStatus>("scan_status"),
  scanListing: (scanId: number, node: number) => invoke<DirListing>("scan_listing", { scanId, node }),
  scanLargestFiles: (scanId: number) => invoke<LargeFileView[]>("scan_largest_files", { scanId }),
  cleanupProviders: () => invoke<ProviderInfo[]>("cleanup_providers"),
  cleanupPreview: (provider: string) => invoke<PreviewResponse>("cleanup_preview", { provider }),
  auditLog: (limit: number, before: number | null = null) => invoke<AuditEntry[]>("audit_log", { limit, before }),
  auditVerify: () => invoke<AuditIntegrity>("audit_verify"),
  projectSearches: () => invoke<ProjectSearch[]>("project_searches"),
  findProjects: (root: string) => invoke<ProjectSearch>("find_projects", { root }),
  removeProjectSearch: (root: string) => invoke<boolean>("remove_project_search", { root }),
  jvmCaches: () => invoke<JvmAnalysis>("jvm_caches"),
  dockerOverview: () => invoke<DockerOverview>("docker_overview"),
  wslDistros: () => invoke<WslInventory>("wsl_distros"),
  softwareInventory: () => invoke<SoftwareInventory>("software_inventory"),
  dependencyGraph: () => invoke<DependencyGraph>("dependency_graph"),
  runtimeInventory: () => invoke<RuntimeInventory>("runtime_inventory"),
  projectCleanupPreview: (projects: string[]) => invoke<PreviewResponse>("project_cleanup_preview", { projects }),
  projectCleanupRun: (projects: string[], approved: string[]) =>
    invoke<CleanupRunResponse>("project_cleanup_run", { projects, approved }),
  cleanupRun: (provider: string, approved: string[]) =>
    invoke<CleanupRunResponse>("cleanup_run", { provider, approved }),
  quarantineContents: () => invoke<QuarantineContents>("quarantine_contents"),
  quarantineRestore: (operationId: string, index: number) =>
    invoke<ManifestEntry>("quarantine_restore", { operationId, index }),
};

/** Turn whatever a rejected command produced into text a person can read. */
export function describeError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (err && typeof err === "object" && "message" in err) return String(err.message);
  return "An unexpected error occurred.";
}
