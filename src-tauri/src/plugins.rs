//! Installed plugins: inspecting a plugin file, approving it, and turning approved plugins into
//! cleanup types. Design: docs/PLUGINS.md.
//!
//! The rules this module keeps:
//! - **Inspecting stores nothing.** It parses the file, shows what it would offer right now
//!   (a live dry run through the normal preview), and remembers the validated result in memory.
//! - **Approving stores exactly what was shown.** `install` takes only the fingerprint of an
//!   inspection this session made; it never reads the file again, so a file swapped after being
//!   inspected changes nothing.
//! - **Stored plugins are re-verified on every use.** A row whose text no longer matches its
//!   fingerprint, or no longer validates, is skipped and reported, never used.
//! - Every approval, change and removal is written to the audit log first.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use sentinel_cleanup::{
    CleanupProvider, Decision, PreviewLimits, ProviderInfo, Risk, RootReport, preview, providers,
};
use sentinel_plugins::{
    AnchorPaths, DeclarativeProvider, MAX_MANIFEST_BYTES, Plugin, load_stored, parse,
};
use sentinel_safety::Policy;
use sentinel_store::{Approval, AuditKind, NewAuditRecord, Outcome, PluginRow};
use serde::Serialize;
use ts_rs::TS;

use crate::audit;
use crate::commands::error::{CommandError, ErrorKind};
use crate::db::Db;

/// Inspections kept in memory, newest last. More than a person would compare at once.
const MAX_PENDING: usize = 8;
const MAX_SHOWN_ITEMS: usize = 25;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderSummary {
    /// The id the cleanup framework uses (`plugin:<plugin>:<type>`).
    pub id: String,
    pub name: String,
    pub anchor: String,
    /// The folder this type looks in, as an absolute path on this computer.
    pub root: Option<String>,
    pub patterns: Vec<String>,
    pub risk: Risk,
    pub min_age_days: u32,
    pub description: String,
    pub on_removal: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub sha256: String,
    pub enabled: bool,
    #[ts(type = "number")]
    pub approved_at_ms: i64,
    /// Why a stored plugin is not in use (damaged or edited), if so.
    pub problem: Option<String>,
    pub providers: Vec<ProviderSummary>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DryRunItem {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// What a real run would do with it, in words.
    pub verdict: String,
    pub eligible: bool,
}

/// What one cleanup type of the plugin would offer if it were approved now.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DryRun {
    pub provider_id: String,
    /// `scanned`, `missing` (the folder does not exist), or why it cannot be used.
    pub root: String,
    #[ts(type = "number")]
    pub eligible_items: u64,
    #[ts(type = "number")]
    pub eligible_bytes: u64,
    pub items: Vec<DryRunItem>,
    #[ts(type = "number")]
    pub more_items: u64,
}

/// What is already installed under the same id, for an update.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Replaces {
    pub version: String,
    pub sha256: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Inspection {
    pub plugin: PluginSummary,
    pub dry_runs: Vec<DryRun>,
    pub replaces: Option<Replaces>,
    /// This exact plugin (same fingerprint) is already installed.
    pub already_installed: bool,
}

/// Inspections made this session, by fingerprint.
#[derive(Default)]
pub(crate) struct Pending(std::sync::Mutex<Vec<(String, Plugin)>>);

impl Pending {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(String, Plugin)>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn remember(&self, plugin: &Plugin) {
        let mut v = self.lock();
        v.retain(|(sha, _)| *sha != plugin.sha256);
        v.push((plugin.sha256.clone(), plugin.clone()));
        if v.len() > MAX_PENDING {
            v.remove(0);
        }
    }

    fn take(&self, sha256: &str) -> Option<Plugin> {
        let mut v = self.lock();
        let at = v
            .iter()
            .position(|(sha, _)| sha.eq_ignore_ascii_case(sha256))?;
        Some(v.remove(at).1)
    }
}

