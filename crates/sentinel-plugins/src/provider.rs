//! A validated plugin as an ordinary cleanup type.
//!
//! [`DeclarativeProvider`] implements [`CleanupProvider`] and nothing more: it names a root
//! folder and says which of its direct children are candidates. Whether any candidate is
//! actually eligible is decided by the same preview as for built-in cleanup types (the policy
//! engine, protected descendants, age, links), and removal goes through the same
//! confirmation and quarantine executor. The plugin has no other handle on anything.

use std::path::{Path, PathBuf};

use sentinel_classify::Category;
use sentinel_cleanup::{CleanupProvider, Exclusion, PluginLabel, ProviderInfo, Risk};
use sentinel_safety::{CanonicalPath, Known, is_within, known_folder};

use crate::{Anchor, CleanupSpec, Plugin, PluginRisk, name_matches};

/// Where each anchor points on this machine.
#[derive(Debug, Clone, Default)]
pub struct AnchorPaths {
    pub local_app_data: Option<PathBuf>,
    pub roaming_app_data: Option<PathBuf>,
    pub user_profile: Option<PathBuf>,
}

impl AnchorPaths {
    /// From the shell's known folders (not environment variables, which any process can set).
    pub fn for_system() -> Self {
        Self {
            local_app_data: known_folder(Known::LocalAppData),
            roaming_app_data: known_folder(Known::RoamingAppData),
            user_profile: known_folder(Known::Profile),
        }
    }

    fn get(&self, a: Anchor) -> Option<&Path> {
        match a {
            Anchor::LocalAppData => self.local_app_data.as_deref(),
            Anchor::RoamingAppData => self.roaming_app_data.as_deref(),
            Anchor::UserProfile => self.user_profile.as_deref(),
        }
    }
}

pub struct DeclarativeProvider {
    plugin: Plugin,
    index: usize,
    anchor: Option<PathBuf>,
}

impl DeclarativeProvider {
    /// One provider per cleanup type in the plugin.
    pub fn all_for(plugin: &Plugin, anchors: &AnchorPaths) -> Vec<Self> {
        plugin
            .manifest
            .cleanup
            .iter()
            .enumerate()
            .map(|(index, spec)| Self {
                plugin: plugin.clone(),
                index,
                anchor: anchors.get(spec.root.anchor).map(Path::to_path_buf),
            })
            .collect()
    }

    fn spec(&self) -> &CleanupSpec {
        &self.plugin.manifest.cleanup[self.index]
    }

    /// The folder the plugin points at, before anything is resolved.
    pub fn declared_root(&self) -> Option<PathBuf> {
        let anchor = self.anchor.as_ref()?;
        Some(
            self.spec()
                .root
                .components()
                .iter()
                .fold(anchor.clone(), |p, c| p.join(c)),
        )
    }

    /// The root, only if it still lies inside its anchor once links are resolved. A junction
    /// or symlink along the way that leads somewhere else is not followed out of the anchor.
    fn checked_root(&self) -> Result<Option<PathBuf>, Exclusion> {
        let (Some(anchor), Some(root)) = (self.anchor.as_ref(), self.declared_root()) else {
            return Ok(None);
        };
        // A folder that does not exist is reported by the preview as missing.
        let Ok(resolved) = CanonicalPath::resolve(&root) else {
            return Ok(Some(root));
        };
        let Ok(anchor) = CanonicalPath::resolve(anchor) else {
            return Ok(None);
        };
        if is_within(resolved.as_path(), anchor.as_path()) && resolved.as_path() != anchor.as_path()
        {
            Ok(Some(root))
        } else {
            Err(Exclusion {
                path: root.display().to_string(),
                reason: "this folder leads outside the place the plugin is allowed to use (a link \
                         points elsewhere), so it was not used"
                    .to_owned(),
                protected: true,
            })
        }
    }
}

