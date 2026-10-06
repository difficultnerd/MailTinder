//! Server-side sessions: the `__session` cookie, the session service, the
//! request extractors and the CSRF layer (S7 3.2, 3.3; S6 4; S3 `Session`).

pub mod cookie;
pub mod csrf;
pub mod extract;
pub mod pre_auth;
pub mod store;
