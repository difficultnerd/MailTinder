//! `cargo run -p testkit --example synth_mailbox -- --seed 1 --persona freelancer --messages 5000 --out DIR`
//!
//! Writes a deterministic synthetic mailbox: `manifest.jsonl` (one JSON object
//! per message, ground truth included), `eml/<id>.eml`, `SHA256SUMS` and, with
//! `--mbox`, `mailbox.mbox` (T-1120a).
//!
//! Same arguments, same bytes, on every run and machine. It sends nothing and
//! touches no network.

use std::io::Write as _;

use testkit::synth;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = synth::parse_args(args)?;
    let generated = synth::generate(&args.config)?;
    synth::write_out(&generated, &args.out, args.force)?;

    let mut out = std::io::stdout();
    writeln!(
        out,
        "wrote {} messages to {}",
        generated.messages.len(),
        args.out.display()
    )
    .map_err(|e| format!("stdout: {e}"))?;
    writeln!(out, "corpus_hash {}", generated.header.corpus_hash)
        .map_err(|e| format!("stdout: {e}"))?;
    Ok(())
}
