#![allow(clippy::pedantic)]

//! The service boot check (T-1119): every variable a service's production
//! start-up path reads and requires must be set, correctly bound and
//! non-blank, by the environment Terraform gives that service.
//!
//! Why it exists: the first automatic staging deploy (run 38174582196, 21:03
//! UTC) built and pushed all three images, then `mt-api` failed with "container
//! failed to start and listen on PORT=8080". `ApiConfig::from_lookup` calls
//! `required()` for `APP_ORIGIN` and `GOOGLE_OAUTH_CLIENT_ID` before the server
//! binds, `cloud_run.tf` set the client id only in `unsub_env` and never set the
//! origin at all, and the first diagnosis missed that the api also needs
//! `UNSUB_BASE_URL`, `UNSUB_AUDIENCE` and `UNSUB_TASKS_CALLER` - variables
//! `ApiConfig` does not read, which the start-up path reads after it (PR #93
//! sets all five). Unit tests of each loader passed, Terraform tests passed,
//! review passed: nothing connected the two sides. This test does.
//!
//! What it proves, and what it does not:
//!
//! * Name coverage, binding and literal validity.
//!   `infra/terraform/modules/runtime/cloud_run.tf` is read as text and its
//!   `locals` env maps become a variable lookup: a literal is passed through AS
//!   WRITTEN (so a blank or malformed literal fails the loader) and a `var.*` /
//!   `local.*` reference is replaced by a placeholder from [`placeholder`],
//!   chosen to satisfy the loader's own validation.
//! * For a reference the guarantee is name coverage, binding and literal
//!   validity ONLY. What a `var.*` / `local.*` reference resolves to is proven
//!   just where `runtime.tftest.hcl` asserts it today
//!   (`api_carries_app_origin_and_client_id` covers `APP_ORIGIN`, the client id,
//!   `UNSUB_BASE_URL`, `UNSUB_AUDIENCE` and a non-blank `UNSUB_TASKS_CALLER`);
//!   it does NOT assert the caller e-mail values of `UNSUB_TASKS_CALLER` /
//!   `WORKER_SCHEDULER_CALLER` against the one allowed invoker (S7 line 536).
//!   This test does not claim more. Terraform files are owner-only: this test
//!   edits none.
//! * Indirect literals are out of scope. The parser classifies an expression; it
//!   never resolves a `local.*`, so a literal reached through a `local` that
//!   points at another `local` is not inspected (only the four env maps are
//!   parsed, not the `locals` block that defines them).
//! * The required variable list is NOT hard-coded here. The loaders start-up
//!   itself calls - `api::startup::ProductionEnv`,
//!   `unsub::config::ProductionEnv` and `worker::startup::ProductionEnv` - are
//!   the source of truth, so a new required variable is caught automatically.
//! * `PORT` is not provided: Cloud Run injects it (the loaders default to 8080).
//! * A `var.*` / `local.*` reference whose name matches no placeholder rule is
//!   treated as NOT provided, so a new required variable with an unexpected name
//!   fails loudly here instead of being silently satisfied - extend
//!   [`placeholder`] with a value its loader accepts.
//! * The parser is deliberately dumb and FAILS - never skips - on a construct it
//!   does not understand and when it finds none of the maps or bindings, so a
//!   rename or restructure cannot silently disable the check.
//!
//! Every behaviour B2 to B8 has a mutation test (S10 10.4): it applies ONE edit
//! to the real text and has to be rejected, because a check that cannot fail
//! proves nothing.

use std::path::Path;

use api::config::Mode;
use api::startup::{ProductionEnv as ApiEnv, SetupError};
use unsub::config::{ProductionEnv as UnsubEnv, ProductionEnvError as UnsubError};
use worker::startup::{ProductionEnv as WorkerEnv, ProductionEnvError as WorkerError};

/// The runtime module, relative to this crate's manifest directory.
const CLOUD_RUN_REL: &str = "../../../infra/terraform/modules/runtime/cloud_run.tf";

/// The three Cloud Run services and the env map each resource must iterate.
const SERVICES: [(&str, &str); 3] = [
    ("api", "api_env"),
    ("unsub", "unsub_env"),
    ("worker", "worker_env"),
];

// ---------------------------------------------------------------------------
// The Terraform env maps
// ---------------------------------------------------------------------------

