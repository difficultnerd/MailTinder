#![allow(clippy::pedantic)]

//! Policy tests for the deploy workflow (T-1104b).
//!
//! The behaviours of docs/backlog/T-1104b-deploy-preflight-and-path-filter.md,
//! as the workflow text has to encode them:
//!
//! - B2 a docs-only push must not start the workflow (`on.push.paths`);
//! - B3 `workflow_dispatch` is never filtered, and with a staging variable
//!   missing a manual run fails loudly, naming it;
//! - B1 while the staging variables are unset the run ends GREEN-SKIPPED with a
//!   `::warning::` annotation and one summary line, and `DEPLOY_ENABLED=true`
//!   turns that silence into a red run;
//! - B4 `deploy-prod` is skipped, with no approval request, while a `PROD_*`
//!   variable is empty;
//! - B5 every Cloud Run service the workflow deploys - and the `firebase.json`
//!   Hosting rewrite's `serviceId` - is a service Terraform declares.
//!
//! Everything is read with `include_str!`, so the policy fails on the commit
//! that changes the workflow rather than the day a run goes red. Every
//! behaviour above also has a mutation test: it applies ONE edit to the real
//! text and has to be rejected, because a check that cannot fail proves
//! nothing (S10 10.4).
//!
//! T-1104d closes the PR #89 review findings S-1 to S-8, which two rework
//! rounds left open: the preflight's variable loops and its two
//! `*_ready=false` assignments (S-1, S-8), the `permissions:` scope (S-2), a
//! catch-all or empty `on.push.paths` list (S-3), the Cloud Run region as the
//! runtime module declares it (S-4), `DEPLOY_ENABLED` (S-6) and the pin rule
//! over every file under `.github/workflows/` rather than `deploy.yml` alone
//! (S-7). Each has its own check and its own mutation, plus one test that runs
//! the extracted preflight script with every variable unset.

use std::ops::Range;
use std::path::Path;

const WORKFLOW: &str = include_str!("../../../../.github/workflows/deploy.yml");
const CLOUD_RUN: &str = include_str!("../../../../infra/terraform/modules/runtime/cloud_run.tf");
const FIREBASE: &str = include_str!("../../../../firebase.json");

/// The runtime module's interface (S-4): its `region` default is the one region
/// every regional service - Cloud Run, Cloud Tasks, Scheduler - takes.
const RUNTIME_VARIABLES: &str =
    include_str!("../../../../infra/terraform/modules/runtime/variables.tf");

/// The deploy runbook. Read only, to prove `DEPLOY_ENABLED` is documented where
/// the preflight's own warning points the owner (S-6).
const README: &str = include_str!("../../../../infra/terraform/README.md");

/// The staging repository variables the preflight has to test (S-1).
const STAGING_VARIABLES: &[&str] = &[
    "STAGING_PROJECT",
    "STAGING_WIF_PROVIDER",
    "STAGING_DEPLOYER",
];

/// The production repository variables the preflight has to test (S-1).
const PROD_VARIABLES: &[&str] = &["PROD_PROJECT", "PROD_WIF_PROVIDER", "PROD_DEPLOYER"];

/// The one summary line `configured` prints when it skips, verbatim.
const SKIP_NOTICE: &str =
    "deploy skipped: staging variables are not set yet (infra/terraform/README.md, Deploying)";

/// The `configured` job's `DEPLOY_ENABLED` failure branch, verbatim.
const DEPLOY_ENABLED_BRANCH: &str = r#"          if [ "${DEPLOY_ENABLED:-}" = "true" ]; then
            echo "::error::deploy is enabled but staging is not configured: set these repository variables:${staging_missing}"
            exit 1
          fi
"#;

/// The `configured` job's manual-run failure branch, verbatim.
const WORKFLOW_DISPATCH_BRANCH: &str = r#"          if [ "${GITHUB_EVENT_NAME:-}" = "workflow_dispatch" ]; then
            echo "::error::manual deploy requested but staging is not configured: set these repository variables:${staging_missing}"
            exit 1
          fi
"#;

// ---------------------------------------------------------------------------
// Reading the workflow as lines
//
// Hand rolled, as in dockerfile_policy.rs: the shape read here is a small,
// fixed one (indent, `key:`/`key: value`, `- item`), and pulling a YAML
// dependency into a test binary would test the parser, not the workflow.
// ---------------------------------------------------------------------------

/// How deep a line is indented.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A line without its indentation.
fn text(line: &str) -> &str {
    line.trim()
}

/// Whether a line is a YAML comment (which can never hold a key).
fn is_comment(line: &str) -> bool {
    text(line).starts_with('#')
}

/// Whether a line is the key `key:`, with or without a value. A step written
/// as the sequence entry `- uses: ...` carries its key after the marker.
fn is_key(line: &str, key: &str) -> bool {
    list_item(line)
        .strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some()
}

/// The value of `key: value`, or `None` for a block opener or another key.
fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = list_item(line).strip_prefix(key)?.strip_prefix(':')?.trim();
    (!rest.is_empty()).then_some(rest)
}

/// A `- item` sequence entry, without the marker.
fn list_item(line: &str) -> &str {
    text(line).strip_prefix("- ").unwrap_or_else(|| text(line))
}

/// The non-blank lines of a text file.
fn yaml_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// The lines of the block opened by `lines[start]`: everything indented deeper
/// than it, up to the next line at its indent or shallower.
fn block_range(lines: &[&str], start: usize) -> Range<usize> {
    let opened = indent(lines[start]);
    let mut end = start + 1;
    while end < lines.len() && indent(lines[end]) > opened {
        end += 1;
    }
    start + 1..end
}

/// The first line in `range` that is the key `key` at exactly `depth`.
fn key_line(lines: &[&str], range: Range<usize>, depth: usize, key: &str) -> Option<usize> {
    range.into_iter().find(|&index| {
        indent(lines[index]) == depth && !is_comment(lines[index]) && is_key(lines[index], key)
    })
}

