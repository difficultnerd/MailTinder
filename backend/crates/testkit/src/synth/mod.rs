//! Synthetic mailbox generator v1 (T-1120a): a deterministic mailbox plus its
//! ground truth, written as `manifest.jsonl`, `eml/<id>.eml` and
//! `SHA256SUMS`.
//!
//! Rules that hold everywhere in this module tree:
//!
//! 1. Deterministic: [`rng::SplitMix64`] is the only random source, no wall
//!    clock, no environment, no `HashMap` iteration order.
//! 2. Reserved domains only (`example.com/net/org`, `*.test`, `*.invalid`) in
//!    every header, body and HTML attribute. There is no `--domain` option.
//! 3. Every `.eml` carries `X-MT-Synthetic-Corpus: <corpus_hash>`; see
//!    [`Generated::files`] and [`corpus_hash`] for how it is computed.
//! 4. Every message carries ground truth (class, bulk band, method, reason,
//!    labels, date, canary, hostile).
//! 5. Hostile content is flagged and can be omitted with `--no-hostile`.
//! 6. Nothing is sent and nothing reaches the network.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use domain::MessageClass;
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

pub mod persona;
pub mod render;
pub mod rng;
pub mod senders;
pub mod threads;

pub use crate::corpus::ExpectedMethod;
pub use threads::{Hostile, Message, Remote, Shape};

use rng::SplitMix64;

/// Bumped when the generator's output shape changes (part of the header).
pub const GENERATOR_VERSION: &str = "synth-mailbox/1";

/// Default `--messages`.
pub const DEFAULT_MESSAGES: usize = 5000;
/// `--slice demo` size (B6).
pub const SLICE_DEMO: usize = 500;
/// The only slice v1 knows.
pub const SLICE_NAME: &str = "demo";
/// Default `--persona`.
pub const DEFAULT_PERSONA: &str = "freelancer";
/// Default `--epoch`.
pub const DEFAULT_EPOCH: OffsetDateTime = time::macros::datetime!(2026-10-01 00:00:00 UTC);
/// The lowest share any class may fall to when thinning (B2's floor).
pub const MIN_CLASS_SHARE_DIVISOR: usize = 20;

/// Everything that changes the output bytes.
#[derive(Clone, Debug)]
pub struct Config {
    pub seed: u64,
    pub persona: String,
    pub messages: usize,
    pub slice: Option<String>,
    pub epoch: OffsetDateTime,
    pub no_hostile: bool,
    pub mbox: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: 1,
            persona: DEFAULT_PERSONA.to_owned(),
            messages: DEFAULT_MESSAGES,
            slice: None,
            epoch: DEFAULT_EPOCH,
            no_hostile: false,
            mbox: false,
        }
    }
}

/// `Config` plus where to write it.
#[derive(Clone, Debug)]
pub struct Args {
    pub config: Config,
    pub out: PathBuf,
    pub force: bool,
}

/// Parse the command line. An unknown flag is an error, so there is no
/// `--domain` option (B3).
pub fn parse_args<I>(args: I) -> Result<Args, String>
where
    I: IntoIterator<Item = String>,
{
    let mut config = Config::default();
    let mut out: Option<PathBuf> = None;
    let mut force = false;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--seed" => {
                let v = value(&mut it, &arg)?;
                config.seed = v
                    .parse::<u64>()
                    .map_err(|_| format!("--seed: `{v}` is not a number"))?;
            }
            "--persona" => config.persona = value(&mut it, &arg)?,
            "--messages" => {
                let v = value(&mut it, &arg)?;
                config.messages = v
                    .parse::<usize>()
                    .map_err(|_| format!("--messages: `{v}` is not a number"))?;
                if config.messages == 0 {
                    return Err("--messages: must be at least 1".to_owned());
                }
            }
            "--slice" => {
                let v = value(&mut it, &arg)?;
                if v != SLICE_NAME {
                    return Err(format!("--slice: unknown slice `{v}`"));
                }
                config.slice = Some(v);
            }
            "--epoch" => {
                let v = value(&mut it, &arg)?;
                config.epoch =
                    OffsetDateTime::parse(&v, &time::format_description::well_known::Rfc3339)
                        .map_err(|_| format!("--epoch: `{v}` is not an RFC 3339 timestamp"))?;
            }
            "--out" => out = Some(PathBuf::from(value(&mut it, &arg)?)),
            "--no-hostile" => config.no_hostile = true,
            "--mbox" => config.mbox = true,
            "--force" => force = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    persona::by_name(&config.persona)?;
    let out = out.ok_or_else(|| "--out is required".to_owned())?;
    Ok(Args { config, out, force })
}

