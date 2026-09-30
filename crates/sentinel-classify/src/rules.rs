//! The rule tables. Every rule carries a stable id and a user-facing reason.
//!
//! Adding a rule: prefer a location rule anchored at a known folder. Only add a name
//! rule for a folder name that means the same thing wherever it appears; ambiguous
//! names (`target`, `dist`, `build`, `bin`, `obj`, `packages`) are deliberately absent.

use crate::Category::{self, *};

/// Base folders location rules are relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Anchor {
    Windows,
    ProgramFiles,
    ProgramFilesX86,
    ProgramData,
    Profile,
    LocalAppData,
    RoamingAppData,
    /// `%CARGO_HOME%`, default `%USERPROFILE%\.cargo`.
    CargoHome,
    /// `%RUSTUP_HOME%`, default `%USERPROFILE%\.rustup`.
    RustupHome,
}

pub struct LocationRule {
    pub id: &'static str,
    pub anchor: Anchor,
    pub rel: &'static [&'static str],
    pub category: Category,
    pub reason: &'static str,
}

/// Matches a folder or file with this exact name (case-insensitive) at any depth.
pub struct NameRule {
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub reason: &'static str,
}

/// Matches an entry directly under a drive root.
pub struct RootRule {
    pub id: &'static str,
    pub matches: fn(&str) -> bool,
    pub category: Category,
    pub reason: &'static str,
}

macro_rules! loc {
    ($id:literal, $anchor:ident, [$($p:literal),*], $cat:ident, $reason:literal) => {
        LocationRule { id: $id, anchor: Anchor::$anchor, rel: &[$($p),*], category: $cat, reason: $reason }
    };
}

