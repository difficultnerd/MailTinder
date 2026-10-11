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
