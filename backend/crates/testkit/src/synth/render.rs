//! Rendering a message to `.eml` bytes, and the provenance header (rule 3).
//!
//! Every header value, body line and HTML attribute is built from the reserved
//! domains in [`crate::synth::senders`]; nothing here reaches the network.

use domain::{HeaderFacts, MailtoTarget, UnsubscribeOptions};
use time::OffsetDateTime;
use url::Url;

use crate::mailbox::state::SeedMessage;
use crate::synth::threads::{Hostile, Message, Remote, Shape};

/// The provenance header every `.eml` carries.
pub const PROVENANCE_HEADER: &str = "X-MT-Synthetic-Corpus";

/// The corpus is 8-bit clean; parts declare it and lines end CRLF.
const CRLF: &str = "\r\n";

/// The top-level headers, in message order, without the provenance header.
pub fn headers(msg: &Message) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![
        ("From".to_owned(), from_header(msg)),
        ("To".to_owned(), msg.to_address.clone()),
        ("Subject".to_owned(), msg.subject.clone()),
        ("Date".to_owned(), date_header(msg.date)),
        ("Message-ID".to_owned(), msg.message_id_header()),
    ];
    if let Some(irt) = &msg.in_reply_to {
        out.push(("In-Reply-To".to_owned(), format!("<{irt}>")));
    }
    if !msg.references.is_empty() {
        let refs: Vec<String> = msg.references.iter().map(|r| format!("<{r}>")).collect();
        out.push(("References".to_owned(), refs.join(" ")));
    }
    if let Some(reply_to) = &msg.reply_to {
        out.push(("Reply-To".to_owned(), reply_to.clone()));
    }
    out.push(("MIME-Version".to_owned(), "1.0".to_owned()));
    out.push((
        "Content-Type".to_owned(),
        format!("multipart/alternative; boundary=\"{}\"", boundary(msg)),
    ));
    if let Some(list_id) = &msg.list_id {
        out.push(("List-ID".to_owned(), format!("<{list_id}>")));
    }
    if let Some(lu) = &msg.list_unsubscribe {
        out.push(("List-Unsubscribe".to_owned(), lu.clone()));
    }
    if msg.method == crate::corpus::ExpectedMethod::OneClick {
        out.push((
            "List-Unsubscribe-Post".to_owned(),
            "List-Unsubscribe=One-Click".to_owned(),
        ));
    }
    if msg.precedence_bulk {
        out.push(("Precedence".to_owned(), "bulk".to_owned()));
    }
    if let Some(fb) = &msg.feedback_id {
        out.push(("Feedback-ID".to_owned(), fb.clone()));
    }
    out.push((
        "Authentication-Results".to_owned(),
        format!(
            "mx.example.com; dkim={} header.d={}",
            if msg.class == domain::MessageClass::Suspect {
                "fail"
            } else {
                "pass"
            },
            msg.domain
        ),
    ));
    out
}

/// The `.eml` bytes without the provenance header.
pub fn eml(msg: &Message) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for (name, value) in headers(msg) {
        push(&mut out, &name);
        push(&mut out, ": ");
        push(&mut out, &value);
        push(&mut out, CRLF);
    }
    push(&mut out, CRLF);
    let part = boundary(msg);
    let charset = if msg.hostile == Hostile::OddEncoding {
        "iso-8859-1"
    } else {
        "utf-8"
    };
    push(&mut out, "--");
    push(&mut out, &part);
    push(&mut out, CRLF);
    push(
        &mut out,
        &format!("Content-Type: text/plain; charset={charset}{CRLF}Content-Transfer-Encoding: 8bit{CRLF}{CRLF}"),
    );
    push(&mut out, &body_text(msg));
    push(&mut out, CRLF);
    if msg.hostile == Hostile::OddEncoding {
        // Latin-1 bytes under a declared single-byte charset: the odd-encoding
        // hostile case (rule 5). No address or URL is on this line.
        out.extend_from_slice(&[0xe9, 0x20, 0xe8, 0x20, 0xfc, 0x20, 0xf1]);
        push(&mut out, CRLF);
    }
    push(&mut out, "--");
    push(&mut out, &part);
    push(&mut out, CRLF);
    push(
        &mut out,
        &format!("Content-Type: text/html; charset=utf-8{CRLF}Content-Transfer-Encoding: 8bit{CRLF}{CRLF}"),
    );
    push(&mut out, &body_html(msg));
    push(&mut out, CRLF);
    push(&mut out, "--");
    push(&mut out, &part);
    push(&mut out, "--");
    push(&mut out, CRLF);
    out
}