/// One `KEY = <expression>` entry of an env map.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    /// The expression as written; for a literal, the quoted content.
    value: String,
    /// True when the expression is a quoted literal.
    literal: bool,
}

/// The three env maps and the binding each service resource uses.
struct TerraformEnv {
    base: Vec<Entry>,
    api: Vec<Entry>,
    unsub: Vec<Entry>,
    worker: Vec<Entry>,
    /// `(service, for_each target)` per service resource. [`parse`] requires
    /// three, so a parse that silently found no binding is impossible.
    bindings: Vec<(String, String)>,
}

impl TerraformEnv {
    /// The map's own entries.
    fn entries(&self, service: &str) -> &[Entry] {
        match service {
            "api" => &self.api,
            "unsub" => &self.unsub,
            "worker" => &self.worker,
            other => unreachable!("no env map for {other}"),
        }
    }

    /// The environment the service's container really gets: `local.base_env`
    /// merged with the service's own entries, the second winning, as
    /// Terraform's `merge` does.
    fn merged(&self, service: &str) -> Vec<Entry> {
        merge(&self.base, self.entries(service))
    }

    /// The map the named resource iterates, as the parse found it.
    fn binding(&self, service: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|(name, _)| name == service)
            .map(|(_, map)| map.as_str())
    }
}

/// `local.base_env` merged with `extra`, the second winning.
fn merge(base: &[Entry], extra: &[Entry]) -> Vec<Entry> {
    let mut merged = base.to_vec();
    for entry in extra {
        match merged.iter_mut().find(|held| held.name == entry.name) {
            Some(slot) => *slot = entry.clone(),
            None => merged.push(entry.clone()),
        }
    }
    merged
}

/// Collapse a line's whitespace, so indentation and column alignment cannot
/// change what the parser sees.
fn squash(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A block that closes at column 0, the only way to leave a top-level resource,
/// so an indented inner block cannot end the body early.
fn is_top_level_close(line: &str) -> bool {
    line.starts_with('}') && line.trim_end() == "}"
}

/// An environment variable name: `[A-Z][A-Z0-9_]*`.
fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// A dotted reference the parser carries as written: `var.env`, `local.unsub_url`,
/// `google_cloud_tasks_queue.unsubscribe.id`,
/// `var.service_accounts["tasks_invoker"]`.
fn is_reference(value: &str) -> bool {
    fn identifier(part: &str) -> bool {
        let mut chars = part.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    !value.is_empty()
        && value.split('.').all(|part| match part.split_once('[') {
            None => identifier(part),
            Some((head, tail)) => {
                identifier(head)
                    && tail
                        .strip_prefix('"')
                        .and_then(|rest| rest.strip_suffix("\"]"))
                        .is_some_and(identifier)
            }
        })
}

/// A trailing `#` comment, keeping a `#` inside a quoted literal.
fn strip_comment(raw: &str) -> &str {
    let mut quoted = false;
    for (index, character) in raw.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '#' if !quoted => return &raw[..index],
            _ => {}
        }
    }
    raw
}

/// The expression as the lookup carries it: `(value, is_literal)`. `None` for
/// anything the parser does not understand, so the caller fails loudly instead
/// of skipping it.
fn classify(value: &str) -> Option<(String, bool)> {
    if let Some(inner) = value.strip_prefix('"') {
        let inner = inner.strip_suffix('"')?;
        return (!inner.contains('\\')).then(|| (inner.to_owned(), true));
    }
    is_reference(value).then(|| (value.to_owned(), false))
}

/// `KEY = <expression>`, one per line. `None` for a line the parser does not
/// understand.
fn parse_entry(flat: &str) -> Option<Entry> {
    let (name, raw) = flat.split_once('=')?;
    let name = name.trim();
    if !is_env_name(name) {
        return None;
    }
    let written = strip_comment(raw).trim();
    if written.is_empty() {
        return None;
    }
    let (value, literal) = classify(written)?;
    Some(Entry {
        name: name.to_owned(),
        value,
        literal,
    })
}

