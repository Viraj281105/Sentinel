//! The recovery planner: given a target ("free about N GB") and a risk ceiling, choose which
//! eligible cleanup items to propose, and say why each was chosen.
//!
//! Pure and deterministic: the same candidates, target and ceiling always give the same plan,
//! and nothing here touches the filesystem. A plan is a *proposal*. Every item in it was
//! already judged eligible by a provider's preview (validated by `sentinel-safety`), and it is
//! validated again, item by item, when someone approves it and the executor runs it.
//!
//! The rule, in plain words:
//! 1. Never include anything above the ceiling, and never accept a ceiling above medium risk
//!    (high-risk items are recommendation-only; protected items are never actionable).
//! 2. Use the lowest risk tier first, and move up a tier only when the one below is used up.
//! 3. Inside a tier, take the largest items first so fewer things are moved, except that when
//!    one item is big enough to finish the plan, take the smallest such item so the plan does
//!    not overshoot the target by more than it must.

use std::collections::BTreeMap;

use serde::Serialize;
use ts_rs::TS;

use crate::{Decision, ItemKind, Preview, Risk};

/// One thing that could be removed, as judged by a provider's preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The provider's id, which the executor needs to run it.
    pub source: String,
    pub source_name: String,
    pub path: String,
    pub kind: ItemKind,
    pub bytes: u64,
    pub risk: Risk,
    /// What removal costs and whether the item comes back.
    pub on_removal: String,
}