/// Every job in the workflow: its name and the range of its own lines.
fn jobs<'a>(lines: &[&'a str]) -> Vec<(&'a str, Range<usize>)> {
    let Some(open) = key_line(lines, 0..lines.len(), 0, "jobs") else {
        return Vec::new();
    };
    let block = block_range(lines, open);
    let mut out = Vec::new();
    let mut index = block.start;
    while index < block.end {
        if indent(lines[index]) == 2 && !is_comment(lines[index]) {
            let mut end = index + 1;
            while end < block.end && !(indent(lines[end]) == 2 && !is_comment(lines[end])) {
                end += 1;
            }
            out.push((text(lines[index]).trim_end_matches(':'), index + 1..end));
            index = end;
        } else {
            index += 1;
        }
    }
    out
}

/// The lines of one job, by name.
fn job(jobs: &[(&str, Range<usize>)], name: &str) -> Option<Range<usize>> {
    jobs.iter()
        .find(|(job_name, _)| *job_name == name)
        .map(|(_, range)| range.clone())
}

/// The jobs a job declares with `needs:` - a scalar or an `[a, b]` list.
fn needs(lines: &[&str], job: Range<usize>) -> Vec<String> {
    job.filter(|&index| indent(lines[index]) == 4)
        .find_map(|index| value_of(lines[index], "needs"))
        .map(|value| {
            value
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A job's `if:` expression.
fn condition<'a>(lines: &[&'a str], job: Range<usize>) -> Option<&'a str> {
    job.filter(|&index| indent(lines[index]) == 4)
        .find_map(|index| value_of(lines[index], "if"))
}

/// Every shell line of a job's `run:` blocks.
fn shell_lines<'a>(lines: &[&'a str], job: Range<usize>) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut index = job.start;
    while index < job.end {
        if value_of(lines[index], "run").is_some() {
            let opened = indent(lines[index]);
            let mut end = index + 1;
            while end < job.end && indent(lines[end]) > opened {
                end += 1;
            }
            out.extend(lines[index + 1..end].iter().copied());
            index = end;
        } else {
            index += 1;
        }
    }
    out
}

/// Whether the shell tests `marker` and exits non-zero under that test: the
/// `if [ ... ]; then ... exit 1; fi` shape.
fn failure_branch(shell: &[&str], marker: &str) -> bool {
    for (index, line) in shell.iter().enumerate() {
        let candidate = text(line);
        if !candidate.starts_with("if ") || !candidate.contains(marker) {
            continue;
        }
        let opened = indent(line);
        for later in &shell[index + 1..] {
            if indent(later) <= opened {
                break;
            }
            if text(later) == "exit 1" {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Reading the deploy targets and the Terraform service names
// ---------------------------------------------------------------------------

/// The Cloud Run service names `cloud_run.tf` declares: the `name = "..."`
/// string of each resource (the IAM bindings name the resource, not a string).
fn declared_services(cloud_run: &str) -> Vec<String> {
    cloud_run
        .lines()
        .filter_map(|line| {
            let rest = text(line).strip_prefix("name")?.trim_start();
            let rest = rest.strip_prefix('=')?.trim_start().strip_prefix('"')?;
            rest.find('"').map(|end| rest[..end].to_owned())
        })
        .collect()
}

/// A template with every `${name}` replaced by each value of that shell loop.
fn expand(target: &str, loops: &[(String, Vec<String>)]) -> Vec<String> {
    let mut out = vec![target.to_owned()];
    for (name, values) in loops {
        let marker = format!("${{{name}}}");
        let mut next = Vec::new();
        for candidate in &out {
            if candidate.contains(&marker) {
                for value in values {
                    next.push(candidate.replace(&marker, value));
                }
            } else {
                next.push(candidate.clone());
            }
        }
        out = next;
    }
    out
}

/// The shell `for <name> in <values>; do` loops of the workflow.
fn loop_variables(lines: &[&str]) -> Vec<(String, Vec<String>)> {
    lines
        .iter()
        .filter_map(|line| {
            let rest = text(line).strip_prefix("for ")?;
            let (name, values) = rest.split_once(" in ")?;
            let values = values.strip_suffix("; do")?;
            Some((
                name.to_owned(),
                values.split_whitespace().map(str::to_owned).collect(),
            ))
        })
        .collect()
}

/// Every Cloud Run service name handed to `gcloud run deploy`, with `${svc}`
/// expanded over the workflow's `for` loops.
fn deploy_targets(lines: &[&str], loops: &[(String, Vec<String>)]) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines {
        if is_comment(line) {
            continue;
        }
        let Some((_, command)) = text(line).split_once("gcloud run deploy") else {
            continue;
        };
        let target = command.split_whitespace().next().unwrap_or_default();
        out.extend(expand(target.trim_matches('"').trim_matches('\''), loops));
    }
    out
}

/// The Cloud Run service `firebase.json` rewrites `/api/**` to.
fn firebase_service_id(firebase: &str) -> Option<&str> {
    let (_, rest) = firebase.split_once("\"serviceId\"")?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    rest.find('"').map(|end| &rest[..end])
}

/// The region of `firebase.json`'s Hosting `run` rewrite (S-4).
fn firebase_run_region(firebase: &str) -> Option<String> {
    let (_, rest) = firebase.split_once("\"run\"")?;
    let run = rest.split('}').next()?;
    let (_, rest) = run.split_once("\"region\"")?;
    let rest = rest
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    rest.find('"').map(|end| rest[..end].to_owned())
}

/// The `region` default of the runtime module, from `variables.tf`.
///
/// Hand rolled as in dockerfile_policy.rs: the shape read here is the HCL
/// `default = "..."` of one variable block, and a HCL dependency would test the
/// parser, not the module.
fn region_default(variables: &str) -> Option<String> {
    let lines: Vec<&str> = variables.lines().collect();
    let open = lines
        .iter()
        .position(|line| text(line) == "variable \"region\" {")?;
    let mut index = open + 1;
    while index < lines.len() && text(lines[index]) != "}" {
        if let Some(value) = hcl_string(lines[index], "default") {
            return Some(value);
        }
        index += 1;
    }
    None
}

/// The value of a `key = "..."` line, if the line is exactly that assignment.
fn hcl_string(line: &str, key: &str) -> Option<String> {
    let rest = text(line).strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start().strip_prefix('"')?;
    rest.find('"').map(|end| rest[..end].to_owned())
}

/// The value of `name` in the workflow's top-level `env` block (S-4).
fn top_level_env(lines: &[&str], name: &str) -> Option<String> {
    let open = key_line(lines, 0..lines.len(), 0, "env")?;
    block_range(lines, open)
        .find_map(|index| value_of(lines[index], name))
        .map(|value| value.trim_matches('"').to_owned())
}

/// Every `gcloud run deploy` command, as the shell sees it: a line that ends in
/// a backslash continues on the next one (S-4 reads the whole command).
fn deploy_commands(lines: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if is_comment(lines[index]) || !text(lines[index]).contains("gcloud run deploy") {
            index += 1;
            continue;
        }
        let mut command = String::new();
        while index < lines.len() {
            command.push_str(text(lines[index]));
            if !command.trim_end().ends_with('\\') {
                break;
            }
            index += 1;
        }
        out.push(command);
        index += 1;
    }
    out
}

/// A job's `run:` blocks, joined as one shell script (S-8 executes it).
fn run_script(lines: &[&str], job: Range<usize>) -> Option<String> {
    let shell = shell_lines(lines, job);
    (!shell.is_empty()).then(|| shell.join("\n"))
}

/// Every workflow file, read at test time rather than through `include_str!`,
/// so a file added to the directory later is covered too (S-7).
fn workflow_sources() -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.github/workflows");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".yml") || name.ends_with(".yaml") {
            out.push((name, std::fs::read_to_string(entry.path())?));
        }
    }
    out.sort();
    Ok(out)
}