/// Parse one env map: `name = {` (plain) or `name = merge(local.base_env, {`
/// (merged). Fails loudly on anything else, so a renamed or restructured map
/// cannot silently disable the check.
fn parse_map(lines: &[String], name: &str, merged: bool) -> Result<Vec<Entry>, String> {
    let opener = if merged {
        format!("{name} = merge(local.base_env, {{")
    } else {
        format!("{name} = {{")
    };
    let closer = if merged { "})" } else { "}" };
    let start = lines
        .iter()
        .position(|line| squash(line) == opener)
        .ok_or_else(|| {
            format!(
            "cloud_run.tf: no `{opener}` line; {name} was renamed, dropped or is no longer a merge \
             of local.base_env (T-1119 requires the service map to inherit local.base_env)"
        )
        })?;
    let mut entries: Vec<Entry> = Vec::new();
    for line in &lines[start + 1..] {
        let flat = squash(line);
        if flat == closer {
            return Ok(entries);
        }
        if flat.is_empty() || flat.starts_with('#') {
            continue;
        }
        let entry = parse_entry(&flat).ok_or_else(|| {
            format!(
                "cloud_run.tf: {name} line the parser does not understand: `{line}`; expected one \
                 `KEY = <expression>` per line, no nested object and no interpolation"
            )
        })?;
        if entries.iter().any(|held| held.name == entry.name) {
            return Err(format!("cloud_run.tf: {name} sets {} twice", entry.name));
        }
        entries.push(entry);
    }
    Err(format!(
        "cloud_run.tf: {name} has no closing `{closer}` line"
    ))
}

/// The `(first body line, closing line)` indices of `name = {` / `merge(...)`.
fn map_span(lines: &[String], name: &str) -> Result<(usize, usize), String> {
    let plain = format!("{name} = {{");
    let merged = format!("{name} = merge(local.base_env, {{");
    let start = lines
        .iter()
        .position(|line| squash(line) == plain || squash(line) == merged)
        .ok_or_else(|| format!("no `{plain}` or `{merged}` line"))?;
    let closer = if squash(&lines[start]) == merged {
        "})"
    } else {
        "}"
    };
    let end = lines[start + 1..]
        .iter()
        .position(|line| squash(line) == closer)
        .map(|offset| start + 1 + offset)
        .ok_or_else(|| format!("`{name}` has no closing `{closer}` line"))?;
    Ok((start + 1, end))
}

/// The `(first body line, closing line)` indices of one service resource.
fn resource_span(lines: &[String], service: &str) -> Result<(usize, usize), String> {
    let header = format!("resource \"google_cloud_run_v2_service\" \"{service}\" {{");
    let start = lines
        .iter()
        .position(|line| squash(line) == header)
        .ok_or_else(|| format!("cloud_run.tf: no `{header}` block"))?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| is_top_level_close(line))
        .map(|offset| start + 1 + offset)
        .ok_or_else(|| format!("cloud_run.tf: resource \"{service}\" is unterminated"))?;
    Ok((start + 1, end))
}

/// Each service resource must bind its container env to the matching map and
/// emit it as `name` / `value`, so the map really becomes the container's
/// environment.
fn check_resource(lines: &[String], service: &str, map: &str) -> Result<(), String> {
    let (start, end) = resource_span(lines, service)?;
    let body: Vec<String> = lines[start..end].iter().map(|line| squash(line)).collect();

    let dynamic = body
        .iter()
        .filter(|line| line.as_str() == "dynamic \"env\" {")
        .count();
    if dynamic != 1 {
        return Err(format!(
            "cloud_run.tf {service}: the service resource must hold exactly one `dynamic \"env\"` \
             block, found {dynamic}"
        ));
    }

    let target = format!("for_each = local.{map}");
    let for_each: Vec<&str> = body
        .iter()
        .filter(|line| line.starts_with("for_each = "))
        .map(String::as_str)
        .collect();
    if for_each.len() != 1 || for_each[0] != target {
        return Err(format!(
            "cloud_run.tf {service}: the container env must iterate `{target}` and nothing else, \
             found {for_each:?}"
        ));
    }

    for emitted in ["name = env.key", "value = env.value"] {
        let count = body.iter().filter(|line| line.as_str() == emitted).count();
        if count != 1 {
            return Err(format!(
                "cloud_run.tf {service}: the `content` block must emit `{emitted}` exactly once, \
                 found {count}; the map has to become the container environment"
            ));
        }
    }
    Ok(())
}

