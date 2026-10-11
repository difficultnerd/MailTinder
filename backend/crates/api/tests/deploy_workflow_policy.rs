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

use std::ops::Range;

const WORKFLOW: &str = include_str!("../../../../.github/workflows/deploy.yml");
const CLOUD_RUN: &str = include_str!("../../../../infra/terraform/modules/runtime/cloud_run.tf");
const FIREBASE: &str = include_str!("../../../../firebase.json");

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

// ---------------------------------------------------------------------------
// The policy
// ---------------------------------------------------------------------------

/// Every way these texts break the T-1104b policy; empty means the workflow is
/// the one the task describes. The tests below mutate one thing at a time.
fn violations(workflow: &str, cloud_run: &str, firebase: &str) -> Vec<String> {
    let lines = yaml_lines(workflow);
    let mut errors = Vec::new();
    check_triggers(&lines, &mut errors);
    check_jobs(&lines, &mut errors);
    check_pins(&lines, &mut errors);
    check_services(&lines, cloud_run, firebase, &mut errors);
    errors
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
                    .map(|index| list_item(lines[index]).trim_matches('"').to_owned())
                    .collect();
                for entry in &entries {
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

    let shell = shell_lines(lines, job);

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

/// Actions stay pinned to a 40-character commit sha (repository rule).
fn check_pins(lines: &[&str], errors: &mut Vec<String>) {
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
                "`uses: {reference}` is not pinned to a 40-character commit sha"
            ));
        }
    }
}

/// B5: every service deployed, and the Hosting rewrite's `serviceId`, is one
/// Terraform declares.
fn check_services(lines: &[&str], cloud_run: &str, firebase: &str, errors: &mut Vec<String>) {
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
    let errors = violations(&mutated, CLOUD_RUN, FIREBASE);
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
    let errors = violations(WORKFLOW, CLOUD_RUN, &mutated);
    assert!(
        !errors.is_empty(),
        "the policy accepted a firebase.json with {from:?} replaced by {to:?}"
    );
    Ok(())
}

/// The real workflow, module and Hosting config hold the policy.
#[test]
fn deploy_policy_accepts_the_real_files() -> Result<(), Box<dyn std::error::Error>> {
    let errors = violations(WORKFLOW, CLOUD_RUN, FIREBASE);
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
