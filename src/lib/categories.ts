import type { Category } from "../bindings/Category";

/** Display names and one-line meanings for storage categories. */
export const CATEGORY: Record<Category, { label: string; description: string }> = {
  windows: { label: "Windows", description: "The operating system and files it manages" },
  applications: { label: "Applications", description: "Installed programs and their data" },
  developerTools: { label: "Developer tools", description: "SDKs, toolchains, IDEs and their data" },
  developerDependencies: {
    label: "Developer dependencies",
    description: "Packages installed into projects or environments (node_modules, virtual environments)",
  },
  packageCaches: { label: "Package caches", description: "Downloaded packages kept by package managers" },
  buildArtifacts: { label: "Build artifacts", description: "Generated build output and tool caches" },
  containers: { label: "Containers", description: "Docker images, containers, volumes and their disks" },
  wsl: { label: "WSL", description: "Windows Subsystem for Linux distributions" },
  browsers: { label: "Browsers", description: "Browser profiles and caches" },
  games: { label: "Games", description: "Game launchers and installed games" },
  logs: { label: "Logs and crash dumps", description: "Log files and crash reports" },
  temporaryFiles: { label: "Temporary files", description: "Temporary files, download caches and the Recycle Bin" },
  userData: { label: "Your files", description: "Documents, pictures, downloads and other personal folders" },
  unknown: {
    label: "Not classified",
    description: "No rule identifies these folders. Sentinel does not guess.",
  },
};
