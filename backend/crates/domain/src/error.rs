//! Domain error type.

/// Errors produced by the domain crate. No I/O and no provider types (INV-7).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("invalid identifier")]
    InvalidId,
    #[error("invalid mailto target")]
    InvalidMailto,
    #[error("transition not allowed")]
    TransitionNotAllowed,
    #[error("invalid value")]
    InvalidValue,
}
