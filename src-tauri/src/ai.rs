//! The optional AI assistant: saved settings and the "ask a question" flow.
//!
//! Policy: docs/AI_POLICY.md. Off until someone turns it on; the model only ever sees the
//! person's own redacted question; its answer is a query that is validated and then run
//! read-only through the same entry point as any other query.

use std::path::Path;
use std::time::Duration;

use sentinel_ai::{
    AiError, AiProvider, OllamaConfig, OllamaProvider, Redactor, TranslateError,
    translate_detailed, validate_loopback_url,
};
use sentinel_classify::Classification;
use sentinel_store::ScanId;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};
use crate::db::Db;
use crate::query::{self, QueryResult};

const KEY_ENABLED: &str = "ai.enabled";
const KEY_URL: &str = "ai.ollama.url";
const KEY_MODEL: &str = "ai.ollama.model";
const MAX_MODEL_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AiSettings {
    /// Off unless someone turned it on.
    pub enabled: bool,
    /// A local Ollama server. Must be on this computer.
    pub base_url: String,
    pub model: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        let d = OllamaConfig::default();
        Self {
            enabled: false,
            base_url: d.base_url,
            model: d.model,
        }
    }
}

/// What the assistant did for one question, with everything a person needs to check it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AskResponse {
    /// Always true: this answer's query was written by a model, not by the person.
    pub ai_generated: bool,
    pub provider: String,
    pub model: String,
    /// The exact (redacted) question text the model was given.
    pub sent_question: String,
    /// The query the model produced, in plain words, before any results.
    pub query_description: String,
    pub result: QueryResult,
}

pub(crate) fn load(db: &Db) -> Result<AiSettings, CommandError> {
    let store = db.lock();
    let defaults = AiSettings::default();
    Ok(AiSettings {
        enabled: store.setting(KEY_ENABLED)?.as_deref() == Some("true"),
        base_url: store.setting(KEY_URL)?.unwrap_or(defaults.base_url),
        model: store.setting(KEY_MODEL)?.unwrap_or(defaults.model),
    })
}

/// Validate and save. A non-local address is refused here, not just when asking.
pub(crate) fn save(db: &Db, wanted: &AiSettings) -> Result<AiSettings, CommandError> {
    let base_url = validate_loopback_url(&wanted.base_url).map_err(|e| {
        CommandError::new(
            ErrorKind::Refused,
            format!("{e}. Use an address on this computer, such as http://127.0.0.1:11434."),
        )
    })?;
    let model = wanted.model.trim().to_owned();
    if model.is_empty()
        || model.chars().count() > MAX_MODEL_CHARS
        || model.chars().any(char::is_control)
    {
        return Err(CommandError::new(
            ErrorKind::InvalidInput,
            "Enter the name of the local model to use, for example llama3.2.",
        ));
    }
    let saved = AiSettings {
        enabled: wanted.enabled,
        base_url,
        model,
    };
    let store = db.lock();
    store.set_setting(KEY_ENABLED, if saved.enabled { "true" } else { "false" })?;
    store.set_setting(KEY_URL, &saved.base_url)?;
    store.set_setting(KEY_MODEL, &saved.model)?;
    tracing::info!(enabled = saved.enabled, model = %saved.model, "AI settings saved");
    Ok(saved)
}

/// The provider for these settings, built only if AI is on.
pub(crate) fn provider(settings: &AiSettings) -> Result<OllamaProvider, CommandError> {
    if !settings.enabled {
        return Err(refused_off());
    }
    OllamaProvider::new(&OllamaConfig {
        base_url: settings.base_url.clone(),
        model: settings.model.clone(),
        timeout: Duration::from_secs(60),
    })
    .map_err(|e| map_ai(&e))
}

fn refused_off() -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        "AI features are turned off. Turn them on in Settings first.",
    )
}