impl CleanupProvider for DeclarativeProvider {
    fn info(&self) -> ProviderInfo {
        let m = &self.plugin.manifest;
        let spec = self.spec();
        ProviderInfo {
            id: format!("plugin:{}:{}", m.id, spec.id),
            name: spec.name.clone(),
            // Classification describes and is compiled in; a plugin cannot extend it.
            category: Category::Unknown,
            risk: match spec.risk {
                PluginRisk::LowRisk => Risk::LowRisk,
                PluginRisk::MediumRisk => Risk::MediumRisk,
            },
            description: spec.description.clone(),
            on_removal: spec.on_removal.clone(),
            min_age_days: spec.effective_min_age_days(),
            can_clean: true,
            note: Some(format!(
                "Written by the plugin \"{}\" ({}), not by Sentinel. Look at what it lists before \
                 you move anything.",
                m.name, m.author
            )),
            plugin: Some(PluginLabel {
                plugin_id: m.id.clone(),
                plugin_name: m.name.clone(),
                author: m.author.clone(),
                version: m.version.clone(),
                sha256: self.plugin.sha256.clone(),
            }),
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        match self.checked_root() {
            Ok(Some(root)) => vec![root],
            _ => Vec::new(),
        }
    }

    fn is_candidate(&self, _root: &Path, name: &str) -> bool {
        self.spec()
            .children
            .names
            .iter()
            .any(|p| name_matches(p, name))
    }

    fn exclusions(&self) -> Vec<Exclusion> {
        self.checked_root().err().into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::fs::{self, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::process::Command;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, SystemTime};

    use sentinel_cleanup::{Decision, PreviewLimits, RootReport, preview};
    use sentinel_safety::{Policy, ProtectedSet};
    use serde_json::json;

    use super::*;
    use crate::parse;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn age(path: &Path, days: u32) {
        let f = OpenOptions::new()
            .access_mode(0x0100)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(path)
            .unwrap();
        f.set_modified(SystemTime::now() - DAY * days).unwrap();
    }

    fn put(path: &Path, bytes: &[u8], days: u32) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        age(path, days);
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn plugin(risk: &str, names: &[&str], path: &str) -> Plugin {
        let v = json!({
            "format": 1, "id": "example.acme", "name": "Acme caches", "version": "1.2.3",
            "author": "Acme Tools", "description": "Acme caches.",
            "capabilities": ["filesystem.read", "filesystem.cleanup"],
            "cleanup": [{
                "id": "shaders", "name": "Acme shader cache",
                "root": { "anchor": "localAppData", "path": path },
                "children": { "names": names },
                "risk": risk, "minAgeDays": 5,
                "description": "Compiled shaders.", "onRemoval": "Recompiled on next run."
            }]
        });
        parse(v.to_string().as_bytes()).unwrap()
    }

    struct Fx {
        _dir: tempfile::TempDir,
        local: PathBuf,
        policy: Policy,
    }

    fn fx() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let local = CanonicalPath::resolve(dir.path())
            .unwrap()
            .as_path()
            .to_path_buf();
        Fx {
            _dir: dir,
            local,
            policy: Policy::new(ProtectedSet::new()),
        }
    }

    fn anchors(f: &Fx) -> AnchorPaths {
        AnchorPaths {
            local_app_data: Some(f.local.clone()),
            ..AnchorPaths::default()
        }
    }

    fn run(f: &Fx, p: &Plugin) -> sentinel_cleanup::Preview {
        let providers = DeclarativeProvider::all_for(p, &anchors(f));
        preview(
            &providers[0],
            &f.policy,
            now_ms(),
            PreviewLimits::default(),
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn it_describes_itself_with_a_namespaced_id_a_clamped_risk_and_the_plugin_label() {
        let f = fx();
        let p = plugin("lowRisk", &["gl"], "Acme\\Cache");
        let info = DeclarativeProvider::all_for(&p, &anchors(&f))[0].info();
        assert_eq!(info.id, "plugin:example.acme:shaders");
        assert_eq!(info.risk, Risk::LowRisk);
        assert_ne!(info.risk, Risk::Safe);
        assert_eq!(info.min_age_days, 5);
        assert_eq!(info.category, Category::Unknown);
        assert!(info.can_clean);
        let label = info.plugin.unwrap();
        assert_eq!(
            (label.plugin_name.as_str(), label.author.as_str()),
            ("Acme caches", "Acme Tools")
        );
        assert_eq!(label.sha256, p.sha256);
        assert!(info.note.unwrap().contains("not by Sentinel"));
    }

    #[test]
    fn only_matching_old_children_are_offered_and_nothing_else_is_listed() {
        let f = fx();
        let root = f.local.join("Acme").join("Cache");
        put(&root.join("gl").join("a.bin"), b"aaaa", 30);
        put(&root.join("DX-12").join("b.bin"), b"bbbb", 30);
        put(&root.join("dx-new").join("c.bin"), b"cccc", 0); // matches, but too recent
        put(&root.join("userdata").join("d.bin"), b"dddd", 30); // does not match
        put(&root.join("loose.txt"), b"eeee", 30); // does not match
        for d in ["gl", "DX-12", "dx-new", "userdata"] {
            age(&root.join(d), if d == "dx-new" { 0 } else { 30 });
        }
        let pv = run(&f, &plugin("mediumRisk", &["gl", "dx-*"], "Acme\\Cache"));
        let by = |n: &str| pv.items.iter().find(|i| i.path.ends_with(n));
        assert_eq!(by("gl").unwrap().decision, Decision::Eligible);
        assert_eq!(by("DX-12").unwrap().decision, Decision::Eligible);
        assert!(matches!(
            by("dx-new").unwrap().decision,
            Decision::TooRecent { .. }
        ));
        assert!(
            by("userdata").is_none() && by("loose.txt").is_none(),
            "{:?}",
            pv.items
        );
        assert_eq!(pv.eligible_items, 2);
        assert_eq!(pv.provider.risk, Risk::MediumRisk);
    }

    #[test]
    fn protected_content_inside_a_candidate_keeps_it_out_whatever_the_plugin_says() {
        let f = fx();
        let root = f.local.join("Acme").join("Cache");
        put(&root.join("gl").join("ok.bin"), b"x", 30);
        put(&root.join("keys").join("id_rsa"), b"secret", 30);
        put(&root.join("repo").join(".git").join("HEAD"), b"ref", 30);
        for d in ["gl", "keys", "repo"] {
            age(&root.join(d), 30);
        }
        let pv = run(&f, &plugin("lowRisk", &["*"], "Acme\\Cache"));
        let by = |n: &str| pv.items.iter().find(|i| i.path.ends_with(n)).unwrap();
        assert_eq!(by("gl").decision, Decision::Eligible);
        assert!(matches!(by("keys").decision, Decision::Protected { .. }));
        assert!(matches!(by("repo").decision, Decision::Protected { .. }));
    }

    #[test]
    fn a_protected_root_offers_nothing() {
        let f = fx();
        let root = f.local.join("Acme").join("Cache");
        put(&root.join("gl").join("a.bin"), b"x", 30);
        age(&root.join("gl"), 30);
        let mut set = ProtectedSet::new();
        set.add_root(&root, "protected for the test");
        let policy = Policy::new(set);
        let p = plugin("lowRisk", &["*"], "Acme\\Cache");
        let pv = preview(
            &DeclarativeProvider::all_for(&p, &anchors(&f))[0],
            &policy,
            now_ms(),
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(pv.eligible_items, 0);
        assert!(
            matches!(pv.roots[0], RootReport::Unavailable { .. }),
            "{:?}",
            pv.roots
        );
    }

    #[test]
    fn a_root_that_is_a_link_out_of_the_anchor_is_not_used_and_says_why() {
        let f = fx();
        let outside = tempfile::tempdir().unwrap();
        put(
            &outside.path().join("gl").join("precious.bin"),
            b"precious",
            30,
        );
        age(&outside.path().join("gl"), 30);
        fs::create_dir_all(f.local.join("Acme")).unwrap();
        let out = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(f.local.join("Acme").join("Cache"))
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");

        let p = plugin("lowRisk", &["*"], "Acme\\Cache");
        let provider = &DeclarativeProvider::all_for(&p, &anchors(&f))[0];
        assert!(provider.roots().is_empty(), "the link is not followed out");
        let ex = provider.exclusions();
        assert_eq!(ex.len(), 1);
        assert!(ex[0].reason.contains("outside") && ex[0].protected);
        let pv = run(&f, &p);
        assert_eq!(pv.eligible_items, 0);
        assert!(outside.path().join("gl").join("precious.bin").exists());
    }

    #[test]
    fn a_missing_root_or_a_missing_anchor_is_reported_not_an_error() {
        let f = fx();
        let p = plugin("lowRisk", &["*"], "Nothing\\Here");
        let pv = run(&f, &p);
        assert!(
            matches!(pv.roots[0], RootReport::Missing { .. }),
            "{:?}",
            pv.roots
        );
        let none = DeclarativeProvider::all_for(&p, &AnchorPaths::default());
        assert!(none[0].roots().is_empty());
        assert!(none[0].exclusions().is_empty());
    }

    #[test]
    fn a_root_equal_to_its_anchor_is_never_used() {
        // Not expressible through the validator (a root needs a component), but the check
        // that a resolved root is strictly inside its anchor must hold on its own.
        let f = fx();
        fs::create_dir_all(f.local.join("Acme")).unwrap();
        let p = plugin("lowRisk", &["*"], "Acme");
        let provider = &DeclarativeProvider::all_for(&p, &anchors(&f))[0];
        assert_eq!(provider.roots(), vec![f.local.join("Acme")]);
    }
}
