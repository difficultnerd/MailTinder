#![allow(clippy::pedantic)]

//! Policy tests for the deploy container image (T-1104).
//!
//! ASVS V13.4.1: the container holds only the release binary.
//! ASVS V13.4.2: the release build has no test or debug features.
//!
//! Both tests read `backend/Dockerfile` - `../../Dockerfile` from this crate -
//! as text, so they fail on the commit that changes the image's shape rather
//! than the day a pull request opens.

use std::path::Path;

/// The Dockerfile's instruction lines, with comments and blank lines removed.
///
/// Dockerfile comments cannot hold an instruction, so stripping them keeps
/// this policy about what the image actually does; the prose above each stage
/// is free to name things the instructions must not.
fn dockerfile() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Dockerfile");
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect())
}

/// Instruction lines grouped by stage: each group starts at its `FROM` line.
fn stages(lines: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for line in lines {
        if line.to_ascii_uppercase().starts_with("FROM ") {
            out.push(vec![line.clone()]);
        } else if let Some(stage) = out.last_mut() {
            stage.push(line.clone());
        }
    }
    out
}

/// The image reference on a stage's `FROM` line, without the `AS <name>` part.
fn base_image(from_line: &str) -> Result<String, Box<dyn std::error::Error>> {
    let rest = from_line.strip_prefix("FROM ").ok_or("not a FROM line")?;
    Ok(rest
        .split_whitespace()
        .next()
        .ok_or("FROM without an image")?
        .to_owned())
}

/// A final stage holds the binary and nothing else: no source tree, no
/// toolchain, no package manager, no shell, no `.git`, and no root user.
#[test]
fn asvs_v13_4_1_dockerfile_final_stage_holds_only_the_binary(
) -> Result<(), Box<dyn std::error::Error>> {
    let lines = dockerfile()?;
    let stages = stages(&lines);
    assert!(
        stages.len() >= 2,
        "expected a build stage and a runtime stage, found {} stage(s)",
        stages.len()
    );
    let runtime = stages.last().ok_or("the Dockerfile has no stage")?;
    let base = runtime.first().ok_or("the runtime stage is empty")?;

    let image = base_image(base)?;
    assert!(
        image.starts_with("gcr.io/distroless/"),
        "the runtime stage must be distroless, found {image}"
    );
    assert!(
        image.ends_with(":nonroot"),
        "the runtime stage must use the nonroot variant, found {image}"
    );

    let copies: Vec<&String> = runtime
        .iter()
        .filter(|line| line.starts_with("COPY "))
        .collect();
    assert_eq!(
        copies.len(),
        1,
        "the runtime stage must copy exactly one file, found {copies:?}"
    );
    let copy = copies[0];
    assert!(
        copy.contains("--from=build"),
        "the runtime stage must copy from the build stage, found {copy}"
    );
    assert_eq!(
        copy.matches("--from=").count(),
        1,
        "the runtime stage must copy from one stage only, found {copy}"
    );

    // `COPY --from=build <source> <destination>`: one source, one destination.
    let paths: Vec<&str> = copy
        .split_whitespace()
        .filter(|token| !token.starts_with("--"))
        .skip(1)
        .collect();
    assert_eq!(
        paths.len(),
        2,
        "COPY must name one source and one destination, found {copy}"
    );
    let (source, destination) = (paths[0], paths[1]);
    assert!(
        source.contains("/target/release/"),
        "the runtime stage must copy the release binary, found {copy}"
    );
    assert!(
        source.contains("${SERVICE}"),
        "the runtime stage must copy the one service named by SERVICE, found {copy}"
    );
    assert!(
        !source.contains('*') && !source.ends_with('/'),
        "the runtime stage must not copy a directory or a glob, found {copy}"
    );
    assert_eq!(
        destination, "/app",
        "the runtime stage must install the binary at /app, found {copy}"
    );

    assert!(
        runtime.iter().any(|line| line == "USER nonroot"),
        "the runtime stage must set USER nonroot"
    );
    assert!(
        !runtime.iter().any(|line| line.starts_with("RUN ")),
        "the runtime stage must not run commands: distroless ships no shell"
    );
    assert!(
        !lines.iter().any(|line| line.contains(".git")),
        "no instruction may name a source-control directory"
    );

    let expected = |line: &String| {
        line.starts_with("FROM ")
            || line.starts_with("ARG ")
            || line.starts_with("COPY ")
            || line == "USER nonroot"
            || line.starts_with("ENTRYPOINT ")
    };
    let unexpected: Vec<&String> = runtime.iter().filter(|line| !expected(line)).collect();
    assert!(
        unexpected.is_empty(),
        "the runtime stage holds more than the binary: {unexpected:?}"
    );

    Ok(())
}