fn value<I: Iterator<Item = String>>(it: &mut I, flag: &str) -> Result<String, String> {
    it.next()
        .ok_or_else(|| format!("{flag}: a value is required"))
}

/// The manifest's first line.
#[derive(Clone, Debug, Serialize)]
pub struct ManifestHeader {
    pub corpus_hash: String,
    pub persona: String,
    pub seed: u64,
    pub slice: Option<String>,
    pub generator_version: String,
    pub messages: usize,
}

/// The ground truth the design document requires on every message.
#[derive(Clone, Debug, Serialize)]
pub struct Expected {
    pub class: MessageClass,
    pub bulk_score_min: u8,
    pub bulk_score_max: u8,
    pub method: ExpectedMethod,
    pub reason: String,
    pub canary: String,
    pub hostile: bool,
}

/// One `manifest.jsonl` line.
#[derive(Clone, Debug, Serialize)]
pub struct GroundTruth {
    pub id: String,
    pub path: String,
    pub message_id: String,
    pub thread: usize,
    pub thread_pos: usize,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub labels: Vec<String>,
    pub internal_date: String,
    pub expected: Expected,
}

/// A generated mailbox: the message set, the files and their hashes.
#[derive(Clone, Debug)]
pub struct Generated {
    pub header: ManifestHeader,
    pub messages: Vec<Message>,
    /// Relative path (forward slashes) to final bytes, sorted by path. Does not
    /// include `SHA256SUMS`.
    pub files: Vec<(String, Vec<u8>)>,
    /// The `SHA256SUMS` file body (B1 compares it).
    pub sums: Vec<u8>,
}

impl Generated {
    /// The final bytes of `manifest.jsonl`.
    pub fn manifest(&self) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p == "manifest.jsonl")
            .map(|(_, b)| b.as_slice())
    }

    /// The final bytes of one `.eml`.
    pub fn eml(&self, id: &str) -> Option<&[u8]> {
        let path = format!("eml/{id}.eml");
        self.files
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, b)| b.as_slice())
    }

    /// The manifest body: every line but the first.
    pub fn manifest_body(&self) -> Vec<u8> {
        let Some(bytes) = self.manifest() else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(bytes);
        match text.find('\n') {
            Some(i) => text[i + 1..].as_bytes().to_vec(),
            None => Vec::new(),
        }
    }
}

/// Generate the mailbox in memory. Nothing is written and nothing is sent.
pub fn generate(config: &Config) -> Result<Generated, String> {
    let persona = persona::by_name(&config.persona)?;
    if config.messages == 0 {
        return Err("messages: must be at least 1".to_owned());
    }
    let slice = config.slice.clone();
    if let Some(name) = slice.as_deref() {
        if name != SLICE_NAME {
            return Err(format!("unknown slice `{name}`"));
        }
    }

    let mut roster_rng = SplitMix64::stream(config.seed, &format!("roster:{}", persona.name));
    let sender_count = usize::try_from(roster_rng.range(
        u64::try_from(persona.senders.0).unwrap_or(40),
        u64::try_from(persona.senders.1).unwrap_or(120),
    ))
    .unwrap_or(40);
    let correspondent_count = usize::try_from(roster_rng.range(
        u64::try_from(persona.correspondents.0).unwrap_or(5),
        u64::try_from(persona.correspondents.1).unwrap_or(15),
    ))
    .unwrap_or(5);
    let roster = senders::roster(
        &mut roster_rng,
        config.seed,
        &persona,
        sender_count,
        correspondent_count,
    );

    // Aim for three times the requested volume so thinning can hold every
    // class share; the cap is the same for a slice of the same run.
    let cap = (config.messages * 3) / roster.len().max(1) + 24;
    let owner = format!("{}@example.com", persona.name);
    let mut pool = threads::build(config.seed, config.epoch, &owner, &roster, cap);
    if config.no_hostile {
        pool.retain(|m| !m.hostile.is_hostile());
    }

    // The mailbox the arguments describe, then the slice as a subset of it.
    let full = select(pool, config.messages);
    let mut selected = if slice.is_some() {
        select(full, SLICE_DEMO)
    } else {
        full
    };
    threads::assign_bands(&mut selected);
    prune_thread_refs(&mut selected);
    threads::sort_by_date(&mut selected);

    Ok(assemble(config, &persona, slice.as_deref(), selected))
}

