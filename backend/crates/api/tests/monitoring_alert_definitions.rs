#![allow(clippy::pedantic)]

//! T-1107b: the monitoring module's alert definitions count what they claim to.
//!
//! The `auth_failures` log-based metric (S6 7) must count a refused or failed
//! authentication attempt and nothing else. It once excluded a fixed pair of
//! success codes (`signed_in`, `stepped_up`), so every successful invite join,
//! mailbox link and reconnect (`joined`, `linked`, `reconnected`) and every
//! step-up success (`success`) was counted as a failure - the alert inflated
//! exactly on the events it exists to notice (review N1 of PR #82).
//!
//! The Terraform filter now names the failure codes explicitly. Its list lives
//! in `infra/terraform/modules/monitoring/metrics.tf`; the crates cannot share
//! it directly, so this test is the bridge. It reads the sign_in / step_up
//! security-event outcome strings out of `backend/crates/api/src/auth/*.rs`,
//! unions them with every `Outcome::code()` value, and fails when an outcome is
//! in neither the success set nor the failure list - so adding an outcome
//! forces a decision in both places. It then reads the Terraform list and fails
//! when the two have drifted. `terraform test` cannot do this: it sees the
//! filter string, not the API code that produces the value.
//!
//! The roots' `module "monitoring"` call must depend on `module.foundation`
//! (review N3), which no module test can see, so the second test reads both
//! `envs/*/main.tf` files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Outcome codes that mean an authentication attempt succeeded. The
/// `auth_failures` metric must never count one of these.
const SUCCESS_OUTCOMES: &[&str] = &[
    "signed_in",
    "joined",
    "linked",
    "reconnected",
    "stepped_up",
    "success",
];

/// Outcome codes a `sign_in` or `step_up` security event logs on a refused or
/// failed attempt. Must equal the `auth_failure_outcomes` local in
/// `infra/terraform/modules/monitoring/metrics.tf`.
const FAILURE_OUTCOMES: &[&str] = &[
    "not_invited",
    "not_registered",
    "invite_invalid",
    "email_mismatch",
    "email_unverified",
    "mailbox_linked_elsewhere",
    "step_up_wrong_account",
    "consent_blocked",
    "cancelled",
    "failed",
    "state_invalid",
    "id_token_invalid",
    "wrong_account",
    "stale_auth_time",
];

/// A boxed error from a message, so helpers can fail with a readable string.
fn boxed(message: String) -> Box<dyn std::error::Error> {
    message.into()
}

