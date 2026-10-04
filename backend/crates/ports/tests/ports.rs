//! Tests for the ports crate (T-201a).

use domain::JobId;
use ports::{TaskName, WrappedKey};
use uuid::Uuid;

/// INV-5: `MailProvider` has no delete method. Hand-maintained list of the
/// trait's method names; keep in step with the trait.
const MAIL_PROVIDER_METHODS: [&str; 12] = [
    "provider",
    "capabilities",
    "list_inbox",
    "get_meta",
    "get_preview",
    "set_labels",
    "trash",
    "report_spam",
    "restore_labels",
    "ensure_label",
    "send_mailto",
    "inbox_count",
];

#[test]
fn inv_5_mail_provider_has_no_delete_method() {
    for name in MAIL_PROVIDER_METHODS {
        assert!(
            !name.to_ascii_lowercase().contains("delete"),
            "MailProvider must not expose a delete method, found `{name}`"
        );
    }
}

/// INV-7: `ports` names provider-neutral types only; no Gmail type appears in
/// any signature. Reads each source file and asserts no non-comment line
/// contains `gmail` or `graph` (case-insensitive).
#[test]
fn inv_7_ports_has_no_provider_specific_types() {
    let files = [
        include_str!("../src/lib.rs"),
        include_str!("../src/clock.rs"),
        include_str!("../src/rng.rs"),
        include_str!("../src/mail.rs"),
        include_str!("../src/app_folder.rs"),
        include_str!("../src/keys.rs"),
        include_str!("../src/scheduler.rs"),
        include_str!("../src/egress.rs"),
        include_str!("../src/identity.rs"),
        include_str!("../src/classifier.rs"),
        include_str!("../src/secrets.rs"),
    ];
    for (i, src) in files.iter().enumerate() {
        for (lineno, line) in src.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            assert!(
                !lower.contains("gmail") && !lower.contains("graph"),
                "ports source {i} line {} mentions a provider-specific type: {line}",
                lineno + 1
            );
        }
    }
}

#[test]
fn wrapped_key_debug_prints_length_only() {
    let key = WrappedKey(vec![0u8; 16]);
    let out = format!("{key:?}");
    assert!(out.contains("16"), "expected length in debug, got {out}");
    assert!(
        !out.contains("0, 0"),
        "expected no raw bytes in debug, got {out}"
    );
}

#[test]
fn task_name_for_job_is_stable_and_hyphen_free() {
    let job = JobId(Uuid::new_v4());
    let name = TaskName::for_job(&job);
    assert!(name.0.starts_with("job-"));
    // The simple UUID has no hyphens.
    assert!(!name.0[4..].contains('-'));
    assert_eq!(name, TaskName::for_job(&job));
}