/// Append a string to a byte buffer.
fn push(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
}

/// The MIME boundary; unique per message and absent from every body.
pub fn boundary(msg: &Message) -> String {
    format!("synth-{}-b", msg.id)
}

/// Insert the provenance header as the last top-level header.
///
/// Byte-exact: bodies may carry single-byte (non-UTF-8) hostile content, so
/// nothing here decodes the message.
pub fn with_provenance(base: &[u8], corpus_hash: &str) -> Vec<u8> {
    match find(base, SEPARATOR) {
        Some(sep) => {
            let mut out = Vec::with_capacity(base.len() + 96);
            out.extend_from_slice(&base[..sep + 2]);
            out.extend_from_slice(PROVENANCE_HEADER.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(corpus_hash.as_bytes());
            out.extend_from_slice(CRLF.as_bytes());
            out.extend_from_slice(&base[sep + 2..]);
            out
        }
        None => base.to_vec(),
    }
}

/// Remove the provenance header line, giving back the pre-hash bytes.
pub fn strip_provenance(final_bytes: &[u8]) -> Vec<u8> {
    let Some(sep) = find(final_bytes, SEPARATOR) else {
        return final_bytes.to_vec();
    };
    let head = &final_bytes[..sep];
    let Some(last) = rfind(head, CRLF.as_bytes()) else {
        return final_bytes.to_vec();
    };
    if !head[last + 2..].starts_with(PROVENANCE_HEADER.as_bytes()) {
        return final_bytes.to_vec();
    }
    let mut out = head[..last].to_vec();
    out.extend_from_slice(&final_bytes[sep..]);
    out
}

/// The header/body separator.
const SEPARATOR: &[u8] = b"\r\n\r\n";

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// The prose of the text part.
pub fn body_text(msg: &Message) -> String {
    use std::fmt::Write as _;

    let mut out = prose(msg);
    if msg.shape == Shape::Forward {
        let _ = write!(
            out,
            "\n\n---------- Forwarded message ----------\nFrom: {} <{}>\nDate: {}\nSubject: {}\n",
            msg.from_display,
            msg.from_address,
            date_header(msg.date),
            msg.subject
        );
    } else if msg.quoted {
        let _ = write!(
            out,
            "\n\n> On {}, {} wrote:\n> {}",
            date_header(msg.date),
            msg.from_display,
            msg.interest
        );
    }
    if msg.hostile == Hostile::HugeBody {
        let _ = write!(out, "\n\n{}\n", prose(msg).repeat(180));
    }
    out.push_str("\n\n--\n");
    out.push_str(&msg.canary);
    out.push('\n');
    out
}

/// The HTML alternative.
pub fn body_html(msg: &Message) -> String {
    use std::fmt::Write as _;

    let mut out = format!("<html><body><p>{}</p>", escape_html(&prose(msg)));
    if matches!(msg.remote, Remote::PixelAndWrappedLinks) {
        let _ = write!(
            out,
            "<p><a href=\"https://track.example.net/c/{}?u=https://news.example.org/read\">Read it online</a></p>",
            msg.id
        );
    }
    if matches!(msg.remote, Remote::Pixel | Remote::PixelAndWrappedLinks) {
        let _ = write!(
            out,
            "<img src=\"https://img.example.com/p/{}.gif\" width=\"1\" height=\"1\" alt=\"\">",
            msg.id
        );
    }
    let _ = write!(out, "<p>{}</p></body></html>", msg.canary);
    out
}

fn prose(msg: &Message) -> String {
    match msg.class {
        domain::MessageClass::List => format!(
            "Hello,\n\nHere is this week's edition on {}. The full piece is online.\n",
            msg.interest
        ),
        domain::MessageClass::BulkNoHeader => format!(
            "Hi,\n\nA quick update about {} from the team.\n",
            msg.interest
        ),
        domain::MessageClass::Notice => format!(
            "Hello,\n\nThis is an automated notice about your {}.\nNo action is needed.\n",
            msg.interest
        ),
        domain::MessageClass::Personal => format!(
            "Hi,\n\nQuick one about {} - are you free at the weekend?\n",
            msg.interest
        ),
        domain::MessageClass::Suspect => format!(
            "Dear customer,\n\nWe noticed unusual activity and your account is on hold.\nConfirm your details within 24 hours: https://secure-notice.example.com/verify/{}\n",
            msg.id
        ),
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The `From` header: the phishing flavour carries a display-name spoof.
pub fn from_header(msg: &Message) -> String {
    if msg.hostile == Hostile::Phishing {
        return format!(
            "\"{} service\" <verify@fenwick-bank.example.net>",
            msg.from_display
        );
    }
    if msg.hostile == Hostile::HeaderInjection {
        // Header injection: the display carries a CRLF and a forged header.
        return format!(
            "\"{}\r\nX-Injected-Promo: winner-{} claim at https://prize.example.com/claim\" <{}>",
            msg.from_display, msg.id, msg.from_address
        );
    }
    format!("{} <{}>", msg.from_display, msg.from_address)
}

/// `internal_date` and the `Date` header come from the same instant.
pub fn date_header(dt: OffsetDateTime) -> String {
    let weekday = match dt.weekday() {
        time::Weekday::Monday => "Mon",
        time::Weekday::Tuesday => "Tue",
        time::Weekday::Wednesday => "Wed",
        time::Weekday::Thursday => "Thu",
        time::Weekday::Friday => "Fri",
        time::Weekday::Saturday => "Sat",
        time::Weekday::Sunday => "Sun",
    };
    let month = match dt.month() {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    };
    format!(
        "{weekday}, {:02} {month} {} {:02}:{:02}:{:02} +0000",
        dt.day(),
        dt.year(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

/// RFC 3339, always UTC: the manifest's `internal_date`.
pub fn rfc3339(dt: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

/// The `FakeMailbox`/`fake-google` seed for a message (B7).
pub fn seed_message(msg: &Message) -> SeedMessage {
    let from_domain = msg
        .from_address
        .rsplit_once('@')
        .map_or_else(String::new, |(_, d)| d.to_lowercase());
    let reply_to_domain = msg
        .reply_to
        .as_deref()
        .and_then(|v| v.rsplit_once('@').map(|(_, d)| d.to_lowercase()));
    SeedMessage {
        from_display: msg.from_display.clone(),
        from_address: msg.from_address.clone(),
        subject: msg.subject.clone(),
        raw_headers: headers(msg),
        facts: HeaderFacts {
            list_unsubscribe: unsubscribe_options(msg),
            list_unsubscribe_present: msg.list_unsubscribe.is_some(),
            list_id: msg.list_id.clone(),
            feedback_id: msg.feedback_id.clone(),
            precedence_bulk: msg.precedence_bulk,
            auto_submitted: msg.class == domain::MessageClass::Notice,
            from_authenticated: msg.class != domain::MessageClass::Suspect,
            esp_hint: None,
            is_reply_or_thread: msg.thread_pos > 0,
            reply_to_mismatch: reply_to_domain.is_some() && reply_to_domain != Some(from_domain),
            display_name_spoof: msg.hostile == Hostile::Phishing,
        },
        preview_text: prose(msg),
        internal_date: msg.date,
        labels: msg.labels.clone(),
    }
}

/// The DKIM-covered unsubscribe options implied by the emitted header.
fn unsubscribe_options(msg: &Message) -> Option<UnsubscribeOptions> {
    let raw = msg.list_unsubscribe.as_deref()?;
    let uri = raw
        .trim()
        .strip_prefix('<')
        .and_then(|v| v.strip_suffix('>'))?;
    let mut out = UnsubscribeOptions::default();
    match msg.method {
        crate::corpus::ExpectedMethod::OneClick => {
            out.one_click_https = Url::parse(uri).ok().filter(|u| u.scheme() == "https");
        }
        crate::corpus::ExpectedMethod::Mailto => {
            if let Some(target) = uri.strip_prefix("mailto:") {
                let (to, subject) = target.split_once('?').unwrap_or((target, ""));
                let subject = subject
                    .split('&')
                    .find_map(|p| p.strip_prefix("subject="))
                    .unwrap_or("");
                out.mailto = MailtoTarget::new(to, Some(subject), None).ok();
            }
        }
        crate::corpus::ExpectedMethod::NeedsAttentionHttps => {
            out.https = Url::parse(uri).ok().filter(|u| u.scheme() == "https");
        }
        _ => return None,
    }
    if out.one_click_https.is_none() && out.https.is_none() && out.mailto.is_none() {
        None
    } else {
        Some(out)
    }
}