fn summarize(
    plugin: &Plugin,
    anchors: &AnchorPaths,
    enabled: bool,
    approved_at_ms: i64,
) -> PluginSummary {
    let m = &plugin.manifest;
    let providers = DeclarativeProvider::all_for(plugin, anchors);
    PluginSummary {
        id: m.id.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        author: m.author.clone(),
        description: m.description.clone(),
        sha256: plugin.sha256.clone(),
        enabled,
        approved_at_ms,
        problem: None,
        providers: providers
            .iter()
            .zip(&m.cleanup)
            .map(|(p, spec)| {
                let info = p.info();
                ProviderSummary {
                    id: info.id,
                    name: info.name,
                    anchor: format!("{:?}", spec.root.anchor),
                    root: p.declared_root().map(|r| r.display().to_string()),
                    patterns: spec.children.names.clone(),
                    risk: info.risk,
                    min_age_days: info.min_age_days,
                    description: info.description,
                    on_removal: info.on_removal,
                }
            })
            .collect(),
    }
}

fn verdict(d: &Decision) -> (String, bool) {
    match d {
        Decision::Eligible => ("would be moved to quarantine".to_owned(), true),
        Decision::TooRecent { .. } => ("kept: changed too recently".to_owned(), false),
        Decision::Protected { reason } => (format!("protected: {reason}"), false),
        Decision::Skipped { reason } => (format!("skipped: {reason}"), false),
    }
}

fn dry_run(provider: &DeclarativeProvider, policy: &Policy, now_ms: i64) -> DryRun {
    let pv = preview(
        provider,
        policy,
        now_ms,
        PreviewLimits::default(),
        &AtomicBool::new(false),
    );
    let root = match pv.roots.first() {
        Some(RootReport::Scanned { .. }) => "scanned".to_owned(),
        Some(RootReport::Missing { .. }) => {
            "missing: that folder does not exist on this computer".to_owned()
        }
        Some(RootReport::Unavailable { reason, .. }) => format!("not used: {reason}"),
        None => provider.exclusions().first().map_or_else(
            || "not used: the place it points to is unavailable".to_owned(),
            |e| format!("not used: {}", e.reason),
        ),
    };
    let mut items: Vec<DryRunItem> = pv
        .items
        .iter()
        .map(|i| {
            let (verdict, eligible) = verdict(&i.decision);
            DryRunItem {
                path: i.path.clone(),
                bytes: i.bytes,
                verdict,
                eligible,
            }
        })
        .collect();
    let more_items = items.len().saturating_sub(MAX_SHOWN_ITEMS) as u64;
    items.truncate(MAX_SHOWN_ITEMS);
    DryRun {
        provider_id: provider.info().id,
        root,
        eligible_items: pv.eligible_items,
        eligible_bytes: pv.eligible_bytes,
        items,
        more_items,
    }
}

