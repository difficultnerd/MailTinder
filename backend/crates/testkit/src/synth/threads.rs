//! Threads, send schedules and per-message ground truth (T-1120a shape and
//! rules 3 to 5).
//!
//! A sender's sends are threaded: a message either starts a subject or joins a
//! recent one, so `Message-ID`, `In-Reply-To` and `References` are consistent
//! and every referenced id is older than the message that references it (B5).

use domain::MessageClass;
use time::{Duration, OffsetDateTime};

use crate::corpus::ExpectedMethod;
use crate::synth::rng::SplitMix64;
use crate::synth::senders::{Archetype, Sender, MAX_AGE_DAYS};

/// A hostile-content flavour (rule 5). `No` is the common case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hostile {
    No,
    Phishing,
    HeaderInjection,
    HugeBody,
    OddEncoding,
}

impl Hostile {
    pub const fn is_hostile(self) -> bool {
        !matches!(self, Self::No)
    }
}

/// Where a message sits in its thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Fresh,
    Reply,
    Forward,
}

/// Remote content the HTML part may carry: a tracking pixel and wrapped links
/// to the testbed host only (reserved domains).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remote {
    None,
    Pixel,
    PixelAndWrappedLinks,
}

/// One generated message: everything the manifest and the `.eml` need.
#[derive(Clone, Debug)]
pub struct Message {
    /// Stable across runs and slices, e.g. `m000123`.
    pub id: String,
    pub index: usize,
    pub sender: usize,
    pub archetype: Archetype,
    pub interest: &'static str,
    pub personal: bool,
    pub from_display: String,
    pub from_address: String,
    pub domain: String,
    pub to_address: String,
    pub reply_to: Option<String>,
    pub subject: String,
    /// Without angle brackets, e.g. `synth-m000123@news.example.org`.
    pub message_id: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub date: OffsetDateTime,
    pub class: MessageClass,
    /// Bulk band decile 0..=9; the score range is this decile (B2).
    pub band: u8,
    pub method: ExpectedMethod,
    pub reason: &'static str,
    pub list_unsubscribe: Option<String>,
    pub list_id: Option<String>,
    pub feedback_id: Option<String>,
    pub precedence_bulk: bool,
    pub shape: Shape,
    pub quoted: bool,
    pub hostile: Hostile,
    pub remote: Remote,
    pub labels: Vec<String>,
    pub canary: String,
    pub thread: usize,
    pub thread_pos: usize,
}

impl Message {
    /// The bulk score window the band promises.
    pub const fn bulk_scores(&self) -> (u8, u8) {
        let lo = self.band * 10;
        let hi = if self.band == 9 { 100 } else { lo + 9 };
        (lo, hi)
    }

    /// The `.eml` path, relative to `--out`, with forward slashes.
    pub fn path(&self) -> String {
        format!("eml/{}.eml", self.id)
    }

    /// The `Message-ID` header value, with angle brackets.
    pub fn message_id_header(&self) -> String {
        format!("<{}>", self.message_id)
    }
}

/// An open thread on one sender.
struct Open {
    base: &'static str,
    subject: String,
    ids: Vec<String>,
    /// The newest date in the thread, so a reply is always later than what it
    /// answers (B5).
    last_date: OffsetDateTime,
}

/// Where a message landed in its thread.
struct Placement {
    shape: Shape,
    thread: usize,
    thread_pos: usize,
    subject: String,
    in_reply_to: Option<String>,
    references: Vec<String>,
    /// A reply must be dated after the message it answers.
    date: OffsetDateTime,
}

/// A send: the day (before epoch) plus the ids of its burst neighbours.
fn schedule(s: &Sender, cap: usize) -> Vec<i64> {
    let mut days: Vec<i64> = Vec::new();
    let mut day = s.first_day;
    while day >= s.last_day && days.len() < cap {
        days.push(day);
        day -= s.step_days;
        if s.step_days <= 0 {
            break;
        }
    }
    for burst in &s.bursts {
        for k in 0..5 {
            let d = burst + k;
            if d >= 1 {
                days.push(d);
            }
        }
    }
    days.sort_unstable();
    days.dedup();
    if days.len() > cap {
        days.drain(0..days.len() - cap);
    }
    days
}

