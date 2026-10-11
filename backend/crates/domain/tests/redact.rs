use domain::redact::{
    approx_tokens, from_domain, is_english, redact, text_tokens_bucket, truncate_words, word_count,
    TextTokensBucket, MAX_TEXT_CHARS, MAX_TEXT_WORDS,
};
use proptest::prelude::*;

fn redaction_available() -> Result<(), Box<dyn std::error::Error>> {
    if redact("mailto:test@example.com") == "[url]" {
        Ok(())
    } else {
        Err("redaction patterns unavailable".into())
    }
}

#[test]
fn exp_2_redact_table() -> Result<(), Box<dyn std::error::Error>> {
    redaction_available()?;
    for (input, expected) in [
        ("Visit https://example.com/a?b=1 now", "Visit [url] now"),
        ("see www.example.org/x.", "see [url]"),
        ("go to example.net/unsubscribe today", "go to [url] today"),
        ("mailto:list@example.com", "[url]"),
        ("Write to jo.bloggs+news@example.co.uk please", "Write to [email] please"),
        ("Call 0412 345 678", "Call [number]"),
        ("Ref 123456", "Ref [number]"),
        ("Order 12345", "Order 12345"),
        ("Card 4111-1111-1111-1111", "Card [number]"),
        ("a\u{202e}b\u{200b}c", "abc"),
        ("line1\nline2\tx", "line1 line2 x"),
        ("HTTPS://user@example.com/x FTP://example.com/x javascript:secret data:secret file:/secret tel:123456", "[url] [url] [url] [url] [url] [url]"),
        ("\u{0}\u{7f}\u{85}\u{61c}\u{2064}\u{2069}\u{feff}x", "x"),
        ("2026-10-03 123.456", "[number]"),
        ("1  2\t3\n4\u{a0}5 6", "[number]"),
        ("https://example.com/x user@example.com 123456 [url] [email] [number]", "[url] [email] [number] [url] [email] [number]"),
        (r#""a b"@x.example"#, "[email]"),
        ("a@[192.0.2.1]", "[email]"),
        ("a@localhost", "[email]"),
        (r#"Mail "x y"@z.test now"#, "Mail [email] now"),
        (r#"a@"b"#, "[email]"),
        (r#""@x"#, "[email]"),
        ("a@<b>", "[email]"),
        ("a@@b", "[email]"),
    ] {
        assert_eq!(redact(input), expected);
    }
    let long_quoted = format!("\"{}\"@x.example", "a".repeat(65));
    assert_eq!(redact(&long_quoted), "[email]");
    Ok(())
}

proptest! {
    #[test]
    fn exp_2_redact_idempotent(s in any::<String>()) {
        let once = redact(&s);
        prop_assert_eq!(redact(&once), once);
    }

    #[test]
    fn exp_2_redact_idempotent_spaced_numbers(
        digits in prop::collection::vec(0u8..10, 6..40),
        separator in "[ \\t\\n\\u{00a0}]{1,5}"
    ) {
        let input = digits.iter().map(u8::to_string).collect::<Vec<_>>().join(&separator);
        let once = redact(&input);
        prop_assert_eq!(&once, "[number]");
        prop_assert_eq!(redact(&once), once);
    }

    #[test]
    fn exp_2_redact_never_leaves_at_sign_address(
        prefix in any::<String>(), suffix in any::<String>(),
        local in "[A-Za-z0-9._%+-]{1,40}", host in "[a-z]{1,20}", tld in "[a-z]{2,8}"
    ) {
        let address = format!("{local}@{host}.{tld}");
        let input = format!("{prefix} {address} {suffix}");
        prop_assert!(!redact(&input).contains(&address));
    }

    #[test]
    fn exp_2_redact_no_at_sign_left_in_adjacent_text(s in any::<String>()) {
        // The generator is unconstrained; the invariant is on the output: no
        // `@` survives flanked by non-whitespace characters, whatever delimits
        // the address (`a@"b`, `"@x`, `a@<b>`, `a@@b`, `a@localhost`).
        let out = redact(&s);
        let chars: Vec<char> = out.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c != '@' {
                continue;
            }
            let left = i.checked_sub(1).map(|j| chars[j]);
            let right = chars.get(i + 1).copied();
            let flanked = left.is_some_and(|c| !c.is_whitespace())
                && right.is_some_and(|c| !c.is_whitespace());
            prop_assert!(!flanked, "at-sign survived redaction flanked by text: {:?}", out);
        }
    }
}

#[test]
fn exp_2_hostile_input_bounded() -> Result<(), Box<dyn std::error::Error>> {
    redaction_available()?;
    for input in [
        "word \u{202e}\u{200b}".repeat(100_000),
        "界".repeat(350_000),
    ] {
        let text = truncate_words(&redact(&input));
        assert!(word_count(&text) <= MAX_TEXT_WORDS);
        assert!(text.chars().count() <= MAX_TEXT_CHARS);
        assert!(!text.contains(['\u{202e}', '\u{200b}']));
    }
    assert_eq!(truncate_words("  one\t two\nthree  "), "one two three");
    Ok(())
}

#[test]
fn text_tokens_bucket_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    redaction_available()?;
    for (words, bucket) in [
        (99, TextTokensBucket::Under100),
        (100, TextTokensBucket::T100To300),
        (300, TextTokensBucket::T100To300),
        (301, TextTokensBucket::Over300),
    ] {
        assert_eq!(text_tokens_bucket(words), bucket);
    }
    Ok(())
}

#[test]
fn approx_tokens_rounds_up() -> Result<(), Box<dyn std::error::Error>> {
    redaction_available()?;
    for (input, tokens) in [
        ("", 0),
        ("x", 1),
        ("12345678", 2),
        ("123456789", 3),
        ("界界界界界", 2),
    ] {
        assert_eq!(approx_tokens(input), tokens);
    }
    Ok(())
}

#[test]
fn is_english_cases() -> Result<(), Box<dyn std::error::Error>> {
    redaction_available()?;
    assert!(is_english("This is a message written in English. The weather has been pleasant this week and we are planning to visit the park with our friends tomorrow afternoon."));
    assert!(!is_english("Dies ist eine Nachricht auf Deutsch. Das Wetter ist diese Woche angenehm gewesen und wir planen morgen Nachmittag mit unseren Freunden den Park zu besuchen."));
    assert!(!is_english(""));
    Ok(())
}

#[test]
fn exp_2_from_domain_only() -> Result<(), Box<dyn std::error::Error>> {
    redaction_available()?;
    for (address, domain) in [
        ("local@EXAMPLE.COM", "example.com"),
        ("no-address", ""),
        ("a@evil.example.com @other.com", ""),
        ("x@[1.2.3.4]", ""),
        ("a@-bad.example", ""),
        ("a@bad..example", ""),
        ("a@nodot", ""),
        ("\"\"@x.example y", ""),
        ("local@news.example.co.uk", "news.example.co.uk"),
    ] {
        assert_eq!(from_domain(address), domain, "from_domain({address:?})");
    }
    Ok(())
}
