//! `fake-google` binary: binds `FAKE_GOOGLE_ADDR` (default `127.0.0.1:0`),
//! prints the bound address as one line, and serves until killed.

use std::io::Write;
use std::sync::Arc;

use fake_google::FakeGoogle;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("FAKE_GOOGLE_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_owned());
    let clock = Arc::new(testkit::clock::VirtualClock::new(testkit::T0));
    let handle = FakeGoogle::start_on(&addr, clock).await?;
    // Print the bound address as one line (println! is banned by Clippy).
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", handle.addr)?;
    out.flush()?;
    // scripts/e2e.sh may ask for the port in a file instead of parsing stdout.
    if let Some(path) = port_file_arg() {
        std::fs::write(path, format!("{}\n", handle.addr.port()))?;
    }
    // Keep serving until the process is killed.
    let _ = addr;
    std::future::pending::<()>().await;
    Ok(())
}

/// The value of an optional `--port-file <path>` argument.
fn port_file_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|arg| arg == "--port-file")
        .and_then(|i| args.get(i + 1).cloned())
}
