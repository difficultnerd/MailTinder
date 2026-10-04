//! Corpus rule tests (T-204).
//!
//! These enforce the S10 section 5 rules: every case loads, uses reserved
//! domains and canaries, covers every required S10 case, and its `.eml` parses
//! back to the expected shape.

use base64::Engine;
use testkit::corpus::{load, RESERVED_DOMAIN_SUFFIXES};

/// Every required S10 section 5 case id (plus the two security/robustness cases).
fn required_ids() -> Vec<&'static str> {
    vec![
        "one-click-covered",
        "one-click-plus-mailto",
        "esp-pass-dmarc-fail",
        "one-click-not-covered",
        "dkim-fail",
        "mailto-covered",
        "mailto-uncovered",
        "https-only-covered",
        "http-plain-link",
        "notice-no-header",
        "relay-no-header",
        "transactional-one-click",
        "bulk-body-link-only",
        "phishing-lookalike",
        "personal-one-to-one",
        "same-sender-list-a",
        "same-sender-list-b",
        "same-sender-no-list",
        "receipt-same-sender-no-lu",
        "spoofed-personal",
        "hostile-headers",
        "hostile-long-preview",
        "remote-images",
        "forged-lower-ar",
    ]
}

#[test]
fn corpus_loads_every_case() -> Result<(), String> {
    let c = load()?;
    if c.cases.len() < 22 {
        return Err(format!("expected at least 22 cases, got {}", c.cases.len()));
    }
    for cs in &c.cases {
        if cs.spec.id.is_empty() || cs.eml.is_empty() || cs.headers.is_empty() {
            return Err(format!("case {}: empty field", cs.spec.id));
        }
    }
    Ok(())
}

#[test]
fn corpus_uses_reserved_domains_only() -> Result<(), String> {
    let c = load()?;
    for cs in &c.cases {
        let from_addr = &cs.spec.from_address;
        if !has_reserved_domain(from_addr) {
            return Err(format!(
                "case {}: non-reserved from_address {from_addr}",
                cs.spec.id
            ));
        }
        if let Some(r) = &cs.spec.reply_to {
            if !has_reserved_domain(r) {
                return Err(format!("case {}: non-reserved reply_to {r}", cs.spec.id));
            }
        }
        if let Some(d) = &cs.spec.dkim_domain {
            if !reserved_domain(d) {
                return Err(format!("case {}: non-reserved dkim_domain {d}", cs.spec.id));
            }
        }
        // Unsubscribe URLs and the remote image host must be reserved.
        if let Some(lu) = &cs.spec.list_unsubscribe {
            for uri in lu.split(',').filter_map(|t| {
                let t = t.trim();
                t.strip_prefix('<').and_then(|t| t.strip_suffix('>'))
            }) {
                if uri.starts_with("http") {
                    let url = url::Url::parse(uri).unwrap_or_else(|_| panic!("{uri}: url"));
                    let host = url.host_str().unwrap_or_default().to_string();
                    if !reserved_domain(&host) {
                        return Err(format!("case {}: non-reserved URL host {host}", cs.spec.id));
                    }
                }
            }
        }
        if cs.spec.remote_images && !reserved_domain("img.example.com") {
            return Err(format!("case {}: img host not reserved", cs.spec.id));
        }
    }
    Ok(())
}

#[test]
fn corpus_every_case_has_display_subject_and_body_canaries() -> Result<(), String> {
    let c = load()?;
    for cs in &c.cases {
        // The manifest must itself be canary-free (canaries are added by the
        // builder), so a leak test can rely on them being present in output.
        for (field, value) in [
            ("display", &cs.spec.from_display),
            ("subject", &cs.spec.subject),
            ("body", &cs.spec.body_text),
        ] {
            if value.contains("CANARY-") {
                return Err(format!(
                    "case {}: manifest must be canary-free ({field})",
                    cs.spec.id
                ));
            }
        }
        // The seed carries all three canaries: display and subject as plaintext
        // (an adapter would see them decoded), body in the preview text. The raw
        // `.eml` may hide a non-ASCII subject or a base64 multipart body, so we
        // assert on the decoded seed rather than raw bytes.
        let seed = cs.seed_message();
        let display = format!("CANARY-{}-display", cs.spec.id);
        let subject = format!("CANARY-{}-subject", cs.spec.id);
        let body = format!("CANARY-{}-body", cs.spec.id);
        if !seed.from_display.contains(&display) {
            return Err(format!("case {}: display canary missing", cs.spec.id));
        }
        if !seed.subject.contains(&subject) {
            return Err(format!("case {}: subject canary missing", cs.spec.id));
        }
        if seed.preview_text != body {
            return Err(format!("case {}: body canary missing", cs.spec.id));
        }
        // And the api-level canary accessor agrees.
        if cs.canaries().len() != 3 || !cs.canaries().iter().any(|c| c.contains("CANARY-")) {
            return Err(format!("case {}: canary count", cs.spec.id));
        }
    }
    Ok(())
}

#[test]
fn corpus_covers_every_s10_required_case() -> Result<(), String> {
    let c = load()?;
    let known: Vec<String> = c.cases.iter().map(|cs| cs.spec.id.clone()).collect();
    for req in required_ids() {
        if !known.iter().any(|k| k == req) {
            return Err(format!("missing required case {req}"));
        }
    }
    Ok(())
}

