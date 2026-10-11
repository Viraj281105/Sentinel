//! Declarative plugins for Sentinel. Design and reasoning: docs/PLUGINS.md.
//!
//! A plugin is a small JSON file that describes *where a tool keeps regenerable files*. It is
//! data, never code, and this crate does no file access at all: it parses and validates the
//! text, and produces a canonical form and its SHA-256. What a plugin offers is later judged
//! by the same preview, planner, confirmation and quarantine machinery as any built-in cleanup
//! type; nothing in the manifest can lower a protected path, risk or age.
//!
//! Everything here fails closed: unknown fields, unknown capabilities, odd characters, long
//! text, paths that could escape or alias something else are all errors, never repaired.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_PROVIDERS: usize = 16;
pub const MAX_ROOT_COMPONENTS: usize = 8;
pub const MAX_PATTERNS: usize = 32;
/// A plugin cannot offer files newer than this many days, whatever it asks for.
pub const MIN_AGE_FLOOR_DAYS: u32 = 3;
pub const DEFAULT_AGE_DAYS: u32 = 7;
const MAX_AGE_DAYS: u32 = 3650;
const MAX_COMPONENT: usize = 64;

pub const CAP_READ: &str = "filesystem.read";
pub const CAP_CLEANUP: &str = "filesystem.cleanup";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    #[error("the file is larger than {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("the file is not valid UTF-8 text")]
    NotUtf8,
    #[error("the file is not a valid plugin: {0}")]
    Json(String),
    #[error("{field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("the stored plugin does not match its recorded fingerprint")]
    HashMismatch,
}

fn bad<T>(field: impl Into<String>, reason: impl Into<String>) -> Result<T, PluginError> {
    Err(PluginError::Invalid {
        field: field.into(),
        reason: reason.into(),
    })
}

/// Where a plugin's root folder is measured from. Documents, Desktop, drive roots and the
/// program and Windows folders are deliberately not available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
    LocalAppData,
    RoamingAppData,
    UserProfile,
}

/// The risk a plugin may claim. `safe` is reserved for built-in cleanup types, so a plugin's
/// items are never part of a plan made with the Safe ceiling; nothing above medium exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginRisk {
    LowRisk,
    MediumRisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootSpec {
    pub anchor: Anchor,
    /// Relative to the anchor, 1 to 8 components, separated by `\` or `/`.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildrenSpec {
    /// Direct children of the root that count as candidates: an exact name, or a name with
    /// one `*` standing for any run of characters.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupSpec {
    pub id: String,
    pub name: String,
    pub root: RootSpec,
    pub children: ChildrenSpec,
    pub risk: PluginRisk,
    /// Defaults to 7 and cannot be below 3.
    pub min_age_days: Option<u32>,
    pub description: String,
    pub on_removal: String,
}

impl CleanupSpec {
    /// The age actually applied: what the plugin asked for, never below the floor.
    pub fn effective_min_age_days(&self) -> u32 {
        self.min_age_days
            .unwrap_or(DEFAULT_AGE_DAYS)
            .max(MIN_AGE_FLOOR_DAYS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub cleanup: Vec<CleanupSpec>,
}

/// A manifest that passed every check, in canonical form, with its fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub manifest: Manifest,
    /// The canonical JSON text. This, not the file the person picked, is what is stored.
    pub canonical: String,
    /// SHA-256 of `canonical`, lower-case hex.
    pub sha256: String,
}

// ---- text rules --------------------------------------------------------------------

/// Characters that make displayed text deceptive: bidirectional overrides and isolates,
/// zero-width and invisible characters.
fn deceptive(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{061C}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{034F}'
            | '\u{180E}'
    )
}

/// Single-line display text: trimmed, bounded, no control or deceptive characters.
fn check_text(field: &str, s: &str, max: usize) -> Result<(), PluginError> {
    let n = s.chars().count();
    if n == 0 || n > max {
        return bad(field, format!("must be between 1 and {max} characters"));
    }
    if s != s.trim() {
        return bad(field, "must not start or end with spaces");
    }
    if s.chars().any(|c| c.is_control() || deceptive(c)) {
        return bad(field, "contains control or invisible characters");
    }
    Ok(())
}