/// Parse a plugin file and show what it would do. Stores nothing.
pub(crate) fn inspect(
    db: &Db,
    pending: &Pending,
    policy: &Policy,
    anchors: &AnchorPaths,
    file: &Path,
    now_ms: i64,
) -> Result<Inspection, CommandError> {
    let invalid = |m: String| CommandError::new(ErrorKind::InvalidInput, m);
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .and_then(|f| {
            f.take(MAX_MANIFEST_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|e| invalid(format!("The plugin file could not be read: {e}")))?;
    let plugin =
        parse(&bytes).map_err(|e| invalid(format!("This is not a usable plugin: {e}.")))?;

    let existing = db
        .lock()
        .plugins()?
        .into_iter()
        .find(|r| r.id == plugin.manifest.id);
    let providers = DeclarativeProvider::all_for(&plugin, anchors);
    let dry_runs = providers
        .iter()
        .map(|p| dry_run(p, policy, now_ms))
        .collect();
    pending.remember(&plugin);
    Ok(Inspection {
        plugin: summarize(&plugin, anchors, false, 0),
        dry_runs,
        already_installed: existing.as_ref().is_some_and(|r| r.sha256 == plugin.sha256),
        replaces: existing.map(|r| Replaces {
            version: r.version,
            sha256: r.sha256,
            enabled: r.enabled,
        }),
    })
}

/// What is being changed about which plugin, for the audit record.
struct Change<'a> {
    action: &'a str,
    id: &'a str,
    version: &'a str,
    sha256: &'a str,
}

fn audit_plugin(
    db: &Db,
    user: &str,
    now_ms: i64,
    c: &Change<'_>,
    extra: serde_json::Value,
) -> Result<(), CommandError> {
    let Change {
        action,
        id,
        version,
        sha256,
    } = *c;
    let rec = NewAuditRecord {
        at_ms: now_ms,
        operation_id: audit::new_operation_id(),
        kind: AuditKind::Plugin,
        provider: None,
        user: user.to_owned(),
        items: 0,
        bytes: 0,
        policy: format!(
            "Plugin {id} {version}: {action}. A plugin can only describe cleanup candidates; nothing is removed without a separate confirmation."
        ),
        approval: Approval::UserApproved,
        outcome: Outcome::Succeeded,
        errors: vec![],
        details: serde_json::json!({ "action": action, "pluginId": id, "version": version, "sha256": sha256, "more": extra }),
    };
    // Same rule as every change: if it cannot be recorded, it does not happen.
    audit::record(db, &rec).map(|_| ()).map_err(|e| {
        CommandError::new(
            ErrorKind::System,
            format!(
                "The change could not be recorded in the audit log, so nothing was changed: {e}"
            ),
        )
    })
}

/// Approve an inspected plugin. Stores what was inspected, enabled.
pub(crate) fn install(
    db: &Db,
    pending: &Pending,
    anchors: &AnchorPaths,
    user: &str,
    sha256: &str,
    now_ms: i64,
) -> Result<PluginSummary, CommandError> {
    let plugin = pending.take(sha256).ok_or_else(|| {
        CommandError::new(
            ErrorKind::NotFound,
            "That plugin was not inspected in this session. Choose the file again to review it.",
        )
    })?;
    let m = &plugin.manifest;
    let previous = db.lock().plugins()?.into_iter().find(|r| r.id == m.id);
    let action = if previous.is_some() {
        "updated"
    } else {
        "approved"
    };
    audit_plugin(
        db,
        user,
        now_ms,
        &Change {
            action,
            id: &m.id,
            version: &m.version,
            sha256: &plugin.sha256,
        },
        serde_json::json!({ "previousSha256": previous.as_ref().map(|p| &p.sha256), "author": m.author }),
    )?;
    db.lock().save_plugin(&PluginRow {
        id: m.id.clone(),
        version: m.version.clone(),
        sha256: plugin.sha256.clone(),
        manifest: plugin.canonical.clone(),
        enabled: true,
        approved_at_ms: now_ms,
    })?;
    Ok(summarize(&plugin, anchors, true, now_ms))
}

/// Every stored plugin, with a problem noted for any that no longer verifies.
pub(crate) fn list(db: &Db, anchors: &AnchorPaths) -> Result<Vec<PluginSummary>, CommandError> {
    let rows = db.lock().plugins()?;
    Ok(rows
        .into_iter()
        .map(|row| match load_stored(&row.manifest, &row.sha256) {
            Ok(p) => summarize(&p, anchors, row.enabled, row.approved_at_ms),
            Err(e) => PluginSummary {
                id: row.id.clone(),
                name: row.id,
                version: row.version,
                author: String::new(),
                description: String::new(),
                sha256: row.sha256,
                enabled: false,
                approved_at_ms: row.approved_at_ms,
                problem: Some(format!("{e}. It is not used.")),
                providers: Vec::new(),
            },
        })
        .collect())
}

pub(crate) fn set_enabled(
    db: &Db,
    user: &str,
    id: &str,
    enabled: bool,
    now_ms: i64,
) -> Result<(), CommandError> {
    let row = db
        .lock()
        .plugins()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| {
            CommandError::new(ErrorKind::NotFound, format!("There is no plugin {id}."))
        })?;
    audit_plugin(
        db,
        user,
        now_ms,
        &Change {
            action: if enabled { "enabled" } else { "disabled" },
            id: &row.id,
            version: &row.version,
            sha256: &row.sha256,
        },
        serde_json::Value::Null,
    )?;
    db.lock().set_plugin_enabled(id, enabled)?;
    Ok(())
}

