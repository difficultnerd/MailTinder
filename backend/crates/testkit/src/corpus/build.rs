//! Header and MIME builder for the synthetic corpus (T-204).
//!
//! Turns a [`CaseSpec`] into RFC 5322 bytes plus the unfolded header list in
//! message order (top first). No real mail, no real ESP domains; every address,
//! URL and domain is reserved (S10 section 5).

use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::{canary, rfc5322_date, CaseSpec, DkimScenario};

const CRLF: &str = "\r\n";

/// Build the `.eml` bytes and the unfolded header list for a case.
pub(crate) fn build_case(spec: &CaseSpec) -> (Vec<u8>, Vec<(String, String)>) {
    let headers = build_headers(spec);
    let body = build_body(spec);
    let eml = render(&headers, &body);
    (eml.into_bytes(), headers)
}

fn build_headers(spec: &CaseSpec) -> Vec<(String, String)> {
    let id = &spec.id;
    let from_domain = spec
        .from_address
        .rsplit_once('@')
        .map(|(_, d)| d.to_owned())
        .unwrap_or_default();
    let dkim_domain = spec
        .dkim_domain
        .clone()
        .unwrap_or_else(|| from_domain.clone());

    // The `b=` value (not a real signature) is SHA-256 of `<id>sig`.
    let b_value = base64_no_pad(&Sha256::digest(format!("{id}sig").as_bytes()));
    let bh = base64_no_pad(&Sha256::digest(build_plain_text(spec).as_bytes()));

    let mut out: Vec<(String, String)> = Vec::new();

    // 1. Authentication-Results (trusted, topmost).
    let ar = match spec.dkim {
        DkimScenario::Unsigned => format!(
            "mx.google.com; dkim=none header.i=@{dkim_domain} header.s=s1; spf=pass smtp.mailfrom={from_domain}; dmarc=pass header.from={from_domain}"
        ),
        DkimScenario::EspPassDmarcFail => format!(
            "mx.google.com; dkim=pass header.i=@example.net header.s=s1 header.b={}; spf=pass smtp.mailfrom={from_domain}; dmarc=fail header.from={from_domain}",
            &b_value[..8]
        ),
        DkimScenario::Fail => format!(
            "mx.google.com; dkim=fail header.i=@{dkim_domain} header.s=s1 header.b={}; spf=pass smtp.mailfrom={from_domain}; dmarc=fail header.from={from_domain}",
            &b_value[..8]
        ),
        DkimScenario::ForgedLowerAr => format!(
            "mx.google.com; dkim=none header.i=@{dkim_domain}; spf=pass smtp.mailfrom={from_domain}; dmarc=fail header.from={from_domain}"
        ),
        _ => format!(
            "mx.google.com; dkim=pass header.i=@{dkim_domain} header.s=s1 header.b={}; spf=pass smtp.mailfrom={from_domain}; dmarc=pass header.from={from_domain}",
            &b_value[..8]
        ),
    };
    out.push(("Authentication-Results".to_owned(), ar));

    let h_list = h_tag(spec);
    if spec.dkim != DkimScenario::Unsigned {
        // 2. DKIM-Signature.
        let sig = format!(
            "v=1; a=rsa-sha256; c=relaxed/relaxed; d={dkim_domain}; s=s1; h={h_list}; bh={bh}; b={b_value}"
        );
        out.push(("DKIM-Signature".to_owned(), sig));

        // 2b. Forged lower Authentication-Results (the forgery T-406 must ignore).
        if spec.dkim == DkimScenario::ForgedLowerAr {
            out.push((
                "Authentication-Results".to_owned(),
                format!(
                    "mx.google.com; dkim=pass header.i=@{dkim_domain} header.s=s1 header.b={}",
                    &b_value[..8]
                ),
            ));
        }
    }

    // From (display carries a canary; encoded if non-ASCII).
    let from_display = canary(spec, "display", &spec.from_display);
    out.push((
        "From".to_owned(),
        format!("{} <{}>", enc(&from_display), spec.from_address),
    ));

    if let Some(r) = &spec.reply_to {
        out.push((
            "Reply-To".to_owned(),
            format!("{} <{}>", enc(&spec.from_display), r),
        ));
    }

    out.push(("To".to_owned(), "reader@example.org".to_owned()));

    let subject = canary(spec, "subject", &spec.subject);
    out.push(("Subject".to_owned(), enc(&subject)));

    out.push(("Date".to_owned(), rfc5322_date(spec)));
    out.push(("Message-Id".to_owned(), format!("<{id}@example.com>")));

    if let Some(li) = &spec.list_id {
        out.push(("List-Id".to_owned(), format!("<{li}>")));
    }
    if let Some(lu) = &spec.list_unsubscribe {
        out.push(("List-Unsubscribe".to_owned(), lu.clone()));
    }
    if spec.one_click_post {
        out.push((
            "List-Unsubscribe-Post".to_owned(),
            "List-Unsubscribe=One-Click".to_owned(),
        ));
    }
    if let Some(fb) = &spec.feedback_id {
        out.push(("Feedback-Id".to_owned(), fb.clone()));
    }
    if let Some(p) = &spec.precedence {
        out.push(("Precedence".to_owned(), p.clone()));
    }
    if let Some(a) = &spec.auto_submitted {
        out.push(("Auto-Submitted".to_owned(), a.clone()));
    }
    for (k, v) in &spec.extra_headers {
        out.push((k.clone(), v.clone()));
    }

    // X-Pad headers to reach header_padding_bytes (hostile 1 MB case).
    if spec.header_padding_bytes > 0 {
        let mut remaining = spec.header_padding_bytes;
        let mut n = 0u32;
        while remaining > 0 {
            n += 1;
            let prefix = format!("X-Pad-{n}: ");
            // Total line must stay ≤ 998 bytes (RFC 5322), so value ≤ 998 - prefix.
            let max_val = u32::try_from(998usize.saturating_sub(prefix.len())).unwrap_or(0);
            let val_len = remaining.min(max_val);
            let line: String = (0..val_len as usize).map(|_| 'x').collect();
            out.push((format!("X-Pad-{n}"), line));
            remaining = remaining.saturating_sub(val_len);
        }
    }

    out
}