/// Parse the whole file: the four maps and the three resource bindings. Fails
/// loudly if any of them is missing or unreadable.
fn parse(text: &str) -> Result<TerraformEnv, String> {
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let base = parse_map(&lines, "base_env", false)?;
    let api = parse_map(&lines, "api_env", true)?;
    let unsub = parse_map(&lines, "unsub_env", true)?;
    let worker = parse_map(&lines, "worker_env", true)?;

    let mut bindings = Vec::new();
    for (service, map) in SERVICES {
        check_resource(&lines, service, map)?;
        bindings.push((service.to_owned(), map.to_owned()));
    }
    if bindings.len() != SERVICES.len() {
        return Err(format!(
            "cloud_run.tf: found {} service bindings, expected {}; the parser must never pass \
             vacuously",
            bindings.len(),
            SERVICES.len()
        ));
    }
    Ok(TerraformEnv {
        base,
        api,
        unsub,
        worker,
        bindings,
    })
}

// ---------------------------------------------------------------------------
// The lookup the loaders read
// ---------------------------------------------------------------------------

/// The placeholder for a reference, by variable name. Only what the loaders
/// validate matters: `*_URL` / `*_AUDIENCE` an https URL, `*_CALLER` / `*_SA` an
/// e-mail-shaped service account, a project name a valid project id,
/// `APP_ORIGIN` an https origin with no trailing slash and no path, the OAuth
/// client id a client-id-shaped string, the user-data KMS key the full resource
/// name Cloud Run injects.
///
/// A name with no rule is NOT provided, so a new required variable fails loudly
/// here instead of being satisfied by a wrong-shaped guess.
fn placeholder(name: &str) -> Option<String> {
    let value = match name {
        "APP_ORIGIN" => "https://mailtinder-prod.web.app",
        "GOOGLE_OAUTH_CLIENT_ID" => "bootcheck.apps.googleusercontent.com",
        _ if name.contains("KMS_KEY") => {
            "projects/mailtinder-prod/locations/europe-west1/keyRings/mt-prod/cryptoKeys/data-key-kek"
        }
        _ if name.ends_with("_URL") || name.ends_with("_AUDIENCE") => {
            "https://mt-bootcheck.a.run.app"
        }
        _ if name.ends_with("_CALLER") || name.ends_with("_SA") => {
            "bootcheck@mailtinder-prod.iam.gserviceaccount.com"
        }
        _ if name.contains("PROJECT") => "mailtinder-prod",
        _ => return None,
    };
    Some(value.to_owned())
}

/// A lookup over one service's merged env map, as the loaders see it.
///
/// A literal is passed through as written; a reference becomes its placeholder,
/// or nothing at all when no rule matches; a name the map does not set is
/// absent. `PORT` is not provided: Cloud Run injects it.
fn lookup(entries: &[Entry]) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| {
        let entry = entries.iter().find(|held| held.name == name)?;
        if entry.literal {
            Some(entry.value.clone())
        } else {
            placeholder(name)
        }
    }
}

// ---------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------

/// The prescribed failure message: the variable, then the service's map.
fn requires(service: &str, map: &str, name: &str) -> String {
    format!("{service} requires {name} but cloud_run.tf {map} does not set it")
}

/// The failure message for a variable that is set to something the loader
/// rejects (a malformed reference placeholder or a blank/ill-formed literal).
fn rejects(service: &str, map: &str, name: &str) -> String {
    format!("{service} rejects the value cloud_run.tf {map} gives {name}")
}

/// Run the api's real production loader over the environment Terraform gives
/// it.
fn check_api(env: &TerraformEnv) -> Result<(), String> {
    let entries = env.merged("api");
    match ApiEnv::from_lookup(lookup(&entries), Mode::Production) {
        Ok(_) => Ok(()),
        Err(SetupError::Missing(name)) => Err(requires("api", "api_env", name)),
        Err(SetupError::Invalid(name)) => Err(rejects("api", "api_env", name)),
        Err(other) => Err(format!("api: the production loader failed: {other}")),
    }
}

/// As [`check_api`], for unsub.
fn check_unsub(env: &TerraformEnv) -> Result<(), String> {
    let entries = env.merged("unsub");
    match UnsubEnv::from_lookup(lookup(&entries)) {
        Ok(_) => Ok(()),
        Err(UnsubError::Missing(name)) => Err(requires("unsub", "unsub_env", name)),
        Err(UnsubError::Invalid(name)) => Err(rejects("unsub", "unsub_env", name)),
    }
}