/// The repository root, three levels above this crate's manifest directory.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// True for a byte that can appear inside a Rust identifier.
fn is_ident_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The index just past the string literal that opens at `open`, honouring `\\`
/// and `\"`. Past the end of the text if the literal is unterminated.
fn skip_string(source: &str, open: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// The index of the `)` that closes the `(` at `open`, skipping strings and
/// comments so a delimiter inside either does not miscount.
fn matching_paren(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth: i32 = 0;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = skip_string(source, i);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The top-level comma-separated arguments of a call body.
fn split_args(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut args = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = skip_string(text, i);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < text.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                args.push(text[start..i].to_owned());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    args.push(text[start..].to_owned());
    args
}

/// The argument lists of every call to `name(...)` that is not a definition.
///
/// The name is matched on identifier boundaries, so `event` does not match
/// `security_event` or `step_up_event`.
fn call_args(source: &str, name: &str) -> Vec<Vec<String>> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = source[from..].find(name) {
        let at = from + pos;
        from = at + name.len();
        let before_ok = at == 0 || !is_ident_char(bytes[at - 1]);
        let open = at + name.len();
        if !before_ok || bytes.get(open) != Some(&b'(') {
            continue;
        }
        // Skip the definition (`fn event(...)`): only calls carry outcomes.
        if source[..at].trim_end().ends_with("fn") {
            continue;
        }
        if let Some(close) = matching_paren(source, open) {
            out.push(split_args(&source[open + 1..close]));
            from = close;
        }
    }
    out
}

/// A string literal, if `text` (trimmed) is exactly one.
fn literal(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// The outcome codes passed to the `sign_in` / `step_up` security events in one
/// source file.
///
/// `event(state, "sign_in"|"step_up", <outcome>, ..)` carries the action in the
/// second argument and the outcome in the third; `step_up_event(state,
/// <outcome>, ..)` fixes the action to `step_up` and takes the outcome second.
/// An outcome that is an expression (`finished.outcome.code()`) has no literal
/// here and is covered by the `Outcome::code()` values instead.
fn security_event_outcomes(source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for args in call_args(source, "event") {
        let action = args.get(1).and_then(|arg| literal(arg));
        if matches!(action.as_deref(), Some("sign_in" | "step_up")) {
            if let Some(code) = args.get(2).and_then(|arg| literal(arg)) {
                out.insert(code);
            }
        }
    }
    for args in call_args(source, "step_up_event") {
        if let Some(code) = args.get(1).and_then(|arg| literal(arg)) {
            out.insert(code);
        }
    }
    out
}

/// Every `Outcome::code()` value in `outcome.rs`'s match arms.
fn outcome_codes(source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in source.lines() {
        if let Some((_, value)) = line.trim().split_once("=> ") {
            if let Some(code) = literal(value.trim_end_matches(',')) {
                out.insert(code);
            }
        }
    }
    out
}

/// The text of a `name = [ ... ]` list literal in a `.tf` file, up to its
/// closing `]` on its own indented line.
fn local_list(source: &str, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let start = source
        .find(&format!("{name} = ["))
        .ok_or_else(|| boxed(format!("no `{name} = [` found")))?;
    let from_start = &source[start..];
    let end = from_start
        .find("\n  ]")
        .ok_or_else(|| boxed(format!("the `{name}` list has no closing `]`")))?;
    Ok(from_start[..end].to_owned())
}

/// The text of a top-level `marker { ... }` block, up to the closing `}` in
/// column zero.
fn block_after(source: &str, marker: &str) -> Option<String> {
    let start = source.find(marker)?;
    let rest = &source[start..];
    let end = rest.find("\n}")?;
    Some(rest[..end + 2].to_owned())
}

#[test]
fn t_1107b_auth_failures_metric_matches_logged_outcomes() -> Result<(), Box<dyn std::error::Error>>
{
    let auth_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/auth");
    let mut logged = BTreeSet::new();
    for entry in std::fs::read_dir(&auth_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path)?;
        if path.file_name().and_then(|name| name.to_str()) == Some("outcome.rs") {
            logged.extend(outcome_codes(&source));
        } else {
            logged.extend(security_event_outcomes(&source));
        }
    }

    // The scanner reads the events: refuse to pass on an unreadable source.
    for expected in ["state_invalid", "wrong_account", "success", "not_invited"] {
        assert!(
            logged.contains(expected),
            "the scanner found no `{expected}` outcome in src/auth, so it is not reading the \
             sign_in/step_up security events and cannot judge the metric"
        );
    }

    for code in &logged {
        let known =
            SUCCESS_OUTCOMES.contains(&code.as_str()) || FAILURE_OUTCOMES.contains(&code.as_str());
        assert!(
            known,
            "src/auth logs the outcome `{code}` on a sign_in/step_up security event, but it is in \
             neither the success set nor the auth_failures failure list in \
             infra/terraform/modules/monitoring/metrics.tf: decide whether it is a success or a \
             failure and update both (T-1107b, S6 7)"
        );
    }

    let metrics =
        std::fs::read_to_string(repo().join("infra/terraform/modules/monitoring/metrics.tf"))?;
    let list = local_list(&metrics, "auth_failure_outcomes")?;
    for code in FAILURE_OUTCOMES {
        let needle = format!("\"{code}\"");
        assert!(
            list.contains(needle.as_str()),
            "the auth_failure_outcomes list in metrics.tf is missing the failure outcome `{code}` \
             that a sign_in/step_up event logs (T-1107b B1)"
        );
    }
    for code in SUCCESS_OUTCOMES {
        let needle = format!("\"{code}\"");
        assert!(
            !list.contains(needle.as_str()),
            "the auth_failure_outcomes list in metrics.tf contains the success outcome `{code}`: \
             every successful join, link, reconnect and step-up is counted as an authentication \
             failure (T-1107b B1)"
        );
    }

    Ok(())
}

#[test]
fn t_1107b_monitoring_module_depends_on_foundation_in_both_roots(
) -> Result<(), Box<dyn std::error::Error>> {
    for env in ["staging", "prod"] {
        let path = repo().join(format!("infra/terraform/envs/{env}/main.tf"));
        let source = std::fs::read_to_string(&path)?;
        let block = block_after(&source, "module \"monitoring\"").ok_or_else(|| {
            boxed(format!(
                "envs/{env}/main.tf has no module \"monitoring\" block"
            ))
        })?;
        assert!(
            block.contains("depends_on = [module.foundation]"),
            "the monitoring module in envs/{env}/main.tf must depend on module.foundation: the \
             notification channel, log-based metrics and alert policies need the Monitoring and \
             Logging APIs it enables, so without it a first apply can fail with 'API not enabled' \
             (T-1107b B3, N3)"
        );
    }
    Ok(())
}
