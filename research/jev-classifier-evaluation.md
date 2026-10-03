# Mail Tinder: Jev (TypeSafe AI) as a Cheap Classifier

Last updated: 3 October 2026
Status: research only. Nothing here is decided until James confirms it.

## Recommendation

Do not use Jev in v1. Keep it on the shortlist for the v2 marketing versus transactional batch job, and test it in the validation spike on James's own labelled mail.

Jev is cheap and fast, and its decision-only output suits our questions well. It falls short on paperwork. It is a hosted service with no self-hosting and no browser build, so every call sends mail content to a new third-party processor. That cuts against the v1 decision in `data-handling-and-in-account-ai.md` (no server-side LLM in v1; Gemini Nano in the browser, Vertex AI only for opt-in v2 edge cases). The vendor launched three weeks ago, publishes no SOC 2 or ISO 27001, offers zero data retention (ZDR) only to enterprise customers, and its accuracy evidence is mostly vendor-reported.

If v2 needs a hosted classifier, Jev is worth a bake-off against Gemini on Vertex AI, provided TypeSafe can supply a DPA, a subprocessor list, ZDR and a security attestation by then.

## What Jev is

- A "System One model" from TypeSafe AI, a San Francisco startup that came out of stealth on 15 September 2026. Current version is Jev 1.13 (`jev-1.13.0`).
- It does not generate text. You send a `state` (text or JSON) and named questions; it returns typed answers with probabilities and a confidence score. Three question types:
  - **Noul:** yes/no, returns a probability.
  - **Choice:** pick one of up to 255 options, returns the pick plus the full distribution.
  - **Score:** rate against an ordered rubric.
- Several questions run in parallel in one call, so "is this bulk?", "marketing or transactional?" and "which of these 12 categories?" cost one request.
- Text only, English strongest. Context: 64k tokens per request, 32k for state plus the longest question.

## Deployment

| Option | Available? | Notes |
| --- | --- | --- |
| Hosted API (`api.typesafe.ai/v1/systemone`) | Yes, early access | Hosted in the United States per the privacy policy. No region choice documented. |
| Cloudflare Workers AI (`typesafe/jev`) | Yes | Listed as a third-party model with zero data retention. Cloudflare appears to proxy to TypeSafe rather than run it; adds Cloudflare as a second processor. No region pinning documented. |
| Self-hosted, Cloud Run in Sydney | No | Proprietary; no weights, container or on-premises option found. |
| In the browser | No | No client-side or WASM build. |
| SDKs | Python and JavaScript | No Rust SDK; a plain HTTP client from the Rust backend would do. |

Rate limits: 100k tokens per second, 80 requests per second, "can change without notice".

## Cost and latency

Price: USD 0.042 per million input tokens; output free. (An unofficial site lists USD 0.25 to 0.42 per million; the official models page and Cloudflare both say 0.042.)

At our assumed 700 to 1,000 input tokens per email (200 to 500 header tokens plus about 500 stripped text):

| Scale | Messages per month | Jev | Claude Haiku 4.5 on every message (from the existing doc) |
| --- | --- | --- | --- |
| Per message | 1 | about USD 0.00003 to 0.00004 | about USD 0.00065 |
| James | 3,000 | about USD 0.13 | about USD 2 |
| Trial (20 users) | 60,000 | about USD 2.50 | about USD 39 |
| 100,000 users | 300 million | about USD 9,000 to 13,000 | about USD 195,000 |

That is roughly 15 to 20 times cheaper than Haiku and comparable to GPT-5 nano or Gemini Flash-Lite per token, with no output charge. TypeSafe concedes it cannot prove the price is not subsidised.

Latency: vendor and secondary sources quote 70 to 500 ms end to end, about 0.1 s of server time. Calls from Sydney to a US endpoint add roughly 150 to 200 ms of round trip (inferred, not measured). Fine for ingest and batch work; marginal for the under 200 ms up-swipe filing budget.

## Accuracy evidence

- **Vendor benchmark:** 67.8% agreement on TypeSafe's own four-workflow suite, level with GPT-5.6 Terra and about 5 to 6 points behind Claude Opus 5 and GPT-5.6 Sol. Not independently reproduced.
- **Independent review (xbill, 24 September 2026)** pulling together preprints and public repos: Jev at 72.5% on a mixed suite, behind frontier models by 6.5 to 11.5 points and level with mid-price LLMs.
  - **Spam:** 98.33% on 18,514 emails, matching a TF-IDF baseline, and it held up better on newer mail.
  - **Phishing:** 62.6% on a dedicated benchmark, against 97.4% for a fine-tuned model.
  - Best-calibrated of the models tested (median ECE 0.071); fitting a temperature on 50 to 300 labels cut calibration error by 74%.
  - Swapping option names changed 32.5% of answers. Non-English text lost 3 to 11 points.
- **TypeSafe's own "jaggedness" page** lists weak spots: literal reading, dates, irrelevant context in the state, adversarial content and option-order bias.