/// The `h=` tag for the DKIM signature.
fn h_tag(spec: &CaseSpec) -> String {
    let mut h: Vec<&str> = vec!["from", "to", "subject", "date"];
    match spec.dkim {
        DkimScenario::PassCoversBoth => {
            h.push("list-unsubscribe");
            h.push("list-unsubscribe-post");
        }
        DkimScenario::PassCoversListUnsub => h.push("list-unsubscribe"),
        _ => {}
    }
    h.join(":")
}

/// The plain-text body, repeated `body_repeat` times, with the body canary.
fn build_plain_text(spec: &CaseSpec) -> String {
    let mut text = String::new();
    for _ in 0..spec.body_repeat.max(1) {
        text.push_str(&spec.body_text);
    }
    if text.is_empty() {
        text.push(' ');
    }
    text.push_str("CANARY-");
    text.push_str(&spec.id);
    text.push_str("-body");
    text
}

fn build_body(spec: &CaseSpec) -> String {
    let plain = build_plain_text(spec);
    match &spec.body_html {
        None => encode64_if_needed(&plain),
        Some(html) => {
            let mut html_body = html.clone();
            if spec.remote_images {
                html_body.push_str("<img src=\"https://img.example.com/p.gif\">");
            }
            html_body.push_str("<p>CANARY-");
            html_body.push_str(&spec.id);
            html_body.push_str("-body</p>");
            format!(
                "Content-Type: multipart/alternative; boundary=\"{SPEC_B}\"{CRLF}\
                 {CRLF}--{SPEC_B}{CRLF}Content-Type: text/plain; charset=utf-8{CRLF}\
                 Content-Transfer-Encoding: base64{CRLF}{CRLF}{}{CRLF}--{SPEC_B}{CRLF}\
                 Content-Type: text/html; charset=utf-8{CRLF}Content-Transfer-Encoding: base64{CRLF}{CRLF}{}{CRLF}--{SPEC_B}--{CRLF}",
                encode64(&plain),
                encode64(&html_body),
            )
        }
    }
}

/// Render the RFC 5322 message: headers (folded) then body, CRLF separated.
fn render(headers: &[(String, String)], body: &str) -> String {
    let mut s = String::new();
    for (k, v) in headers {
        push_folded(&mut s, k, v);
    }
    s.push_str(CRLF); // header/body separator blank line
    s.push_str(body);
    if !body.ends_with('\n') {
        s.push_str(CRLF);
    }
    s
}

/// Push `Name: value`, folding on whitespace to keep lines ≤ 78 columns.
fn push_folded(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    let mut cur_len = name.len() + 2;
    let mut result = String::new();
    let words: Vec<&str> = value.split(' ').collect();
    let mut first_word = true;
    for w in words {
        let need = w.len() + usize::from(!first_word);
        if cur_len + need > 78 && !first_word {
            result.push_str(CRLF);
            result.push(' ');
            cur_len = 1;
        } else if !first_word {
            result.push(' ');
            cur_len += 1;
        }
        result.push_str(w);
        cur_len += w.len();
        first_word = false;
    }
    out.push_str(&result);
    out.push_str(CRLF);
}

/// RFC 2047-encode non-ASCII text; leave plain ASCII as-is.
fn enc(s: &str) -> String {
    if s.chars().all(|c| c <= '\u{7f}') {
        s.to_owned()
    } else {
        let b64 = base64_no_pad(s.as_bytes());
        format!("=?UTF-8?B?{b64}?=")
    }
}

fn encode64_if_needed(s: &str) -> String {
    if s.chars().all(|c| c <= '\u{7f}') {
        format!(
            "Content-Type: text/plain; charset=utf-8{CRLF}Content-Transfer-Encoding: 7bit{CRLF}{CRLF}{s}"
        )
    } else {
        encode64(s)
    }
}

fn encode64(s: &str) -> String {
    let wrapped = base64_no_pad(s.as_bytes());
    wrapped
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn base64_no_pad(data: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    STANDARD_NO_PAD.encode(data)
}

const SPEC_B: &str = "MT-BOUNDARY-204";