pub const LOCATION_RULES: &[LocationRule] = &[
    // Windows
    loc!("windows", Windows, [], Windows, "Windows system files"),
    loc!(
        "windows.temp",
        Windows,
        ["Temp"],
        TemporaryFiles,
        "Windows temporary files"
    ),
    loc!(
        "windows.update-download",
        Windows,
        ["SoftwareDistribution", "Download"],
        TemporaryFiles,
        "Windows Update download cache"
    ),
    loc!("windows.logs", Windows, ["Logs"], Logs, "Windows log files"),
    loc!(
        "windows.minidump",
        Windows,
        ["Minidump"],
        Logs,
        "Windows crash dumps"
    ),
    loc!(
        "windows.installer",
        Windows,
        ["Installer"],
        Windows,
        "Windows Installer cache, needed to repair and uninstall programs"
    ),
    // Applications
    loc!(
        "program-files",
        ProgramFiles,
        [],
        Applications,
        "Installed applications"
    ),
    loc!(
        "program-files-x86",
        ProgramFilesX86,
        [],
        Applications,
        "Installed 32-bit applications"
    ),
    loc!(
        "program-data",
        ProgramData,
        [],
        Applications,
        "Application data shared by all users"
    ),
    loc!(
        "program-data.package-cache",
        ProgramData,
        ["Package Cache"],
        PackageCaches,
        "Installer package cache used to repair and uninstall programs"
    ),
    loc!(
        "program-data.wer",
        ProgramData,
        ["Microsoft", "Windows", "WER"],
        Logs,
        "Windows Error Reporting crash reports"
    ),
    loc!(
        "local-app-data",
        LocalAppData,
        [],
        Applications,
        "Application data for this user"
    ),
    loc!(
        "roaming-app-data",
        RoamingAppData,
        [],
        Applications,
        "Application settings for this user"
    ),
    loc!(
        "local-app-data.programs",
        LocalAppData,
        ["Programs"],
        Applications,
        "Applications installed for this user"
    ),
    loc!(
        "local-app-data.packages",
        LocalAppData,
        ["Packages"],
        Applications,
        "Microsoft Store app data"
    ),
    // Temporary files and logs
    loc!(
        "user.temp",
        LocalAppData,
        ["Temp"],
        TemporaryFiles,
        "Temporary files for this user"
    ),
    loc!(
        "user.crash-dumps",
        LocalAppData,
        ["CrashDumps"],
        Logs,
        "Application crash dumps"
    ),
    loc!(
        "user.wer",
        LocalAppData,
        ["Microsoft", "Windows", "WER"],
        Logs,
        "Windows Error Reporting crash reports"
    ),
    // User data
    loc!("user.desktop", Profile, ["Desktop"], UserData, "Desktop"),
    loc!(
        "user.documents",
        Profile,
        ["Documents"],
        UserData,
        "Documents"
    ),
    loc!(
        "user.downloads",
        Profile,
        ["Downloads"],
        UserData,
        "Downloads"
    ),
    loc!("user.pictures", Profile, ["Pictures"], UserData, "Pictures"),
    loc!("user.videos", Profile, ["Videos"], UserData, "Videos"),
    loc!("user.music", Profile, ["Music"], UserData, "Music"),
    loc!("user.onedrive", Profile, ["OneDrive"], UserData, "OneDrive"),
    // Browsers
    loc!(
        "browser.chrome",
        LocalAppData,
        ["Google", "Chrome"],
        Browsers,
        "Google Chrome profiles and cache"
    ),
    loc!(
        "browser.edge",
        LocalAppData,
        ["Microsoft", "Edge"],
        Browsers,
        "Microsoft Edge profiles and cache"
    ),
    loc!(
        "browser.brave",
        LocalAppData,
        ["BraveSoftware"],
        Browsers,
        "Brave profiles and cache"
    ),
    loc!(
        "browser.firefox-local",
        LocalAppData,
        ["Mozilla"],
        Browsers,
        "Firefox cache"
    ),
    loc!(
        "browser.firefox",
        RoamingAppData,
        ["Mozilla"],
        Browsers,
        "Firefox profiles"
    ),
    loc!(
        "browser.opera",
        RoamingAppData,
        ["Opera Software"],
        Browsers,
        "Opera profiles and cache"
    ),
    // Package caches
    loc!(
        "npm.cache",
        LocalAppData,
        ["npm-cache"],
        PackageCaches,
        "npm download cache; npm re-downloads packages when needed"
    ),
    loc!(
        "npm.global",
        RoamingAppData,
        ["npm"],
        DeveloperDependencies,
        "Globally installed npm packages"
    ),
    loc!(
        "pnpm.store",
        LocalAppData,
        ["pnpm", "store"],
        PackageCaches,
        "pnpm content-addressable package store"
    ),
    loc!(
        "pnpm.cache",
        LocalAppData,
        ["pnpm-cache"],
        PackageCaches,
        "pnpm metadata cache"
    ),
    loc!(
        "yarn.cache",
        LocalAppData,
        ["Yarn", "Cache"],
        PackageCaches,
        "Yarn download cache"
    ),
    loc!(
        "pip.cache",
        LocalAppData,
        ["pip", "cache"],
        PackageCaches,
        "pip download and wheel cache"
    ),
    loc!(
        "uv.cache",
        LocalAppData,
        ["uv", "cache"],
        PackageCaches,
        "uv package cache"
    ),
    loc!(
        "nuget.packages",
        Profile,
        [".nuget", "packages"],
        PackageCaches,
        "NuGet global packages folder"
    ),
    loc!(
        "nuget.http-cache",
        LocalAppData,
        ["NuGet", "v3-cache"],
        PackageCaches,
        "NuGet HTTP cache"
    ),
    loc!(
        "maven.repository",
        Profile,
        [".m2", "repository"],
        PackageCaches,
        "Maven local repository of downloaded dependencies"
    ),
    loc!(
        "gradle.caches",
        Profile,
        [".gradle", "caches"],
        PackageCaches,
        "Gradle dependency and build cache"
    ),
    loc!(
        "gradle.wrapper",
        Profile,
        [".gradle", "wrapper", "dists"],
        DeveloperTools,
        "Gradle distributions downloaded by the Gradle wrapper"
    ),
    loc!(
        "cargo.registry",
        CargoHome,
        ["registry"],
        PackageCaches,
        "Cargo crate registry cache"
    ),
    loc!(
        "cargo.git",
        CargoHome,
        ["git"],
        PackageCaches,
        "Cargo git dependency cache"
    ),
    loc!(
        "cargo.home",
        CargoHome,
        [],
        DeveloperTools,
        "Rust tools installed with Cargo"
    ),
    loc!(
        "rustup.home",
        RustupHome,
        [],
        DeveloperTools,
        "Rust toolchains managed by rustup"
    ),
    loc!(
        "go.mod-cache",
        Profile,
        ["go", "pkg", "mod"],
        PackageCaches,
        "Go module cache"
    ),
    // Developer tools
    loc!(
        "vscode.profile",
        Profile,
        [".vscode"],
        DeveloperTools,
        "VS Code extensions and command-line data"
    ),
    loc!(
        "vscode.user-data",
        RoamingAppData,
        ["Code"],
        DeveloperTools,
        "VS Code settings, caches and workspace state"
    ),
    loc!(
        "vscode.install",
        LocalAppData,
        ["Programs", "Microsoft VS Code"],
        DeveloperTools,
        "VS Code installation"
    ),
    loc!(
        "jetbrains.caches",
        LocalAppData,
        ["JetBrains"],
        DeveloperTools,
        "JetBrains IDE caches and indexes"
    ),
    loc!(
        "jetbrains.settings",
        RoamingAppData,
        ["JetBrains"],
        DeveloperTools,
        "JetBrains IDE settings and plugins"
    ),
    loc!(
        "android.sdk",
        LocalAppData,
        ["Android", "Sdk"],
        DeveloperTools,
        "Android SDK"
    ),
    loc!(
        "android.avd",
        Profile,
        [".android"],
        DeveloperTools,
        "Android emulator images and settings"
    ),
    loc!(
        "visual-studio",
        ProgramFiles,
        ["Microsoft Visual Studio"],
        DeveloperTools,
        "Visual Studio"
    ),
    loc!(
        "visual-studio-x86",
        ProgramFilesX86,
        ["Microsoft Visual Studio"],
        DeveloperTools,
        "Visual Studio and Build Tools"
    ),
    loc!(
        "windows-kits",
        ProgramFilesX86,
        ["Windows Kits"],
        DeveloperTools,
        "Windows SDK"
    ),
    loc!(
        "dotnet",
        ProgramFiles,
        ["dotnet"],
        DeveloperTools,
        ".NET SDKs and runtimes"
    ),
    loc!(
        "nodejs",
        ProgramFiles,
        ["nodejs"],
        DeveloperTools,
        "Node.js installation"
    ),
    loc!(
        "git",
        ProgramFiles,
        ["Git"],
        DeveloperTools,
        "Git for Windows"
    ),
    loc!(
        "java",
        ProgramFiles,
        ["Java"],
        DeveloperTools,
        "Java installations"
    ),
    // Containers and WSL
    loc!(
        "docker.desktop-data",
        LocalAppData,
        ["Docker"],
        Containers,
        "Docker Desktop data, including the disk that holds images, containers and volumes"
    ),
    loc!(
        "docker.program-data",
        ProgramData,
        ["DockerDesktop"],
        Containers,
        "Docker Desktop machine data"
    ),
    loc!(
        "docker.program",
        ProgramFiles,
        ["Docker"],
        Containers,
        "Docker Desktop installation"
    ),
    loc!(
        "wsl.disks",
        LocalAppData,
        ["wsl"],
        Wsl,
        "WSL distribution disks"
    ),
    // Games
    loc!(
        "steam",
        ProgramFilesX86,
        ["Steam"],
        Games,
        "Steam and its installed games"
    ),
    loc!(
        "epic",
        ProgramFiles,
        ["Epic Games"],
        Games,
        "Epic Games launcher and games"
    ),
];

