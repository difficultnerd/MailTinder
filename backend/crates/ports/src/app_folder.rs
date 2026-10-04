//! The `AppFolderStore` port: the user's own app folder in their Drive.

use async_trait::async_trait;

use crate::mail::{MailError, MailboxCtx};

/// An `ETag` for optimistic concurrency on the app folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ETag(pub String);

/// An app folder error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AppFolderError {
    #[error("etag conflict")]
    Conflict,
    #[error(transparent)]
    Mail(#[from] MailError),
}

/// The user's app folder, holding ciphertext. Written only by the server while
/// the user is in a session.
#[async_trait]
pub trait AppFolderStore: Send + Sync {
    /// Returns the ciphertext and its `ETag`, or `None` when absent.
    async fn read(&self, mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError>;
    async fn write(
        &self,
        mb: &MailboxCtx,
        bytes: &[u8],
        if_match: Option<&ETag>,
    ) -> Result<ETag, AppFolderError>;
    async fn delete(&self, mb: &MailboxCtx) -> Result<(), MailError>;
}