// ---------------------------------------------------------------------------
// The policy
// ---------------------------------------------------------------------------

/// Every way these texts break the T-1104b policy, extended by T-1104d with the
/// `permissions:` scope, the Cloud Run region, the documented `DEPLOY_ENABLED`
/// switch and the pin rule over the whole `.github/workflows/` directory; empty
/// means the workflow is the one the task describes. The tests below mutate one
/// thing at a time.
fn violations(
    workflow: &str,
    cloud_run: &str,
    firebase: &str,
    runtime_variables: &str,
    readme: &str,
) -> Vec<String> {
    let lines = yaml_lines(workflow);
    let mut errors = Vec::new();
    check_triggers(&lines, &mut errors);
    check_permissions(&lines, &mut errors);
    check_jobs(&lines, &mut errors);
    check_pins("deploy.yml", &lines, &mut errors);
    match workflow_sources() {
        Err(error) => errors.push(format!(".github/workflows could not be read: {error}")),
        Ok(sources) => check_other_workflows(&sources, &mut errors),
    }
    check_services(&lines, cloud_run, firebase, runtime_variables, &mut errors);
    check_deploy_enabled_docs(&lines, readme, &mut errors);
    errors
}

/// S-2: the checkout needs `contents: read` and nothing more, top level and in
/// the preflight. A wider scope hands every job a token no step uses.
fn check_permissions(lines: &[&str], errors: &mut Vec<String>) {
    check_contents_read(lines, 0..lines.len(), 0, "the workflow", errors);

    let all_jobs = jobs(lines);
    if let Some(configured) = job(&all_jobs, "configured") {
        check_contents_read(lines, configured, 4, "`configured`", errors);
    }
}

/// The `contents:` scope a `permissions:` block at `depth` declares.
fn check_contents_read(
    lines: &[&str],
    scope: Range<usize>,
    depth: usize,
    label: &str,
    errors: &mut Vec<String>,
) {
    let contents = key_line(lines, scope, depth, "permissions")
        .and_then(|open| key_line(lines, block_range(lines, open), depth + 2, "contents"))
        .and_then(|line| value_of(lines[line], "contents"))
        .map(|value| value.trim_matches('"'));
    if contents != Some("read") {
        errors.push(format!(
            "{label} does not declare `permissions: contents: read` (found {contents:?}): the \
             checkout needs read, and anything wider is a token no step here uses"
        ));
    }
}

/// S-7: the pin rule is a repository rule, so it covers every file under
/// `.github/workflows/`, read at test time - not just the workflow this policy
/// is about. `deploy.yml` itself is checked above, from the text under test.
///
/// Every file in the directory is pinned today, so the check needs no exclusion
/// list; the owner fixes a workflow that is not, and the pull request that adds
/// it lists the file.
fn check_other_workflows(sources: &[(String, String)], errors: &mut Vec<String>) {
    if sources.is_empty() {
        errors.push(".github/workflows holds no workflow file".to_owned());
    }
    for (name, source) in sources {
        if name == "deploy.yml" {
            continue;
        }
        check_pins(name, &yaml_lines(source), errors);
    }
}

/// S-6: `DEPLOY_ENABLED` is a documented switch, not a variable only the shell
/// knows. The workflow's own comment block has to name it, and so has the
/// runbook the skip notice points the owner at.
fn check_deploy_enabled_docs(lines: &[&str], readme: &str, errors: &mut Vec<String>) {
    if !lines
        .iter()
        .any(|line| is_comment(line) && line.contains("DEPLOY_ENABLED"))
    {
        errors.push(
            "no comment in deploy.yml names `DEPLOY_ENABLED`: nobody reading the workflow learns \
             the switch exists"
                .to_owned(),
        );
    }
    if !readme.contains("DEPLOY_ENABLED") {
        errors.push(
            "infra/terraform/README.md does not name `DEPLOY_ENABLED`: the notice this workflow \
             prints points the owner at a runbook that never mentions it"
                .to_owned(),
        );
    }
}