pub const NAME_RULES: &[NameRule] = &[
    NameRule {
        id: "name.node-modules",
        name: "node_modules",
        category: DeveloperDependencies,
        reason: "Installed npm/pnpm/Yarn packages for a project; restored by the package manager",
    },
    NameRule {
        id: "name.venv",
        name: ".venv",
        category: DeveloperDependencies,
        reason: "Python virtual environment; recreated from the project's requirements",
    },
    NameRule {
        id: "name.pycache",
        name: "__pycache__",
        category: BuildArtifacts,
        reason: "Compiled Python bytecode; Python regenerates it automatically",
    },
    NameRule {
        id: "name.pytest-cache",
        name: ".pytest_cache",
        category: BuildArtifacts,
        reason: "pytest cache; regenerated on the next test run",
    },
    NameRule {
        id: "name.mypy-cache",
        name: ".mypy_cache",
        category: BuildArtifacts,
        reason: "mypy type-check cache; regenerated automatically",
    },
    NameRule {
        id: "name.ruff-cache",
        name: ".ruff_cache",
        category: BuildArtifacts,
        reason: "Ruff lint cache; regenerated automatically",
    },
    NameRule {
        id: "name.tox",
        name: ".tox",
        category: BuildArtifacts,
        reason: "tox test environments; recreated on the next tox run",
    },
    NameRule {
        id: "name.next",
        name: ".next",
        category: BuildArtifacts,
        reason: "Next.js build output and cache",
    },
    NameRule {
        id: "name.nuxt",
        name: ".nuxt",
        category: BuildArtifacts,
        reason: "Nuxt build output",
    },
    NameRule {
        id: "name.svelte-kit",
        name: ".svelte-kit",
        category: BuildArtifacts,
        reason: "SvelteKit build output",
    },
    NameRule {
        id: "name.turbo",
        name: ".turbo",
        category: BuildArtifacts,
        reason: "Turborepo cache",
    },
    NameRule {
        id: "name.parcel-cache",
        name: ".parcel-cache",
        category: BuildArtifacts,
        reason: "Parcel bundler cache",
    },
    NameRule {
        id: "name.angular-cache",
        name: ".angular",
        category: BuildArtifacts,
        reason: "Angular CLI build cache",
    },
];

