//! Local-only AI abstraction for Sentinel. Policy: docs/AI_POLICY.md.
//!
//! - [`AiProvider`] is the only way text reaches a model, and the only implementation,
//!   [`OllamaProvider`], refuses any endpoint that is not on this computer.
//! - [`Redactor`] strips secrets and personal names and paths from text before it is sent.
//! - [`parse_query`] turns a model's answer into a [`StructuredQuery`] or an error; it never
//!   repairs or guesses.
//! - [`translate`] joins them: a typed question in, a validated query out.
//!
//! The model is never given filesystem-derived text in this milestone, and its output is a
//! suggestion that is validated and never executed.

mod ollama;
mod provider;
mod query;
mod redact;
mod translate;

pub use ollama::{OllamaConfig, OllamaProvider, validate_loopback_url};
pub use provider::{AiConfig, AiError, AiProvider, AiRequest};
pub use query::{QueryError, QueryVocabulary, Sort, StructuredQuery, Target, parse_query};
pub use redact::{RedactionContext, Redactor};
pub use translate::{TranslateError, system_prompt, translate};
