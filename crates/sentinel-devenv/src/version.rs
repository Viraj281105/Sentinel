//! Version parsing and requirement matching for runtime versions.
//!
//! Understands the forms projects actually use: exact and prefix versions (`22.4.0`,
//! `3.12`), comparison sets (`>=18 <23`, `>=3.9,<3.13`), `||` alternatives, caret and
//! tilde ranges (`^20.1`, `~3.11`), PEP 440 compatible release (`~=3.11`), wildcards
//! (`3.*`, `18.x`), and minimum-version fields (Cargo `rust-version`, `go.mod`).
//! Anything else is [`Satisfies::Unknown`], never a guess.

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Satisfies {
    Yes,
    No,
    /// The requirement is not in a form Sentinel understands (e.g. `lts/*`).
    Unknown,
}

/// Numeric components of a version: `v22.4.0` -> [22, 4, 0]; `33.0.0-jre` -> [33, 0, 0];
/// `go1.23.1` -> [1, 23, 1]. Stops at the first non-numeric component.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim();
    let s = s.strip_prefix("go").unwrap_or(s);
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let mut out = Vec::new();
    for part in s.split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        out.push(digits.parse().ok()?);
        if digits.len() != part.len() {
            break; // e.g. "0-jre": keep 0, stop
        }
    }
    (!out.is_empty()).then_some(out)
}

