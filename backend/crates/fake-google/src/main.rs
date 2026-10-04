//! `fake-google` binary: binds `FAKE_GOOGLE_ADDR` (default `127.0.0.1:0`),
//! prints the bound address as one line, and serves until killed.

use std::io::Write;
use std::sync::Arc;

use fake_google::FakeGoogle;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("FAKE_GOOGLE_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_owned());
    let clock = Arc::new(testkit::clock::VirtualClock::new(testkit::T0));
    let handle = FakeGoogle::start(clock).await?;
    // Print the bound address as one line (println! is banned by Clippy).
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", handle.addr)?;
    out.flush()?;
    // Keep serving until the process is killed.
    let _ = addr;
    std::future::pending::<()>().await;
    Ok(())
}
