//! The `api` binary (T-500b). A thin shell: it calls `startup::run`, logs one
//! line when that fails and returns the matching [`ExitCode`].

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match api::startup::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(
                event = "op",
                route = "api.startup",
                outcome = "failure",
                error = error.name()
            );
            ExitCode::FAILURE
        }
    }
}