/// Build every candidate message for the roster. The caller thins the pool.
pub fn build(
    seed: u64,
    epoch: OffsetDateTime,
    owner: &str,
    roster: &[Sender],
    cap_per_sender: usize,
) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    let mut next: usize = 1;
    for s in roster {
        let mut sends = SplitMix64::stream(seed, &format!("sends:{}", s.index));
        let mut threads: Vec<Open> = Vec::new();
        for day in schedule(s, cap_per_sender) {
            let id = format!("m{next:06}");
            next += 1;
            let mut rst = SplitMix64::stream(seed, &format!("msg:{id}"));
            let address = s.address_on(day).to_owned();
            let domain = address
                .rsplit_once('@')
                .map_or_else(|| s.domain.clone(), |(_, d)| d.to_owned());

            let date = date_for(epoch, day, &mut rst);
            let placement = place(&mut sends, s, &mut threads, &id, &domain, date);

            let hostile = hostile_of(&mut rst);
            let class = if hostile == Hostile::Phishing {
                MessageClass::Suspect
            } else {
                s.class
            };
            let mut msg = Message {
                id,
                index: out.len(),
                sender: s.index,
                archetype: s.archetype,
                interest: s.interest,
                personal: s.personal,
                from_display: s.display.clone(),
                from_address: address,
                domain,
                to_address: owner.to_owned(),
                reply_to: None,
                subject: placement.subject,
                message_id: String::new(),
                in_reply_to: placement.in_reply_to,
                references: placement.references,
                date: placement.date,
                class,
                band: 0,
                method: ExpectedMethod::None,
                reason: "",
                list_unsubscribe: None,
                list_id: None,
                feedback_id: None,
                precedence_bulk: class == MessageClass::BulkNoHeader,
                shape: placement.shape,
                quoted: placement.shape == Shape::Reply && rst.chance(500),
                hostile,
                remote: Remote::None,
                labels: Vec::new(),
                canary: String::new(),
                thread: placement.thread,
                thread_pos: placement.thread_pos,
            };
            msg.message_id = format!("synth-{}@{}", msg.id, msg.domain);
            msg.canary = format!("CANARY-SYNTH-{}", msg.id);
            signals(&mut msg, s, &mut rst, day);
            out.push(msg);
        }
    }
    out
}

/// Place a message in its thread and return the thread facts.
fn place(
    sends: &mut SplitMix64,
    s: &Sender,
    threads: &mut Vec<Open>,
    id: &str,
    domain: &str,
    date: OffsetDateTime,
) -> Placement {
    let message_id = format!("synth-{id}@{domain}");
    let reply_chance = if s.personal { 550 } else { 220 };
    if !threads.is_empty() && sends.chance(reply_chance) {
        let window = threads.len().min(3);
        let ti = threads.len() - 1 - sends.index(window);
        if threads[ti].ids.len() < 40 {
            let prev = threads[ti].ids.last().cloned();
            let references = threads[ti].ids.clone();
            let pos = references.len();
            let shape = if sends.chance(70) {
                Shape::Forward
            } else {
                Shape::Reply
            };
            let subject = match shape {
                Shape::Forward => format!("Fwd: {}", threads[ti].base),
                _ => drift(&threads[ti].subject, sends),
            };
            // A reply is always later than the message it answers (B5).
            let date = date.max(threads[ti].last_date + Duration::minutes(1));
            threads[ti].last_date = date;
            subject.clone_into(&mut threads[ti].subject);
            threads[ti].ids.push(message_id);
            return Placement {
                shape,
                thread: ti,
                thread_pos: pos,
                subject,
                in_reply_to: prev,
                references,
                date,
            };
        }
    }
    let base = *sends.pick(s.subjects());
    let subject = base.to_owned();
    threads.push(Open {
        base,
        subject: subject.clone(),
        ids: vec![message_id],
        last_date: date,
    });
    Placement {
        shape: Shape::Fresh,
        thread: threads.len() - 1,
        thread_pos: 0,
        subject,
        in_reply_to: None,
        references: Vec::new(),
        date,
    }
}

