//! The structured query a model may produce, and its strict validation.
//!
//! A model's answer is untrusted text. It is accepted only if it is exactly one JSON object
//! with known fields, in range, with categories from the fixed vocabulary. Anything else is
//! an error: nothing is repaired, defaulted around, or guessed (docs/AI_POLICY.md, 5).

use serde::{Deserialize, Serialize};

const MAX_CATEGORIES: usize = 8;
const MAX_PATH_TEXT: usize = 80;
const MAX_BYTES: u64 = 1 << 50;
const MAX_DAYS: u32 = 36_500;
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    Folders,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    SizeDesc,
    SizeAsc,
    Name,
}

/// A read-only filter over saved analyses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredQuery {
    pub target: Target,
    /// Category keys from the vocabulary; empty means any.
    pub categories: Vec<String>,
    /// Text the path must contain (case-insensitive).
    pub path_contains: Option<String>,
    pub min_bytes: Option<u64>,
    /// Not changed for at least this many days.
    pub older_than_days: Option<u32>,
    pub sort: Sort,
    pub limit: u32,
}

/// What the model may name. Built from the application's own category list.
#[derive(Debug, Clone)]
pub struct QueryVocabulary {
    pub categories: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("the answer was not a single JSON object")]
    NotJson,
    #[error("the answer did not match the expected format: {0}")]
    Malformed(String),
    #[error("{0} is not a category Sentinel knows")]
    UnknownCategory(String),
    #[error("{field} is out of range")]
    OutOfRange { field: &'static str },
    #[error("{field} contains characters that are not allowed")]
    BadText { field: &'static str },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Raw {
    target: Target,
    #[serde(default)]
    categories: Vec<String>,
    path_contains: Option<String>,
    min_bytes: Option<u64>,
    older_than_days: Option<u32>,
    sort: Option<Sort>,
    limit: Option<u32>,
}

/// The JSON object in a model's answer. Allows one surrounding code fence (models add them
/// habitually) and nothing else: no prose before or after.
fn extract_object(answer: &str) -> Result<&str, QueryError> {
    let mut s = answer.trim();
    if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        s = rest
            .trim()
            .strip_suffix("```")
            .ok_or(QueryError::NotJson)?
            .trim();
    }
    (s.starts_with('{') && s.ends_with('}'))
        .then_some(s)
        .ok_or(QueryError::NotJson)
}

fn clean_text(s: Option<String>, field: &'static str) -> Result<Option<String>, QueryError> {
    let Some(s) = s else { return Ok(None) };
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.chars().count() > MAX_PATH_TEXT {
        return Err(QueryError::OutOfRange { field });
    }
    if s.chars().any(char::is_control) {
        return Err(QueryError::BadText { field });
    }
    Ok(Some(s.to_owned()))
}

