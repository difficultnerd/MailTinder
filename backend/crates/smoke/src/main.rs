//! `smoke`: the post-deploy staging smoke binary (T-1105, S10 3.1).
//!
//! Not a service: nothing deploys it, nothing depends on it, and it has no
//! listener. It runs once, from `.github/workflows/smoke.yml`, against the
//! staging deployment that just went out, and it is the gate between a staging
//! deploy and the production promotion (S11 2.1).
//!
//! Every check runs even when an earlier one failed; the process exits 1 if any
//! of them did. Output is one line per check holding the check's name and
//! `pass` or `fail` - never a document, a token, a URL or an error detail, so a
//! smoke log can be read in the open (S10 3.1, S5).
//!
//! The conventions this binary keeps, as the services do: the clock and the RNG
//! come only from the adapters, and nothing here can reach production (its
//! identity exists only in the staging root, `infra/terraform/envs/staging/
//! smoke.tf`).
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::single_match_else,
    clippy::redundant_closure,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::uninlined_format_args,
    clippy::similar_names,
    clippy::needless_pass_by_value,
    clippy::items_after_statements,
    clippy::unused_async,
    clippy::ignored_unit_patterns
)]

mod checks;
mod unused;

use std::io::Write;
use std::process::ExitCode;

use checks::{build_ports, SmokeConfig, SmokeError};

/// Run every smoke check, then exit 1 if any failed.
#[tokio::main]
async fn main() -> ExitCode {
    let cfg = match SmokeConfig::from_env() {
        Ok(cfg) => cfg,
        Err(error) => {
            // The variable's NAME only (SmokeError's Display never carries a
            // value).
            line("config", Err(error));
            return ExitCode::FAILURE;
        }
    };
    let ports = match build_ports(&cfg) {
        Ok(ports) => ports,
        Err(error) => {
            line("config", Err(error));
            return ExitCode::FAILURE;
        }
    };

    let mut failed = false;
    failed |= line("kms_round_trip", checks::kms_round_trip(&ports).await);
    failed |= line(
        "firestore_round_trip",
        checks::firestore_round_trip(&ports).await,
    );
    failed |= line(
        "tasks_schedule_and_cancel",
        checks::tasks_schedule_and_cancel(&ports, &cfg).await,
    );
    failed |= line(
        checks::AC4_CHECK,
        checks::un_02_ac4_staging_unsub_refuses_metadata_target(&ports, &cfg).await,
    );
    failed |= line(
        "internal_services_not_public",
        checks::internal_services_not_public(&cfg).await,
    );
    failed |= line(
        "hosting_and_api_headers",
        checks::hosting_and_api_headers(&cfg).await,
    );

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Write one line for a check: its name and `pass` or `fail`. Returns true when
/// the check failed, so the caller can accumulate.
///
/// `println!` is denied by Clippy; the line goes to stdout through `writeln!`,
/// as in the other binaries. Nothing but the name and the outcome is written,
/// so a failure's detail never reaches the log.
fn line(name: &str, result: Result<(), SmokeError>) -> bool {
    let outcome = if result.is_ok() { "pass" } else { "fail" };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{name}: {outcome}");
    let _ = out.flush();
    result.is_err()
}
