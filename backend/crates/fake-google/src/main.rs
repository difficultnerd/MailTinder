//! `fake-google` binary: binds `FAKE_GOOGLE_ADDR` (default `127.0.0.1:0`),
//! prints the bound address as one line, and serves until killed.

use std::io::Write;

use fake_google::FakeGoogle;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("FAKE_GOOGLE_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_owned());
    // The standalone HTTP fake validates Gmail/Drive expiry using real time.
    // OAuth must issue tokens on that same clock, not the unit-test epoch.
    let clock = adapters_gcp::production_clock();
    let handle = FakeGoogle::start_on(&addr, clock).await?;
    // Print the bound address as one line (println! is banned by Clippy).
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", handle.addr)?;
    out.flush()?;
    // scripts/e2e.sh may ask for the port in a file instead of parsing stdout.
    if let Some(path) = std::env::var_os("FAKE_GOOGLE_PORT_FILE") {
        std::fs::write(path, format!("{}\n", handle.addr.port()))?;
    }
    // Keep serving until the process is killed.
    let _ = addr;
    std::future::pending::<()>().await;
    Ok(())
}