What this means for us: marketing senders control the email content, and some disguise promotions as receipts. Weakness on adversarial content and phishing is the wrong profile for a "safe to trash" decision. Long stripped bodies also hurt, since accuracy falls as unrelated context grows. Bulk versus personal looks strong, but the header rules already do that for free.

## Data handling

| Question | Answer | Source |
| --- | --- | --- |
| Training on inputs | No. "We will not train or fine tune any artificial intelligence or machine learning models on your prompts or other Input." | Privacy policy, models page |
| Retention | Not specified for API inputs. General wording: "as long as reasonably necessary ... or otherwise in support of our business or commercial purposes." | Privacy policy |
| ZDR | Enterprise customers only, or through Cloudflare Workers AI | Legal page, Cloudflare model page |
| Where data goes | United States. No subprocessor list found. | Privacy policy |
| DPA | Referenced on the legal docs page; the document itself was not reachable (404) | docs.typesafe.ai/legal |
| Certifications | None found (no SOC 2, no ISO 27001) | Privacy policy, docs |

## Fit with our constraints

- **Google Workspace API policy.** Jev does not train on inputs, so it is not the banned "generalised model" transfer in the narrow sense. It is still a transfer of restricted Gmail data to a third party, so Limited Use applies: user-facing feature only, explicit consent, named in the privacy policy, and in scope for the CASA assessment. Same position as any hosted LLM in layer 3.
- **Data minimisation.** Worse than the current plan. Header rules and Gemini Nano keep content in our backend or on the user's device. Jev would send headers plus 500 tokens of body for every message to a US processor. Headers alone would cut exposure but lose most of Jev's value over the rules.
- **ASVS L2.** Workable but adds work: a new external dependency to threat model, an outbound secret to store and rotate, untrusted output to validate (its typed output makes that easy), and data protection requirements on a processor with no attestation and an unstated retention period. CASA reviewers will ask for the DPA and retention terms we cannot currently produce.
- **Data residency.** Not a deciding factor. Consumer Gmail is not stored in Australia either (James, 3 October 2026), so a US endpoint adds no new residency exposure. What matters is the processor's terms.
- **Vendor risk.** Three weeks old, early access behind a waitlist, model aliases that change answers without notice, rate limits that change without notice. Pin `jev-1.13.0` rather than `jev-latest` if used.

## Compared with the current plan

| Option | Cost per message | Latency | Content leaves our control? | v1 fit |
| --- | --- | --- | --- | --- |
| Header rules | about 0 | under 1 ms | No | Yes (bulk score, unsubscribe eligibility) |
| Sender history plus Gemini Nano in Chrome | about 0 | on device | No | Yes (current v1 plan) |
| Jev, hosted | about USD 0.00003 to 0.00004 | 0.1 to 0.5 s plus Sydney to US round trip | Yes, to a US startup, no attestation | No |
| Gemini on Vertex AI (v2, opt-in) | higher per token, see existing doc | 0.5 to 2 s | Yes, to Google as our processor, ZDR possible, Sydney region available | v2 only |

Where Jev would earn its place: the v2 marketing versus transactional batch job, run with consent over headers plus a short snippet, if it beats Vertex AI on accuracy in our spike and TypeSafe can close the paperwork gap.

## Next steps if James wants to keep it on the list

1. Add Jev to the validation spike in `cheap-email-classification.md`: run James's own 500 labelled messages through it (his mail, his consent), using Choice questions with options in a fixed, neutral order. Compare with header rules and Gemini.
2. Ask TypeSafe (privacy@typesafe.ai) for the DPA, subprocessor list, API input retention period, ZDR terms for small customers, a security attestation and any non-US region plans.
3. Revisit at v2 planning.

## Sources

- TypeSafe AI home page (claims, pricing): https://typesafe.ai
- TypeSafe docs index: https://docs.typesafe.ai/llms.txt
- Models page (pricing, limits, context, language, training): https://docs.typesafe.ai/models.md
- API reference: https://docs.typesafe.ai/api.md
- Jev 1.13 jaggedness (failure modes): https://docs.typesafe.ai/model-jaggedness/jev-1.13.md
- Legal page (DPA, ZDR for enterprise): https://docs.typesafe.ai/legal.md
- Privacy policy (US hosting, no training, retention): https://typesafe.ai/legal/privacy-policy
- Cloudflare Workers AI model page (third-party, zero data retention, pricing): https://developers.cloudflare.com/ai/models/typesafe/jev/
- DataCamp explainer (launch date, vendor benchmark, limits): https://www.datacamp.com/blog/system-one-models-jev
- Independent evidence review, 24 September 2026: https://dev.to/gde/jev-after-eight-days-of-independent-tests-level-with-mid-price-llms-behind-the-frontier-1kln
- Funding and cost claim scrutiny: https://www.remio.ai/post/typesafe-ai-jev-funding-puts-a-445-cost-claim-under-scrutiny
- Unofficial third-party site (not affiliated with TypeSafe; ignore its pricing): https://jevtypesafeai.com/
- Existing project research: `research/cheap-email-classification.md`, `research/data-handling-and-in-account-ai.md`