/// S-4: one region, declared once by the runtime module and used everywhere -
/// the workflow's `REGION`, every `gcloud run deploy` command (as
/// `--region "${REGION}"`, never a literal) and the Hosting rewrite.
fn check_regions(
    lines: &[&str],
    runtime_variables: &str,
    firebase: &str,
    errors: &mut Vec<String>,
) {
    let Some(declared) = region_default(runtime_variables) else {
        errors.push(
            "infra/terraform/modules/runtime/variables.tf declares no `region` default: the \
             workflow's region cannot be checked against the module"
                .to_owned(),
        );
        return;
    };
    let Some(region) = top_level_env(lines, "REGION") else {
        errors.push(
            "the workflow's top-level `env` has no `REGION`: the deploy commands and the Hosting \
             rewrite would be left to guess"
                .to_owned(),
        );
        return;
    };
    if region != declared {
        errors.push(format!(
            "the workflow deploys to `{region}`, but the runtime module's `region` default is \
             `{declared}`: the services it deploys to are in the other region"
        ));
    }

    let commands = deploy_commands(lines);
    if commands.is_empty() {
        errors.push("no `gcloud run deploy` command was found in the workflow".to_owned());
    }
    for command in &commands {
        if !command.contains("--region \"${REGION}\"") {
            errors.push(format!(
                "`{command}` does not pass `--region \"${{REGION}}\"`: a literal or missing region \
                 puts the service beside the ones Terraform manages"
            ));
        }
    }

    match firebase_run_region(firebase) {
        None => errors.push("firebase.json's Hosting `run` rewrite names no `region`".to_owned()),
        Some(hosting) => {
            if hosting != region {
                errors.push(format!(
                    "firebase.json's rewrite runs in `{hosting}`, but the workflow deploys to \
                     `{region}`: the rewrite would reach no service"
                ));
            }
        }
    }
}

/// B2 and B3: only a push that can change a deployed artifact starts a run, and
/// a manual run is never filtered.
fn check_triggers(lines: &[&str], errors: &mut Vec<String>) {
    let Some(open) = key_line(lines, 0..lines.len(), 0, "on") else {
        errors.push("the workflow has no top-level `on:` block".to_owned());
        return;
    };
    let on = block_range(lines, open);

    for index in on.clone() {
        if indent(lines[index]) != 2 || is_comment(lines[index]) {
            continue;
        }
        for trigger in ["pull_request", "pull_request_target"] {
            if is_key(lines[index], trigger) {
                errors.push(format!(
                    "`on.{trigger}` is a trigger: a pull request must never reach the WIF token"
                ));
            }
        }
    }

    match key_line(lines, on.clone(), 2, "push") {
        None => errors.push("`on.push` is missing: nothing would start a deploy".to_owned()),
        Some(push) => match key_line(lines, block_range(lines, push), 4, "paths") {
            None => errors.push(
                "`on.push` has no `paths:` filter: a docs-only merge would build and deploy"
                    .to_owned(),
            ),
            Some(paths) => {
                let entries: Vec<String> = block_range(lines, paths)
                    .map(|index| {
                        list_item(lines[index])
                            .trim_matches(|quote| quote == '"' || quote == '\'')
                            .to_owned()
                    })
                    .collect();
                if entries.is_empty() {
                    errors.push(
                        "`on.push.paths` is an empty list: it matches nothing, so a change that \
                         should deploy would never start a run"
                            .to_owned(),
                    );
                }
                for entry in &entries {
                    if entry == "*" || entry == "**" || entry == "**/*" {
                        errors.push(format!(
                            "`on.push.paths` lists `{entry}`, a catch-all: it brings back the \
                             docs-only merge the filter exists to stop"
                        ));
                    }
                    if entry.starts_with("docs/") || entry.starts_with("infra/") {
                        errors.push(format!(
                            "`on.push.paths` lists `{entry}`, which cannot change a deployed \
                             artifact: this workflow deploys with `--image` only and never \
                             applies Terraform"
                        ));
                    }
                }
                for required in [
                    "backend/**",
                    "app/**",
                    "firebase.json",
                    ".firebaserc",
                    ".github/workflows/deploy.yml",
                ] {
                    if !entries.iter().any(|entry| entry == required) {
                        errors.push(format!("`on.push.paths` is missing `{required}`"));
                    }
                }
            }
        },
    }

    match key_line(lines, on, 2, "workflow_dispatch") {
        None => errors.push(
            "`on.workflow_dispatch` is missing: the owner could not deploy by hand".to_owned(),
        ),
        Some(dispatch) => {
            if key_line(lines, block_range(lines, dispatch), 4, "paths").is_some() {
                errors.push(
                    "`on.workflow_dispatch` carries a `paths:` filter: a manual run must never be \
                     filtered"
                        .to_owned(),
                );
            }
        }
    }
}