/// The release build is `--release --locked` with no feature selection, so
/// nothing behind a feature gate (the e2e entry points) reaches the image.
#[test]
fn asvs_v13_4_2_dockerfile_builds_release_without_features(
) -> Result<(), Box<dyn std::error::Error>> {
    let lines = dockerfile()?;
    let stages = stages(&lines);
    let build = stages.first().ok_or("the Dockerfile has no build stage")?;

    let image = base_image(build.first().ok_or("the build stage is empty")?)?;
    assert!(
        image.starts_with("rust:"),
        "the build stage must use the official Rust image, found {image}"
    );
    let tag = image
        .split_once(':')
        .map(|(_, tag)| tag)
        .unwrap_or_default();
    let pinned = tag != "latest"
        && tag != "stable"
        && tag.chars().any(|c| c.is_ascii_digit())
        && !tag.is_empty();
    assert!(
        pinned,
        "the Rust image tag must be pinned to a version, found {image}"
    );

    let builds: Vec<&String> = build
        .iter()
        .filter(|line| line.contains("cargo build"))
        .collect();
    assert_eq!(
        builds.len(),
        1,
        "expected exactly one cargo build line, found {builds:?}"
    );
    let build_cmd = builds[0];
    assert!(
        build_cmd.contains("--release"),
        "the build must be a release build, found {build_cmd}"
    );
    assert!(
        build_cmd.contains("--locked"),
        "the build must use the locked dependency set, found {build_cmd}"
    );
    assert!(
        build_cmd.contains("-p \"${SERVICE}\"") || build_cmd.contains("-p ${SERVICE}"),
        "the build must name the service from SERVICE, found {build_cmd}"
    );

    // A debug or feature-selected build would compile the e2e entry points and
    // every other test-only affordance into the image that runs in production.
    let banned = [
        "--features",
        "--all-features",
        "--no-default-features",
        "testkit",
        "MT_E2E",
        "cargo test",
        "debug_assertions",
        "--dev",
        "--debug",
    ];
    for line in &lines {
        for needle in banned {
            assert!(
                !line.contains(needle),
                "the release image must not use {needle}, found: {line}"
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// The build context (backend/.dockerignore) must keep every file a manifest
// names. Cargo loads every workspace manifest before it builds anything, so a
// `[[test]]`/`[[bench]]`/`[[example]]` target whose file the ignore list strips
// makes `cargo build -p <service>` fail on manifest loading, and every image
// build with it. The Dockerfile policy above cannot see that - it reads the
// Dockerfile, not the ignore list - so this checks the two together.
// ---------------------------------------------------------------------------

/// Every pattern line of `backend/.dockerignore` (comments and blanks removed).
fn dockerignore_patterns() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.dockerignore");
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect())
}

/// Every workspace manifest, as a path relative to `backend/`.
fn workspace_manifests() -> Result<Vec<std::path::PathBuf>, Box<dyn std::error::Error>> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    let mut out = vec![std::path::PathBuf::from("Cargo.toml")];
    for entry in std::fs::read_dir(crates)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            out.push(
                Path::new("crates")
                    .join(entry.file_name())
                    .join("Cargo.toml"),
            );
        }
    }
    out.sort();
    Ok(out)
}

/// The value of a `key = "..."` line, if the line is exactly that assignment.
///
/// A hand parser rather than the `toml` crate on purpose: the `api` crate's
/// manifest is not this task's to change, and the shape read here (a small
/// `[[table]]` with string `name`/`path`) is all a manifest target needs.
fn toml_string(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    rest.find('"').map(|end| rest[..end].to_owned())
}