/// Subject drift: a reply now and then renames the thread.
fn drift(subject: &str, rng: &mut SplitMix64) -> String {
    if rng.chance(80) {
        let tail = [" (updated)", " - new date", ": follow-up"];
        format!("Re: {subject}{}", tail[rng.index(tail.len())])
    } else {
        format!("Re: {subject}")
    }
}

fn hostile_of(rst: &mut SplitMix64) -> Hostile {
    if !rst.chance(45) {
        return Hostile::No;
    }
    match rst.below(100) {
        0..=49 => Hostile::Phishing,
        50..=69 => Hostile::HeaderInjection,
        70..=84 => Hostile::HugeBody,
        _ => Hostile::OddEncoding,
    }
}

/// `internal_date` equals the `Date` header: both come from here.
pub fn date_for(epoch: OffsetDateTime, day: i64, rng: &mut SplitMix64) -> OffsetDateTime {
    let secs = i64::try_from(rng.range(6 * 3600, 21 * 3600)).unwrap_or(12 * 3600);
    epoch - Duration::days(day) - Duration::seconds(secs)
}

/// The bulk band each class may claim. Wider as the class gets noisier.
const fn band_span(class: MessageClass) -> (u8, u8) {
    match class {
        MessageClass::Personal => (0, 3),
        MessageClass::Notice => (3, 4),
        MessageClass::BulkNoHeader | MessageClass::List => (6, 4),
        MessageClass::Suspect => (4, 6),
    }
}

/// Assign bulk bands so every decile gets a fair share (B2): each class cycles
/// evenly through the deciles it may claim.
pub fn assign_bands(messages: &mut [Message]) {
    let mut counters = [0u8; 5];
    for msg in messages.iter_mut() {
        let slot = match msg.class {
            MessageClass::List => 0,
            MessageClass::BulkNoHeader => 1,
            MessageClass::Notice => 2,
            MessageClass::Personal => 3,
            MessageClass::Suspect => 4,
        };
        let (low, span) = band_span(msg.class);
        let k = counters[slot];
        counters[slot] = counters[slot].wrapping_add(1);
        msg.band = low + (k % span);
        if msg.band > 9 {
            msg.band = 9;
        }
    }
}

/// Method, reason, unsubscribe header, labels, remote content and reply-to.
fn signals(msg: &mut Message, s: &Sender, rst: &mut SplitMix64, day: i64) {
    let method = method_for(msg.class, rst);
    msg.method = method;
    msg.reason = reason_for(msg.class, method);
    msg.list_unsubscribe = match method {
        ExpectedMethod::None => None,
        ExpectedMethod::NeedsAttentionHttp => {
            Some(format!("<http://u.{}/u/{}>", msg.domain, msg.id))
        }
        ExpectedMethod::NeedsAttentionHttps => {
            Some(format!("<https://u.{}/u/{}>", msg.domain, msg.id))
        }
        ExpectedMethod::OneClick => {
            Some(format!("<https://u.{}/one-click/{}>", msg.domain, msg.id))
        }
        ExpectedMethod::Mailto => Some(format!(
            "<mailto:unsubscribe@{}?subject=unsubscribe>",
            msg.domain
        )),
    };
    msg.list_id = matches!(msg.class, MessageClass::List | MessageClass::BulkNoHeader)
        .then(|| s.list_id.clone());
    msg.feedback_id =
        (msg.class == MessageClass::List).then(|| format!("{}:{}", msg.domain, msg.id));
    if msg.class == MessageClass::Suspect && rst.chance(600) {
        msg.reply_to = Some("replies@account-check.test".to_owned());
    }
    msg.remote = if msg.personal {
        Remote::None
    } else {
        match rst.below(100) {
            0..=24 => Remote::PixelAndWrappedLinks,
            25..=59 => Remote::Pixel,
            _ => Remote::None,
        }
    };
    msg.labels = labels_for(msg.class, day, rst);
}

