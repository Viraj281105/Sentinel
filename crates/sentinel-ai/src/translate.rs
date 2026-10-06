//! Natural-language question in, validated structured query out.
//!
//! The only text sent to the model is the person's own question (redacted anyway) and a
//! fixed description of the query format. No file names, paths, project names or contents
//! are ever put in a prompt here (docs/AI_POLICY.md, decision 3), so there is no
//! prompt-injection surface from the filesystem.

use crate::provider::{AiError, AiProvider, AiRequest};
use crate::query::{QueryError, QueryVocabulary, StructuredQuery, parse_query};
use crate::redact::Redactor;

const MAX_QUESTION_CHARS: usize = 500;
const MAX_ANSWER_TOKENS: u32 = 300;

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("the question is empty")]
    Empty,
    #[error("the question is too long (at most {MAX_QUESTION_CHARS} characters)")]
    TooLong,
    #[error(transparent)]
    Provider(#[from] AiError),
    #[error("the model's answer was not a valid query: {0}")]
    InvalidAnswer(#[from] QueryError),
}

/// The fixed instructions. Contains only the format and the category vocabulary.
pub fn system_prompt(vocab: &QueryVocabulary) -> String {
    format!(
        "You translate a question about disk space into one JSON object and nothing else.\n\
         Fields (all optional except target):\n\
         - \"target\": \"folders\" or \"files\" (required)\n\
         - \"categories\": array of at most 8 of: {cats}\n\
         - \"pathContains\": short text the path must contain\n\
         - \"minBytes\": integer, minimum size in bytes\n\
         - \"olderThanDays\": integer >= 1, not changed for at least this many days\n\
         - \"sort\": \"sizeDesc\", \"sizeAsc\" or \"name\"\n\
         - \"limit\": integer from 1 to 200\n\
         Use only these fields. Output only the JSON object. If the question cannot be \
         expressed this way, output {{\"target\":\"folders\"}}.",
        cats = vocab.categories.join(", ")
    )
}

/// Ask `provider` to turn `question` into a query. Read-only: nothing is executed.
pub fn translate(
    provider: &dyn AiProvider,
    redactor: &mut Redactor,
    vocab: &QueryVocabulary,
    question: &str,
) -> Result<StructuredQuery, TranslateError> {
    let question = question.trim();
    if question.is_empty() {
        return Err(TranslateError::Empty);
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(TranslateError::TooLong);
    }
    let req = AiRequest {
        system: system_prompt(vocab),
        user: redactor.redact(question),
        json_mode: true,
        max_tokens: MAX_ANSWER_TOKENS,
    };
    let answer = provider.complete(&req)?;
    let mut q = parse_query(&answer, vocab)?;
    // The model saw placeholders; show the person their own words.
    if let Some(p) = q.path_contains.take() {
        q.path_contains = Some(redactor.restore(&p)).filter(|s| !s.trim().is_empty());
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Mutex;

    use super::*;
    use crate::query::{Sort, Target};
    use crate::redact::RedactionContext;

    struct Fake {
        answer: Result<String, &'static str>,
        seen: Mutex<Vec<AiRequest>>,
    }

    impl Fake {
        fn says(a: &str) -> Self {
            Self {
                answer: Ok(a.to_owned()),
                seen: Mutex::default(),
            }
        }
    }

    impl AiProvider for Fake {
        fn id(&self) -> &str {
            "fake"
        }
        fn complete(&self, req: &AiRequest) -> Result<String, AiError> {
            self.seen.lock().unwrap().push(req.clone());
            self.answer
                .clone()
                .map_err(|m| AiError::Unavailable(m.to_owned()))
        }
    }

    fn vocab() -> QueryVocabulary {
        QueryVocabulary {
            categories: vec!["developerDependencies".into(), "windows".into()],
        }
    }

    fn redactor() -> Redactor {
        Redactor::new(RedactionContext {
            user_names: vec!["VIRAJ".into()],
            host_names: vec!["DESKTOP-9KQ2".into()],
        })
    }

    #[test]
    fn turns_a_question_into_a_validated_query() {
        let p = Fake::says(
            r#"{"target":"folders","categories":["developerDependencies"],"minBytes":1073741824,"olderThanDays":90,"sort":"sizeDesc","limit":10}"#,
        );
        let q = translate(
            &p,
            &mut redactor(),
            &vocab(),
            "Which old dependency folders over 1 GB can I see?",
        )
        .unwrap();
        assert_eq!(q.target, Target::Folders);
        assert_eq!(
            (q.categories.as_slice(), q.limit, q.sort),
            (
                &["developerDependencies".to_owned()][..],
                10,
                Sort::SizeDesc
            )
        );
    }

    #[test]
    fn the_model_sees_only_the_redacted_question_and_the_fixed_format() {
        let p = Fake::says(r#"{"target":"files"}"#);
        let question = r"What is in C:\Users\VIRAJ\Documents\Secret Plans\q3.xlsx on DESKTOP-9KQ2? mail viraj@example.com";
        translate(&p, &mut redactor(), &vocab(), question).unwrap();
        let seen = p.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let sent = format!("{}\n{}", seen[0].system, seen[0].user);
        for leaked in [
            "VIRAJ",
            "viraj",
            "Secret Plans",
            "q3",
            "DESKTOP-9KQ2",
            "example.com",
        ] {
            assert!(
                !sent.contains(leaked),
                "{leaked:?} reached the model: {sent}"
            );
        }
        assert!(seen[0].json_mode);
        assert!(seen[0].max_tokens <= 300);
        assert!(seen[0].system.contains("developerDependencies, windows"));
        // Nothing but the question and the fixed prompt: no environment, no file data.
        assert!(!seen[0].system.contains("C:\\"));
    }

    #[test]
    fn placeholders_in_the_answer_are_shown_as_the_persons_own_words() {
        // Whatever placeholder the redactor chose for the folder is mapped back.
        let mut r = redactor();
        let shown = r.redact(r"C:\Clients\Acme\x.docx");
        assert!(shown.contains("<dir-1>"));
        let p = Fake::says(r#"{"target":"files","pathContains":"<dir-2>"}"#);
        let q = translate(&p, &mut r, &vocab(), r"things in C:\Clients\Acme").unwrap();
        assert_eq!(q.path_contains.as_deref(), Some("Acme"));
    }

    #[test]
    fn a_bad_answer_is_an_error_never_a_guess() {
        for answer in [
            "Sure, I can help!",
            r#"{"target":"files","delete":true}"#,
            r#"{"target":"files","categories":["bitcoin"]}"#,
            r#"{"target":"files","limit":9999}"#,
        ] {
            let p = Fake::says(answer);
            let r = translate(&p, &mut redactor(), &vocab(), "big files");
            assert!(
                matches!(r, Err(TranslateError::InvalidAnswer(_))),
                "{answer}: {r:?}"
            );
        }
    }

    #[test]
    fn provider_failures_and_bad_questions_are_reported() {
        let down = Fake {
            answer: Err("connection refused"),
            seen: Mutex::default(),
        };
        assert!(matches!(
            translate(&down, &mut redactor(), &vocab(), "big files"),
            Err(TranslateError::Provider(AiError::Unavailable(_)))
        ));
        let p = Fake::says(r#"{"target":"files"}"#);
        assert!(matches!(
            translate(&p, &mut redactor(), &vocab(), "   "),
            Err(TranslateError::Empty)
        ));
        let long = "a ".repeat(300);
        assert!(matches!(
            translate(&p, &mut redactor(), &vocab(), &long),
            Err(TranslateError::TooLong)
        ));
        assert!(
            p.seen.lock().unwrap().is_empty(),
            "rejected questions never reach the model"
        );
    }
}