/// As [`check_api`], for the worker.
fn check_worker(env: &TerraformEnv) -> Result<(), String> {
    let entries = env.merged("worker");
    match WorkerEnv::from_lookup(lookup(&entries)) {
        Ok(_) => Ok(()),
        Err(WorkerError::Missing(name)) => Err(requires("worker", "worker_env", name)),
        Err(WorkerError::Invalid(name)) => Err(rejects("worker", "worker_env", name)),
    }
}

/// Supplemental guard, NOT a verification of ASVS V13.3.1 (Secret Manager
/// design review and Gitleaks remain the controls): no env entry whose name
/// contains `SECRET`, `TOKEN`, `PASSWORD` or `KEY` may carry a DIRECT literal
/// value - it must reference an input or Secret Manager.
///
/// Only direct literals: the parser classifies an expression and never resolves
/// a `local.*`, so an indirect literal is out of scope.
fn check_no_literal_secrets(env: &TerraformEnv) -> Result<(), String> {
    for (map, entries) in [
        ("base_env", &env.base),
        ("api_env", &env.api),
        ("unsub_env", &env.unsub),
        ("worker_env", &env.worker),
    ] {
        for entry in entries {
            let secret_like = ["SECRET", "TOKEN", "PASSWORD", "KEY"]
                .iter()
                .any(|needle| entry.name.contains(needle));
            if secret_like && entry.literal {
                return Err(format!(
                    "cloud_run.tf {map}: {} carries a literal value; a secret-looking variable must \
                     reference an input or Secret Manager, never a literal (ASVS V13.3.1)",
                    entry.name
                ));
            }
        }
    }
    Ok(())
}

/// The whole check: parse the Terraform text, then run each service's real
/// production loader over the environment that service is given.
fn check(text: &str) -> Result<(), String> {
    let env = parse(text)?;
    check_no_literal_secrets(&env)?;
    check_api(&env)?;
    check_unsub(&env)?;
    check_worker(&env)
}

/// `infra/terraform/modules/runtime/cloud_run.tf`, read as text so a moved file
/// fails here instead of the check reading a stale copy.
fn cloud_run_tf() -> Result<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CLOUD_RUN_REL);
    std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Apply one edit to the real text and require the check to reject it, naming
/// what the task says it must name. `assert_ne!` guards the test itself: a
/// mutation that no longer matches the file would otherwise pass silently.
fn mutation(
    expected: &[&str],
    edit: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = cloud_run_tf()?;
    let mutated = edit(&text)?;
    assert_ne!(
        mutated, text,
        "the mutation changed nothing in cloud_run.tf; the test is stale"
    );
    let error = match check(&mutated) {
        Ok(()) => {
            return Err(format!(
                "the boot check accepted a mutated cloud_run.tf (expected to name {expected:?})"
            )
            .into())
        }
        Err(error) => error,
    };
    for needle in expected {
        assert!(
            error.contains(needle),
            "the failure must name {needle:?}, got {error:?}"
        );
    }
    Ok(())
}