/// B1, B3, B4 and B5: the preflight decides whether anything runs, and both
/// guards read it.
fn check_jobs(lines: &[&str], errors: &mut Vec<String>) {
    let jobs = jobs(lines);

    match job(&jobs, "configured") {
        None => errors.push(
            "there is no `configured` job: the pipeline cannot tell whether the deploy variables \
             are set"
                .to_owned(),
        ),
        Some(configured) => check_configured(lines, configured, errors),
    }

    match job(&jobs, "build") {
        None => errors.push("there is no `build` job".to_owned()),
        Some(build) => {
            if !needs(lines, build.clone())
                .iter()
                .any(|name| name == "configured")
            {
                errors.push(
                    "`build` does not `needs: configured`: it would build before the variables are \
                     checked"
                        .to_owned(),
                );
            }
            match condition(lines, build) {
                None => errors.push(
                    "`build` has no `if:` condition: it must run only on \
                     `needs.configured.outputs.staging_ready`"
                        .to_owned(),
                ),
                Some(expression) => {
                    if !expression.contains("needs.configured.outputs.staging_ready == 'true'") {
                        errors.push(
                            "`build` does not run only when \
                             `needs.configured.outputs.staging_ready == 'true'`"
                                .to_owned(),
                        );
                    }
                    if !expression.contains("github.repository ==") {
                        errors.push(
                            "`build` dropped the `github.repository` guard: a fork would present \
                             a token"
                                .to_owned(),
                        );
                    }
                }
            }
        }
    }

    match job(&jobs, "deploy-staging") {
        None => errors.push("there is no `deploy-staging` job".to_owned()),
        Some(staging) => {
            if !needs(lines, staging).iter().any(|name| name == "build") {
                errors.push("`deploy-staging` does not `needs: build`".to_owned());
            }
        }
    }

    match job(&jobs, "deploy-prod") {
        None => errors.push("there is no `deploy-prod` job".to_owned()),
        Some(prod) => {
            let prod_needs = needs(lines, prod.clone());
            for required in ["deploy-staging", "configured"] {
                if !prod_needs.iter().any(|name| name == required) {
                    errors.push(format!(
                        "`deploy-prod` does not `needs:` `{required}`: it cannot read the \
                         production variables, and it would ask for an approval while production \
                         is unset"
                    ));
                }
            }
            match condition(lines, prod) {
                None => errors.push(
                    "`deploy-prod` has no `if:` condition: it must run only on \
                     `needs.configured.outputs.prod_ready`"
                        .to_owned(),
                ),
                Some(expression) => {
                    if !expression.contains("needs.configured.outputs.prod_ready == 'true'") {
                        errors.push(
                            "`deploy-prod` does not run only when \
                             `needs.configured.outputs.prod_ready == 'true'`"
                                .to_owned(),
                        );
                    }
                }
            }
        }
    }
}

/// B1: the `configured` job reads repository variables (so it cannot name an
/// environment), exports both answers, and ends the run in one of the three
/// ways B1 describes.
fn check_configured(lines: &[&str], job: Range<usize>, errors: &mut Vec<String>) {
    if job
        .clone()
        .any(|index| indent(lines[index]) == 4 && is_key(lines[index], "environment"))
    {
        errors.push(
            "`configured` names an `environment:`: it would hold a WIF token there, and a job \
             with an environment cannot read a repository variable the README documents"
                .to_owned(),
        );
    }

    for output in ["staging_ready", "prod_ready"] {
        let declared = job
            .clone()
            .any(|index| indent(lines[index]) == 6 && value_of(lines[index], output).is_some());
        if !declared {
            errors.push(format!(
                "`configured` does not export `{output}`: the jobs below cannot gate on it"
            ));
        }
    }

    // S-1: the preflight reports what is missing BY NAME, so every variable is
    // named twice - as an env key the step reads, and in the loop that tests
    // it. A variable left out of either reads as set.
    for (environment, required) in [
        ("staging", STAGING_VARIABLES),
        ("production", PROD_VARIABLES),
    ] {
        for name in required {
            let read = job.clone().any(|index| {
                value_of(lines[index], name)
                    .is_some_and(|value| value.contains(&format!("vars.{name}")))
            });
            if !read {
                errors.push(format!(
                    "`configured` does not read `vars.{name}`: the {environment} deploy would run \
                     against an empty value"
                ));
            }
        }
    }

    // S-6: the switch that turns the quiet skip into a loud failure.
    if !job.clone().any(|index| {
        value_of(lines[index], "DEPLOY_ENABLED")
            .is_some_and(|value| value.contains("vars.DEPLOY_ENABLED"))
    }) {
        errors.push(
            "`configured` does not read `vars.DEPLOY_ENABLED`: the owner could not make a missing \
             staging variable fail loudly"
                .to_owned(),
        );
    }

    let shell = shell_lines(lines, job);

    // S-1, second half: the loops are what actually test the variables.
    let loops = loop_variables(&shell);
    for (environment, required) in [
        ("staging", STAGING_VARIABLES),
        ("production", PROD_VARIABLES),
    ] {
        let covered = loops.iter().any(|(_, values)| {
            required
                .iter()
                .all(|name| values.iter().any(|value| value.as_str() == *name))
        });
        if !covered {
            errors.push(format!(
                "`configured`'s {environment} loop does not name all of {}: a variable the loop \
                 skips is never tested, so an unset one counts as set",
                required.join(", ")
            ));
        }
    }

    // S-8: the assignments the loop makes. Without them the output stays
    // `true`, and a deploy would run against an empty project.
    for output in ["staging_ready", "prod_ready"] {
        let assignment = format!("{output}=false");
        if !shell.iter().any(|line| text(line) == assignment) {
            errors.push(format!(
                "`configured` never assigns `{assignment}`: a missing variable would not be \
                 reported"
            ));
        }
    }

    if !failure_branch(&shell, "DEPLOY_ENABLED") {
        errors.push(
            "`configured` has no failure branch for `DEPLOY_ENABLED=true`: once the owner has \
             deployed staging, a missing variable must fail loudly instead of skipping quietly"
                .to_owned(),
        );
    }
    if !failure_branch(&shell, "workflow_dispatch") {
        errors.push(
            "`configured` has no failure branch for a manual run: `workflow_dispatch` with a \
             staging variable unset must fail"
                .to_owned(),
        );
    }
    if !shell.iter().any(|line| line.contains("::warning::")) {
        errors.push(
            "`configured` prints no `::warning::` annotation when it skips: the run page has to \
             say why"
                .to_owned(),
        );
    }
    if !shell
        .iter()
        .any(|line| line.contains("${GITHUB_STEP_SUMMARY}"))
    {
        errors.push("`configured` writes no summary line when it skips".to_owned());
    }
    if !shell.iter().any(|line| line.contains(SKIP_NOTICE)) {
        errors.push(format!("`configured` never says `{SKIP_NOTICE}`"));
    }
}

