use api::classify::{
    input::build_input,
    prompt::{render_model_text, BULK_QUESTION, CLASS_OPTIONS, CLASS_QUESTION, QUESTION_VERSION},
};
use domain::redact::{word_count, TextTokensBucket, INPUT_VERSION};
use domain::{LabelSet, MailboxId, MessageId, MessageMeta, SenderKey};
use obs::Sensitive;
use ports::{MailProvider, MailboxCtx};
use testkit::{corpus::load, mailbox::FakeMailbox};

fn meta(case: &testkit::corpus::CorpusCase) -> Result<MessageMeta, Box<dyn std::error::Error>> {
    let seed = case.seed_message();
    Ok(MessageMeta {
        mailbox: MailboxId(uuid::Uuid::nil()),
        id: MessageId::new(format!("message-identifier-{}", case.spec.id))?,
        internal_date: seed.internal_date,
        sender: SenderKey::from_address(&seed.from_address),
        from_address: seed.from_address,
        from_display: seed.from_display,
        subject: seed.subject,
        labels: LabelSet::from_ids(seed.labels),
        facts: seed.facts,
    })
}

#[tokio::test]
async fn exp_2_input_redaction() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load()?;
    assert!(!corpus.cases.is_empty());
    let fake = FakeMailbox::new();
    let ctx = MailboxCtx {
        mailbox: MailboxId(uuid::Uuid::nil()),
        access_token: Sensitive::new("test-token".to_owned()),
    };
    let addresses = corpus_addresses(&corpus.cases);
    for case in &corpus.cases {
        let mut metadata = meta(case)?;
        metadata
            .from_display
            .push_str(" display@example.com https://example.com/display 654321");
        metadata
            .subject
            .push_str(" subject@example.com www.example.com/subject 654321");
        metadata.facts.list_id =
            Some("list@example.com https://example.com/list 654321".to_owned());
        let mut seed = case.seed_message();
        seed.preview_text = format!(
            "CANARY-{}-body {} {} {} receiver@example.com 654321 https://example.com/private {}",
            case.spec.id,
            seed.preview_text,
            case.spec.body_text,
            addresses.join(" "),
            "word ".repeat(600)
        );
        let id = fake.seed(&ctx.mailbox, seed);
        // Exercise the MailProvider boundary before minimising the service input.
        metadata.id = fake.get_meta(&ctx, &id).await?.id;
        let body = fake
            .get_text(&ctx, &id, domain::redact::MODEL_TEXT_FETCH_CHARS)
            .await?;
        let (input, _) = build_input(&metadata, &body);
        let rendered = Sensitive::new(render_model_text(&input));
        let output = rendered.expose();
        for address in &addresses {
            assert!(
                !output.contains(address),
                "case {} leaked a corpus address",
                case.spec.id
            );
        }
        for needle in [
            "http://",
            "https://",
            "www.",
            "mailto:",
            "654321",
            "receiver@example.com",
            metadata.id.as_str(),
        ] {
            assert!(
                !output.contains(needle),
                "case {} leaked a forbidden field",
                case.spec.id
            );
        }
        for (header, value) in &case.headers {
            if matches!(
                header.to_ascii_lowercase().as_str(),
                "to" | "cc" | "bcc" | "list-unsubscribe"
            ) && !value.is_empty()
            {
                assert!(
                    !output.contains(value),
                    "case {} leaked a header",
                    case.spec.id
                );
            }
        }
        assert!(word_count(&input.text) <= 500);
        assert!(input.text.chars().count() <= 3000);
        assert!(
            output.contains("[url]") && output.contains("[email]") && output.contains("[number]")
        );
        assert_eq!(input.input_version, INPUT_VERSION);
        // Preserve the real List-Id and unsubscribe facts in a second rendering:
        // planting hostile fields above must not hide a corpus-specific leak.
        let original = fake.get_meta(&ctx, &id).await?;
        let (original_input, _) = build_input(&original, &body);
        let original_rendered = Sensitive::new(render_model_text(&original_input));
        for (header, value) in &case.headers {
            if matches!(
                header.to_ascii_lowercase().as_str(),
                "to" | "cc" | "bcc" | "list-unsubscribe"
            ) && !value.is_empty()
            {
                assert!(!original_rendered.expose().contains(value));
            }
        }
        assert!(!original_rendered.expose().contains(original.id.as_str()));
    }
    Ok(())
}

