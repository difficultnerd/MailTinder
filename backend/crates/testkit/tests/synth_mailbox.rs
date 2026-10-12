//! T-1120a: behaviours B1 to B8 of the synthetic mailbox generator.
//!
//! Every check reads the emitted files, not the generator's internals: the
//! manifest, the `.eml` bytes and `SHA256SUMS` are the deliverable.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use fake_google::seeder::FakeGoogleSeeder;
use fake_google::state::{FakeMailboxKey, FakeState};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use testkit::contract::mail_provider::MailSeeder;
use testkit::synth::{self, Config, Generated};

const PROVENANCE: &str = "X-MT-Synthetic-Corpus:";
/// The mailbox the B7 seeder writes into (reserved domain).
const EMAIL: &str = "owner@example.com";
const ONE_CLICK_HEADER: &str = "X-MT-Synthetic-Corpus";

/// A manifest line, as emitted.
#[derive(Debug, Deserialize)]
struct Line {
    id: String,
    path: String,
    message_id: String,
    thread_pos: usize,
    labels: Vec<String>,
    internal_date: String,
    expected: Expect,
}

#[derive(Debug, Deserialize)]
struct Expect {
    class: String,
    bulk_score_min: u8,
    bulk_score_max: u8,
    method: String,
    reason: String,
    canary: String,
    hostile: bool,
}

fn config(seed: u64, persona: &str, messages: usize) -> Config {
    Config {
        seed,
        persona: persona.to_owned(),
        messages,
        ..Config::default()
    }
}

fn manifest_lines(generated: &Generated) -> Result<Vec<Line>, String> {
    let body = generated.manifest_body();
    let text = String::from_utf8_lossy(&body);
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str::<Line>(line).map_err(|e| format!("manifest: {e}"))?);
    }
    Ok(out)
}

fn first_line(generated: &Generated) -> Result<serde_json::Value, String> {
    let bytes = generated
        .manifest()
        .ok_or_else(|| "no manifest".to_owned())?;
    let text = String::from_utf8_lossy(bytes);
    let head = text
        .lines()
        .next()
        .ok_or_else(|| "empty manifest".to_owned())?;
    serde_json::from_str(head).map_err(|e| format!("manifest header: {e}"))
}

// --- byte helpers (the test's own, independent of the generator) ------------

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// The `.eml` bytes with the `X-MT-Synthetic-Corpus` line removed.
fn strip_provenance(bytes: &[u8]) -> Vec<u8> {
    let Some(sep) = find(bytes, b"\r\n\r\n") else {
        return bytes.to_vec();
    };
    let head = &bytes[..sep];
    let Some(last) = rfind(head, b"\r\n") else {
        return bytes.to_vec();
    };
    if !head[last + 2..].starts_with(PROVENANCE.as_bytes()) {
        return bytes.to_vec();
    }
    let mut out = head[..last].to_vec();
    out.extend_from_slice(&bytes[sep..]);
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Recompute `corpus_hash` from the emitted files, exactly as rule 3 defines
/// it: strip the provenance line from every `.eml`, drop the manifest's first
/// line, hash each file, sort by path, hash the joined lines.
fn recompute(generated: &Generated, tamper_eml: Option<&str>, tamper_manifest: bool) -> String {
    let mut entries: Vec<(String, String)> = Vec::new();
    for (path, bytes) in &generated.files {
        if !path.starts_with("eml/") {
            continue;
        }
        let mut base = strip_provenance(bytes);
        if tamper_eml == Some(path.as_str()) {
            let last = base.len() - 1;
            base[last] ^= 0x01;
        }
        entries.push((path.clone(), sha256_hex(&base)));
    }
    let mut body = generated.manifest_body();
    if tamper_manifest {
        if let Some(byte) = body.iter_mut().find(|b| **b == b'0') {
            *byte = b'1';
        }
    }
    entries.push(("manifest.jsonl".to_owned(), sha256_hex(&body)));
    entries.sort();
    let joined = entries
        .iter()
        .map(|(path, hash)| format!("{path} {hash}"))
        .collect::<Vec<String>>()
        .join("\n");
    sha256_hex(joined.as_bytes())
}

// --- host extraction (B3) --------------------------------------------------

fn is_local_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_host_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')
}

