//! INV-7: core domain code does not import provider-specific types or I/O crates.
const MANIFEST: &str = include_str!("../Cargo.toml");
const ALLOWED: [&str; 8] = [
    "serde",
    "uuid",
    "time",
    "url",
    "thiserror",
    "html5ever",
    "unicode-segmentation",
    "percent-encoding",
];
const ALLOWED_DEV: [&str; 3] = ["proptest", "serde_json", "unicode-segmentation"];

/// Parse the `[dependencies]` and `[dev-dependencies]` tables by hand (no toml
/// crate): a dependency line is `name.workspace = true` or `name = ...` inside
/// the table. Fail with the offending name if any dependency is outside
/// ALLOWED / `ALLOWED_DEV`.
#[test]
fn inv_7_domain_depends_on_no_io_or_provider_crates() -> Result<(), String> {
    let mut table: Option<&str> = None;
    for line in MANIFEST.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            table = Some(line.trim_matches(['[', ']']));
            continue;
        }
        let Some(table) = table else { continue };
        if table != "dependencies" && table != "dev-dependencies" {
            continue;
        }
        // A dependency line: `name.workspace = true` or `name = "..."`.
        // The name is the text before the first `.` or `=`.
        let name = line
            .split(['.', '='])
            .next()
            .map(str::trim)
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let allowed: &[&str] = if table == "dev-dependencies" {
            &ALLOWED_DEV
        } else {
            &ALLOWED
        };
        if !allowed.contains(&name) {
            return Err(format!(
                "domain crate depends on disallowed crate `{name}` in [{table}]"
            ));
        }
    }
    Ok(())
}