fn cmp(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
        .find(|o| *o != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

fn prefix_eq(v: &[u64], p: &[u64]) -> bool {
    p.iter().enumerate().all(|(i, x)| v.get(i) == Some(x))
}

/// A version pattern whose trailing components may be wildcards (`3.*`, `18.x`).
fn pattern(s: &str) -> Option<Vec<u64>> {
    let trimmed = s
        .trim()
        .trim_end_matches(".*")
        .trim_end_matches(".x")
        .trim_end_matches(".X");
    if matches!(trimmed, "*" | "x" | "X" | "") {
        return Some(Vec::new());
    }
    parse_version(trimmed)
}

fn comparator(c: &str, v: &[u64]) -> Option<bool> {
    let c = c.trim();
    let ops = ["~=", "==", ">=", "<=", "!=", ">", "<", "=", "^", "~"];
    let op = ops
        .iter()
        .find(|o| c.starts_with(**o))
        .copied()
        .unwrap_or("");
    let rest = c[op.len()..].trim();
    let p = pattern(rest)?;
    Some(match op {
        "" | "=" | "==" => prefix_eq(v, &p),
        "!=" => !prefix_eq(v, &p),
        ">=" => cmp(v, &p) != Ordering::Less,
        "<=" => cmp(v, &p) != Ordering::Greater || prefix_eq(v, &p),
        ">" => cmp(v, &p) == Ordering::Greater && !prefix_eq(v, &p),
        "<" => cmp(v, &p) == Ordering::Less && !prefix_eq(v, &p),
        "^" => {
            // Same leftmost non-zero component, and at least p.
            let lead = p
                .iter()
                .position(|&x| x != 0)
                .unwrap_or(p.len().saturating_sub(1));
            cmp(v, &p) != Ordering::Less && prefix_eq(v, &p[..=lead.min(p.len().saturating_sub(1))])
        }
        "~" => {
            // ~1.2.3 -> >=1.2.3 <1.3 ; ~1.2 -> <1.3 ; ~1 -> <2
            let keep = if p.len() >= 2 { 2 } else { 1 };
            cmp(v, &p) != Ordering::Less && prefix_eq(v, &p[..keep.min(p.len())])
        }
        "~=" => {
            // PEP 440: ~=3.11 -> >=3.11, ==3.* ; ~=3.11.2 -> >=3.11.2, ==3.11.*
            if p.len() < 2 {
                return None;
            }
            cmp(v, &p) != Ordering::Less && prefix_eq(v, &p[..p.len() - 1])
        }
        _ => return None,
    })
}

/// Does `version` satisfy `requirement`? `minimum` treats a bare version as a minimum
/// (Cargo `rust-version`, `go.mod` `go`), not an exact prefix.
pub fn satisfies(requirement: &str, version: &str, minimum: bool) -> Satisfies {
    let Some(v) = parse_version(version) else {
        return Satisfies::Unknown;
    };
    let req = requirement.trim();
    if req.is_empty() {
        return Satisfies::Unknown;
    }
    let mut any_known = false;
    for alt in req.split("||") {
        let parts: Vec<&str> = alt
            .split([',', ' '])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        // Join a bare operator with the following version (">= 18" -> ">=18").
        let mut comps: Vec<String> = Vec::new();
        for p in parts {
            match comps.last_mut() {
                Some(last) if last.chars().all(|c| "<>=!~^".contains(c)) => last.push_str(p),
                _ => comps.push(p.to_owned()),
            }
        }
        let mut all = true;
        let mut understood = !comps.is_empty();
        for c in &comps {
            let c = if minimum && c.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
                format!(">={c}")
            } else {
                c.clone()
            };
            match comparator(&c, &v) {
                Some(ok) => all &= ok,
                None => understood = false,
            }
        }
        if understood {
            any_known = true;
            if all {
                return Satisfies::Yes;
            }
        }
    }
    if any_known {
        Satisfies::No
    } else {
        Satisfies::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Satisfies::{No, Unknown, Yes};

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("v22.4.0"), Some(vec![22, 4, 0]));
        assert_eq!(parse_version("3.12"), Some(vec![3, 12]));
        assert_eq!(parse_version("33.0.0-jre"), Some(vec![33, 0, 0]));
        assert_eq!(parse_version("go1.23.1"), Some(vec![1, 23, 1]));
        assert_eq!(parse_version("21.0.9+7"), Some(vec![21, 0, 9]));
        assert_eq!(parse_version("stable"), None);
    }

    #[test]
    fn node_style_ranges() {
        assert_eq!(satisfies(">=20", "22.4.0", false), Yes);
        assert_eq!(satisfies(">=20", "18.19.0", false), No);
        assert_eq!(satisfies(">= 18 <21", "20.1.0", false), Yes);
        assert_eq!(satisfies(">=18 <21", "22.0.0", false), No);
        assert_eq!(satisfies("^20.1", "20.9.0", false), Yes);
        assert_eq!(satisfies("^20.1", "21.0.0", false), No);
        assert_eq!(satisfies("^0.3.1", "0.3.9", false), Yes);
        assert_eq!(satisfies("^0.3.1", "0.4.0", false), No);
        assert_eq!(satisfies("~18.2", "18.2.5", false), Yes);
        assert_eq!(satisfies("~18.2", "18.3.0", false), No);
        assert_eq!(satisfies("16 || 18", "18.1.0", false), Yes);
        assert_eq!(satisfies("18.x", "18.20.1", false), Yes);
        assert_eq!(satisfies("22.4.0", "22.4.0", false), Yes);
        assert_eq!(satisfies("22", "22.4.0", false), Yes, ".nvmrc major only");
        assert_eq!(satisfies("lts/*", "22.4.0", false), Unknown);
        assert_eq!(satisfies("node", "22.4.0", false), Unknown);
    }

    #[test]
    fn python_specifiers() {
        assert_eq!(satisfies(">=3.11", "3.14.7", false), Yes);
        assert_eq!(satisfies(">=3.9,<3.13", "3.14.7", false), No);
        assert_eq!(satisfies(">=3.9,<3.13", "3.11.9", false), Yes);
        assert_eq!(satisfies("~=3.11", "3.14.7", false), Yes);
        assert_eq!(satisfies("~=3.11.2", "3.12.0", false), No);
        assert_eq!(satisfies("==3.11.*", "3.11.9", false), Yes);
        assert_eq!(satisfies("!=3.12", "3.12.1", false), No);
        assert_eq!(
            satisfies("3.12", "3.12.4", false),
            Yes,
            ".python-version prefix"
        );
        assert_eq!(satisfies("<=3.12", "3.12.4", false), Yes);
    }

    #[test]
    fn minimum_version_fields() {
        assert_eq!(
            satisfies("1.85", "1.98.1", true),
            Yes,
            "rust-version is a minimum"
        );
        assert_eq!(satisfies("1.85", "1.80.0", true), No);
        assert_eq!(satisfies("1.23", "1.23.1", true), Yes, "go directive");
        assert_eq!(
            satisfies("1.85", "1.98.1", false),
            No,
            "without minimum it is a prefix"
        );
    }
}