/// Every address host and URL host in a file: headers, text part and HTML part.
fn hosts_in(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte != b'@' || i == 0 || !is_local_char(bytes[i - 1]) {
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && is_host_char(bytes[j]) {
            j += 1;
        }
        let host = &bytes[i + 1..j];
        if host.contains(&b'.') {
            out.push(String::from_utf8_lossy(host).to_lowercase());
        }
    }
    let text = String::from_utf8_lossy(bytes).to_lowercase();
    for scheme in ["http://", "https://"] {
        for (i, _) in text.match_indices(scheme) {
            let rest = &text[i + scheme.len()..];
            let end = rest
                .find(|c: char| {
                    !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':' || c == '[')
                })
                .unwrap_or(rest.len());
            let host = rest[..end].split(':').next().unwrap_or_default();
            if !host.is_empty() {
                out.push(host.to_owned());
            }
        }
    }
    out
}

/// The reserved-domain rule (rule 2), dot-boundary exact.
fn reserved_host(host: &str) -> bool {
    let h = host
        .trim_matches(|c: char| c == '.' || c == '-')
        .to_lowercase();
    if h.is_empty() {
        return false;
    }
    for suffix in testkit::corpus::RESERVED_DOMAIN_SUFFIXES {
        if let Some(s) = suffix.strip_prefix('.') {
            if h.ends_with(&format!(".{s}")) || h == s {
                return true;
            }
        } else if h == suffix || h.ends_with(&format!(".{suffix}")) {
            return true;
        }
    }
    false
}

/// Header lines (name, value) of an emitted `.eml`, unfolded enough for ids.
fn headers_in(bytes: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for line in text.split("\r\n") {
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            // A folded continuation of the previous header.
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        out.push((name.to_owned(), value.trim().to_owned()));
    }
    out
}

