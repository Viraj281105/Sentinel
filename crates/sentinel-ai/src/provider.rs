//! The provider interface and its errors.

use crate::ollama::OllamaConfig;

/// One request to a model. Text only; no files, no tools, no function calling.
#[derive(Debug, Clone)]
pub struct AiRequest {
    pub system: String,
    pub user: String,
    /// Ask the model to answer with a single JSON object.
    pub json_mode: bool,
    pub max_tokens: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("AI features are turned off")]
    Disabled,
    #[error("{0} is not on this computer; Sentinel only uses local models")]
    NotLocal(String),
    #[error("the local model could not be reached: {0}")]
    Unavailable(String),
    #[error("the local model did not answer in time")]
    Timeout,
    #[error("the local model's answer could not be used: {0}")]
    BadResponse(String),
    #[error("the local model's answer was too large")]
    TooLarge,
}

/// Something that can complete a prompt. Implementations must only talk to this computer.
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &str;
    fn complete(&self, req: &AiRequest) -> Result<String, AiError>;
}

/// Settings. AI is **off** unless someone turns it on.
#[derive(Debug, Clone, Default)]
pub struct AiConfig {
    pub enabled: bool,
    pub ollama: OllamaConfig,
}
