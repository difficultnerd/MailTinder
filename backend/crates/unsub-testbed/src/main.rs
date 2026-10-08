//! `unsub-testbed` binary: binds `TESTBED_ADDR` (default `127.0.0.1:0`), prints
//! `TESTBED_ADDR=<addr> TESTBED_HTTPS_ADDR=<addr>` on one line so
//! `scripts/e2e.sh` can read it, and serves until killed.

use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("TESTBED_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_owned());
    let address: std::net::SocketAddr = address.parse()?;
    let testbed = unsub_testbed::start_at(address, true).await?;
    // println! is banned by Clippy; write one line to stdout instead.
    let (http, https) = (testbed.addr, testbed.https_addr);
    let mut out = std::io::stdout().lock();
    writeln!(out, "TESTBED_ADDR={http} TESTBED_HTTPS_ADDR={https}")?;
    out.flush()?;
    // scripts/e2e.sh may ask for the port in a file instead of parsing stdout.
    if let Some(path) = port_file_arg() {
        std::fs::write(path, format!("{}\n", http.port()))?;
    }
    // Keep serving until the process is killed.
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
