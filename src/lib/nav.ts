import {
  Activity,
  Boxes,
  Brush,
  Container,
  FolderGit2,
  HardDrive,
  Layers,
  LayoutDashboard,
  type LucideIcon,
  Package,
  Settings,
  Sparkles,
} from "lucide-react";

export type PageId =
  | "overview"
  | "storage"
  | "cleanup"
  | "software"
  | "dependencies"
  | "projects"
  | "environments"
  | "containers"
  | "assistant"
  | "activity"
  | "settings";

export interface PageDef {
  id: PageId;
  label: string;
  icon: LucideIcon;
  /** Roadmap phase that delivers this page; `null` once it is implemented. */
  plannedPhase: number | null;
  summary: string;
}

export const PAGES: PageDef[] = [
  { id: "overview", label: "Overview", icon: LayoutDashboard, plannedPhase: null, summary: "Drive health, reclaimable space and warnings." },
  { id: "storage", label: "Storage", icon: HardDrive, plannedPhase: null, summary: "Drive usage, category breakdown and drill-down into large directories." },
  { id: "cleanup", label: "Cleanup", icon: Brush, plannedPhase: null, summary: "Policy-checked cleanup with dry-run, quarantine and restore." },
  { id: "software", label: "Software", icon: Package, plannedPhase: 5, summary: "Installed applications from the registry, winget and the Store, deduplicated." },
  { id: "dependencies", label: "Dependencies", icon: Layers, plannedPhase: 5, summary: "Runtimes, the projects that use them, and their disk footprint." },
  { id: "projects", label: "Projects", icon: FolderGit2, plannedPhase: 4, summary: "Detected development projects, their artifacts and environments." },
  { id: "environments", label: "Environments", icon: Boxes, plannedPhase: 5, summary: "Language runtimes and toolchains with active and stale versions." },
  { id: "containers", label: "Containers", icon: Container, plannedPhase: 6, summary: "Docker images, containers, volumes and WSL distributions (read-only)." },
  { id: "assistant", label: "AI Assistant", icon: Sparkles, plannedPhase: 8, summary: "Optional, advisory natural-language analysis. Off by default." },
  { id: "activity", label: "Activity", icon: Activity, plannedPhase: null, summary: "Audit log of every cleanup operation." },
  { id: "settings", label: "Settings", icon: Settings, plannedPhase: null, summary: "Protected locations, logging and preferences." },
];