/// Actions stay pinned to a 40-character commit sha (repository rule). `source`
/// names the file the reference was read from, because S-7 runs this over every
/// workflow.
fn check_pins(source: &str, lines: &[&str], errors: &mut Vec<String>) {
    for line in lines {
        if is_comment(line) {
            continue;
        }
        let Some(reference) = value_of(line, "uses") else {
            continue;
        };
        let reference = reference.split_whitespace().next().unwrap_or_default();
        let pin = reference
            .rsplit_once('@')
            .map(|(_, pin)| pin)
            .unwrap_or_default();
        let pinned = pin.len() == 40 && pin.chars().all(|c| c.is_ascii_hexdigit());
        if !pinned {
            errors.push(format!(
                "`{source}` uses `{reference}`, which is not pinned to a 40-character commit sha"
            ));
        }
    }
}

/// B5: every service deployed, and the Hosting rewrite's `serviceId`, is one
/// Terraform declares - and S-4: the region is the runtime module's, in the
/// commands and in the rewrite.
fn check_services(
    lines: &[&str],
    cloud_run: &str,
    firebase: &str,
    runtime_variables: &str,
    errors: &mut Vec<String>,
) {
    let declared = declared_services(cloud_run);
    if declared.len() < 3 {
        errors.push(format!(
            "cloud_run.tf declares {} Cloud Run service name(s), found {declared:?}: the check is \
             not reading the module, or a service went missing",
            declared.len()
        ));
    }

    let targets = deploy_targets(lines, &loop_variables(lines));
    if targets.is_empty() {
        errors.push("no `gcloud run deploy` target was found in the workflow".to_owned());
    }
    for target in &targets {
        if !declared.iter().any(|name| name == target) {
            errors.push(format!(
                "`gcloud run deploy {target}` names no service declared in cloud_run.tf \
                 (declared: {})",
                declared.join(", ")
            ));
        }
    }

    match firebase_service_id(firebase) {
        None => errors.push("firebase.json has no Hosting `serviceId`".to_owned()),
        Some(service_id) => {
            if !declared.iter().any(|name| name == service_id) {
                errors.push(format!(
                    "firebase.json rewrites /api/** to `{service_id}`, which cloud_run.tf does not \
                     declare (declared: {})",
                    declared.join(", ")
                ));
            }
        }
    }

    check_regions(lines, runtime_variables, firebase, errors);
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// Run the policy over the real files, then over one edit of the workflow, and
/// require the edit to be rejected. `assert_ne!` guards the test itself: a
/// mutation that no longer matches the workflow would otherwise pass silently.
fn workflow_mutation(from: &str, to: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mutated = WORKFLOW.replacen(from, to, 1);
    assert_ne!(
        mutated, WORKFLOW,
        "the mutation {from:?} matches nothing in deploy.yml; the test is stale"
    );
    let errors = violations(&mutated, CLOUD_RUN, FIREBASE, RUNTIME_VARIABLES, README);
    assert!(
        !errors.is_empty(),
        "the policy accepted a deploy.yml with {from:?} replaced by {to:?}"
    );
    Ok(())
}

/// As `workflow_mutation`, for one edit of `firebase.json`.
fn firebase_mutation(from: &str, to: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mutated = FIREBASE.replacen(from, to, 1);
    assert_ne!(
        mutated, FIREBASE,
        "the mutation {from:?} matches nothing in firebase.json; the test is stale"
    );
    let errors = violations(WORKFLOW, CLOUD_RUN, &mutated, RUNTIME_VARIABLES, README);
    assert!(
        !errors.is_empty(),
        "the policy accepted a firebase.json with {from:?} replaced by {to:?}"
    );
    Ok(())
}

/// The real workflow, module and Hosting config hold the policy.
#[test]
fn deploy_policy_accepts_the_real_files() -> Result<(), Box<dyn std::error::Error>> {
    let errors = violations(WORKFLOW, CLOUD_RUN, FIREBASE, RUNTIME_VARIABLES, README);
    assert!(
        errors.is_empty(),
        "deploy.yml is not the workflow T-1104b describes: {errors:#?}"
    );
    let declared = declared_services(CLOUD_RUN);
    assert!(
        declared.len() >= 3,
        "cloud_run.tf should declare at least the three runtime services, found {declared:?}"
    );
    Ok(())
}

/// B5: `deploy-prod` without `configured` in `needs:` cannot read `prod_ready`.
#[test]
fn deploy_policy_rejects_prod_without_the_configured_needs(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "    needs: [deploy-staging, configured]\n",
        "    needs: deploy-staging\n",
    )
}

/// B5: a `gcloud run deploy` target Terraform does not create.
#[test]
fn deploy_policy_rejects_a_deploy_target_terraform_does_not_declare(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("\"mt-${svc}\"", "\"api\"")
}

/// B5: a Hosting rewrite to a service Terraform does not create.
#[test]
fn deploy_policy_rejects_a_firebase_service_id_terraform_does_not_declare(
) -> Result<(), Box<dyn std::error::Error>> {
    firebase_mutation("\"serviceId\": \"mt-api\"", "\"serviceId\": \"api\"")
}

/// B1: no preflight job at all.
#[test]
fn deploy_policy_rejects_a_workflow_without_the_configured_job(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("  configured:\n", "  preflight:\n")
}

