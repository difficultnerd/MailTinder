//! INV-5: no Gmail code path may permanently delete a message (S10 6.4).
//!
//! A structural check over the crate's own source, so a future edit that
//! reaches for a Gmail removal call fails here as well as in CI's Semgrep
//! rule. `drive.rs` (T-405) is exempt: Drive has its own delete, which is not
//! a mail delete.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// The forbidden tokens, split in source so this checker does not match
/// itself (the file is not scanned, but keeping it clean keeps the grep honest).
const FORBIDDEN: [&str; 4] = [
    concat!("batch", "Delete"),
    concat!("Method", "::DELETE"),
    concat!("\"", "DELETE", "\""),
    concat!("/", "delete"),
];

#[test]
fn inv_5_gmail_adapter_has_no_delete_call() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&src).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("drive.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read source file");
        for token in FORBIDDEN {
            assert!(
                !text.contains(token),
                "{} contains a permanent-delete token (INV-5)",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 5,
        "expected at least 5 source files, saw {checked}"
    );
}
