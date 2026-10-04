//! A placeholder mail provider that refuses everything (until T-203).

use async_trait::async_trait;
use domain::{LabelSet, MailtoTarget, MessageId, MessageMeta, Provider};
use ports::{
    ListOrder, MailError, MailProvider, MailboxCtx, MessagePage, PageToken, ProviderCapabilities,
};

/// A mail provider that returns `Invalid("no_mailbox_fake")` for every call.
pub struct NullMailProvider;

#[async_trait]
impl MailProvider for NullMailProvider {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            labels_are_sets: true,
            spam_is_label: true,
        }
    }
    async fn list_inbox(
        &self,
        _mb: &MailboxCtx,
        _page: Option<PageToken>,
        _order: ListOrder,
    ) -> Result<MessagePage, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn get_meta(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<MessageMeta, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn get_preview(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<String, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn set_labels(
        &self,
        _mb: &MailboxCtx,
        _id: &MessageId,
        _add: &LabelSet,
        _remove: &LabelSet,
    ) -> Result<LabelSet, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn trash(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<LabelSet, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn report_spam(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<LabelSet, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn restore_labels(
        &self,
        _mb: &MailboxCtx,
        _id: &MessageId,
        _exact: &LabelSet,
    ) -> Result<(), MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn ensure_label(&self, _mb: &MailboxCtx, _name: &str) -> Result<String, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn send_mailto(&self, _mb: &MailboxCtx, _to: &MailtoTarget) -> Result<(), MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
    async fn inbox_count(&self, _mb: &MailboxCtx) -> Result<u64, MailError> {
        Err(MailError::Invalid("no_mailbox_fake".to_owned()))
    }
}