/// B1: `build` that does not wait for the preflight.
#[test]
fn deploy_policy_rejects_a_build_without_the_configured_dependency(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("    needs: configured\n", "")
}

/// B1: `build` that does not gate on `staging_ready`.
#[test]
fn deploy_policy_rejects_a_build_without_the_staging_ready_gate(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("needs.configured.outputs.staging_ready == 'true' && ", "")
}

/// B1: `build` that no longer checks its own repository.
#[test]
fn deploy_policy_rejects_a_build_without_the_repository_guard(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(" && github.repository == 'difficultnerd/MailTinder'", "")
}

/// B1: the `DEPLOY_ENABLED=true` branch, without which a configured repository
/// would go back to failing silently.
#[test]
fn deploy_policy_rejects_a_configured_job_without_the_deploy_enabled_failure(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(DEPLOY_ENABLED_BRANCH, "")
}

/// B1/B3: the branch that fails a manual run.
#[test]
fn deploy_policy_rejects_a_configured_job_without_the_manual_run_failure(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(WORKFLOW_DISPATCH_BRANCH, "")
}

/// B1: a skip with nothing on the run page to explain it.
#[test]
fn deploy_policy_rejects_a_skip_without_the_warning_annotation(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("          echo \"::warning::${notice}\"\n", "")
}

/// B1: a skip without the one summary line.
#[test]
fn deploy_policy_rejects_a_skip_without_the_summary_line() -> Result<(), Box<dyn std::error::Error>>
{
    workflow_mutation(
        "          echo \"${notice}\" >> \"${GITHUB_STEP_SUMMARY}\"\n",
        "",
    )
}

/// B1: a skip that never states the notice the task names, word for word.
#[test]
fn deploy_policy_rejects_a_skip_without_the_notice() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(SKIP_NOTICE, "something else")
}

/// B4: `deploy-prod` that does not gate on `prod_ready`.
#[test]
fn deploy_policy_rejects_a_prod_job_without_the_prod_ready_gate(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "    if: needs.configured.outputs.prod_ready == 'true'\n",
        "",
    )
}

/// B2: a push trigger with no `paths` filter.
#[test]
fn deploy_policy_rejects_a_push_without_a_path_filter() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("    paths:\n", "")
}

/// B2: a docs path in the filter would start a deploy for a docs-only merge.
#[test]
fn deploy_policy_rejects_a_docs_path_in_the_filter() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "      - .firebaserc\n",
        "      - .firebaserc\n      - docs/**\n",
    )
}

/// B2: Terraform paths cannot change what this workflow deploys.
#[test]
fn deploy_policy_rejects_an_infra_path_in_the_filter() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "      - .firebaserc\n",
        "      - .firebaserc\n      - infra/**\n",
    )
}

/// B3: no manual trigger at all.
#[test]
fn deploy_policy_rejects_a_workflow_without_manual_dispatch(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("  workflow_dispatch:\n", "")
}

/// B3: a manual trigger that is filtered.
#[test]
fn deploy_policy_rejects_a_filtered_manual_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "  workflow_dispatch:\n",
        "  workflow_dispatch:\n    paths:\n      - backend/**\n",
    )
}

/// No trigger may bring a fork's pull request into the token exchange.
#[test]
fn deploy_policy_rejects_a_pull_request_trigger() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "  workflow_dispatch:\n",
        "  pull_request:\n  workflow_dispatch:\n",
    )
}

/// Every `uses:` stays pinned to a 40-character commit sha.
#[test]
fn deploy_policy_rejects_an_unpinned_action() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
        "actions/checkout@v7",
    )
}

/// A job with an environment cannot read the repository variables, and would
/// hold the environment's token.
#[test]
fn deploy_policy_rejects_an_environment_on_the_configured_job(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("    outputs:\n", "    environment: staging\n    outputs:\n")
}

// ---------------------------------------------------------------------------
// T-1104d: the PR #89 review findings S-1 to S-4, S-6 and S-7, and the PR #91
// finding S-8. Each check above has the mutation that proves it can fail.
// ---------------------------------------------------------------------------

/// S-1: a staging variable the preflight's loop does not test reads as set.
#[test]
fn deploy_policy_rejects_a_staging_loop_that_skips_a_variable(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "for name in STAGING_PROJECT STAGING_WIF_PROVIDER STAGING_DEPLOYER; do",
        "for name in STAGING_PROJECT STAGING_WIF_PROVIDER; do",
    )
}

/// S-1: the same for the production loop, which decides the approval request.
#[test]
fn deploy_policy_rejects_a_prod_loop_that_skips_a_variable(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "for name in PROD_PROJECT PROD_WIF_PROVIDER PROD_DEPLOYER; do",
        "for name in PROD_PROJECT PROD_WIF_PROVIDER; do",
    )
}

/// S-1: a variable named in the loop but never read by the step, which tests an
/// empty string and reports the variable set.
#[test]
fn deploy_policy_rejects_a_configured_job_that_does_not_read_a_variable(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "          STAGING_DEPLOYER: ${{ vars.STAGING_DEPLOYER }}\n",
        "",
    )
}

/// S-2: a wider `contents:` scope than the checkout needs.
#[test]
fn deploy_policy_rejects_a_write_contents_permission() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "permissions:\n  contents: read\n  id-token: write\n",
        "permissions:\n  contents: write\n  id-token: write\n",
    )
}

/// S-2: the preflight's own `permissions:` block, dropped.
#[test]
fn deploy_policy_rejects_a_configured_job_without_the_contents_read_permission(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("    permissions:\n      contents: read\n", "")
}

/// S-3: a catch-all filter starts a deploy for a docs-only merge again.
#[test]
fn deploy_policy_rejects_a_catch_all_path_filter() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation(
        "      - .firebaserc\n",
        "      - .firebaserc\n      - '**'\n",
    )
}