/// The lines of `text`, for the mutation helpers.
fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// Join lines back into a file, keeping the trailing newline.
fn join_lines(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Replace exactly one occurrence, or fail: a mutation that matches nothing
/// would leave the check passing and the test meaningless.
fn replace_once(text: &str, from: &str, to: &str) -> Result<String, String> {
    let count = text.matches(from).count();
    if count != 1 {
        return Err(format!(
            "the mutation {from:?} matches {count} times in cloud_run.tf, expected once; the test \
             is stale"
        ));
    }
    Ok(text.replacen(from, to, 1))
}

/// The index of the `key = ...` line inside `map`.
fn entry_line(lines: &[String], map: &str, key: &str) -> Result<usize, String> {
    let (start, end) = map_span(lines, map)?;
    (start..end)
        .find(|&index| {
            squash(&lines[index])
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        })
        .ok_or_else(|| format!("{map} does not set {key}; the test is stale"))
}

/// Delete one entry from `map`.
fn drop_entry(text: &str, map: &str, key: &str) -> Result<String, String> {
    let mut lines = lines(text);
    let index = entry_line(&lines, map, key)?;
    lines.remove(index);
    Ok(join_lines(&lines))
}

/// Set one entry's value in `map` (the key is rewritten canonically).
fn set_entry(text: &str, map: &str, key: &str, value: &str) -> Result<String, String> {
    let mut lines = lines(text);
    let index = entry_line(&lines, map, key)?;
    lines[index] = format!("    {key} = {value}");
    Ok(join_lines(&lines))
}

/// Insert a raw line as the first line of `map`, for the unsupported-construct
/// mutation.
fn insert_in_map(text: &str, map: &str, raw: &str) -> Result<String, String> {
    let mut lines = lines(text);
    let (start, _) = map_span(&lines, map)?;
    lines.insert(start, raw.to_owned());
    Ok(join_lines(&lines))
}

/// Replace one line inside one service resource, so an edit anchored on
/// `name = env.key` cannot hit another service by accident.
fn replace_in_resource(text: &str, service: &str, from: &str, to: &str) -> Result<String, String> {
    let mut lines = lines(text);
    let (start, end) = resource_span(&lines, service)?;
    let index = (start..end)
        .find(|&index| squash(&lines[index]) == from)
        .ok_or_else(|| format!("resource {service} has no `{from}` line; the test is stale"))?;
    lines[index] = to.to_owned();
    Ok(join_lines(&lines))
}

// ---------------------------------------------------------------------------
// B1: the real file passes
// ---------------------------------------------------------------------------

/// B1: with `cloud_run.tf` as it is, all three services pass, and the parser
/// found every map and binding (it can never pass vacuously).
#[test]
fn b1_every_required_variable_is_set_for_all_three_services(
) -> Result<(), Box<dyn std::error::Error>> {
    let text = cloud_run_tf()?;
    let env = parse(&text)?;
    assert_eq!(
        env.bindings.len(),
        SERVICES.len(),
        "every service resource must bind its own map, found {:?}",
        env.bindings
    );
    for (service, map) in SERVICES {
        assert_eq!(
            env.binding(service),
            Some(map),
            "resource {service} must iterate local.{map}"
        );
    }
    check(&text)?;
    Ok(())
}

/// B1, the other half: each start-up path resolves its environment through the
/// same loader this test drives, so start-up and the check cannot diverge.
///
/// The wiring, not a runtime proof; source paths are relative to this crate's
/// manifest directory.
#[test]
fn b1_each_startup_path_loads_its_environment_through_the_same_loader(
) -> Result<(), Box<dyn std::error::Error>> {
    for (relative, service) in [
        ("../unsub/src/main.rs", "unsub"),
        ("../worker/src/main.rs", "worker"),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        assert!(
            source.contains("ProductionEnv::from_env()"),
            "{service} must load its whole environment through ProductionEnv"
        );
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/startup.rs");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    assert!(
        source.contains("ProductionEnv::from_env(Mode::Production)"),
        "api's build_production_ports must load its whole environment through ProductionEnv"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B2 to B8: mutations
// ---------------------------------------------------------------------------

/// B2: deleting `APP_ORIGIN` from `api_env` fails, naming `APP_ORIGIN` and
/// `api`.
#[test]
fn b2_api_without_app_origin_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    mutation(&["api requires APP_ORIGIN", "api_env"], |text| {
        drop_entry(text, "api_env", "APP_ORIGIN")
    })
}

/// B3: deleting `UNSUB_AUDIENCE` from `unsub_env`, or `WORKER_AUDIENCE` from
/// `worker_env`, fails likewise.
#[test]
fn b3_unsub_or_worker_without_its_audience_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    mutation(&["unsub requires UNSUB_AUDIENCE", "unsub_env"], |text| {
        drop_entry(text, "unsub_env", "UNSUB_AUDIENCE")
    })?;
    mutation(&["worker requires WORKER_AUDIENCE", "worker_env"], |text| {
        drop_entry(text, "worker_env", "WORKER_AUDIENCE")
    })
}

/// B4: a literal secret-looking value in any env map fails the test.
#[test]
fn b4_a_literal_secret_looking_value_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    mutation(&["MT_SECRET_OAUTH_CLIENT", "base_env", "literal"], |text| {
        set_entry(
            text,
            "base_env",
            "MT_SECRET_OAUTH_CLIENT",
            "\"sk-bootcheck-not-a-secret\"",
        )
    })?;
    mutation(&["MT_KMS_KEY", "literal"], |text| {
        set_entry(text, "base_env", "MT_KMS_KEY", "\"bootcheck-key-name\"")
    })
}

