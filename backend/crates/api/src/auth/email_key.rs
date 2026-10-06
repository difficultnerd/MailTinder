//! The keyed email lookup hash: `EmailLookupHash` for invites and requests.
//!
//! The one implementation lives in `svc-common`, shared with the `mt-admin`
//! bootstrap tool (T-507); this module re-exports it so the API keeps a single
//! import path.

pub use svc_common::invites::email_lookup_hash;
