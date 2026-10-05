//! Docker inspection through the CLI. See the crate docs for the safety rules.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// Output beyond this is cut off and the command treated as failed.
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DockerState {
    NotInstalled,
    /// The CLI is there but the engine does not answer (Docker Desktop stopped).
    NotRunning,
    Running,
}

/// One row of `docker system df`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UsageRow {
    /// "Images", "Containers", "Local Volumes" or "Build Cache".
    pub kind: String,
    #[ts(type = "number | null")]
    pub total: Option<u64>,
    #[ts(type = "number | null")]
    pub active: Option<u64>,
    /// Approximate: Docker prints rounded figures.
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    #[ts(type = "number | null")]
    pub reclaimable_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `created`, `paused`...
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DockerImage {
    pub id: String,
    pub repository: String,
    pub tag: String,
    /// Approximate (rounded by Docker).
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    pub created: String,
    /// Untagged (`<none>`): usually a superseded build.
    pub dangling: bool,
    /// Containers (running or stopped) created from this image.
    #[ts(type = "number")]
    pub containers: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DockerVolume {
    pub name: String,
    pub driver: String,
    /// Approximate; `null` when `docker system df -v` did not report it.
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
    /// Containers using it; `null` when unknown.
    #[ts(type = "number | null")]
    pub links: Option<u64>,
    /// Docker Compose project that created it, from its labels.
    pub compose_project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DockerOverview {
    pub state: DockerState,
    pub cli_path: Option<String>,
    pub server_version: Option<String>,
    pub usage: Vec<UsageRow>,
    pub containers: Vec<DockerContainer>,
    pub images: Vec<DockerImage>,
    pub volumes: Vec<DockerVolume>,
    pub warnings: Vec<String>,
}

impl DockerOverview {
    fn empty(state: DockerState, cli_path: Option<String>) -> Self {
        Self {
            state,
            cli_path,
            server_version: None,
            usage: Vec::new(),
            containers: Vec::new(),
            images: Vec::new(),
            volumes: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// The only commands Sentinel ever sends to Docker. All read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cmd {
    Version,
    Containers,
    Images,
    Volumes,
    Usage,
    UsageVerbose,
}

impl Cmd {
    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Version => &["version", "--format", "{{.Server.Version}}"],
            Self::Containers => &["ps", "-a", "--format", "{{json .}}"],
            Self::Images => &["images", "--format", "{{json .}}"],
            Self::Volumes => &["volume", "ls", "--format", "{{json .}}"],
            Self::Usage => &["system", "df", "--format", "{{json .}}"],
            Self::UsageVerbose => &["system", "df", "-v", "--format", "{{json .}}"],
        }
    }
    fn timeout(self) -> Duration {
        match self {
            // Sizing volumes walks them and can be slow.
            Self::UsageVerbose => Duration::from_secs(60),
            _ => Duration::from_secs(20),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RunError {
    /// The engine did not answer.
    NotRunning,
    Failed(String),
}

pub(crate) trait Runner {
    fn run(&self, cmd: Cmd) -> Result<String, RunError>;
}

/// Does Docker's error text mean the engine is simply not running?
fn engine_down(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    [
        "failed to connect to the docker api",
        "cannot connect to the docker daemon",
        "error during connect",
        "daemon is not running",
        "dockerdesktoplinuxengine",
    ]
    .iter()
    .any(|m| s.contains(m))
}

/// Runs the resolved `docker.exe` with fixed arguments: no shell, no window, bounded
/// time and output.
pub(crate) struct Cli {
    exe: PathBuf,
}

fn read_capped<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.by_ref().take(MAX_OUTPUT as u64 + 1).read_to_end(&mut buf);
        // Keep draining so the child never blocks on a full pipe.
        let _ = std::io::copy(&mut r, &mut std::io::sink());
        buf
    })
}

impl Runner for Cli {
    fn run(&self, cmd: Cmd) -> Result<String, RunError> {
        let mut c = Command::new(&self.exe);
        c.args(cmd.args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = c
            .spawn()
            .map_err(|e| RunError::Failed(format!("docker could not be started: {e}")))?;
        let out = child.stdout.take().map(read_capped);
        let err = child.stderr.take().map(read_capped);
        let deadline = Instant::now() + cmd.timeout();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RunError::Failed("docker did not answer in time".into()));
                }
            }
        };
        let stdout = out.and_then(|h| h.join().ok()).unwrap_or_default();
        let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
        let stderr = String::from_utf8_lossy(&stderr[..stderr.len().min(4096)]).into_owned();
        if !status.success() {
            return Err(if engine_down(&stderr) {
                RunError::NotRunning
            } else {
                RunError::Failed(stderr.trim().chars().take(300).collect())
            });
        }
        if stdout.len() > MAX_OUTPUT {
            return Err(RunError::Failed(
                "docker's answer was too large to read".into(),
            ));
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }
}