/// B5: the parser never passes vacuously - a renamed map or an unsupported
/// construct is rejected, not skipped.
#[test]
fn b5_a_renamed_map_or_an_unsupported_construct_is_rejected(
) -> Result<(), Box<dyn std::error::Error>> {
    mutation(&["api_env", "merge"], |text| {
        replace_once(
            text,
            "api_env = merge(local.base_env, {",
            "api_environment = merge(local.base_env, {",
        )
    })?;
    // A line with no `KEY = <expression>`: the parser must say what it could not
    // read rather than skipping it.
    mutation(&["api_env", "does not understand", "MT_BROKEN"], |text| {
        insert_in_map(text, "api_env", "    MT_BROKEN")
    })?;
    // And a map whose binding is gone: the resource must still name the map.
    mutation(&["worker_env"], |text| {
        replace_once(
            text,
            "  worker_env = merge(local.base_env, {",
            "  workers_env = merge(local.base_env, {",
        )
    })
}

/// B6: deleting any other variable the api's start-up path requires fails,
/// naming the variable.
#[test]
fn b6_api_without_each_remaining_required_variable_is_rejected(
) -> Result<(), Box<dyn std::error::Error>> {
    for (map, key) in [
        ("api_env", "UNSUB_BASE_URL"),
        ("api_env", "UNSUB_AUDIENCE"),
        ("api_env", "UNSUB_TASKS_CALLER"),
        ("base_env", "GOOGLE_CLOUD_PROJECT"),
        ("api_env", "GOOGLE_OAUTH_CLIENT_ID"),
    ] {
        let expected = format!("api requires {key}");
        mutation(&[expected.as_str()], |text| drop_entry(text, map, key))?;
    }
    Ok(())
}

/// B7: rebinding the worker resource's `for_each` to another service's map, or
/// setting a required variable to a blank literal, fails the test.
#[test]
fn b7_a_rebound_for_each_or_a_blank_required_value_is_rejected(
) -> Result<(), Box<dyn std::error::Error>> {
    mutation(&["worker", "local.worker_env"], |text| {
        replace_in_resource(
            text,
            "worker",
            "for_each = local.worker_env",
            "for_each = local.unsub_env",
        )
    })?;
    mutation(&["unsub requires UNSUB_TASKS_CALLER"], |text| {
        set_entry(text, "unsub_env", "UNSUB_TASKS_CALLER", "\"\"")
    })
}

/// B9 (T-1119 review F1): the KMS key name is part of the contract too. Every
/// service's start-up path builds its KMS client from `MT_KMS_KEY`, which
/// Terraform sets once in `base_env`; deleting it there must fail naming the
/// variable, so a service cannot silently fall back to the hard-coded key path.
///
/// The check reports the first service it reaches, so the per-service half is
/// asserted against each service's own loader below.
#[test]
fn b9_mt_kms_key_is_required_by_every_service_that_reads_it(
) -> Result<(), Box<dyn std::error::Error>> {
    mutation(&["api requires MT_KMS_KEY", "api_env"], |text| {
        drop_entry(text, "base_env", "MT_KMS_KEY")
    })?;

    let without = drop_entry(&cloud_run_tf()?, "base_env", "MT_KMS_KEY")?;
    let env = parse(&without)?;
    assert!(
        matches!(
            UnsubEnv::from_lookup(lookup(&env.merged("unsub"))),
            Err(UnsubError::Missing("MT_KMS_KEY"))
        ),
        "unsub must refuse an environment without MT_KMS_KEY"
    );
    assert!(
        matches!(
            WorkerEnv::from_lookup(lookup(&env.merged("worker"))),
            Err(WorkerError::Missing("MT_KMS_KEY"))
        ),
        "worker must refuse an environment without MT_KMS_KEY"
    );
    Ok(())
}

/// B8: swapping `value = env.value` for `value = env.key` - or `name = env.key`
/// for `name = env.value` - in any service's `content` block fails the test.
#[test]
fn b8_a_swapped_name_or_value_binding_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    mutation(&["api", "value = env.value"], |text| {
        replace_in_resource(text, "api", "value = env.value", "value = env.key")
    })?;
    mutation(&["worker", "name = env.key"], |text| {
        replace_in_resource(text, "worker", "name = env.key", "name = env.value")
    })
}
