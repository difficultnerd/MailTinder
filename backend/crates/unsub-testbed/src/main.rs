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
    // scripts/e2e.sh may ask for the port in a file instead of parsing stdout:
    // line 1 is the plain-HTTP port (what `wait_port_file` reads), line 2 is
    // the HTTPS listener address, which the e2e harness needs as the one-click
    // target (T-1101g).
    if let Some(path) = std::env::var_os("TESTBED_PORT_FILE") {
        std::fs::write(path, format!("{}\n{}\n", http.port(), https))?;
    }
    // Keep serving until the process is killed.
    std::future::pending::<()>().await;
    Ok(())
}