fn corpus_addresses(cases: &[testkit::corpus::CorpusCase]) -> Vec<&str> {
    cases
        .iter()
        .flat_map(|case| {
            std::iter::once(case.spec.from_address.as_str())
                .chain(case.spec.reply_to.as_deref())
                .chain(case.headers.iter().flat_map(|(_, value)| {
                    value
                        .split(|c: char| c.is_whitespace() || "<>\"(),:;".contains(c))
                        .filter(|part| {
                            part.split_once('@').is_some_and(|(local, domain)| {
                                !local.is_empty()
                                    && local
                                        .chars()
                                        .all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
                                    && domain.contains('.')
                                    && domain
                                        .chars()
                                        .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
                            })
                        })
                }))
        })
        .collect()
}

#[test]
fn exp_2_canary_in_subject_and_body_allowed_but_addresses_not(
) -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load()?;
    for case in &corpus.cases {
        let metadata = meta(case)?;
        let body = format!("CANARY-{}-body CANARY-address@example.com", case.spec.id);
        let (input, _) = build_input(&metadata, &body);
        let text = Sensitive::new(render_model_text(&input));
        assert!(text
            .expose()
            .contains(&format!("CANARY-{}-subject", case.spec.id)));
        assert!(text
            .expose()
            .contains(&format!("CANARY-{}-body", case.spec.id)));
        assert!(!text.expose().contains("CANARY-address@example.com"));
        assert!(text.expose().contains("[email]"));
    }
    Ok(())
}

#[test]
fn cl_03_ac1_render_is_identical_for_both_models() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load()?;
    for case in &corpus.cases {
        let (input, _) = build_input(&meta(case)?, &case.spec.body_text);
        let gemini = Sensitive::new(render_model_text(&input));
        let jev = Sensitive::new(render_model_text(&input));
        assert_eq!(gemini.expose().as_bytes(), jev.expose().as_bytes());
    }
    assert_eq!(QUESTION_VERSION, "1");
    assert_eq!(CLASS_QUESTION, "Which kind of email is this?");
    assert_eq!(BULK_QUESTION, "How likely is it that this email was sent in bulk to many people, from 0 (certainly one-to-one) to 100 (certainly bulk)?");
    assert_eq!(
        CLASS_OPTIONS.map(|(class, _)| class),
        ["list", "bulk_no_header", "notice", "personal", "suspect"]
    );
    Ok(())
}

#[test]
fn asvs_v14_2_3_only_allowlisted_fields_rendered() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load()?;
    let case = corpus.cases.first().ok_or("empty corpus")?;
    let mut metadata = meta(case)?;
    metadata.from_display = String::new();
    metadata.subject = String::new();
    metadata.facts = domain::HeaderFacts::default();
    let (input, _) = build_input(&metadata, "");
    let rendered = Sensitive::new(render_model_text(&input));
    let fields: Vec<_> = rendered
        .expose()
        .lines()
        .filter_map(|line| line.split_once(": ").map(|(name, _)| name))
        .collect();
    assert_eq!(
        fields,
        [
            "from_name",
            "from_domain",
            "list_id",
            "list_unsubscribe",
            "list_unsubscribe_post",
            "precedence",
            "auto_submitted",
            "esp",
            "authentication",
            "subject",
            "text"
        ]
    );
    assert_eq!(rendered.expose(), &format!("from_name: none\nfrom_domain: {}\nlist_id: none\nlist_unsubscribe: absent\nlist_unsubscribe_post: absent\nprecedence: none\nauto_submitted: none\nesp: none\nauthentication: from_authenticated=fail\nsubject: none\ntext: none", domain::redact::from_domain(&metadata.from_address)));
    Ok(())
}