/// `1.2GB` / `512MB` / `12.3kB` / `0B` -> bytes. Docker uses decimal units and may append
/// a percentage (`1.2GB (45%)`). Anything else is `None`.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.split_whitespace().next()?;
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mult: f64 = match unit.to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "pb" => 1e15,
        _ => return None,
    };
    (n.is_finite() && n >= 0.0).then(|| (n * mult).round() as u64)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .chars()
        .take(300)
        .collect()
}

fn number(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// Value of `key` in Docker's `k=v,k=v` label string.
fn label(labels: &str, key: &str) -> Option<String> {
    labels
        .split(',')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .map(str::to_owned)
        .filter(|v| !v.is_empty())
}

/// One JSON object per line, as `--format '{{json .}}'` prints. Returns the objects and
/// how many non-empty lines were not understood.
fn json_lines(out: &str) -> (Vec<Value>, u64) {
    let mut bad = 0;
    let vals = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| match serde_json::from_str::<Value>(l) {
            Ok(v) if v.is_object() => Some(v),
            _ => {
                bad += 1;
                None
            }
        })
        .collect();
    (vals, bad)
}

fn parse_usage(out: &str) -> (Vec<UsageRow>, u64) {
    let (rows, bad) = json_lines(out);
    let rows = rows
        .iter()
        .map(|r| UsageRow {
            kind: text(r, "Type"),
            total: number(r, "TotalCount"),
            active: number(r, "Active"),
            size_bytes: parse_size(&text(r, "Size")),
            reclaimable_bytes: parse_size(&text(r, "Reclaimable")),
        })
        .collect();
    (rows, bad)
}

fn parse_containers(out: &str) -> (Vec<DockerContainer>, u64) {
    let (rows, bad) = json_lines(out);
    let rows = rows
        .iter()
        .map(|r| DockerContainer {
            id: text(r, "ID"),
            name: text(r, "Names"),
            image: text(r, "Image"),
            state: text(r, "State"),
            status: text(r, "Status"),
        })
        .collect();
    (rows, bad)
}

fn parse_images(out: &str, containers: &[DockerContainer]) -> (Vec<DockerImage>, u64) {
    let (rows, bad) = json_lines(out);
    let rows = rows
        .iter()
        .map(|r| {
            let id = text(r, "ID");
            let repository = text(r, "Repository");
            let tag = text(r, "Tag");
            let dangling = repository == "<none>";
            let bare = id.trim_start_matches("sha256:");
            let computed = containers
                .iter()
                .filter(|c| {
                    let img = c.image.as_str();
                    img == format!("{repository}:{tag}")
                        || (tag == "latest" && img == repository)
                        || (!bare.is_empty()
                            && !img.is_empty()
                            && bare.starts_with(img.trim_start_matches("sha256:")))
                })
                .count() as u64;
            // Docker reports its own count; fall back to matching only when it does not.
            let count = number(r, "Containers").unwrap_or(computed);
            DockerImage {
                id,
                repository,
                tag,
                size_bytes: parse_size(&text(r, "Size")),
                created: text(r, "CreatedSince"),
                dangling,
                containers: count,
            }
        })
        .collect();
    (rows, bad)
}

