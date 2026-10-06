//! `mt-admin`: the MailTinder bootstrap tool (T-507).
//!
//! Run by James on his own machine, with his own Google credentials, to create
//! the first invite and (after signing in) mark his user as admin. The API never
//! grants the admin flag (S7 3.7, ASVS V6.3.2). The raw invite token goes to
//! stdout only, inside the link, and is never logged.
//!
//! Production adapters use Application Default Credentials
//! (`gcloud auth application-default login`).
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use std::io::{self, Write as _};
use std::process::{Command as ProcessCommand, ExitCode};
use std::sync::Arc;

use adapters_gcp::{
    production_clock, production_rng, CloudKms, FirestoreConfig, FirestoreStore, GcpHttp,
    KmsSystemKeyService, SecretManagerSecrets, SecretsConfig, StaticTokenSource, TokenSource,
};
use admin_cli::commands::{invite, list_users, make_admin, CliError, Deps, MakeAdminResult};
use clap::{Parser, Subcommand};
use domain::{EmailAddress, UserId};
use obs::Sensitive;
use ports::{SecretName, Secrets};
use url::Url;
use uuid::Uuid;

/// The fixed GCP region (T-1102a Terraform `var.region`).
const LOCATION: &str = "us-central1";
/// The fixed KMS key ring (T-1102a Terraform).
const KEY_RING: &str = "mailtinder";
/// The system-fields KMS key that seals pre-user data (S6 5).
const SYSTEM_KEY: &str = "system-fields";
/// The default app origin used to build the invite link.
const DEFAULT_ORIGIN: &str = "https://mailtinder.app";

#[derive(Parser)]
#[command(
    name = "mt-admin",
    version,
    about = "MailTinder bootstrap: create the first invite, set the first admin"
)]
struct Cli {
    /// GCP project that holds Firestore, KMS and Secret Manager.
    #[arg(long)]
    project: String,
    /// App origin used to build the invite link.
    #[arg(long, default_value = DEFAULT_ORIGIN)]
    origin: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create or refresh the pending invite for an address and print the link.
    Invite {
        /// The address to invite.
        email: String,
    },
    /// List users (id, created, admin, mailbox count). No addresses.
    ListUsers,
    /// Mark an existing user as admin.
    MakeAdmin {
        /// The user ID to promote.
        user_id: String,
    },
}

/// A failure before or during a command; `code` is the process exit code.
enum RunError {
    /// A failure building the adapters (credentials, project, network).
    Setup,
    /// A command failure.
    Cli(CliError),
}

impl RunError {
    fn code(&self) -> u8 {
        match self {
            RunError::Setup => 1,
            RunError::Cli(e) => e.exit_code(),
        }
    }

    fn message(&self) -> String {
        match self {
            RunError::Setup => "setup failed; check gcloud ADC and the project".to_owned(),
            RunError::Cli(e) => e.to_string(),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let _ = writeln!(io::stderr(), "mt-admin: {}", err.message());
            ExitCode::from(err.code())
        }
    }
}

async fn run(cli: Cli) -> Result<(), RunError> {
    let project = cli.project.clone();
    let origin = Url::parse(&cli.origin).map_err(|_| RunError::Setup)?;

    let clock = production_clock();
    let rng = production_rng();
    let token = adc_token().map_err(|()| RunError::Setup)?;
    let tokens: Arc<dyn TokenSource> = Arc::new(StaticTokenSource(token));
    let http = Arc::new(GcpHttp::new(tokens).map_err(|_| RunError::Setup)?);

    let store = FirestoreStore::new(Arc::clone(&http), FirestoreConfig::new(project.clone()));
    let key_name = format!(
        "projects/{project}/locations/{LOCATION}/keyRings/{KEY_RING}/cryptoKeys/{SYSTEM_KEY}"
    );
    let system_keys =
        KmsSystemKeyService::new(Arc::new(CloudKms::new(Arc::clone(&http), key_name)));
    let secrets = SecretManagerSecrets::new(
        Arc::clone(&http),
        SecretsConfig {
            project_id: project,
        },
    );
    let email_lookup_key = secrets
        .get(SecretName::EmailLookupHmacKey)
        .await
        .map_err(|_| RunError::Setup)?;
    let pseudo_key = secrets
        .get(SecretName::LogPseudonymHmacKey)
        .await
        .map_err(|_| RunError::Setup)?;

    let deps = Deps {
        store: &store,
        system_keys: &system_keys,
        email_lookup_key: &email_lookup_key,
        clock: clock.as_ref(),
        rng: rng.as_ref(),
        app_origin: &origin,
        pseudo_key: &pseudo_key,
    };

    match cli.command {
        Command::Invite { email } => {
            let email =
                EmailAddress::parse(&email).map_err(|_| RunError::Cli(CliError::BadInput))?;
            let link = invite(&deps, &email).await.map_err(RunError::Cli)?;
            write_line(link.as_str()).map_err(|_| RunError::Setup)?;
            Ok(())
        }
        Command::ListUsers => {
            let lines = list_users(&deps).await.map_err(RunError::Cli)?;
            let mut out = String::new();
            for line in &lines {
                out.push_str(&line.line());
                out.push('\n');
            }
            write_stdout(&out).map_err(|_| RunError::Setup)?;
            Ok(())
        }
        Command::MakeAdmin { user_id } => {
            let uuid = Uuid::parse_str(&user_id).map_err(|_| RunError::Cli(CliError::BadInput))?;
            let result = make_admin(&deps, &UserId::new(uuid))
                .await
                .map_err(RunError::Cli)?;
            let text = match result {
                MakeAdminResult::Granted => "admin granted",
                MakeAdminResult::AlreadyAdmin => "already admin",
            };
            write_line(text).map_err(|_| RunError::Setup)?;
            Ok(())
        }
    }
}

/// The Application Default Credentials access token, read once through the
/// `gcloud` CLI (`gcloud auth application-default login`). The tool is
/// short-lived, so one token is enough.
fn adc_token() -> Result<Sensitive<String>, ()> {
    let output = ProcessCommand::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    let raw = String::from_utf8(output.stdout).map_err(|_| ())?;
    let token = raw.trim();
    if token.is_empty() {
        return Err(());
    }
    Ok(Sensitive::new(token.to_owned()))
}

/// Write one line to stdout; the CLI's only output channel (T-307 forbids the
/// print macros so nothing reaches a log unredacted).
fn write_line(text: &str) -> io::Result<()> {
    write_stdout(text)?;
    write_stdout("\n")
}

fn write_stdout(text: &str) -> io::Result<()> {
    io::stdout().write_all(text.as_bytes())
}
