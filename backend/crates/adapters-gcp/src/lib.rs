//! Firestore, Cloud KMS, Cloud Tasks, Secret Manager, Vertex AI.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::return_self_not_must_use,
    clippy::many_single_char_names,
    clippy::single_match,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::redundant_pattern_matching,
    clippy::redundant_closure,
    clippy::map_unwrap_or,
    clippy::cast_possible_wrap,
    clippy::unnested_or_patterns
)]

pub mod firestore;
pub mod gcp_http;
pub mod os_rng;
pub mod system_clock;
pub mod token_source;

pub use firestore::{FirestoreConfig, FirestoreStore};
pub use gcp_http::{GcpError, GcpHttp, PLATFORM_HOSTS};
pub use os_rng::OsRng;
pub use system_clock::SystemClock;
pub use token_source::{MetadataTokenSource, StaticTokenSource, TokenSource};