/// Thin the pool to `target` messages, keeping each class's share (B2/B6).
fn select(pool: Vec<Message>, target: usize) -> Vec<Message> {
    if pool.len() <= target {
        return pool;
    }
    let mut groups: [Vec<usize>; 5] = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for (i, m) in pool.iter().enumerate() {
        groups[slot(m.class)].push(i);
    }
    let floor = (target / MIN_CLASS_SHARE_DIVISOR).max(1);
    let mut quota = [0usize; 5];
    for (c, group) in groups.iter().enumerate() {
        quota[c] = (target * group.len()) / pool.len();
        if group.len() >= floor {
            quota[c] = quota[c].max(floor);
        } else {
            quota[c] = group.len();
        }
    }
    // Largest remainder to reach exactly `target`.
    let sum: usize = quota.iter().sum();
    if sum < target {
        let mut rest = target - sum;
        let mut order: Vec<usize> = (0..5).collect();
        order.sort_by_key(|c| (std::cmp::Reverse(groups[*c].len()), *c));
        for c in order {
            if rest == 0 {
                break;
            }
            let room = groups[c].len().saturating_sub(quota[c]);
            let add = room.min(rest);
            quota[c] += add;
            rest -= add;
        }
    }
    while quota.iter().sum::<usize>() > target {
        // Take from the largest quota that is still above the floor; if the
        // floor alone overshoots a very small target, take from any quota.
        let above = (0..5)
            .filter(|c| quota[*c] > floor)
            .max_by_key(|c| quota[*c]);
        let any = (0..5).filter(|c| quota[*c] > 0).max_by_key(|c| quota[*c]);
        match above.or(any) {
            Some(c) => quota[c] -= 1,
            None => break,
        }
    }

    let mut chosen: Vec<usize> = Vec::with_capacity(target);
    for (c, group) in groups.iter().enumerate() {
        let n = quota[c].min(group.len());
        if n == 0 {
            continue;
        }
        for k in 0..n {
            chosen.push(group[(k * group.len()) / n]);
        }
    }
    chosen.sort_unstable();
    chosen.dedup();
    let mut out = Vec::with_capacity(chosen.len());
    for i in chosen {
        if let Some(m) = pool.get(i).cloned() {
            out.push(m);
        }
    }
    out
}

fn slot(class: MessageClass) -> usize {
    match class {
        MessageClass::List => 0,
        MessageClass::BulkNoHeader => 1,
        MessageClass::Notice => 2,
        MessageClass::Personal => 3,
        MessageClass::Suspect => 4,
    }
}

/// Drop references to messages that are not in the set (only possible in a
/// slice), so `In-Reply-To` and `References` stay consistent (B5).
fn prune_thread_refs(messages: &mut [Message]) {
    let ids: BTreeSet<String> = messages.iter().map(|m| m.message_id.clone()).collect();
    for m in messages.iter_mut() {
        if m.in_reply_to.as_ref().is_some_and(|id| !ids.contains(id)) {
            m.in_reply_to = None;
        }
        m.references.retain(|id| ids.contains(id));
    }
}