pub(crate) fn remove(db: &Db, user: &str, id: &str, now_ms: i64) -> Result<(), CommandError> {
    let row = db
        .lock()
        .plugins()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| {
            CommandError::new(ErrorKind::NotFound, format!("There is no plugin {id}."))
        })?;
    audit_plugin(
        db,
        user,
        now_ms,
        &Change {
            action: "removed",
            id: &row.id,
            version: &row.version,
            sha256: &row.sha256,
        },
        serde_json::Value::Null,
    )?;
    db.lock().remove_plugin(id)?;
    Ok(())
}

/// The built-in cleanup types plus those of enabled plugins that still verify.
pub(crate) fn all_providers(db: &Db) -> Vec<Box<dyn CleanupProvider>> {
    providers_with(db, &AnchorPaths::for_system())
}

pub(crate) fn providers_with(db: &Db, anchors: &AnchorPaths) -> Vec<Box<dyn CleanupProvider>> {
    let mut all = providers::builtin();
    let rows = match db.lock().plugins() {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(error = %err, "plugins could not be read; only built-in cleanup types are offered");
            return all;
        }
    };
    for row in rows.into_iter().filter(|r| r.enabled) {
        match load_stored(&row.manifest, &row.sha256) {
            Ok(plugin) => all.extend(
                DeclarativeProvider::all_for(&plugin, anchors)
                    .into_iter()
                    .map(|p| Box::new(p) as Box<dyn CleanupProvider>),
            ),
            Err(err) => {
                tracing::warn!(plugin = %row.id, error = %err, "a stored plugin no longer verifies; skipped");
            }
        }
    }
    all
}

