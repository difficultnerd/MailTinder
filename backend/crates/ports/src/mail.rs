//! The `MailProvider` port: the provider-neutral boundary to a mail service.

use async_trait::async_trait;
use domain::{LabelSet, MailboxId, MailtoTarget, MessageId, MessageMeta, Provider};
use obs::Sensitive;
use time::OffsetDateTime;

/// The context needed to act on one mailbox.
pub struct MailboxCtx {
    pub mailbox: MailboxId,
    pub access_token: Sensitive<String>,
}

/// An opaque page token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageToken(pub String);

/// How to page the inbox. `NewestFirst` pages from the top; `NewerThan` and
/// `OlderThan` bound the page by internal date (Feed new mail first, then
/// backlog, S2 FD-03). All three return newest first within the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListOrder {
    NewestFirst,
    NewerThan(OffsetDateTime),
    OlderThan(OffsetDateTime),
}

/// One page of messages.
#[derive(Clone, Debug, PartialEq)]
pub struct MessagePage {
    pub items: Vec<MessageMeta>,
    pub next: Option<PageToken>,
}

/// Flags the shared contract suite (T-203) reads instead of assuming Gmail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Gmail true; Graph (v2) false (one folder plus categories).
    pub labels_are_sets: bool,
    /// Gmail true.
    pub spam_is_label: bool,
}

/// A mail provider error. `Invalid` carries a static reason chosen by the
/// adapter, never the provider's error text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MailError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("rate limited")]
    RateLimited { retry_after_s: u64 },
    #[error("transient")]
    Transient,
    #[error("invalid: {0}")]
    Invalid(String),
}

/// The provider-neutral mail boundary. No delete method exists or may be added
/// (INV-5).
#[async_trait]
pub trait MailProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn list_inbox(
        &self,
        mb: &MailboxCtx,
        page: Option<PageToken>,
        order: ListOrder,
    ) -> Result<MessagePage, MailError>;
    async fn get_meta(&self, mb: &MailboxCtx, id: &MessageId) -> Result<MessageMeta, MailError>;
    async fn get_preview(&self, mb: &MailboxCtx, id: &MessageId) -> Result<String, MailError>;
    async fn set_labels(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
        add: &LabelSet,
        remove: &LabelSet,
    ) -> Result<LabelSet, MailError>;
    /// Returns the labels before the trash.
    async fn trash(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError>;
    /// Returns the labels before the report.
    async fn report_spam(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError>;
    async fn restore_labels(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
        exact: &LabelSet,
    ) -> Result<(), MailError>;
    /// Returns the label ID.
    async fn ensure_label(&self, mb: &MailboxCtx, name: &str) -> Result<String, MailError>;
    async fn send_mailto(&self, mb: &MailboxCtx, to: &MailtoTarget) -> Result<(), MailError>;
    async fn inbox_count(&self, mb: &MailboxCtx) -> Result<u64, MailError>;
}