fn header_value(bytes: &[u8], name: &str) -> Option<String> {
    headers_in(bytes)
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

// --- B1 --------------------------------------------------------------------

#[test]
fn b1_same_arguments_give_byte_identical_output() -> Result<(), String> {
    let cfg = config(7, "freelancer", 240);
    let a = synth::generate(&cfg)?;
    let b = synth::generate(&cfg)?;
    if a.files != b.files {
        return Err("two runs with the same arguments differ".to_owned());
    }
    if a.sums != b.sums {
        return Err("SHA256SUMS differs between two identical runs".to_owned());
    }
    if a.header.corpus_hash != b.header.corpus_hash {
        return Err("corpus_hash differs between two identical runs".to_owned());
    }

    // A different seed must give different bytes.
    let other = synth::generate(&config(8, "freelancer", 240))?;
    if other.sums == a.sums {
        return Err("a different seed produced identical output".to_owned());
    }

    // SHA256SUMS covers every emitted file and excludes itself.
    let sums = String::from_utf8_lossy(&a.sums).to_string();
    let mut listed: BTreeMap<String, String> = BTreeMap::new();
    for line in sums.lines() {
        let (hash, path) = line
            .split_once("  ")
            .ok_or_else(|| format!("bad SHA256SUMS line: {line}"))?;
        if listed.insert(path.to_owned(), hash.to_owned()).is_some() {
            return Err(format!("duplicate SHA256SUMS entry: {path}"));
        }
    }
    if listed.contains_key("SHA256SUMS") {
        return Err("SHA256SUMS lists itself".to_owned());
    }
    if listed.len() != a.files.len() {
        return Err(format!(
            "SHA256SUMS covers {} of {} files",
            listed.len(),
            a.files.len()
        ));
    }
    for (path, bytes) in &a.files {
        match listed.get(path) {
            None => return Err(format!("{path} is not in SHA256SUMS")),
            Some(hash) if *hash != sha256_hex(bytes) => {
                return Err(format!("{path}: SHA256SUMS is wrong"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

// --- B2 --------------------------------------------------------------------

#[test]
fn b2_five_thousand_messages_cover_every_class_and_band() -> Result<(), String> {
    let generated = synth::generate(&config(1, "freelancer", 5000))?;
    let lines = manifest_lines(&generated)?;
    if lines.len() != 5000 {
        return Err(format!("expected 5000 messages, got {}", lines.len()));
    }
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut bands = [0usize; 10];
    for line in &lines {
        let expect = &line.expected;
        *classes.entry(expect.class.clone()).or_default() += 1;
        if expect.bulk_score_min % 10 != 0 || expect.bulk_score_max > 100 {
            return Err(format!("{}: bad bulk window", line.id));
        }
        if expect.bulk_score_max < expect.bulk_score_min {
            return Err(format!("{}: inverted bulk window", line.id));
        }
        if expect.reason.is_empty() {
            return Err(format!("{}: no reason", line.id));
        }
        if ![
            "one_click",
            "mailto",
            "needs_attention_https",
            "needs_attention_http",
            "none",
        ]
        .contains(&expect.method.as_str())
        {
            return Err(format!("{}: unknown method {}", line.id, expect.method));
        }
        if line.path != format!("eml/{}.eml", line.id) {
            return Err(format!(
                "{}: path {} is not derived from the id",
                line.id, line.path
            ));
        }
        bands[usize::from(expect.bulk_score_min / 10)] += 1;
    }
    for class in ["list", "bulk_no_header", "notice", "personal", "suspect"] {
        let n = classes.get(class).copied().unwrap_or(0);
        if n < 100 {
            return Err(format!("class {class} has {n} messages, need 100"));
        }
    }
    for (decile, n) in bands.iter().enumerate() {
        if *n < 50 {
            return Err(format!(
                "bulk band decile {decile} has {n} messages, need 50"
            ));
        }
    }
    // The manifest is sorted by internal_date (RFC 3339 sorts chronologically).
    for pair in lines.windows(2) {
        if pair[0].internal_date > pair[1].internal_date {
            return Err(format!(
                "manifest is not sorted by internal_date: {} before {}",
                pair[0].id, pair[1].id
            ));
        }
    }
    Ok(())
}

// --- B3 --------------------------------------------------------------------

#[test]
fn b3_every_address_and_url_host_is_reserved() -> Result<(), String> {
    let generated = synth::generate(&config(3, "parent", 300))?;
    let mut checked = 0usize;
    for (path, bytes) in &generated.files {
        if !path.starts_with("eml/") {
            continue;
        }
        checked += 1;
        let hosts = hosts_in(bytes);
        if hosts.is_empty() {
            return Err(format!("{path}: no address or URL found"));
        }
        for host in hosts {
            if !reserved_host(&host) {
                return Err(format!("{path}: non-reserved host `{host}`"));
            }
        }
    }
    if checked == 0 {
        return Err("no .eml files were emitted".to_owned());
    }
    Ok(())
}

#[test]
fn b3_there_is_no_domain_option() -> Result<(), String> {
    let args = ["--seed", "1", "--out", "out", "--domain", "real-brand.com"];
    if synth::parse_args(args.map(str::to_owned)).is_ok() {
        return Err("--domain was accepted; v1 has no domain option".to_owned());
    }
    let source = include_str!("../examples/synth_mailbox.rs");
    if source.contains("--domain") {
        return Err("the example mentions --domain".to_owned());
    }
    Ok(())
}

#[test]
fn b3_no_hostile_message_survives_without_hostile_flag() -> Result<(), String> {
    let cfg = config(5, "student", 300);
    let with = synth::generate(&cfg)?;
    let hostile = manifest_lines(&with)?
        .iter()
        .filter(|l| l.expected.hostile)
        .count();
    if hostile == 0 {
        return Err("the default output has no hostile message to omit".to_owned());
    }
    let without = synth::generate(&Config {
        no_hostile: true,
        ..cfg.clone()
    })?;
    for line in manifest_lines(&without)? {
        if line.expected.hostile {
            return Err(format!("{}: hostile with --no-hostile", line.id));
        }
    }
    Ok(())
}

// --- B4 --------------------------------------------------------------------

#[test]
fn b4_canaries_are_unique_and_absent_from_the_t204_corpus() -> Result<(), String> {
    let corpus = testkit::corpus::load()?;
    let existing: BTreeSet<String> = corpus.all_canaries().into_iter().collect();
    let generated = synth::generate(&config(11, "freelancer", 300))?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for line in manifest_lines(&generated)? {
        let canary = &line.expected.canary;
        if existing.contains(canary) {
            return Err(format!("{canary} collides with a T-204 canary"));
        }
        if !seen.insert(canary.clone()) {
            return Err(format!("duplicate canary {canary}"));
        }
        let bytes = generated
            .eml(&line.id)
            .ok_or_else(|| format!("{}: missing .eml", line.id))?;
        if !String::from_utf8_lossy(bytes).contains(canary.as_str()) {
            return Err(format!("{}: canary is not planted in the message", line.id));
        }
    }
    Ok(())
}

// --- B5 --------------------------------------------------------------------

#[test]
fn b5_thread_references_exist_earlier_in_the_mailbox() -> Result<(), String> {
    let generated = synth::generate(&config(13, "parent", 400))?;
    let lines = manifest_lines(&generated)?;
    let mut position: BTreeMap<String, usize> = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        position.insert(line.message_id.clone(), i);
    }
    for (i, line) in lines.iter().enumerate() {
        let bytes = generated
            .eml(&line.id)
            .ok_or_else(|| format!("{}: missing .eml", line.id))?;
        for name in ["In-Reply-To", "References"] {
            let Some(value) = header_value(bytes, name) else {
                continue;
            };
            for id in value.split_whitespace() {
                let id = id.trim_matches(|c| c == '<' || c == '>');
                match position.get(id) {
                    None => return Err(format!("{}: {name} names unknown id {id}", line.id)),
                    Some(j) if *j >= i => {
                        return Err(format!("{}: {name} names a later id {id}", line.id));
                    }
                    Some(_) => {}
                }
            }
        }
    }
    let replies = lines.iter().filter(|l| l.thread_pos > 0).count();
    if replies < 20 {
        return Err(format!(
            "only {replies} threaded messages; the check is weak"
        ));
    }
    Ok(())
}

// --- B6 --------------------------------------------------------------------

#[test]
fn b6_demo_slice_has_500_messages_and_the_same_class_mix() -> Result<(), String> {
    let base = config(1, "freelancer", 2000);
    let full = synth::generate(&base)?;
    let slice = synth::generate(&Config {
        slice: Some("demo".to_owned()),
        ..base.clone()
    })?;
    let full_lines = manifest_lines(&full)?;
    let slice_lines = manifest_lines(&slice)?;
    if slice_lines.len() != 500 {
        return Err(format!("the demo slice has {} messages", slice_lines.len()));
    }
    for class in ["list", "bulk_no_header", "notice", "personal", "suspect"] {
        let f = share_bp(&full_lines, class);
        let s = share_bp(&slice_lines, class);
        if (f - s).abs() > 300 {
            return Err(format!(
                "{class}: full {f} basis points vs slice {s} (more than 3 points)"
            ));
        }
    }
    let full_ids: BTreeSet<&str> = full_lines.iter().map(|l| l.id.as_str()).collect();
    for line in &slice_lines {
        if !full_ids.contains(line.id.as_str()) {
            return Err(format!("{}: in the slice but not in the full run", line.id));
        }
    }
    if slice_lines.len() >= full_lines.len() {
        return Err("the slice is not smaller than the full run".to_owned());
    }
    Ok(())
}

/// A class's share of a set, in basis points (10 000 = 100%).
fn share_bp(lines: &[Line], class: &str) -> i64 {
    if lines.is_empty() {
        return 0;
    }
    let n = lines.iter().filter(|l| l.expected.class == class).count();
    let n = i64::try_from(n).unwrap_or(0);
    let total = i64::try_from(lines.len()).unwrap_or(1);
    (10_000 * n) / total
}

// --- B7 --------------------------------------------------------------------

#[tokio::test]
async fn b7_fake_google_seeder_reproduces_message_count_and_labels() -> Result<(), String> {
    let generated = synth::generate(&Config {
        slice: Some("demo".to_owned()),
        ..config(2, "student", 1000)
    })?;
    let mut state = FakeState::default();
    state.mailbox(EMAIL);
    let state = Arc::new(Mutex::new(state));
    let seeder = FakeGoogleSeeder::new(state.clone(), FakeMailboxKey(EMAIL.to_owned()));
    for message in &generated.messages {
        seeder.seed(&synth::render::seed_message(message)).await?;
    }
    let guard = state
        .lock()
        .map_err(|_| "fake state is poisoned".to_owned())?;
    let mailbox = guard
        .mailboxes
        .get(EMAIL)
        .ok_or_else(|| format!("{EMAIL}: missing mailbox"))?;
    if mailbox.messages.len() != generated.messages.len() {
        return Err(format!(
            "seeded {} of {} messages",
            mailbox.messages.len(),
            generated.messages.len()
        ));
    }
    for (stored, message) in mailbox.messages.values().zip(generated.messages.iter()) {
        let want: BTreeSet<String> = message.labels.iter().cloned().collect();
        if stored.labels != want {
            return Err(format!("{}: labels differ after seeding", message.id));
        }
    }
    for (line, stored) in manifest_lines(&generated)?
        .iter()
        .zip(mailbox.messages.values())
    {
        let want: BTreeSet<String> = line.labels.iter().cloned().collect();
        if stored.labels != want {
            return Err(format!("{}: manifest labels differ", line.id));
        }
    }
    Ok(())
}

// --- B8 --------------------------------------------------------------------

#[test]
fn b8_provenance_header_and_corpus_hash() -> Result<(), String> {
    let generated = synth::generate(&config(17, "freelancer", 200))?;
    let header = first_line(&generated)?;
    let claimed = header["corpus_hash"]
        .as_str()
        .ok_or_else(|| "manifest header has no corpus_hash".to_owned())?
        .to_owned();
    if header["persona"] != "freelancer" || header["seed"] != 17 {
        return Err("manifest header does not describe the run".to_owned());
    }

    let lines = manifest_lines(&generated)?;
    let by_id: BTreeMap<&str, &Line> = lines.iter().map(|l| (l.id.as_str(), l)).collect();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (path, bytes) in &generated.files {
        if !path.starts_with("eml/") {
            continue;
        }
        let text = String::from_utf8_lossy(bytes).to_string();
        let provenance = text
            .split("\r\n")
            .find(|l| l.starts_with(PROVENANCE))
            .ok_or_else(|| format!("{path}: no provenance header"))?;
        if provenance != format!("{PROVENANCE} {claimed}") {
            return Err(format!("{path}: provenance header is {provenance}"));
        }
        if !text.contains(ONE_CLICK_HEADER) {
            return Err(format!("{path}: provenance header is missing"));
        }
        let message_id =
            header_value(bytes, "Message-ID").ok_or_else(|| format!("{path}: no Message-ID"))?;
        let id = message_id.trim_matches(|c| c == '<' || c == '>').to_owned();
        if !id.starts_with("synth-") {
            return Err(format!("{path}: Message-ID {id} is not synth-"));
        }
        if !ids.insert(id.clone()) {
            return Err(format!("duplicate Message-ID {id}"));
        }
        // internal_date equals the Date header.
        let date = header_value(bytes, "Date").ok_or_else(|| format!("{path}: no Date"))?;
        let parsed =
            time::OffsetDateTime::parse(&date, &time::format_description::well_known::Rfc2822)
                .map_err(|e| format!("{path}: Date {date}: {e}"))?;
        let internal = by_id
            .get(path.trim_start_matches("eml/").trim_end_matches(".eml"))
            .ok_or_else(|| format!("{path}: not in the manifest"))?
            .internal_date
            .clone();
        if internal != synth::render::rfc3339(parsed) {
            return Err(format!("{path}: internal_date {internal} != Date {date}"));
        }
    }

    let recomputed = recompute(&generated, None, false);
    if recomputed != claimed {
        return Err(format!(
            "corpus_hash does not recompute: {recomputed} vs {claimed}"
        ));
    }
    let touched: Vec<&String> = generated
        .files
        .iter()
        .filter(|(p, _)| p.starts_with("eml/"))
        .map(|(p, _)| p)
        .collect();
    let first = touched
        .first()
        .map(|p| (*p).clone())
        .ok_or_else(|| "no .eml files".to_owned())?;
    if recompute(&generated, Some(&first), false) == claimed {
        return Err("changing an .eml byte did not change corpus_hash".to_owned());
    }
    if recompute(&generated, None, true) == claimed {
        return Err("changing a manifest line did not change corpus_hash".to_owned());
    }
    Ok(())
}

// --- traps -----------------------------------------------------------------

#[test]
fn t1120a_small_message_counts_are_exact() -> Result<(), String> {
    for n in [1usize, 3, 7, 40, 500] {
        let generated = synth::generate(&config(23, "parent", n))?;
        if generated.messages.len() != n {
            return Err(format!(
                "--messages {n} produced {} messages",
                generated.messages.len()
            ));
        }
        if manifest_lines(&generated)?.len() != n {
            return Err(format!("--messages {n}: manifest has the wrong line count"));
        }
    }
    Ok(())
}

#[test]
fn t1120a_out_is_refused_when_it_is_not_empty_without_force() -> Result<(), String> {
    let generated = synth::generate(&config(19, "freelancer", 40))?;
    let dir = std::env::temp_dir().join(format!("t1120a-refuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
    std::fs::write(dir.join("keep.txt"), b"keep").map_err(|e| format!("write: {e}"))?;
    let refused = synth::write_out(&generated, &dir, false);
    if refused.is_ok() {
        return Err("a non-empty --out was accepted without --force".to_owned());
    }
    synth::write_out(&generated, &dir, true)?;
    let manifest = dir.join("manifest.jsonl");
    if !manifest.is_file() {
        return Err("--force did not write the manifest".to_owned());
    }
    if !dir.join("SHA256SUMS").is_file() {
        return Err("SHA256SUMS was not written".to_owned());
    }
    let emls = std::fs::read_dir(dir.join("eml"))
        .map_err(|e| format!("eml dir: {e}"))?
        .count();
    if emls != generated.messages.len() {
        return Err(format!("wrote {emls} .eml files"));
    }
    let sums = std::fs::read(dir.join("SHA256SUMS")).map_err(|e| format!("sums: {e}"))?;
    if sums != generated.sums {
        return Err("the written SHA256SUMS differs from the generated one".to_owned());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("cleanup: {e}"))?;
    Ok(())
}