fn python_install(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower
        .strip_prefix("python")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

pub const ROOT_RULES: &[RootRule] = &[
    RootRule {
        id: "root.recycle-bin",
        matches: |n| n.eq_ignore_ascii_case("$Recycle.Bin"),
        category: TemporaryFiles,
        reason: "Recycle Bin: deleted files that can still be restored",
    },
    RootRule {
        id: "root.system-volume-information",
        matches: |n| n.eq_ignore_ascii_case("System Volume Information"),
        category: Windows,
        reason: "System restore points and volume shadow copies",
    },
    RootRule {
        id: "root.recovery",
        matches: |n| n.eq_ignore_ascii_case("Recovery"),
        category: Windows,
        reason: "Windows recovery environment",
    },
    RootRule {
        id: "root.windows-old",
        matches: |n| n.eq_ignore_ascii_case("Windows.old"),
        category: Windows,
        reason: "Previous Windows installation kept after an upgrade",
    },
    RootRule {
        id: "root.pagefile",
        matches: |n| n.eq_ignore_ascii_case("pagefile.sys"),
        category: Windows,
        reason: "Windows page file (virtual memory)",
    },
    RootRule {
        id: "root.swapfile",
        matches: |n| n.eq_ignore_ascii_case("swapfile.sys"),
        category: Windows,
        reason: "Windows swap file for Store apps",
    },
    RootRule {
        id: "root.hiberfil",
        matches: |n| n.eq_ignore_ascii_case("hiberfil.sys"),
        category: Windows,
        reason: "Hibernation file (memory saved when the PC hibernates or fast-starts)",
    },
    RootRule {
        id: "root.python",
        matches: python_install,
        category: DeveloperTools,
        reason: "Python installation",
    },
    RootRule {
        id: "root.riot",
        matches: |n| n.eq_ignore_ascii_case("Riot Games"),
        category: Games,
        reason: "Riot Games client and games",
    },
    RootRule {
        id: "root.xbox",
        matches: |n| n.eq_ignore_ascii_case("XboxGames"),
        category: Games,
        reason: "Games installed by the Xbox app",
    },
];