fn check_id(
    field: &str,
    s: &str,
    (min, max): (usize, usize),
    allow_dot: bool,
) -> Result<(), PluginError> {
    let ok_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let n = s.chars().count();
    if !(min..=max).contains(&n) {
        return bad(field, format!("must be {min} to {max} characters"));
    }
    let sep = |c: char| c == '-' || (allow_dot && c == '.');
    let mut prev_sep = true; // forbids a leading separator
    for c in s.chars() {
        if sep(c) {
            if prev_sep {
                return bad(field, "separators cannot be doubled or leading");
            }
            prev_sep = true;
        } else if ok_char(c) {
            prev_sep = false;
        } else {
            return bad(
                field,
                "may contain only lower-case letters, digits, '-' and '.'",
            );
        }
    }
    if prev_sep {
        return bad(field, "cannot end with a separator");
    }
    Ok(())
}

fn check_version(s: &str) -> Result<(), PluginError> {
    let parts: Vec<&str> = s.split('.').collect();
    let ok = s.len() <= 20
        && parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 6 && p.chars().all(|c| c.is_ascii_digit()));
    if ok {
        Ok(())
    } else {
        bad("version", "must look like 1.2.3")
    }
}

// ---- path rules --------------------------------------------------------------------

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// First folders under the profile that are personal data or system-managed, never a plugin
/// root. (The policy engine protects them again at run time; this is the early, clear error.)
const PROFILE_FORBIDDEN: [&str; 14] = [
    "desktop",
    "documents",
    "downloads",
    "pictures",
    "videos",
    "music",
    "appdata",
    "contacts",
    "favorites",
    "links",
    "saved games",
    "searches",
    "3d objects",
    "onedrive",
];

/// One folder or file name. `wildcard` allows a single `*`.
fn check_component(field: &str, s: &str, wildcard: bool) -> Result<(), PluginError> {
    let n = s.chars().count();
    if n == 0 || n > MAX_COMPONENT {
        return bad(
            field,
            format!("names must be 1 to {MAX_COMPONENT} characters"),
        );
    }
    if s == "." || s == ".." || s.starts_with("...") {
        return bad(field, "dot names are not allowed");
    }
    if s.ends_with('.') || s.ends_with(' ') || s.starts_with(' ') {
        return bad(
            field,
            "names cannot start or end with a space or end with a dot",
        );
    }
    for c in s.chars() {
        if c.is_control() || deceptive(c) {
            return bad(
                field,
                "names cannot contain control or invisible characters",
            );
        }
        if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '/' | '\\') {
            return bad(field, "names cannot contain \\ / : < > \" | ?");
        }
        if c == '*' && !wildcard {
            return bad(field, "wildcards are not allowed in a folder path");
        }
        if c == '~' {
            return bad(field, "names with '~' (short-name aliases) are not allowed");
        }
    }
    if s.matches('*').count() > 1 {
        return bad(field, "at most one * is allowed in a name");
    }
    let stem = s.split('.').next().unwrap_or(s).to_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return bad(field, "reserved device name");
    }
    Ok(())
}

fn root_components(path: &str) -> Vec<&str> {
    path.split(['\\', '/']).collect()
}

fn check_root(field: &str, root: &RootSpec) -> Result<(), PluginError> {
    let comps = root_components(&root.path);
    if comps.is_empty() || comps.len() > MAX_ROOT_COMPONENTS {
        return bad(
            field,
            format!("the path needs 1 to {MAX_ROOT_COMPONENTS} folders"),
        );
    }
    for c in &comps {
        if c.is_empty() {
            return bad(
                field,
                "the path cannot start or end with a separator or have empty parts",
            );
        }
        check_component(field, c, false)?;
    }
    if root.anchor == Anchor::UserProfile {
        let first = comps[0].to_lowercase();
        if PROFILE_FORBIDDEN.contains(&first.as_str()) || first.starts_with("onedrive") {
            return bad(field, "personal folders are never available to a plugin");
        }
    }
    // The built-in protected names (.git, .ssh, .env, keys, virtual disks...) apply to any
    // component, exactly as at run time. Only the name rules are used here; the machine's
    // protected folders are applied when the plugin runs.
    let probe = comps
        .iter()
        .fold(Path::new(r"C:\anchor").to_path_buf(), |p, c| p.join(c));
    if sentinel_safety::Policy::new(sentinel_safety::ProtectedSet::new())
        .check_protected(&probe)
        .is_err()
    {
        return bad(field, "the path includes a protected name");
    }
    Ok(())
}

/// Does `pattern` (an exact name, or one with a single `*`) match `name`? Case-insensitive,
/// linear time, and never matches across a path separator.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    if name.contains(['\\', '/']) {
        return false;
    }
    let (p, n) = (pattern.to_lowercase(), name.to_lowercase());
    match p.split_once('*') {
        None => p == n,
        Some((head, tail)) => {
            n.len() >= head.len() + tail.len() && n.starts_with(head) && n.ends_with(tail)
        }
    }
}