/// Validate a model's answer against the vocabulary.
pub fn parse_query(answer: &str, vocab: &QueryVocabulary) -> Result<StructuredQuery, QueryError> {
    let json = extract_object(answer)?;
    let raw: Raw = serde_json::from_str(json).map_err(|e| QueryError::Malformed(e.to_string()))?;

    if raw.categories.len() > MAX_CATEGORIES {
        return Err(QueryError::OutOfRange {
            field: "categories",
        });
    }
    let mut categories = Vec::new();
    for c in &raw.categories {
        let known = vocab
            .categories
            .iter()
            .find(|k| k.eq_ignore_ascii_case(c.trim()))
            .ok_or_else(|| QueryError::UnknownCategory(c.chars().take(40).collect()))?;
        if !categories.contains(known) {
            categories.push(known.clone());
        }
    }
    if raw.min_bytes.is_some_and(|b| b > MAX_BYTES) {
        return Err(QueryError::OutOfRange { field: "minBytes" });
    }
    if raw.older_than_days.is_some_and(|d| d == 0 || d > MAX_DAYS) {
        return Err(QueryError::OutOfRange {
            field: "olderThanDays",
        });
    }
    let limit = match raw.limit {
        None => DEFAULT_LIMIT,
        Some(l) if (1..=MAX_LIMIT).contains(&l) => l,
        Some(_) => return Err(QueryError::OutOfRange { field: "limit" }),
    };
    Ok(StructuredQuery {
        target: raw.target,
        categories,
        path_contains: clean_text(raw.path_contains, "pathContains")?,
        min_bytes: raw.min_bytes,
        older_than_days: raw.older_than_days,
        sort: raw.sort.unwrap_or(Sort::SizeDesc),
        limit,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn vocab() -> QueryVocabulary {
        QueryVocabulary {
            categories: vec![
                "developerDependencies".into(),
                "windows".into(),
                "userData".into(),
            ],
        }
    }

    #[test]
    fn accepts_a_complete_valid_answer_and_normalizes_categories() {
        let q = parse_query(
            r#"{"target":"folders","categories":["DEVELOPERDEPENDENCIES","windows","windows"],
                "pathContains":"  projects ","minBytes":1073741824,"olderThanDays":90,
                "sort":"sizeAsc","limit":5}"#,
            &vocab(),
        )
        .unwrap();
        assert_eq!(q.target, Target::Folders);
        assert_eq!(
            q.categories,
            ["developerDependencies", "windows"],
            "vocabulary spelling, deduplicated"
        );
        assert_eq!(q.path_contains.as_deref(), Some("projects"));
        assert_eq!(
            (q.min_bytes, q.older_than_days, q.sort, q.limit),
            (Some(1 << 30), Some(90), Sort::SizeAsc, 5)
        );
    }

    #[test]
    fn defaults_apply_only_to_fields_that_are_absent() {
        let q = parse_query(r#"{"target":"files"}"#, &vocab()).unwrap();
        assert_eq!((q.sort, q.limit), (Sort::SizeDesc, 20));
        assert!(q.categories.is_empty() && q.path_contains.is_none());
        let q = parse_query(r#"{"target":"files","pathContains":"   "}"#, &vocab()).unwrap();
        assert_eq!(q.path_contains, None);
    }

    #[test]
    fn one_code_fence_is_tolerated_prose_is_not() {
        assert!(parse_query("```json\n{\"target\":\"files\"}\n```", &vocab()).is_ok());
        assert!(parse_query("```\n{\"target\":\"files\"}\n```", &vocab()).is_ok());
        for bad in [
            "Sure! Here you go: {\"target\":\"files\"}",
            "{\"target\":\"files\"} Hope that helps!",
            "```json\n{\"target\":\"files\"}",
            "[{\"target\":\"files\"}]",
            "",
            "I cannot help with that.",
            "{\"target\":\"files\"}{\"target\":\"folders\"}",
        ] {
            let r = parse_query(bad, &vocab());
            assert!(
                matches!(r, Err(QueryError::NotJson | QueryError::Malformed(_))),
                "{bad:?} must be refused, got {r:?}"
            );
        }
    }

    #[test]
    fn unknown_fields_and_wrong_types_are_refused_not_ignored() {
        for bad in [
            r#"{"target":"files","delete":true}"#,
            r#"{"target":"files","command":"rm -rf"}"#,
            r#"{"target":"everything"}"#,
            r#"{"target":"files","limit":"ten"}"#,
            r#"{"target":"files","minBytes":-5}"#,
            r#"{"categories":["windows"]}"#,
            r#"{"target":"files","sort":"random"}"#,
        ] {
            assert!(
                matches!(parse_query(bad, &vocab()), Err(QueryError::Malformed(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn ranges_and_the_vocabulary_are_enforced() {
        let e = |j: &str| parse_query(j, &vocab()).unwrap_err();
        assert_eq!(
            e(r#"{"target":"files","limit":0}"#),
            QueryError::OutOfRange { field: "limit" }
        );
        assert_eq!(
            e(r#"{"target":"files","limit":201}"#),
            QueryError::OutOfRange { field: "limit" }
        );
        assert_eq!(
            e(r#"{"target":"files","olderThanDays":0}"#),
            QueryError::OutOfRange {
                field: "olderThanDays"
            }
        );
        assert_eq!(
            e(r#"{"target":"files","olderThanDays":40000}"#),
            QueryError::OutOfRange {
                field: "olderThanDays"
            }
        );
        assert_eq!(
            e(&format!(r#"{{"target":"files","minBytes":{}}}"#, u64::MAX)),
            QueryError::OutOfRange { field: "minBytes" }
        );
        assert_eq!(
            e(r#"{"target":"files","categories":["passwords"]}"#),
            QueryError::UnknownCategory("passwords".into())
        );
        let nine = r#"["windows","windows","windows","windows","windows","windows","windows","windows","windows"]"#;
        assert_eq!(
            e(&format!(r#"{{"target":"files","categories":{nine}}}"#)),
            QueryError::OutOfRange {
                field: "categories"
            }
        );
        let long = "a".repeat(81);
        assert_eq!(
            e(&format!(r#"{{"target":"files","pathContains":"{long}"}}"#)),
            QueryError::OutOfRange {
                field: "pathContains"
            }
        );
        assert_eq!(
            e(r#"{"target":"files","pathContains":"a\u0000b"}"#),
            QueryError::BadText {
                field: "pathContains"
            }
        );
    }
}