/// The eligible items of a preview. Analysis-only providers contribute nothing.
pub fn candidates_from_preview(p: &Preview) -> Vec<Candidate> {
    if !p.provider.can_clean {
        return Vec::new();
    }
    p.items
        .iter()
        .filter(|i| i.decision == Decision::Eligible && i.bytes > 0)
        .map(|i| Candidate {
            source: p.provider.id.to_owned(),
            source_name: p.provider.name.to_owned(),
            path: i.path.clone(),
            kind: i.kind,
            bytes: i.bytes,
            risk: p.provider.risk,
            on_removal: p.provider.on_removal.to_owned(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("choose an amount of space to free (more than zero)")]
    NoTarget,
    #[error("plans can include at most medium-risk items; higher risk is never automatic")]
    CeilingTooHigh,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanEntry {
    pub source: String,
    pub source_name: String,
    pub path: String,
    pub kind: ItemKind,
    #[ts(type = "number")]
    pub bytes: u64,
    pub risk: Risk,
    pub on_removal: String,
    /// Why this item is in the plan, in plain words.
    pub why: String,
    /// Space recovered by the plan up to and including this item.
    #[ts(type = "number")]
    pub running_bytes: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Plan {
    #[ts(type = "number")]
    pub target_bytes: u64,
    pub max_risk: Risk,
    pub entries: Vec<PlanEntry>,
    #[ts(type = "number")]
    pub planned_bytes: u64,
    pub reaches_target: bool,
    /// How far short of the target the plan falls (0 when it reaches it).
    #[ts(type = "number")]
    pub shortfall_bytes: u64,
    /// Everything that could be removed within the ceiling, whether or not it is needed.
    #[ts(type = "number")]
    pub available_bytes: u64,
    /// More that exists at a higher risk than the ceiling allows, so the person can decide
    /// whether to raise it. Never included.
    #[ts(type = "number")]
    pub above_ceiling_bytes: u64,
    #[ts(type = "number")]
    pub above_ceiling_items: u64,
    /// Things the person should know, such as why the plan is short.
    pub notes: Vec<String>,
}

fn risk_name(r: Risk) -> &'static str {
    match r {
        Risk::Safe => "safe",
        Risk::LowRisk => "low-risk",
        Risk::MediumRisk => "medium-risk",
        Risk::HighRisk => "high-risk",
        Risk::Protected => "protected",
    }
}

fn gb(b: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let (mut v, mut u) = (b as f64, 0);
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} bytes")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Drop duplicates and overlaps so no byte is counted or moved twice. Where two candidates
/// overlap (the same path, or one inside the other) the lower-risk one is kept; at equal risk
/// the outer one, which already counts the inner one's bytes.
fn dedupe(all: Vec<Candidate>) -> Vec<Candidate> {
    let mut keyed: Vec<(String, Candidate)> = all.into_iter().map(|c| (norm(&c.path), c)).collect();
    keyed.sort_by(|(ka, a), (kb, b)| {
        (a.risk, ka.len(), ka, &a.source).cmp(&(b.risk, kb.len(), kb, &b.source))
    });
    let mut kept: Vec<(String, Candidate)> = Vec::new();
    for (k, c) in keyed {
        let overlaps = kept.iter().any(|(o, _)| {
            k == *o || k.starts_with(&format!("{o}\\")) || o.starts_with(&format!("{k}\\"))
        });
        if !overlaps {
            kept.push((k, c));
        }
    }
    kept.into_iter().map(|(_, c)| c).collect()
}

pub fn plan(
    candidates: &[Candidate],
    target_bytes: u64,
    max_risk: Risk,
) -> Result<Plan, PlanError> {
    if target_bytes == 0 {
        return Err(PlanError::NoTarget);
    }
    if max_risk > Risk::MediumRisk {
        return Err(PlanError::CeilingTooHigh);
    }
    // Nothing at high risk or above is ever actionable, whatever a provider claims.
    let actionable: Vec<Candidate> = dedupe(
        candidates
            .iter()
            .filter(|c| c.risk <= Risk::MediumRisk && c.bytes > 0)
            .cloned()
            .collect(),
    );
    let (within, above): (Vec<_>, Vec<_>) =
        actionable.into_iter().partition(|c| c.risk <= max_risk);
    let available_bytes = within.iter().map(|c| c.bytes).sum();

    let mut tiers: BTreeMap<Risk, Vec<Candidate>> = BTreeMap::new();
    for c in within {
        tiers.entry(c.risk).or_default().push(c);
    }
    let mut entries: Vec<PlanEntry> = Vec::new();
    let mut total = 0u64;
    let mut used_lower: Option<(usize, u64)> = None;
    'tiers: for (risk, mut items) in tiers {
        // Largest first; ties by path so the result never depends on input order.
        items.sort_by(|a, b| {
            b.bytes
                .cmp(&a.bytes)
                .then_with(|| norm(&a.path).cmp(&norm(&b.path)))
        });
        let mut first_in_tier = true;
        while total < target_bytes && !items.is_empty() {
            let remaining = target_bytes - total;
            // The smallest item that finishes the plan, if any; otherwise the largest.
            let finishing = items.iter().rposition(|c| c.bytes >= remaining);
            let (idx, how) = match finishing {
                Some(i) => (i, Pick::Finishes(remaining)),
                None => (0, Pick::Largest),
            };
            let c = items.remove(idx);
            total += c.bytes;
            let mut why = match how {
                Pick::Finishes(r) => format!(
                    "The smallest {} item that covers the {} still needed, so the plan overshoots as little as possible.",
                    risk_name(risk),
                    gb(r)
                ),
                Pick::Largest => format!(
                    "The largest {} item left, so fewer items need to be moved.",
                    risk_name(risk)
                ),
            };
            if first_in_tier && let Some((n, b)) = used_lower {
                why = format!(
                    "Lower-risk options were used up first ({n} items, {}). {why}",
                    gb(b)
                );
            }
            first_in_tier = false;
            entries.push(PlanEntry {
                source: c.source,
                source_name: c.source_name,
                path: c.path,
                kind: c.kind,
                bytes: c.bytes,
                risk: c.risk,
                on_removal: c.on_removal,
                why,
                running_bytes: total,
            });
        }
        if total >= target_bytes {
            break 'tiers;
        }
        used_lower = Some((entries.len(), total));
    }

    let reaches_target = total >= target_bytes;
    let shortfall_bytes = target_bytes.saturating_sub(total);
    let above_ceiling_bytes = above.iter().map(|c| c.bytes).sum();
    let above_ceiling_items = above.len() as u64;
    let mut notes = Vec::new();
    if !reaches_target {
        notes.push(format!(
            "Everything Sentinel can offer within the {} ceiling adds up to {}, which is {} short of the {} you asked for.",
            risk_name(max_risk),
            gb(available_bytes),
            gb(shortfall_bytes),
            gb(target_bytes)
        ));
        if above_ceiling_bytes > 0 {
            notes.push(format!(
                "A further {} in {} items is available at a higher risk. They are not included; raising the ceiling is your choice.",
                gb(above_ceiling_bytes),
                above_ceiling_items
            ));
        }
    }
    if entries.is_empty() && available_bytes == 0 {
        notes.push("Nothing eligible was found. Items that are recent, in use, protected or that Sentinel could not verify are never offered.".into());
    }
    Ok(Plan {
        target_bytes,
        max_risk,
        entries,
        planned_bytes: total,
        reaches_target,
        shortfall_bytes,
        available_bytes,
        above_ceiling_bytes,
        above_ceiling_items,
        notes,
    })
}

enum Pick {
    Finishes(u64),
    Largest,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const MB: u64 = 1024 * 1024;

    fn c(path: &str, mb: u64, risk: Risk) -> Candidate {
        Candidate {
            source: "p".into(),
            source_name: "Provider".into(),
            path: path.into(),
            kind: ItemKind::Folder,
            bytes: mb * MB,
            risk,
            on_removal: "comes back".into(),
        }
    }

    fn paths(p: &Plan) -> Vec<&str> {
        p.entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn lower_risk_is_used_up_before_any_higher_risk_item() {
        let cands = [
            c(r"C:\a", 100, Risk::Safe),
            c(r"C:\b", 50, Risk::Safe),
            c(r"C:\huge", 5000, Risk::MediumRisk),
        ];
        // 120 MB needs both safe items; the huge medium one is not touched even though it
        // alone would cover it.
        let p = plan(&cands, 120 * MB, Risk::MediumRisk).unwrap();
        assert_eq!(paths(&p), [r"C:\a", r"C:\b"]);
        assert!(p.reaches_target);
        assert_eq!(p.planned_bytes, 150 * MB);
        // Past the safe tier, the next tier is entered and the reason says why.
        let p = plan(&cands, 200 * MB, Risk::MediumRisk).unwrap();
        assert_eq!(paths(&p), [r"C:\a", r"C:\b", r"C:\huge"]);
        assert!(
            p.entries[2]
                .why
                .contains("Lower-risk options were used up first (2 items")
        );
    }

    #[test]
    fn the_ceiling_is_never_exceeded_and_what_is_left_out_is_reported() {
        let cands = [
            c(r"C:\a", 100, Risk::Safe),
            c(r"C:\big", 900, Risk::MediumRisk),
        ];
        let p = plan(&cands, 500 * MB, Risk::Safe).unwrap();
        assert_eq!(paths(&p), [r"C:\a"]);
        assert!(!p.reaches_target);
        assert_eq!(p.shortfall_bytes, 400 * MB);
        assert_eq!(p.above_ceiling_bytes, 900 * MB);
        assert_eq!(p.above_ceiling_items, 1);
        assert!(p.notes.iter().any(|n| n.contains("short of")));
        assert!(
            p.notes
                .iter()
                .any(|n| n.contains("raising the ceiling is your choice"))
        );
    }

    #[test]
    fn high_risk_and_protected_are_never_planned_and_cannot_be_a_ceiling() {
        for r in [Risk::HighRisk, Risk::Protected] {
            assert_eq!(plan(&[], MB, r).unwrap_err(), PlanError::CeilingTooHigh);
        }
        let cands = [
            c(r"C:\vol", 9999, Risk::HighRisk),
            c(r"C:\x", 1, Risk::Protected),
        ];
        let p = plan(&cands, MB, Risk::MediumRisk).unwrap();
        assert!(p.entries.is_empty());
        assert_eq!(p.available_bytes, 0);
        assert_eq!(
            p.above_ceiling_bytes, 0,
            "not even offered as 'higher risk'"
        );
        assert_eq!(
            plan(&cands, 0, Risk::Safe).unwrap_err(),
            PlanError::NoTarget
        );
    }

    #[test]
    fn inside_a_tier_the_plan_finishes_with_the_least_overshoot() {
        let cands = [
            c(r"C:\a", 500, Risk::Safe),
            c(r"C:\b", 300, Risk::Safe),
            c(r"C:\c", 120, Risk::Safe),
            c(r"C:\d", 90, Risk::Safe),
        ];
        // Need 600: 500 is the largest that does not finish; then 100 more is needed and
        // the smallest item covering that is 120, not 300.
        let p = plan(&cands, 600 * MB, Risk::Safe).unwrap();
        assert_eq!(paths(&p), [r"C:\a", r"C:\c"]);
        assert_eq!(p.planned_bytes, 620 * MB);
        assert!(p.entries[1].why.contains("smallest safe item that covers"));
        // A single item that covers everything is chosen alone, and it is the smallest one.
        let p = plan(&cands, 100 * MB, Risk::Safe).unwrap();
        assert_eq!(paths(&p), [r"C:\c"]);
    }

    #[test]
    fn the_result_does_not_depend_on_input_order() {
        let mut cands = vec![
            c(r"C:\a", 100, Risk::Safe),
            c(r"C:\b", 100, Risk::Safe),
            c(r"C:\c", 100, Risk::LowRisk),
            c(r"C:\d", 70, Risk::MediumRisk),
        ];
        let first = plan(&cands, 250 * MB, Risk::MediumRisk).unwrap();
        cands.reverse();
        let second = plan(&cands, 250 * MB, Risk::MediumRisk).unwrap();
        assert_eq!(paths(&first), paths(&second));
        assert_eq!(first.planned_bytes, second.planned_bytes);
    }

    #[test]
    fn duplicates_and_nested_items_are_counted_once() {
        let mut dup = c(r"C:\Cache", 100, Risk::MediumRisk);
        dup.source = "other".into();
        let cands = [
            c(r"c:\cache", 100, Risk::Safe),
            dup,
            c(r"C:\Cache\inner", 40, Risk::Safe),
            c(r"C:\Other", 10, Risk::Safe),
        ];
        let p = plan(&cands, 1000 * MB, Risk::MediumRisk).unwrap();
        assert_eq!(p.entries.len(), 2, "{:?}", paths(&p));
        assert_eq!(p.available_bytes, 110 * MB);
        assert_eq!(
            p.entries
                .iter()
                .find(|e| e.path.eq_ignore_ascii_case(r"C:\cache"))
                .unwrap()
                .risk,
            Risk::Safe,
            "the lower-risk copy wins"
        );
    }

    #[test]
    fn where_items_overlap_the_lower_risk_one_is_kept() {
        let cands = [
            c(r"C:\Proj", 500, Risk::MediumRisk),
            c(r"C:\Proj\cache", 80, Risk::Safe),
        ];
        let p = plan(&cands, 1000 * MB, Risk::MediumRisk).unwrap();
        assert_eq!(paths(&p), [r"C:\Proj\cache"]);
        assert_eq!(p.available_bytes, 80 * MB);
    }

    #[test]
    fn nothing_eligible_says_so() {
        let p = plan(&[], 10 * MB, Risk::Safe).unwrap();
        assert!(!p.reaches_target);
        assert!(p.notes.iter().any(|n| n.contains("Nothing eligible")));
    }
}