impl RootSpec {
    /// The folders after the anchor, in order.
    pub fn components(&self) -> Vec<&str> {
        root_components(&self.path)
    }
}

// ---- validation --------------------------------------------------------------------

fn validate(m: &mut Manifest) -> Result<(), PluginError> {
    if m.format != 1 {
        return bad("format", "only format 1 is supported");
    }
    check_id("id", &m.id, (3, 64), true)?;
    check_text("name", &m.name, 64)?;
    check_version(&m.version)?;
    check_text("author", &m.author, 64)?;
    check_text("description", &m.description, 400)?;

    for c in &m.capabilities {
        if c != CAP_READ && c != CAP_CLEANUP {
            return bad(
                "capabilities",
                format!(
                    "'{c}' is not available; this version supports only {CAP_READ} and {CAP_CLEANUP}"
                ),
            );
        }
    }
    m.capabilities.sort();
    m.capabilities.dedup();
    if m.cleanup.is_empty() || m.cleanup.len() > MAX_PROVIDERS {
        return bad(
            "cleanup",
            format!("a plugin needs 1 to {MAX_PROVIDERS} cleanup types"),
        );
    }
    if m.capabilities != [CAP_CLEANUP, CAP_READ] {
        return bad(
            "capabilities",
            format!("a plugin with cleanup types must declare both {CAP_READ} and {CAP_CLEANUP}"),
        );
    }

    let mut seen = Vec::new();
    for (i, c) in m.cleanup.iter_mut().enumerate() {
        let at = |f: &str| format!("cleanup[{i}].{f}");
        check_id(&at("id"), &c.id, (1, 32), false)?;
        if seen.contains(&c.id) {
            return bad(at("id"), "duplicate id within the plugin");
        }
        seen.push(c.id.clone());
        check_text(&at("name"), &c.name, 64)?;
        check_text(&at("description"), &c.description, 300)?;
        check_text(&at("onRemoval"), &c.on_removal, 400)?;
        if let Some(d) = c.min_age_days
            && !(MIN_AGE_FLOOR_DAYS..=MAX_AGE_DAYS).contains(&d)
        {
            return bad(
                at("minAgeDays"),
                format!("must be between {MIN_AGE_FLOOR_DAYS} and {MAX_AGE_DAYS} days"),
            );
        }
        // Normalize the path so the canonical form (and its fingerprint) is unambiguous.
        check_root(&at("root.path"), &c.root)?;
        c.root.path = c.root.components().join("\\");

        let names = &c.children.names;
        if names.is_empty() || names.len() > MAX_PATTERNS {
            return bad(
                at("children.names"),
                format!("needs 1 to {MAX_PATTERNS} names"),
            );
        }
        let mut lowered = Vec::new();
        for n in names {
            check_component(&at("children.names"), n, true)?;
            let l = n.to_lowercase();
            if lowered.contains(&l) {
                return bad(at("children.names"), format!("'{n}' is listed twice"));
            }
            lowered.push(l);
        }
        c.min_age_days = Some(c.effective_min_age_days());
    }
    Ok(())
}

fn fingerprint(canonical: &str) -> String {
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Parse and validate a plugin file's bytes. Nothing is read from disk here.
pub fn parse(bytes: &[u8]) -> Result<Plugin, PluginError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(PluginError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| PluginError::NotUtf8)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut manifest: Manifest =
        serde_json::from_str(text).map_err(|e| PluginError::Json(e.to_string()))?;
    validate(&mut manifest)?;
    let canonical =
        serde_json::to_string(&manifest).map_err(|e| PluginError::Json(e.to_string()))?;
    let sha256 = fingerprint(&canonical);
    Ok(Plugin {
        manifest,
        canonical,
        sha256,
    })
}

/// Load a plugin from its stored canonical text, checking it still matches the fingerprint
/// recorded when it was approved. A stored plugin that was edited fails here.
pub fn load_stored(canonical: &str, expected_sha256: &str) -> Result<Plugin, PluginError> {
    if fingerprint(canonical) != expected_sha256.to_ascii_lowercase() {
        return Err(PluginError::HashMismatch);
    }
    let p = parse(canonical.as_bytes())?;
    // Canonical text is a fixed point: re-validating it must not change it.
    if p.canonical != canonical {
        return Err(PluginError::HashMismatch);
    }
    Ok(p)
}

#[cfg(test)]
mod tests;