/// Volume sizes and link counts from `system df -v`, which prints one object holding a
/// `Volumes` array. Unrecognized shapes give an empty list, never an error.
fn parse_volume_usage(out: &str) -> Vec<(String, Option<u64>, Option<u64>)> {
    let Ok(v) = serde_json::from_str::<Value>(out.trim()) else {
        return Vec::new();
    };
    v.get("Volumes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    (
                        text(x, "Name"),
                        parse_size(&text(x, "Size")),
                        number(x, "Links"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn overview_with(runner: &dyn Runner, cli_path: Option<String>) -> DockerOverview {
    let version = match runner.run(Cmd::Version) {
        Ok(v) => v.trim().to_owned(),
        Err(RunError::NotRunning) => {
            return DockerOverview::empty(DockerState::NotRunning, cli_path);
        }
        Err(RunError::Failed(msg)) => {
            let mut o = DockerOverview::empty(DockerState::NotRunning, cli_path);
            o.warnings.push(format!("Docker did not answer: {msg}"));
            return o;
        }
    };
    let mut o = DockerOverview::empty(DockerState::Running, cli_path);
    o.server_version = Some(version).filter(|v| !v.is_empty());

    let ask = |cmd: Cmd, what: &str, warnings: &mut Vec<String>| match runner.run(cmd) {
        Ok(s) => Some(s),
        Err(e) => {
            warnings.push(match e {
                RunError::NotRunning => format!("Docker stopped answering while reading {what}."),
                RunError::Failed(m) => format!("Could not read {what}: {m}"),
            });
            None
        }
    };
    let note_bad = |bad: u64, what: &str, warnings: &mut Vec<String>| {
        if bad > 0 {
            warnings.push(format!(
                "{bad} {what} lines were not understood and were left out."
            ));
        }
    };

    if let Some(s) = ask(Cmd::Usage, "disk usage", &mut o.warnings) {
        let (rows, bad) = parse_usage(&s);
        o.usage = rows;
        note_bad(bad, "disk usage", &mut o.warnings);
    }
    if let Some(s) = ask(Cmd::Containers, "containers", &mut o.warnings) {
        let (rows, bad) = parse_containers(&s);
        o.containers = rows;
        note_bad(bad, "container", &mut o.warnings);
    }
    if let Some(s) = ask(Cmd::Images, "images", &mut o.warnings) {
        let (rows, bad) = parse_images(&s, &o.containers);
        o.images = rows;
        note_bad(bad, "image", &mut o.warnings);
    }
    if let Some(s) = ask(Cmd::Volumes, "volumes", &mut o.warnings) {
        let (rows, bad) = json_lines(&s);
        o.volumes = rows
            .iter()
            .map(|r| DockerVolume {
                name: text(r, "Name"),
                driver: text(r, "Driver"),
                size_bytes: None,
                links: None,
                compose_project: label(&text(r, "Labels"), "com.docker.compose.project"),
            })
            .collect();
        note_bad(bad, "volume", &mut o.warnings);
    }
    if !o.volumes.is_empty() {
        // Sizes need the slower verbose form; without it volumes are listed unsized.
        if let Some(s) = ask(Cmd::UsageVerbose, "volume sizes", &mut o.warnings) {
            for (name, size, links) in parse_volume_usage(&s) {
                if let Some(v) = o.volumes.iter_mut().find(|v| v.name == name) {
                    v.size_bytes = size;
                    v.links = links;
                }
            }
        }
    }
    o.images.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then_with(|| a.repository.cmp(&b.repository))
    });
    o.volumes.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    o
}

/// `docker.exe` in Docker Desktop's folder, else the first one on `PATH`. Always an
/// absolute path, so a `docker.exe` in the current folder is never picked up.
fn find_docker() -> Option<PathBuf> {
    let pf = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let known = pf.map(|p| {
        p.join("Docker")
            .join("Docker")
            .join("resources")
            .join("bin")
            .join("docker.exe")
    });
    if let Some(k) = known.filter(|k| k.is_file()) {
        return Some(k);
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|d| d.is_absolute())
            .map(|d| d.join("docker.exe"))
            .find(|p| p.is_file())
    })
}