#[test]
fn corpus_eml_parses_back() -> Result<(), String> {
    let c = load()?;
    for cs in &c.cases {
        let text = String::from_utf8_lossy(&cs.eml);
        let sep = text
            .find("\r\n\r\n")
            .ok_or_else(|| format!("case {}: no header/body separator", cs.spec.id))?;
        let (header_block, _body) = text.split_at(sep);
        let unfolded = unfold(header_block);
        if unfolded.len() < 6 {
            return Err(format!(
                "case {}: only {} header lines",
                cs.spec.id,
                unfolded.len()
            ));
        }
        // Subject decodes (encoded words) back to spec subject + canary.
        let subject = unfolded
            .iter()
            .find(|l| l.to_ascii_lowercase().starts_with("subject:"))
            .cloned()
            .ok_or_else(|| format!("case {}: no subject", cs.spec.id))?;
        let expected_subject = format!("{} CANARY-{}-subject", cs.spec.subject, cs.spec.id);
        let decoded = decode_header_value(subject[8..].trim());
        if decoded != expected_subject {
            return Err(format!("case {}: subject decode mismatch", cs.spec.id));
        }
    }
    Ok(())
}

/// Split the header block into logical (unfolded) headers, one per line.
fn unfold(header_block: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in header_block.split("\r\n") {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(line.trim_start());
            }
        } else {
            out.push(line.to_owned());
        }
    }
    out
}

#[test]
fn corpus_hostile_header_block_is_about_one_megabyte() -> Result<(), String> {
    let c = load()?;
    let h = c
        .case("hostile-headers")
        .ok_or_else(|| "hostile-headers case".to_owned())?;
    let text = String::from_utf8_lossy(&h.eml);
    let header_block = text.split("\r\n\r\n").next().unwrap_or("");
    if header_block.len() < 1_040_000 {
        return Err(format!(
            "hostile header block is {} bytes, expected ~1 MB",
            header_block.len()
        ));
    }
    Ok(())
}

#[test]
fn corpus_dkim_h_tag_matches_scenario() -> Result<(), String> {
    let c = load()?;
    let both = c
        .case("one-click-covered")
        .ok_or_else(|| "one-click-covered".to_owned())?;
    if !dkim_h_list(&both.eml).contains("list-unsubscribe:list-unsubscribe-post") {
        return Err("one-click-covered h= wrong".into());
    }
    let mailto = c
        .case("mailto-covered")
        .ok_or_else(|| "mailto-covered".to_owned())?;
    let mh = dkim_h_list(&mailto.eml);
    if !mh.contains("list-unsubscribe") || mh.contains("list-unsubscribe-post") {
        return Err("mailto-covered h= wrong".into());
    }
    let uncovered = c
        .case("one-click-not-covered")
        .ok_or_else(|| "one-click-not-covered".to_owned())?;
    if dkim_h_list(&uncovered.eml) != "from:to:subject:date" {
        return Err("one-click-not-covered h= wrong".into());
    }
    Ok(())
}

#[test]
fn corpus_seed_message_facts_follow_expected_method() -> Result<(), String> {
    let c = load()?;
    let one = c
        .case("one-click-covered")
        .ok_or_else(|| "one-click-covered".to_owned())?;
    let seed = one.seed_message();
    if seed.facts.list_unsubscribe.is_none()
        || !seed.facts.list_unsubscribe_present
        || !seed.facts.from_authenticated
    {
        return Err("one-click-covered facts wrong".into());
    }

    let forged = c
        .case("forged-lower-ar")
        .ok_or_else(|| "forged-lower-ar".to_owned())?;
    let seed = forged.seed_message();
    if seed.facts.list_unsubscribe.is_some() || seed.facts.from_authenticated {
        return Err("forged-lower-ar facts wrong".into());
    }

    let personal = c
        .case("personal-one-to-one")
        .ok_or_else(|| "personal-one-to-one".to_owned())?;
    let seed = personal.seed_message();
    if seed.facts.list_unsubscribe.is_some() || seed.facts.list_unsubscribe_present {
        return Err("personal facts wrong".into());
    }

    let mailto = c
        .case("mailto-covered")
        .ok_or_else(|| "mailto-covered".to_owned())?;
    let seed = mailto.seed_message();
    if seed
        .facts
        .list_unsubscribe
        .unwrap_or_default()
        .mailto
        .is_none()
    {
        return Err("mailto-covered mailto missing".into());
    }
    Ok(())
}

fn has_reserved_domain(addr: &str) -> bool {
    let domain = addr.rsplit('@').next().unwrap_or("");
    reserved_domain(domain)
}

fn reserved_domain(d: &str) -> bool {
    let d = d.to_lowercase();
    RESERVED_DOMAIN_SUFFIXES
        .iter()
        .any(|s| d == *s || d.ends_with(&format!(".{s}")))
}

fn dkim_h_list(eml: &[u8]) -> String {
    let text = String::from_utf8_lossy(eml);
    let header_block = text.split("\r\n\r\n").next().unwrap_or("");
    let unfolded = unfold(header_block);
    for line in &unfolded {
        if line.to_ascii_lowercase().starts_with("dkim-signature:") {
            let v = &line["DKIM-Signature:".len()..];
            for piece in v.split(';') {
                let p = piece.trim();
                if let Some(h) = p.strip_prefix("h=") {
                    return h.to_owned();
                }
            }
        }
    }
    String::new()
}

/// Decode an RFC 2047 encoded-word header value.
fn decode_header_value(v: &str) -> String {
    let mut out = String::new();
    let mut rest = v;
    while let Some(start) = rest.find("=?") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some(end) = after.find("?=") {
            let word = &after[..end + 2];
            let parts: Vec<&str> = word[2..word.len() - 2].split('?').collect();
            if parts.len() == 3 && parts[1].eq_ignore_ascii_case("b") {
                if let Ok(dec) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(parts[2]) {
                    out.push_str(&String::from_utf8_lossy(&dec));
                }
            }
            rest = &after[end + 2..];
        } else {
            out.push_str(after);
            break;
        }
    }
    out.push_str(rest);
    out
}
