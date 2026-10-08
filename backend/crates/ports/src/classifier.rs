//! The `Classifier` port: the pluggable classification boundary.

use std::fmt;

use async_trait::async_trait;
use domain::Classification;

/// A classifier identifier, for example `header_rules@1`, `gemini@flash-lite`
/// or `jev@1.13.0`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClassifierId(pub String);

/// The input to a classifier. Built in memory by T-903 from the S4 5.5
/// allowlist. Never stored, never logged. Debug prints `..`.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassifierInput {
    pub from_display: String,
    pub from_domain: String,
    pub list_id: Option<String>,
    pub has_list_unsubscribe: bool,
    pub has_list_unsubscribe_post: bool,
    pub precedence: Option<String>,
    pub auto_submitted: Option<String>,
    pub esp_header_names: Vec<String>,
    pub auth_summary: String,
    pub subject: String,
    /// Stripped, redacted, about 500 tokens.
    pub text: String,
    pub input_version: &'static str,
}

impl fmt::Debug for ClassifierInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClassifierInput { .. }")
    }
}

/// A classifier error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClassifierError {
    #[error("timeout")]
    Timeout,
    #[error("http {0}")]
    Http(u16),
    #[error("invalid output")]
    InvalidOutput,
    #[error("disabled")]
    Disabled,
    #[error("unavailable")]
    Unavailable,
}

/// The pluggable classification boundary.
#[async_trait]
pub trait Classifier: Send + Sync {
    fn id(&self) -> ClassifierId;
    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError>;
}