/// How to show a cleanup type's name where its origin matters.
pub(crate) fn display_name(info: &ProviderInfo) -> String {
    match &info.plugin {
        Some(p) => format!("{} (plugin: {} by {})", info.name, p.plugin_name, p.author),
        None => info.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, SystemTime};

    use sentinel_safety::ProtectedSet;
    use serde_json::json;

    use super::*;

    fn now() -> i64 {
        crate::db::now_ms()
    }

    struct Fx {
        _dir: tempfile::TempDir,
        local: std::path::PathBuf,
        anchors: AnchorPaths,
        policy: Policy,
        db: Db,
        pending: Pending,
    }

    fn fx() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let local = sentinel_safety::CanonicalPath::resolve(dir.path())
            .unwrap()
            .as_path()
            .to_path_buf();
        Fx {
            anchors: AnchorPaths {
                local_app_data: Some(local.clone()),
                ..AnchorPaths::default()
            },
            policy: Policy::new(ProtectedSet::new()),
            db: Db::in_memory(),
            pending: Pending::default(),
            local,
            _dir: dir,
        }
    }

    fn age(path: &Path, days: u64) {
        let f = fs::OpenOptions::new()
            .access_mode(0x0100)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(path)
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
            .unwrap();
    }

    fn plugin_json(version: &str, risk: &str) -> String {
        json!({
            "format": 1, "id": "example.acme", "name": "Acme caches", "version": version,
            "author": "Acme Tools", "description": "Acme caches.",
            "capabilities": ["filesystem.read", "filesystem.cleanup"],
            "cleanup": [{
                "id": "shaders", "name": "Acme shader cache",
                "root": { "anchor": "localAppData", "path": "Acme\\Cache" },
                "children": { "names": ["gl", "dx-*"] },
                "risk": risk, "minAgeDays": 5,
                "description": "Compiled shaders.", "onRemoval": "Recompiled on next run."
            }]
        })
        .to_string()
    }

    fn file_with(f: &Fx, text: &str) -> std::path::PathBuf {
        let p = f.local.join("plugin.json");
        fs::write(&p, text).unwrap();
        p
    }

    fn seed_cache(f: &Fx) {
        let root = f.local.join("Acme").join("Cache");
        fs::create_dir_all(root.join("gl")).unwrap();
        fs::write(root.join("gl").join("a.bin"), vec![0u8; 5000]).unwrap();
        fs::create_dir_all(root.join("keep")).unwrap();
        age(&root.join("gl").join("a.bin"), 30);
        age(&root.join("gl"), 30);
    }

    fn inspect_file(f: &Fx, text: &str) -> Result<Inspection, CommandError> {
        let p = file_with(f, text);
        inspect(&f.db, &f.pending, &f.policy, &f.anchors, &p, now())
    }

    #[test]
    fn inspecting_shows_a_live_dry_run_and_stores_nothing() {
        let f = fx();
        seed_cache(&f);
        let i = inspect_file(&f, &plugin_json("1.0.0", "lowRisk")).unwrap();
        assert_eq!(i.plugin.id, "example.acme");
        assert!(!i.already_installed && i.replaces.is_none());
        assert_eq!(i.plugin.providers[0].patterns, ["gl", "dx-*"]);
        assert!(
            i.plugin.providers[0]
                .root
                .as_ref()
                .unwrap()
                .ends_with("Acme\\Cache")
        );
        let d = &i.dry_runs[0];
        assert_eq!((d.root.as_str(), d.eligible_items), ("scanned", 1));
        assert!(d.items.iter().any(|x| x.path.ends_with("gl") && x.eligible));
        assert!(f.db.lock().plugins().unwrap().is_empty(), "nothing stored");
        assert!(
            f.db.lock().audit_records(10, None).unwrap().is_empty(),
            "nothing audited"
        );
        assert_eq!(
            providers_with(&f.db, &f.anchors).len(),
            providers::builtin().len()
        );
    }

    #[test]
    fn bad_files_are_refused_with_the_reason() {
        let f = fx();
        for (text, expect) in [
            (plugin_json("1.0.0", "safe"), "risk"),
            ("not json".to_owned(), "not a usable plugin"),
            (
                plugin_json("1.0.0", "lowRisk").replace("Acme\\\\Cache", "..\\\\Windows"),
                "dot",
            ),
        ] {
            let e = inspect_file(&f, &text).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput);
            assert!(
                e.message.to_lowercase().contains(expect),
                "{expect}: {}",
                e.message
            );
        }
        let e = inspect(
            &f.db,
            &f.pending,
            &f.policy,
            &f.anchors,
            &f.local.join("missing.json"),
            now(),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        let big = " ".repeat(MAX_MANIFEST_BYTES + 10);
        assert!(inspect_file(&f, &big).is_err());
    }

    #[test]
    fn approving_stores_exactly_what_was_inspected_even_if_the_file_changes_afterwards() {
        let f = fx();
        seed_cache(&f);
        let i = inspect_file(&f, &plugin_json("1.0.0", "lowRisk")).unwrap();
        // The file is swapped for something else after it was inspected.
        file_with(&f, &plugin_json("9.9.9", "mediumRisk"));
        let s = install(
            &f.db,
            &f.pending,
            &f.anchors,
            "tester",
            &i.plugin.sha256,
            now(),
        )
        .unwrap();
        assert_eq!((s.version.as_str(), s.enabled), ("1.0.0", true));
        let rows = f.db.lock().plugins().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].version, "1.0.0");
        assert_eq!(rows[0].sha256, i.plugin.sha256);
        // It was audited, with its fingerprint.
        let log = f.db.lock().audit_records(10, None).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].kind, AuditKind::Plugin);
        assert!(log[0].details.to_string().contains(&i.plugin.sha256));
        // A second approval of the same inspection is not possible.
        assert_eq!(
            install(
                &f.db,
                &f.pending,
                &f.anchors,
                "tester",
                &i.plugin.sha256,
                now()
            )
            .unwrap_err()
            .kind,
            ErrorKind::NotFound
        );
    }

    #[test]
    fn only_something_inspected_this_session_can_be_approved() {
        let f = fx();
        let e = install(
            &f.db,
            &f.pending,
            &f.anchors,
            "tester",
            &"a".repeat(64),
            now(),
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound);
        assert!(f.db.lock().plugins().unwrap().is_empty());
        assert!(f.db.lock().audit_records(10, None).unwrap().is_empty());
    }

    #[test]
    fn enabled_plugins_become_cleanup_types_and_disabled_or_removed_ones_do_not() {
        let f = fx();
        seed_cache(&f);
        let i = inspect_file(&f, &plugin_json("1.0.0", "lowRisk")).unwrap();
        install(
            &f.db,
            &f.pending,
            &f.anchors,
            "tester",
            &i.plugin.sha256,
            now(),
        )
        .unwrap();
        let ids = |f: &Fx| -> Vec<String> {
            providers_with(&f.db, &f.anchors)
                .iter()
                .map(|p| p.info().id)
                .collect()
        };
        assert!(ids(&f).contains(&"plugin:example.acme:shaders".to_owned()));
        let info = providers_with(&f.db, &f.anchors).pop().unwrap().info();
        assert_eq!(info.risk, Risk::LowRisk);
        assert_eq!(
            display_name(&info),
            "Acme shader cache (plugin: Acme caches by Acme Tools)"
        );

        set_enabled(&f.db, "tester", "example.acme", false, now()).unwrap();
        assert!(!ids(&f).iter().any(|i| i.starts_with("plugin:")));
        assert!(!list(&f.db, &f.anchors).unwrap()[0].enabled);
        set_enabled(&f.db, "tester", "example.acme", true, now()).unwrap();
        assert!(ids(&f).iter().any(|i| i.starts_with("plugin:")));

        remove(&f.db, "tester", "example.acme", now()).unwrap();
        assert!(!ids(&f).iter().any(|i| i.starts_with("plugin:")));
        assert!(list(&f.db, &f.anchors).unwrap().is_empty());
        assert_eq!(
            remove(&f.db, "tester", "example.acme", now())
                .unwrap_err()
                .kind,
            ErrorKind::NotFound
        );
        // approved, disabled, enabled, removed
        let actions: Vec<String> =
            f.db.lock()
                .audit_records(10, None)
                .unwrap()
                .iter()
                .map(|r| r.details["action"].as_str().unwrap().to_owned())
                .collect();
        assert_eq!(actions, ["removed", "enabled", "disabled", "approved"]);
    }

    #[test]
    fn a_stored_plugin_that_was_edited_is_reported_and_never_used() {
        let f = fx();
        let i = inspect_file(&f, &plugin_json("1.0.0", "lowRisk")).unwrap();
        install(
            &f.db,
            &f.pending,
            &f.anchors,
            "tester",
            &i.plugin.sha256,
            now(),
        )
        .unwrap();
        // Someone edits the stored text, e.g. to claim a different folder.
        let mut row = f.db.lock().plugins().unwrap().remove(0);
        row.manifest = row.manifest.replace("Acme\\\\Cache", "Other\\\\Place");
        f.db.lock().save_plugin(&row).unwrap();
        assert!(
            !providers_with(&f.db, &f.anchors)
                .iter()
                .any(|p| p.info().id.starts_with("plugin:"))
        );
        let listed = list(&f.db, &f.anchors).unwrap();
        assert!(listed[0].problem.as_ref().unwrap().contains("fingerprint"));
        assert!(!listed[0].enabled);
    }

    #[test]
    fn a_new_version_replaces_the_old_one_after_its_own_approval_and_says_so() {
        let f = fx();
        let a = inspect_file(&f, &plugin_json("1.0.0", "lowRisk")).unwrap();
        install(
            &f.db,
            &f.pending,
            &f.anchors,
            "tester",
            &a.plugin.sha256,
            now(),
        )
        .unwrap();
        let again = inspect_file(&f, &plugin_json("1.0.0", "lowRisk")).unwrap();
        assert!(again.already_installed);
        // A changed file under the same id is a different, unapproved plugin.
        let b = inspect_file(&f, &plugin_json("1.1.0", "mediumRisk")).unwrap();
        assert!(!b.already_installed);
        let r = b.replaces.unwrap();
        assert_eq!(
            (r.version.as_str(), r.sha256.as_str()),
            ("1.0.0", a.plugin.sha256.as_str())
        );
        assert_ne!(b.plugin.sha256, a.plugin.sha256);
        assert_eq!(
            f.db.lock().plugins().unwrap()[0].version,
            "1.0.0",
            "unchanged until approved"
        );
        install(
            &f.db,
            &f.pending,
            &f.anchors,
            "tester",
            &b.plugin.sha256,
            now(),
        )
        .unwrap();
        assert_eq!(f.db.lock().plugins().unwrap()[0].version, "1.1.0");
        let log = f.db.lock().audit_records(10, None).unwrap();
        assert_eq!(log[0].details["action"], "updated");
    }

    #[test]
    fn plugin_items_are_never_in_a_safe_plan_but_can_be_planned_labelled_and_run_through_quarantine()
     {
        use crate::recovery::{self, ApprovedItem};
        use sentinel_quarantine::{EntryStatus, Quarantine};

        let f = fx();
        seed_cache(&f);
        let i = inspect_file(&f, &plugin_json("1.0.0", "lowRisk")).unwrap();
        install(
            &f.db,
            &f.pending,
            &f.anchors,
            "tester",
            &i.plugin.sha256,
            now(),
        )
        .unwrap();
        // Only the plugin's own cleanup types, so the test never looks at this machine.
        let plugin_only: Vec<Box<dyn CleanupProvider>> = providers_with(&f.db, &f.anchors)
            .into_iter()
            .filter(|p| p.info().plugin.is_some())
            .collect();
        assert_eq!(plugin_only.len(), 1);

        // The Safe ceiling never includes a plugin's items; they are reported as available
        // at a higher risk.
        let safe = recovery::build(&f.policy, &plugin_only, &[], now(), 1000, Risk::Safe).unwrap();
        assert!(safe.plan.entries.is_empty());
        assert_eq!(safe.plan.above_ceiling_items, 1);

        // At a low-risk ceiling it is planned, and named as the plugin's.
        let low =
            recovery::build(&f.policy, &plugin_only, &[], now(), 1000, Risk::LowRisk).unwrap();
        assert_eq!(low.plan.entries.len(), 1);
        let e = &low.plan.entries[0];
        assert_eq!(e.source, "plugin:example.acme:shaders");
        assert!(
            e.source_name.contains("plugin: Acme caches by Acme Tools"),
            "{}",
            e.source_name
        );
        assert_eq!(e.risk, Risk::LowRisk);

        // Running it uses the normal quarantine: moved, not deleted, and audited.
        let qdir = tempfile::tempdir().unwrap();
        let q = Quarantine::at(qdir.path().join("q"));
        let approved = [ApprovedItem {
            source: e.source.clone(),
            path: e.path.clone(),
        }];
        let out = recovery::execute(&q, &f.policy, &f.db, "tester", &plugin_only, &[], &approved)
            .unwrap();
        let run = out.parts[0]
            .run
            .as_ref()
            .expect("the plugin's items were run");
        assert_eq!(run.manifest.entries[0].status, EntryStatus::Quarantined);
        assert!(!f.local.join("Acme").join("Cache").join("gl").exists());
        assert!(qdir.path().join("q").exists());
        assert!(
            f.db.lock()
                .audit_records(10, None)
                .unwrap()
                .iter()
                .any(|r| r.provider.as_deref() == Some("plugin:example.acme:shaders"))
        );
    }
}