/// The targets a manifest declares with `[[test]]`, `[[bench]]` or
/// `[[example]]`: `(default directory, name, explicit path)`.
fn declared_targets(text: &str) -> Vec<(&'static str, String, Option<String>)> {
    let mut out = Vec::new();
    let mut kind: Option<&'static str> = None;
    let mut name: Option<String> = None;
    let mut path: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if let (Some(directory), Some(target)) = (kind, name.take()) {
                out.push((directory, target, path.take()));
            }
            path = None;
            kind = match line {
                "[[test]]" => Some("tests"),
                "[[bench]]" => Some("benches"),
                "[[example]]" => Some("examples"),
                _ => None,
            };
        } else if kind.is_some() {
            if let Some(value) = toml_string(line, "name") {
                name = Some(value);
            } else if let Some(value) = toml_string(line, "path") {
                path = Some(value);
            }
        }
    }
    if let (Some(directory), Some(target)) = (kind, name.take()) {
        out.push((directory, target, path));
    }
    out
}

/// Split a `/`-separated path into its non-empty, non-`.` segments.
fn segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

/// Match one pattern segment against one path segment: `*` is any run of
/// characters, and a segment never contains `/`.
fn segment_matches(pattern: &str, segment: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == segment,
        Some((head, tail)) => {
            segment.starts_with(head) && {
                let rest = &segment[head.len()..];
                (0..=rest.len())
                    .any(|n| rest.is_char_boundary(n) && segment_matches(tail, &rest[n..]))
            }
        }
    }
}

/// Whether a `.dockerignore` pattern matches a path relative to the build
/// context, where `**` crosses `/` and `*` does not.
fn glob_matches(pattern: &str, path: &str) -> bool {
    fn rec(pats: &[&str], segs: &[&str]) -> bool {
        match pats.split_first() {
            None => true,
            Some((&"**", rest)) => (0..=segs.len()).any(|skip| rec(rest, &segs[skip..])),
            Some((pat, rest)) => match segs.split_first() {
                Some((seg, tail)) => segment_matches(pat, seg) && rec(rest, tail),
                None => false,
            },
        }
    }
    let pats = segments(pattern);
    if pats.is_empty() {
        return false;
    }
    rec(&pats, &segments(path))
}

/// Whether `path` is excluded by `pattern`, itself or through a parent
/// directory (a `.dockerignore` entry naming a directory hides everything under
/// it, which is the shape both the rule and the failure take).
fn ignored_by(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    let parts = segments(path);
    (1..=parts.len()).any(|n| glob_matches(pattern, &parts[..n].join("/")))
}

/// `.dockerignore` must not hide a file a manifest names: cargo fails to load
/// the manifest, so the image never builds (F1).
#[test]
fn asvs_v13_4_1_dockerignore_keeps_manifest_named_targets() -> Result<(), Box<dyn std::error::Error>>
{
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let patterns = dockerignore_patterns()?;
    let mut checked = 0_usize;
    for manifest in workspace_manifests()? {
        let text = std::fs::read_to_string(root.join(&manifest))?;
        let directory = manifest.parent().unwrap_or_else(|| Path::new(""));
        for (kind, name, explicit) in declared_targets(&text) {
            let file = explicit.unwrap_or_else(|| format!("{kind}/{name}.rs"));
            let target = directory.join(&file).to_string_lossy().replace('\\', "/");
            checked += 1;
            for pattern in &patterns {
                assert!(
                    !ignored_by(pattern, &target),
                    "manifest {} declares the target `{name}` at {target}, but the \
                     .dockerignore pattern {pattern:?} excludes it: cargo cannot load the \
                     manifest, so every `cargo build -p <service>` in the image build fails",
                    manifest.display()
                );
            }
        }
    }
    assert!(
        checked > 0,
        "no manifest-declared `[[test]]`/`[[bench]]`/`[[example]]` target was parsed; the check is not reading the manifests"
    );
    Ok(())
}