/// S-3: an empty `paths:` list matches nothing, so a deployable change would
/// never start a run.
#[test]
fn deploy_policy_rejects_an_empty_path_filter() -> Result<(), Box<dyn std::error::Error>> {
    let lines = yaml_lines("on:\n  push:\n    paths:\n  workflow_dispatch:\n");
    let mut errors = Vec::new();
    check_triggers(&lines, &mut errors);
    assert!(
        errors.iter().any(|error| error.contains("empty list")),
        "the triggers check accepted an empty `on.push.paths`: {errors:#?}"
    );
    Ok(())
}

/// S-4 (a): a workflow region the runtime module does not declare.
#[test]
fn deploy_policy_rejects_a_region_the_runtime_module_does_not_declare(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("  REGION: us-central1\n", "  REGION: europe-west1\n")
}

/// S-4 (b): a literal region in a `gcloud run deploy` command, which would
/// deploy beside the services Terraform manages.
#[test]
fn deploy_policy_rejects_a_literal_deploy_region() -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("--region \"${REGION}\"", "--region \"us-central1\"")
}

/// S-4 (c): a Hosting rewrite in another region than the one deployed to.
#[test]
fn deploy_policy_rejects_a_firebase_region_the_workflow_does_not_deploy_to(
) -> Result<(), Box<dyn std::error::Error>> {
    firebase_mutation(
        "\"region\": \"us-central1\"",
        "\"region\": \"europe-west1\"",
    )
}

/// S-6: the switch that turns the quiet skip into a loud failure, unread.
#[test]
fn deploy_policy_rejects_a_configured_job_without_the_deploy_enabled_variable(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("          DEPLOY_ENABLED: ${{ vars.DEPLOY_ENABLED }}\n", "")
}

/// S-7: the pin rule covers every workflow, not only the deploy one.
#[test]
fn deploy_policy_rejects_an_unpinned_action_in_another_workflow(
) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.github/workflows/ci.yml");
    let source = std::fs::read_to_string(path)?;
    let mutated = source.replacen(
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
        "actions/checkout@v7",
        1,
    );
    assert_ne!(
        mutated, source,
        "ci.yml no longer pins actions/checkout; the test is stale"
    );
    let mut sources = workflow_sources()?;
    let ci = sources
        .iter_mut()
        .find(|(name, _)| name == "ci.yml")
        .ok_or("the workflow directory holds no ci.yml")?;
    ci.1 = mutated;
    let mut errors = Vec::new();
    check_other_workflows(&sources, &mut errors);
    assert!(
        !errors.is_empty(),
        "the pin check accepted an unpinned `uses:` in ci.yml"
    );
    Ok(())
}

/// S-7: every file under `.github/workflows/` is read by the scan, and every one
/// of them is pinned. `deploy.yml` is checked through the text under test, so
/// the scan skips it.
#[test]
fn deploy_policy_pins_every_workflow_file() -> Result<(), Box<dyn std::error::Error>> {
    let sources = workflow_sources()?;
    let names: Vec<&str> = sources.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        names.contains(&"deploy.yml") && names.len() > 1,
        "the pin scan read {names:?}, which is not the workflow directory"
    );
    let mut errors = Vec::new();
    check_other_workflows(&sources, &mut errors);
    assert!(errors.is_empty(), "{errors:#?}");
    Ok(())
}

/// S-8: the `staging_ready=false` assignment the loop makes.
#[test]
fn deploy_policy_rejects_a_configured_job_without_the_staging_false_assignment(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("              staging_ready=false\n", "")
}

/// S-8: the `prod_ready=false` assignment.
#[test]
fn deploy_policy_rejects_a_configured_job_without_the_prod_false_assignment(
) -> Result<(), Box<dyn std::error::Error>> {
    workflow_mutation("              prod_ready=false\n", "")
}

/// S-8: the extracted script, run with every variable unset, has to answer
/// `false` for both environments - reading the text alone cannot see a script
/// that reports nothing at all.
#[test]
fn deploy_policy_runs_the_preflight_with_every_variable_unset(
) -> Result<(), Box<dyn std::error::Error>> {
    let lines = yaml_lines(WORKFLOW);
    let configured =
        job(&jobs(&lines), "configured").ok_or("deploy.yml has no `configured` job to run")?;
    let script = run_script(&lines, configured).ok_or("`configured` has no `run:` block to run")?;

    // Cargo gives integration tests a scratch directory inside the target tree.
    // `std::env::temp_dir` is banned (rust.lang.security.temp-dir): a fixed name
    // in a world-writable directory invites a symlink attack, and CI runs
    // semgrep with `p/rust`, where the rule is an error.
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("deploy-policy-preflight");
    std::fs::create_dir_all(&directory)?;
    let output = directory.join(format!("deploy-policy-{}.out", std::process::id()));
    let summary = directory.join(format!("deploy-policy-{}.summary", std::process::id()));
    let mut command = std::process::Command::new("bash");
    command
        .arg("-c")
        .arg(&script)
        .env("GITHUB_OUTPUT", &output)
        .env("GITHUB_STEP_SUMMARY", &summary);
    for name in STAGING_VARIABLES
        .iter()
        .copied()
        .chain(PROD_VARIABLES.iter().copied())
        .chain(["DEPLOY_ENABLED", "GITHUB_EVENT_NAME"])
    {
        command.env_remove(name);
    }
    let status = command.status()?;
    assert!(
        status.success(),
        "the preflight exited with {status} with every variable unset"
    );

    let written = std::fs::read_to_string(&output).unwrap_or_default();
    for expected in ["staging_ready=false", "prod_ready=false"] {
        assert!(
            written.contains(expected),
            "the preflight wrote {written:?} with every variable unset"
        );
    }
    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_file(&summary);
    Ok(())
}