fn method_for(class: MessageClass, rst: &mut SplitMix64) -> ExpectedMethod {
    let roll = rst.below(1000);
    match class {
        MessageClass::List => match roll {
            0..=699 => ExpectedMethod::OneClick,
            700..=849 => ExpectedMethod::Mailto,
            850..=949 => ExpectedMethod::NeedsAttentionHttps,
            _ => ExpectedMethod::NeedsAttentionHttp,
        },
        MessageClass::BulkNoHeader => ExpectedMethod::None,
        MessageClass::Notice => match roll {
            0..=549 => ExpectedMethod::None,
            550..=799 => ExpectedMethod::Mailto,
            800..=899 => ExpectedMethod::NeedsAttentionHttps,
            900..=949 => ExpectedMethod::NeedsAttentionHttp,
            _ => ExpectedMethod::OneClick,
        },
        MessageClass::Personal => match roll {
            0..=949 => ExpectedMethod::None,
            _ => ExpectedMethod::Mailto,
        },
        MessageClass::Suspect => match roll {
            0..=449 => ExpectedMethod::None,
            450..=749 => ExpectedMethod::NeedsAttentionHttp,
            750..=899 => ExpectedMethod::Mailto,
            _ => ExpectedMethod::OneClick,
        },
    }
}

fn reason_for(class: MessageClass, method: ExpectedMethod) -> &'static str {
    match (class, method) {
        (MessageClass::List, ExpectedMethod::OneClick) => "one-click list",
        (MessageClass::List, ExpectedMethod::Mailto) => "mailto list",
        (MessageClass::List, ExpectedMethod::NeedsAttentionHttps) => "list, https unsubscribe",
        (MessageClass::List, ExpectedMethod::NeedsAttentionHttp) => "list, http unsubscribe",
        (MessageClass::List, ExpectedMethod::None) => "list with no unsubscribe",
        (MessageClass::BulkNoHeader, _) => "bulk with no unsubscribe header",
        (MessageClass::Notice, ExpectedMethod::None) => "transactional notice",
        (MessageClass::Notice, ExpectedMethod::OneClick) => "notice, one-click unsubscribe",
        (MessageClass::Notice, ExpectedMethod::Mailto) => "notice, mailto unsubscribe",
        (MessageClass::Notice, _) => "notice, needs attention",
        (MessageClass::Personal, _) => "personal conversation",
        (MessageClass::Suspect, _) => "suspicious sender",
    }
}

/// `INBOX` always; unread share rises with age (rule: age).
fn labels_for(class: MessageClass, day: i64, rst: &mut SplitMix64) -> Vec<String> {
    let mut labels = vec!["INBOX".to_owned()];
    let age = u64::try_from(day.clamp(0, MAX_AGE_DAYS)).unwrap_or(0);
    let span = u64::try_from(MAX_AGE_DAYS).unwrap_or(3650);
    let unread = 150 + (600 * age) / span;
    if rst.below(1000) < unread {
        labels.push("UNREAD".to_owned());
    }
    if class == MessageClass::Suspect && rst.chance(500) {
        labels.push("SPAM".to_owned());
    }
    if rst.chance(30) {
        labels.push("STARRED".to_owned());
    }
    labels
}

/// Sort by `internal_date`, ties broken by id, so the output order is total.
pub fn sort_by_date(messages: &mut [Message]) {
    messages.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
}
