//! The error type shared by the `svc-common` helpers that write records or
//! raise Needs Attention items (T-701).

/// A failure in a shared service helper.
#[derive(Debug, thiserror::Error)]
pub enum SvcError {
    /// The store rejected the read or write.
    #[error("store")]
    Store,
    /// A key-service failure while sealing or opening a field.
    #[error("crypto")]
    Crypto,
    /// The user, mailbox or encrypted field the helper needed is missing.
    #[error("not found")]
    NotFound,
    /// The request carried data the helper refuses (for example a non-https
    /// Needs Attention link).
    #[error("invalid: {0}")]
    Invalid(&'static str),
}