fn map_ai(e: &AiError) -> CommandError {
    match e {
        AiError::Disabled => refused_off(),
        AiError::NotLocal(_) => CommandError::new(ErrorKind::Refused, e.to_string()),
        AiError::Unavailable(_) | AiError::Timeout => CommandError::new(
            ErrorKind::System,
            format!("{e}. Is Ollama running on this computer, and is the model downloaded?"),
        ),
        AiError::BadResponse(_) | AiError::TooLarge => {
            CommandError::new(ErrorKind::System, e.to_string())
        }
    }
}

fn map_translate(e: TranslateError) -> CommandError {
    match e {
        TranslateError::Empty | TranslateError::TooLong => {
            CommandError::new(ErrorKind::InvalidInput, format!("{e}."))
        }
        TranslateError::Provider(inner) => map_ai(&inner),
        TranslateError::InvalidAnswer(_) => CommandError::new(
            ErrorKind::InvalidInput,
            format!("{e}. Nothing was run. Try asking in other words."),
        ),
    }
}

/// Answer `question` about a saved analysis. The query the model writes is serialized and
/// run through [`query::run_json`], so it is validated a second time at the single entry
/// point, exactly as a query written by a person would be.
pub(crate) fn ask(
    db: &Db,
    classify: &dyn Fn(&Path) -> Option<Classification>,
    settings: &AiSettings,
    provider: &dyn AiProvider,
    redactor: &mut Redactor,
    scan_id: Option<ScanId>,
    question: &str,
) -> Result<AskResponse, CommandError> {
    if !settings.enabled {
        return Err(refused_off());
    }
    // Do not spend a model call when there is nothing to query.
    {
        let store = db.lock();
        let found = match scan_id {
            Some(id) => store.scan(id)?,
            None => store.latest_scan()?,
        };
        if found.is_none() {
            return Err(CommandError::new(
                ErrorKind::NotFound,
                "There is no saved analysis to ask about. Analyze a drive on the Storage page first.",
            ));
        }
    }
    let t = translate_detailed(provider, redactor, &query::vocabulary(), question)
        .map_err(map_translate)?;
    let json = serde_json::to_string(&t.query)
        .map_err(|e| CommandError::internal("preparing the query", e))?;
    let result = query::run_json(db, classify, scan_id, &json)?;
    Ok(AskResponse {
        ai_generated: true,
        provider: provider.id().to_owned(),
        model: settings.model.clone(),
        sent_question: t.sent,
        query_description: t.query.describe(),
        result,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::Mutex;

    use sentinel_ai::{AiRequest, RedactionContext};
    use sentinel_classify::Classifier;
    use sentinel_scanner::scan::{ScanControl, ScanOptions, scan};
    use sentinel_store::Retention;

    use super::*;

    struct Fake {
        answer: String,
        calls: Mutex<u32>,
    }

    impl Fake {
        fn says(a: &str) -> Self {
            Self {
                answer: a.to_owned(),
                calls: Mutex::new(0),
            }
        }
        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl AiProvider for Fake {
        fn id(&self) -> &str {
            "fake"
        }
        fn complete(&self, _req: &AiRequest) -> Result<String, AiError> {
            *self.calls.lock().unwrap() += 1;
            Ok(self.answer.clone())
        }
    }

    fn on() -> AiSettings {
        AiSettings {
            enabled: true,
            ..AiSettings::default()
        }
    }

    fn redactor() -> Redactor {
        Redactor::new(RedactionContext {
            user_names: vec!["VIRAJ".into()],
            host_names: vec![],
        })
    }

    fn with_analysis() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big").join("blob.bin");
        std::fs::create_dir_all(big.parent().unwrap()).unwrap();
        std::fs::write(&big, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let tree = scan(dir.path(), &ScanOptions::default(), &ScanControl::new()).unwrap();
        let db = Db::in_memory();
        db.lock()
            .save_scan(&tree, &[], 1, crate::db::now_ms(), Retention::default())
            .unwrap();
        (dir, db)
    }

    fn classify(c: &Classifier) -> impl Fn(&Path) -> Option<Classification> + '_ {
        |p| c.classify(p)
    }

    #[test]
    fn off_by_default_and_settings_roundtrip() {
        let db = Db::in_memory();
        let s = load(&db).unwrap();
        assert!(!s.enabled);
        assert_eq!(s, AiSettings::default());
        let saved = save(
            &db,
            &AiSettings {
                enabled: true,
                base_url: "http://localhost:11434/".into(),
                model: "  qwen2.5  ".into(),
            },
        )
        .unwrap();
        assert_eq!(saved.base_url, "http://127.0.0.1:11434");
        assert_eq!(saved.model, "qwen2.5");
        assert_eq!(load(&db).unwrap(), saved);
    }

    #[test]
    fn a_remote_address_or_empty_model_is_refused_and_nothing_is_saved() {
        let db = Db::in_memory();
        for url in [
            "https://api.example.com",
            "http://192.168.1.9:11434",
            "http://evil.com",
        ] {
            let e = save(
                &db,
                &AiSettings {
                    enabled: true,
                    base_url: url.into(),
                    model: "llama3.2".into(),
                },
            )
            .unwrap_err();
            assert_eq!(e.kind, ErrorKind::Refused, "{url}");
        }
        let e = save(
            &db,
            &AiSettings {
                enabled: true,
                model: "   ".into(),
                ..AiSettings::default()
            },
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        assert_eq!(load(&db).unwrap(), AiSettings::default());
    }

    #[test]
    fn no_provider_is_built_while_ai_is_off() {
        let e = provider(&AiSettings::default()).err().unwrap();
        assert_eq!(e.kind, ErrorKind::Refused);
        assert!(provider(&on()).is_ok());
    }

    #[test]
    fn asking_while_off_never_reaches_the_model() {
        let (_d, db) = with_analysis();
        let c = Classifier::new(&std::collections::HashMap::new());
        let p = Fake::says(r#"{"target":"folders"}"#);
        let e = ask(
            &db,
            &classify(&c),
            &AiSettings::default(),
            &p,
            &mut redactor(),
            None,
            "big folders",
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Refused);
        assert_eq!(p.calls(), 0);
    }

    #[test]
    fn with_no_saved_analysis_the_model_is_not_asked() {
        let db = Db::in_memory();
        let c = Classifier::new(&std::collections::HashMap::new());
        let p = Fake::says(r#"{"target":"folders"}"#);
        let e = ask(
            &db,
            &classify(&c),
            &on(),
            &p,
            &mut redactor(),
            None,
            "big folders",
        )
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound);
        assert_eq!(p.calls(), 0);
    }

    #[test]
    fn a_question_becomes_a_labelled_answer_with_the_redacted_text_and_results() {
        let (_d, db) = with_analysis();
        let c = Classifier::new(&std::collections::HashMap::new());
        let p = Fake::says(r#"{"target":"folders","minBytes":1048576,"sort":"sizeDesc"}"#);
        let r = ask(
            &db,
            &classify(&c),
            &on(),
            &p,
            &mut redactor(),
            None,
            r"folders over 1 MB in C:\Users\VIRAJ\Projects",
        )
        .unwrap();
        assert!(r.ai_generated);
        assert_eq!(r.provider, "fake");
        assert!(!r.sent_question.contains("VIRAJ"), "{}", r.sent_question);
        assert!(r.query_description.starts_with("Folders"));
        assert!(r.query_description.contains("at least 1.0 MB"));
        assert!(r.result.matches >= 1);
        assert_eq!(p.calls(), 1);
    }

    #[test]
    fn an_unusable_answer_runs_nothing_and_says_so() {
        let (_d, db) = with_analysis();
        let c = Classifier::new(&std::collections::HashMap::new());
        for answer in [
            "I would delete everything",
            r#"{"target":"folders","delete":true}"#,
        ] {
            let p = Fake::says(answer);
            let e = ask(
                &db,
                &classify(&c),
                &on(),
                &p,
                &mut redactor(),
                None,
                "free up space",
            )
            .unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput, "{answer}");
            assert!(e.message.contains("Nothing was run"), "{}", e.message);
        }
    }
}