/// Build the manifest, the `.eml` files and `SHA256SUMS`.
fn assemble(
    config: &Config,
    persona: &persona::Persona,
    slice: Option<&str>,
    messages: Vec<Message>,
) -> Generated {
    let emls: Vec<(String, Vec<u8>)> = messages
        .iter()
        .map(|m| (m.path(), render::eml(m)))
        .collect();
    let ground_truth: Vec<GroundTruth> = messages.iter().map(ground_truth).collect();
    let body_lines: Vec<String> = ground_truth
        .iter()
        .map(|g| serde_json::to_string(g).unwrap_or_else(|_| "{}".to_owned()))
        .collect();
    let mut manifest_body = body_lines.join("\n");
    manifest_body.push('\n');

    let hash = corpus_hash(&emls, manifest_body.as_bytes());
    let header = ManifestHeader {
        corpus_hash: hash.clone(),
        persona: persona.name.to_owned(),
        seed: config.seed,
        slice: slice.map(str::to_owned),
        generator_version: GENERATOR_VERSION.to_owned(),
        messages: messages.len(),
    };
    let header_line = serde_json::to_string(&header).unwrap_or_else(|_| "{}".to_owned());

    let mut files: Vec<(String, Vec<u8>)> = emls
        .iter()
        .map(|(path, base)| (path.clone(), render::with_provenance(base, &hash)))
        .collect();
    files.push((
        "manifest.jsonl".to_owned(),
        format!("{header_line}\n{manifest_body}").into_bytes(),
    ));
    if config.mbox {
        files.push(("mailbox.mbox".to_owned(), mbox(&messages)));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sums = String::new();
    {
        use std::fmt::Write as _;
        for (path, bytes) in &files {
            let _ = writeln!(sums, "{}  {path}", sha256_hex(bytes));
        }
    }

    Generated {
        header,
        messages,
        files,
        sums: sums.into_bytes(),
    }
}

/// `corpus_hash`: every header-stripped `.eml` plus the manifest body, hashed
/// per file, sorted by forward-slash relative path, joined with `\n`, hashed.
///
/// It never references itself: the caller passes the `.eml` bytes *without* the
/// provenance header and the manifest *without* its first line (rule 3).
pub fn corpus_hash(emls: &[(String, Vec<u8>)], manifest_body: &[u8]) -> String {
    let mut lines: Vec<(String, String)> = emls
        .iter()
        .map(|(path, base)| (path.clone(), sha256_hex(base)))
        .collect();
    lines.push(("manifest.jsonl".to_owned(), sha256_hex(manifest_body)));
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    let joined = lines
        .iter()
        .map(|(path, digest)| format!("{path} {digest}"))
        .collect::<Vec<String>>()
        .join("\n");
    sha256_hex(joined.as_bytes())
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn ground_truth(msg: &Message) -> GroundTruth {
    let (min, max) = msg.bulk_scores();
    GroundTruth {
        id: msg.id.clone(),
        path: msg.path(),
        message_id: msg.message_id.clone(),
        thread: msg.thread,
        thread_pos: msg.thread_pos,
        from: msg.from_address.clone(),
        to: msg.to_address.clone(),
        subject: msg.subject.clone(),
        labels: msg.labels.clone(),
        internal_date: render::rfc3339(msg.date),
        expected: Expected {
            class: msg.class,
            bulk_score_min: min,
            bulk_score_max: max,
            method: msg.method,
            reason: msg.reason.to_owned(),
            canary: msg.canary.clone(),
            hostile: msg.hostile.is_hostile(),
        },
    }
}

/// The `--mbox` export: `From ` separators, LF line endings, no escaping.
fn mbox(messages: &[Message]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for msg in messages {
        out.extend_from_slice(
            format!(
                "From {} {}\n",
                msg.from_address,
                render::date_header(msg.date)
            )
            .as_bytes(),
        );
        // CRLF to LF, byte for byte: bodies may carry non-UTF-8 hostile bytes.
        for byte in render::eml(msg) {
            if byte != b'\r' {
                out.push(byte);
            }
        }
        out.push(b'\n');
    }
    out
}

/// Write a generated mailbox to `--out`.
///
/// A non-empty `--out` is refused without `--force`; nothing is written outside
/// it.
pub fn write_out(generated: &Generated, out: &Path, force: bool) -> Result<(), String> {
    if out.exists() {
        let mut entries = std::fs::read_dir(out)
            .map_err(|e| format!("--out {}: {e}", out.display()))?
            .filter_map(Result::ok);
        if entries.next().is_some() && !force {
            return Err(format!(
                "--out {} is not empty; pass --force to overwrite",
                out.display()
            ));
        }
    }
    std::fs::create_dir_all(out).map_err(|e| format!("--out {}: {e}", out.display()))?;
    for (rel, bytes) in &generated.files {
        let path = rel
            .split('/')
            .fold(out.to_path_buf(), |p, part| p.join(part));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let sums = out.join("SHA256SUMS");
    std::fs::write(&sums, &generated.sums).map_err(|e| format!("{}: {e}", sums.display()))?;
    Ok(())
}
