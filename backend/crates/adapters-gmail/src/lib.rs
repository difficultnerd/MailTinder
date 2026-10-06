//! Gmail and Drive over HTTP (v1).
//!
//! Every Gmail wire type stays private to this crate (XC-02): the only public
//! items are [`GmailProvider`] and the [`GmailHttp`] client it is built from.
//! Later M4 tasks add label changes, trash, send and the Drive app-folder store.
//!
//! The spec (T-401) mandates exact signatures without `#[must_use]` or `# Errors`
//! doc sections, so the pedantic style lints that conflict with that shape are
//! allowed at the crate level rather than suppressing each item.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::return_self_not_must_use,
    clippy::many_single_char_names,
    clippy::single_match,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::module_name_repetitions,
    clippy::similar_names,
    clippy::struct_field_names,
    clippy::doc_markdown,
    clippy::redundant_closure_for_method_calls,
    clippy::map_unwrap_or,
    clippy::comparison_chain,
    clippy::cast_possible_truncation
)]

mod auth_results;
mod client;
mod drive;
mod errors;
mod headers;
pub mod identity;
mod list_unsubscribe;
mod modify;
pub mod pkce;
mod read;
pub mod scopes;
mod send;

pub use client::GmailHttp;
pub use drive::DriveAppFolder;
pub use read::GmailProvider;
pub use send::INVITE_BODY_TEMPLATE;
pub use send::INVITE_SUBJECT;

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::path::Path;

    /// XC-02: callers see only `GmailProvider` and `GmailHttp`; no Gmail wire
    /// type (or the T-405 store, added later) is re-exported from the crate root.
    ///
    /// T-502a makes the Google identity modules public (T-502b, T-503, T-504 and
    /// `svc-common` reach `GoogleIdentity` through them). Those modules hold no
    /// Gmail wire type, so the rule this test enforces is unchanged.
    #[test]
    fn xc_02_gmail_types_not_public() {
        let lib = include_str!("lib.rs");
        let allowed = [
            "GmailProvider",
            "GmailHttp",
            "DriveAppFolder",
            "INVITE_SUBJECT",
            "INVITE_BODY_TEMPLATE",
        ];
        let allowed_modules = ["pub mod identity;", "pub mod pkce;", "pub mod scopes;"];
        for line in lib.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("pub use ") {
                let name = rest
                    .trim_end_matches(';')
                    .rsplit("::")
                    .next()
                    .unwrap_or_default()
                    .trim();
                assert!(
                    allowed.contains(&name),
                    "unexpected public re-export: {name}"
                );
            }
            assert!(
                !line.starts_with("pub mod ") || allowed_modules.contains(&line),
                "no module may be public: {line}"
            );
        }
    }

    /// INV-7: `domain` gains no Gmail type or dependency from this task.
    #[test]
    fn inv_7_domain_has_no_gmail_dependency() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate dir")
            .join("domain/Cargo.toml");
        let text = std::fs::read_to_string(&manifest).expect("read domain manifest");
        for banned in ["adapters-gmail", "reqwest", "mailparse"] {
            assert!(
                !text.contains(banned),
                "domain/Cargo.toml must not mention {banned}"
            );
        }
    }
}
