//! A preview of what Docker's own prune commands would remove. Nothing is executed.
//!
//! The preview is computed from what was already read from the engine. Each category
//! names the command a person could run themselves in a terminal; Sentinel never runs it,
//! because pruning deletes data that cannot be quarantined or restored.
//!
//! Two figures are kept apart on purpose:
//! - `item_bytes`: the sum of the listed items' sizes, which overstates what is freed
//!   whenever items share layers (images share base layers; build cache entries "shared"
//!   with images free almost nothing);
//! - `docker_reclaimable_bytes`: Docker's own estimate from `docker system df`.
//!
//! Nothing here adds the categories into one grand total, since they overlap.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::docker::{BuildCacheEntry, DockerOverview, UsageRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PruneRisk {
    /// Recreated automatically or trivially.
    Low,
    /// Needs a re-pull, rebuild or restart to get back; some local-only work could be lost.
    Medium,
    /// Can hold data that exists nowhere else.
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PruneItem {
    pub label: String,
    pub detail: String,
    /// Approximate; `null` when Docker does not report it.
    #[ts(type = "number | null")]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PruneCategory {
    pub id: String,
    pub title: String,
    /// For a person to run themselves. Sentinel never runs it.
    pub command: String,
    pub risk: PruneRisk,
    pub items: Vec<PruneItem>,
    /// Sum of the sizes Docker reported for the items (overstates; see the module docs).
    #[ts(type = "number | null")]
    pub item_bytes: Option<u64>,
    /// Docker's own reclaimable estimate for this kind of data, when it gives one.
    #[ts(type = "number | null")]
    pub docker_reclaimable_bytes: Option<u64>,
    pub caution: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrunePreview {
    pub categories: Vec<PruneCategory>,
}

fn sum_known(items: &[PruneItem]) -> Option<u64> {
    let known: Vec<u64> = items.iter().filter_map(|i| i.bytes).collect();
    (!known.is_empty()).then(|| known.iter().sum())
}

fn reclaimable(usage: &[UsageRow], kind: &str) -> Option<u64> {
    usage
        .iter()
        .find(|u| u.kind == kind)
        .and_then(|u| u.reclaimable_bytes)
}

/// A volume named by 64 hex digits was created without a name (`VOLUME` in a Dockerfile,
/// or `-v /path`).
fn anonymous(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

fn cache_item(e: &BuildCacheEntry) -> PruneItem {
    let label: String = e.description.chars().take(110).collect();
    PruneItem {
        label: if label.is_empty() {
            e.id.clone()
        } else {
            label
        },
        detail: format!(
            "{} · last used {}{}",
            e.cache_type,
            if e.last_used.is_empty() {
                "unknown"
            } else {
                &e.last_used
            },
            if e.shared {
                " · shared with an image"
            } else {
                ""
            }
        ),
        bytes: e.size_bytes,
    }
}

/// What Docker's prune commands would remove, from what is already known. Pure.
pub fn build_preview(o: &DockerOverview) -> PrunePreview {
    let mut categories = Vec::new();

    let stopped: Vec<PruneItem> = o
        .containers
        .iter()
        .filter(|c| matches!(c.state.as_str(), "exited" | "created" | "dead"))
        .map(|c| PruneItem {
            label: c.name.clone(),
            detail: format!("{} · {}", c.image, c.status),
            bytes: None,
        })
        .collect();
    categories.push(PruneCategory {
        id: "containers".into(),
        title: "Stopped containers".into(),
        command: "docker container prune".into(),
        risk: PruneRisk::Medium,
        item_bytes: None,
        docker_reclaimable_bytes: reclaimable(&o.usage, "Containers"),
        caution:
            "Anything a container wrote to its own filesystem (not to a volume) is lost with it. \
                  Removing a stopped container can also leave its image unused."
                .into(),
        items: stopped,
    });

    let dangling: Vec<PruneItem> = o
        .images
        .iter()
        .filter(|i| i.dangling && i.containers == 0)
        .map(|i| PruneItem {
            label: i.id.clone(),
            detail: format!("untagged · {}", i.created),
            bytes: i.size_bytes,
        })
        .collect();
    categories.push(PruneCategory {
        id: "dangling-images".into(),
        title: "Untagged images".into(),
        command: "docker image prune".into(),
        risk: PruneRisk::Low,
        item_bytes: sum_known(&dangling),
        docker_reclaimable_bytes: None,
        caution:
            "Usually superseded builds. Layers shared with other images are not freed, so the \
                  total is an upper bound."
                .into(),
        items: dangling,
    });

    let unused: Vec<PruneItem> = o
        .images
        .iter()
        .filter(|i| !i.dangling && i.containers == 0)
        .map(|i| PruneItem {
            label: format!("{}:{}", i.repository, i.tag),
            detail: i.created.clone(),
            bytes: i.size_bytes,
        })
        .collect();
    categories.push(PruneCategory {
        id: "unused-images".into(),
        title: "Tagged images no container uses".into(),
        command: "docker image prune -a".into(),
        risk: PruneRisk::Medium,
        item_bytes: sum_known(&unused),
        docker_reclaimable_bytes: reclaimable(&o.usage, "Images"),
        caution: "Getting one back means pulling it again or rebuilding it. An image you built \
                  locally can only come back from its Dockerfile and sources, which may no longer \
                  exist. Layers shared between images are counted once by Docker, so the item \
                  total overstates what is freed."
            .into(),
        items: unused,
    });

    let cache: Vec<PruneItem> = o
        .build_cache
        .iter()
        .filter(|e| !e.in_use)
        .map(cache_item)
        .collect();
    let shared = o
        .build_cache
        .iter()
        .filter(|e| !e.in_use && e.shared)
        .count();
    categories.push(PruneCategory {
        id: "build-cache".into(),
        title: "Build cache not in use".into(),
        command: "docker builder prune".into(),
        risk: PruneRisk::Low,
        item_bytes: sum_known(&cache),
        docker_reclaimable_bytes: reclaimable(&o.usage, "Build Cache"),
        caution: if shared > 0 {
            format!(
                "{shared} of these entries are shared with images, so removing them frees little; \
                 trust Docker's own estimate over the item total. The next build is slower while \
                 the cache is rebuilt."
            )
        } else {
            "The next build is slower while the cache is rebuilt.".into()
        },
        items: cache,
    });

    let volumes: Vec<PruneItem> = o
        .volumes
        .iter()
        .filter(|v| v.links == Some(0))
        .map(|v| PruneItem {
            label: v.name.clone(),
            detail: [
                Some(if anonymous(&v.name) {
                    "anonymous (prune removes it)".to_owned()
                } else {
                    "named (only `prune -a` removes it)".to_owned()
                }),
                v.compose_project
                    .as_ref()
                    .map(|p| format!("Compose project {p}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
            bytes: v.size_bytes,
        })
        .collect();
    categories.push(PruneCategory {
        id: "volumes".into(),
        title: "Volumes no container uses".into(),
        command: "docker volume prune".into(),
        risk: PruneRisk::High,
        item_bytes: sum_known(&volumes),
        docker_reclaimable_bytes: reclaimable(&o.usage, "Local Volumes"),
        caution:
            "Volumes hold databases, uploads and other data that usually exists nowhere else, \
                  and a volume is only \"unused\" while its containers are stopped or removed. \
                  Sentinel never recommends this."
                .into(),
        items: volumes,
    });

    PrunePreview { categories }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::docker::{DockerContainer, DockerImage, DockerState, DockerVolume};

    fn overview() -> DockerOverview {
        DockerOverview {
            state: DockerState::Running,
            cli_path: None,
            server_version: None,
            usage: vec![
                UsageRow {
                    kind: "Images".into(),
                    total: Some(3),
                    active: Some(1),
                    size_bytes: Some(6_000),
                    reclaimable_bytes: Some(3_000),
                },
                UsageRow {
                    kind: "Build Cache".into(),
                    total: Some(2),
                    active: Some(0),
                    size_bytes: Some(900),
                    reclaimable_bytes: Some(0),
                },
                UsageRow {
                    kind: "Local Volumes".into(),
                    total: Some(2),
                    active: Some(1),
                    size_bytes: Some(70),
                    reclaimable_bytes: Some(20),
                },
            ],
            containers: vec![
                DockerContainer {
                    id: "1".into(),
                    name: "web".into(),
                    image: "app:latest".into(),
                    state: "running".into(),
                    status: "Up".into(),
                },
                DockerContainer {
                    id: "2".into(),
                    name: "old".into(),
                    image: "x".into(),
                    state: "exited".into(),
                    status: "Exited (0)".into(),
                },
                DockerContainer {
                    id: "3".into(),
                    name: "frozen".into(),
                    image: "x".into(),
                    state: "paused".into(),
                    status: "Paused".into(),
                },
            ],
            images: vec![
                DockerImage {
                    id: "a".into(),
                    repository: "app".into(),
                    tag: "latest".into(),
                    size_bytes: Some(2_000),
                    created: "1d".into(),
                    dangling: false,
                    containers: 1,
                },
                DockerImage {
                    id: "b".into(),
                    repository: "redis".into(),
                    tag: "7".into(),
                    size_bytes: Some(3_000),
                    created: "2w".into(),
                    dangling: false,
                    containers: 0,
                },
                DockerImage {
                    id: "c".into(),
                    repository: "<none>".into(),
                    tag: "<none>".into(),
                    size_bytes: Some(1_000),
                    created: "3m".into(),
                    dangling: true,
                    containers: 0,
                },
            ],
            volumes: vec![
                DockerVolume {
                    name: "pgdata".into(),
                    driver: "local".into(),
                    size_bytes: Some(50),
                    links: Some(0),
                    compose_project: Some("shop".into()),
                },
                DockerVolume {
                    name: "a".repeat(64),
                    driver: "local".into(),
                    size_bytes: Some(20),
                    links: Some(0),
                    compose_project: None,
                },
                DockerVolume {
                    name: "busy".into(),
                    driver: "local".into(),
                    size_bytes: Some(5),
                    links: Some(1),
                    compose_project: None,
                },
                DockerVolume {
                    name: "unknown".into(),
                    driver: "local".into(),
                    size_bytes: None,
                    links: None,
                    compose_project: None,
                },
            ],
            build_cache: vec![
                BuildCacheEntry {
                    id: "x1".into(),
                    cache_type: "regular".into(),
                    description: "pulled from docker.io/library/python".into(),
                    size_bytes: Some(400),
                    in_use: false,
                    shared: true,
                    last_used: "8 days ago".into(),
                },
                BuildCacheEntry {
                    id: "x2".into(),
                    cache_type: "regular".into(),
                    description: String::new(),
                    size_bytes: Some(500),
                    in_use: false,
                    shared: false,
                    last_used: String::new(),
                },
                BuildCacheEntry {
                    id: "x3".into(),
                    cache_type: "regular".into(),
                    description: "busy".into(),
                    size_bytes: Some(9),
                    in_use: true,
                    shared: false,
                    last_used: "now".into(),
                },
            ],
            warnings: vec![],
            prune_preview: None,
        }
    }

    fn cat<'a>(p: &'a PrunePreview, id: &str) -> &'a PruneCategory {
        p.categories.iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn lists_only_what_each_prune_would_remove() {
        let p = build_preview(&overview());
        assert_eq!(p.categories.len(), 5);

        let c = cat(&p, "containers");
        assert_eq!(
            c.items.len(),
            1,
            "running and paused containers are left alone"
        );
        assert_eq!(c.items[0].label, "old");
        assert_eq!(c.command, "docker container prune");

        assert_eq!(cat(&p, "dangling-images").items.len(), 1);
        assert_eq!(cat(&p, "dangling-images").risk, PruneRisk::Low);
        let unused = cat(&p, "unused-images");
        assert_eq!(
            unused.items.len(),
            1,
            "the image in use by a container is excluded"
        );
        assert_eq!(unused.items[0].label, "redis:7");
        assert_eq!(unused.docker_reclaimable_bytes, Some(3_000));
        assert_eq!(unused.risk, PruneRisk::Medium);
    }

    #[test]
    fn build_cache_leads_with_dockers_figure_and_warns_about_shared_entries() {
        let p = build_preview(&overview());
        let c = cat(&p, "build-cache");
        assert_eq!(c.items.len(), 2, "in-use entry excluded");
        assert_eq!(c.item_bytes, Some(900));
        assert_eq!(c.docker_reclaimable_bytes, Some(0));
        assert!(c.caution.contains("1 of these entries are shared"));
        assert_eq!(
            c.items[1].label, "x2",
            "falls back to the id without a description"
        );
        assert!(c.items[0].detail.contains("shared with an image"));
    }

    #[test]
    fn volumes_are_high_risk_and_only_unlinked_ones_are_listed() {
        let p = build_preview(&overview());
        let c = cat(&p, "volumes");
        assert_eq!(c.risk, PruneRisk::High);
        assert_eq!(
            c.items.len(),
            2,
            "linked and unknown-use volumes are excluded"
        );
        let named = c.items.iter().find(|i| i.label == "pgdata").unwrap();
        assert!(named.detail.contains("only `prune -a` removes it"));
        assert!(named.detail.contains("Compose project shop"));
        let anon = c.items.iter().find(|i| i.label.len() == 64).unwrap();
        assert!(anon.detail.contains("anonymous"));
        assert!(c.caution.contains("never recommends"));
        assert_eq!(c.item_bytes, Some(70));
    }

    #[test]
    fn an_empty_engine_gives_empty_categories_and_no_total() {
        let mut o = overview();
        o.containers.clear();
        o.images.clear();
        o.volumes.clear();
        o.build_cache.clear();
        o.usage.clear();
        let p = build_preview(&o);
        assert!(
            p.categories
                .iter()
                .all(|c| c.items.is_empty() && c.item_bytes.is_none())
        );
    }
}