#[test]
fn exp_2_from_domain_empty_for_hostile_from() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load()?;
    let mut metadata = meta(corpus.cases.first().ok_or("empty corpus")?)?;
    // A hostile From carrying a second address, whitespace and a second domain.
    metadata.from_address = "a@evil.example.com @other.com".to_owned();
    let (input, _) = build_input(&metadata, "");
    let rendered = Sensitive::new(render_model_text(&input));
    let rendered = rendered.expose();
    // The literal rendered line, not a string built from `from_domain(...)`
    // (which would make the assertion tautological): the domain feature is
    // empty for a hostile sender.
    let domain_line = rendered
        .lines()
        .find(|line| line.starts_with("from_domain: "))
        .ok_or("missing from_domain line")?;
    assert_eq!(domain_line, "from_domain: none");
    for leaked in ["evil.example.com", "other.com"] {
        assert!(!rendered.contains(leaked), "hostile From leaked {leaked}");
    }
    Ok(())
}

#[test]
fn exp_2_field_caps_and_facts_before_truncation() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load()?;
    let mut metadata = meta(corpus.cases.first().ok_or("empty corpus")?)?;
    metadata.from_display = "界".repeat(101);
    metadata.subject = "界".repeat(301);
    metadata.facts.list_id = Some("界".repeat(201));
    metadata.facts.precedence_bulk = true;
    metadata.facts.auto_submitted = true;
    metadata.facts.esp_hint = Some("mailchimp".to_owned());
    metadata.facts.from_authenticated = true;
    metadata.facts.list_unsubscribe_present = true;
    metadata.facts.list_unsubscribe = Some(domain::UnsubscribeOptions {
        one_click_https: Some(url::Url::parse("https://example.com/secret")?),
        ..Default::default()
    });
    let (input, facts) = build_input(&metadata, &"a".repeat(4000));
    assert_eq!(input.from_display.chars().count(), 100);
    assert_eq!(input.subject.chars().count(), 300);
    assert_eq!(
        input
            .list_id
            .as_deref()
            .ok_or("missing list id")?
            .chars()
            .count(),
        200
    );
    assert_eq!(input.text.chars().count(), 3000);
    assert_eq!(facts.text_tokens_bucket, TextTokensBucket::Under100);
    let (_, facts) = build_input(&metadata, &"word ".repeat(700));
    assert_eq!(facts.text_tokens_bucket, TextTokensBucket::Over300);
    let rendered = Sensitive::new(render_model_text(&input));
    for expected in [
        "list_unsubscribe: present",
        "list_unsubscribe_post: one-click",
        "precedence: bulk",
        "auto_submitted: auto-generated",
        "esp: mailchimp",
        "authentication: from_authenticated=pass",
    ] {
        assert!(rendered.expose().contains(expected));
    }
    Ok(())
}

#[tokio::test]
async fn get_text_matches_preview_at_300() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = load()?;
    let fake = FakeMailbox::new();
    let ctx = MailboxCtx {
        mailbox: MailboxId(uuid::Uuid::nil()),
        access_token: Sensitive::new("test-token".to_owned()),
    };
    let mut seed = corpus.cases.first().ok_or("empty corpus")?.seed_message();
    seed.preview_text = "word e\u{301} 界 ".repeat(600);
    let id = fake.seed(&ctx.mailbox, seed);
    assert_eq!(
        fake.get_preview(&ctx, &id).await?,
        fake.get_text(&ctx, &id, 300).await?
    );
    let text = fake.get_text(&ctx, &id, 4000).await?;
    assert!(text.chars().count() > 300);
    assert_eq!(
        domain::text::sanitise_plain(&text, 300),
        fake.get_preview(&ctx, &id).await?
    );
    Ok(())
}