/// Ask Docker what it holds. Read-only; never starts Docker.
#[cfg(windows)]
pub fn inspect_docker() -> DockerOverview {
    let Some(exe) = find_docker() else {
        return DockerOverview::empty(DockerState::NotInstalled, None);
    };
    let o = overview_with(&Cli { exe: exe.clone() }, Some(display(&exe)));
    tracing::info!(state = ?o.state, images = o.images.len(), volumes = o.volumes.len(), "docker inspection finished");
    o
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashMap;

    use super::*;

    struct Fake(HashMap<&'static str, Result<String, RunError>>);
    impl Fake {
        fn key(c: Cmd) -> &'static str {
            match c {
                Cmd::Version => "version",
                Cmd::Containers => "ps",
                Cmd::Images => "images",
                Cmd::Volumes => "volumes",
                Cmd::Usage => "df",
                Cmd::UsageVerbose => "dfv",
            }
        }
    }
    impl Runner for Fake {
        fn run(&self, cmd: Cmd) -> Result<String, RunError> {
            match self.0.get(Self::key(cmd)) {
                Some(Ok(s)) => Ok(s.clone()),
                Some(Err(RunError::NotRunning)) => Err(RunError::NotRunning),
                Some(Err(RunError::Failed(m))) => Err(RunError::Failed(m.clone())),
                None => Err(RunError::Failed("not stubbed".into())),
            }
        }
    }

    #[test]
    fn parses_docker_sizes() {
        assert_eq!(parse_size("1.2GB"), Some(1_200_000_000));
        assert_eq!(parse_size("512MB"), Some(512_000_000));
        assert_eq!(parse_size("12.3kB"), Some(12_300));
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("1.5GB (45%)"), Some(1_500_000_000));
        assert_eq!(parse_size("N/A"), None);
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("12"), None);
        assert_eq!(parse_size("3 bananas"), None);
    }

    #[test]
    fn engine_down_messages_are_recognized() {
        assert!(engine_down(
            "failed to connect to the docker API at npipe:////./pipe/dockerDesktopLinuxEngine"
        ));
        assert!(engine_down(
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock"
        ));
        assert!(!engine_down("unknown flag: --nope"));
    }

    #[test]
    fn a_stopped_engine_is_reported_not_failed() {
        let f = Fake(HashMap::from([("version", Err(RunError::NotRunning))]));
        let o = overview_with(&f, Some("docker.exe".into()));
        assert_eq!(o.state, DockerState::NotRunning);
        assert!(o.warnings.is_empty());
        assert!(o.images.is_empty());
    }

    #[test]
    fn reads_a_running_engine() {
        let ps = concat!(
            r#"{"ID":"c1","Image":"redis:7","Names":"cache","State":"running","Status":"Up 2 hours"}"#,
            "\n",
            r#"{"ID":"c2","Image":"app","Names":"web","State":"exited","Status":"Exited (0) 3 days ago"}"#,
            "\n",
            "not json\n"
        );
        let images = concat!(
            r#"{"ID":"sha256:aaa111","Repository":"redis","Tag":"7","Size":"117MB","CreatedSince":"2 weeks ago"}"#,
            "\n",
            r#"{"ID":"sha256:bbb222","Repository":"app","Tag":"latest","Size":"1.2GB","CreatedSince":"3 days ago"}"#,
            "\n",
            r#"{"ID":"sha256:ccc333","Repository":"<none>","Tag":"<none>","Size":"800MB","CreatedSince":"1 month ago"}"#,
            "\n"
        );
        let df = concat!(
            r#"{"Type":"Images","TotalCount":"3","Active":"2","Size":"2.1GB","Reclaimable":"800MB (38%)"}"#,
            "\n",
            r#"{"Type":"Build Cache","TotalCount":"9","Active":"0","Size":"500MB","Reclaimable":"500MB"}"#,
            "\n"
        );
        let vols = concat!(
            r#"{"Driver":"local","Name":"pgdata","Scope":"local","Labels":"a=b,com.docker.compose.project=shop"}"#,
            "\n",
            r#"{"Driver":"local","Name":"scratch","Scope":"local"}"#,
            "\n"
        );
        let dfv = r#"{"Images":[],"Volumes":[{"Name":"pgdata","Links":"1","Size":"3.4GB"},{"Name":"scratch","Links":"0","Size":"12kB"}]}"#;
        let f = Fake(HashMap::from([
            ("version", Ok("29.1.0\n".to_owned())),
            ("ps", Ok(ps.to_owned())),
            ("images", Ok(images.to_owned())),
            ("volumes", Ok(vols.to_owned())),
            ("df", Ok(df.to_owned())),
            ("dfv", Ok(dfv.to_owned())),
        ]));
        let o = overview_with(&f, None);
        assert_eq!(o.state, DockerState::Running);
        assert_eq!(o.server_version.as_deref(), Some("29.1.0"));
        assert_eq!(o.containers.len(), 2);
        assert_eq!(o.usage[0].reclaimable_bytes, Some(800_000_000));
        assert_eq!(o.usage[1].kind, "Build Cache");

        // Largest first; matching containers to images by tag, bare name and id.
        assert_eq!(o.images[0].repository, "app");
        assert_eq!(o.images[0].containers, 1, "image 'app' matches tag latest");
        let redis = o.images.iter().find(|i| i.repository == "redis").unwrap();
        assert_eq!(redis.containers, 1);
        let dangling = o.images.iter().find(|i| i.dangling).unwrap();
        assert_eq!(dangling.containers, 0);

        assert_eq!(o.volumes[0].name, "pgdata", "largest volume first");
        assert_eq!(o.volumes[0].size_bytes, Some(3_400_000_000));
        assert_eq!(o.volumes[0].links, Some(1));
        assert_eq!(o.volumes[0].compose_project.as_deref(), Some("shop"));
        assert_eq!(o.volumes[1].compose_project, None);
        assert_eq!(o.volumes[1].links, Some(0));

        assert_eq!(o.warnings.len(), 1);
        assert!(o.warnings[0].contains("1 container lines were not understood"));
    }

    #[test]
    fn a_failing_sub_command_is_a_warning_not_a_crash() {
        let f = Fake(HashMap::from([
            ("version", Ok("29.1.0".to_owned())),
            ("ps", Err(RunError::Failed("boom".into()))),
        ]));
        let o = overview_with(&f, None);
        assert_eq!(o.state, DockerState::Running);
        assert!(
            o.warnings
                .iter()
                .any(|w| w.contains("Could not read containers: boom"))
        );
    }

    #[test]
    fn docker_own_container_count_wins_over_matching() {
        let c = [DockerContainer {
            id: "c".into(),
            name: "n".into(),
            image: "app".into(),
            state: "running".into(),
            status: String::new(),
        }];
        let (imgs, _) = parse_images(
            r#"{"ID":"x1","Repository":"app","Tag":"latest","Size":"1MB","Containers":"0"}"#,
            &c,
        );
        assert_eq!(imgs[0].containers, 0);
        let (imgs, _) = parse_images(
            r#"{"ID":"x1","Repository":"app","Tag":"latest","Size":"1MB","Containers":"N/A"}"#,
            &c,
        );
        assert_eq!(imgs[0].containers, 1);
    }

    #[test]
    fn unrecognized_volume_usage_leaves_volumes_unsized() {
        assert!(parse_volume_usage("[1,2,3]").is_empty());
        assert!(parse_volume_usage("garbage").is_empty());
    }
}
