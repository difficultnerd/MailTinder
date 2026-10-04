//! `write-corpus <dir>` writes every corpus case as `<id>.eml` into `<dir>`.
//!
//! Used by the e2e harness (T-1101) and fake-google seeding (T-205a). Output
//! goes to a temp or `target/` folder; nothing it writes is committed.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let mut args = std::env::args_os();
    let _prog = args.next();
    let dir = args
        .next()
        .ok_or_else(|| "usage: write-corpus <dir>".to_owned())
        .map(PathBuf::from)?;
    let corpus = testkit::corpus::load()?;
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
    let mut out = std::io::stdout();
    for c in &corpus.cases {
        let path = dir.join(format!("{}.eml", c.spec.id));
        fs::write(&path, &c.eml).map_err(|e| format!("write {}: {e}", path.display()))?;
        writeln!(out, "{}", path.display()).map_err(|e| format!("stdout: {e}"))?;
    }
    Ok(())
}
