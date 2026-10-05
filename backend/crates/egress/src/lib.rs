//! `HttpEgress` implementation with the per-service allowlist and SSRF checks.

#![allow(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::return_self_not_must_use,
    clippy::many_single_char_names,
    clippy::single_match,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::map_unwrap_or,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::uninlined_format_args,
    clippy::unused_self
)]

pub mod allowlist;
pub mod client;
pub mod ranges;
pub mod resolver;
pub mod target;
#[cfg(feature = "test-policy")]
pub mod test_policy;

pub use allowlist::{allowlist, allows, one_click_permitted, AllowedEndpoint, Service};
pub use client::{
    ProdEgress, CALL_MAX_BODY, CALL_MAX_TIMEOUT, ONE_CLICK_BODY, ONE_CLICK_MAX_BODY,
    ONE_CLICK_TIMEOUT, USER_AGENT,
};
pub use ranges::classify_ip;
pub use resolver::{Resolver, SystemResolver};
pub use target::{
    check_host_name, check_one_click_url, MAX_URL_LEN, ONE_CLICK_PORT, REFUSED_NAMES,
    REFUSED_SUFFIXES,
};
#[cfg(feature = "test-policy")]
pub use test_policy::TestOverride;
