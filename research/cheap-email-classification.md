# Mail Tinder: Cheap Email Classification

Last updated: 3 October 2026
Status: research recommendation for version 1. Nothing here is decided until James confirms it.

## Recommendation

Build a three-layer cascade in the Rust backend and keep large language models (LLMs) off the hot path.

1. **Header rules (free, deterministic).** Score every message for "bulk likelihood" from standard mail headers and provider labels. This drives the card score and decides whether a left swipe can unsubscribe at all.
2. **Local embeddings plus per-user nearest neighbour (near free, private).** Embed sender, subject and snippet with a small open model running inside the backend. Suggest filing categories and detect keep patterns by comparing against the user's own past decisions. No mail content leaves our server.
3. **Small hosted LLM, by exception only.** Call a cheap model for the rare cases the first two layers cannot handle (proposing a name for a new category, ambiguous marketing versus transactional calls in v2). Off by default; behind an explicit consent setting.

Classify per sender where possible and cache the result. Most mail comes from senders the user has seen before, so most messages never need layer 2 work beyond a vector lookup, and almost none reach layer 3.

At trial scale (20 users) every option costs under about AUD 60 a month, so cost does not pick the winner. Privacy, Google policy and consistency across Gmail and Outlook do. At "very high usage" the cascade is what keeps cost flat; an LLM-per-message design does not survive scale (see Cost).

## What needs classifying

| Feature | Question asked | When | Latency need | Release |
| --- | --- | --- | --- | --- |
| Card score | Is this bulk or list mail? | At ingest | None (precomputed) | v1 |
| Left swipe routing | Can we unsubscribe, or is this personal mail? | At ingest | None | v1 |
| Up-swipe filing | Which of this user's categories fits? Is this a new type? | On swipe | Under about 200 ms, feels instant | v1 |
| Keep-learning | Do repeated right swipes form a pattern? | Background | None | v1 |
| Marketing versus transactional | Safe to trash this historical message? | Batch job | None | v2 |

Two of these are generic (bulk, marketing versus transactional) and three are personal to each user (filing, new type, keep pattern). Personal tasks suit nearest neighbour over the user's own examples far better than a global model, because the categories are user-named and change over time.

## Decision for James: what the card score means

The stories call it a "spam confidence score". By the time mail reaches the inbox, Gmail and Outlook have already removed most spam. What the user can act on is "is this a mailing list I can leave". This matches recommendation A4 in `docs/plan-open-questions.md`.

Recommended: relabel the card indicator to "bulk likelihood" (or a simple List, Maybe, Personal badge) and show the reason on tap, for example "Has unsubscribe link, sent through Mailchimp". An explainable badge also guards against a careless left swipe on personal mail.

## Layer 1: header rules

### Signals available

| Signal | Source | Meaning |
| --- | --- | --- |
| `List-Unsubscribe`, `List-Unsubscribe-Post` | RFC 2369, RFC 8058 | Sender runs a list. Since 2024, Gmail and Yahoo require one-click unsubscribe from senders of about 5,000 messages a day, so large marketing senders almost always carry it. |
| `List-Id` | RFC 2919 | Mailing list or discussion list. |
| `Precedence: bulk` or `list`, `Auto-Submitted` | Convention, RFC 3834 | Automated or bulk send. |
| Sending-platform fingerprints (`Feedback-ID`, `X-Mailgun-*`, `X-SG-EID`, `X-MC-User`, Amazon SES headers) | Email service providers | Sent through a bulk platform. Transactional mail also uses these platforms, so this raises bulk likelihood without implying marketing. |
| `Authentication-Results` (SPF, DKIM, DMARC) | Receiving server | Alignment helps trust the sender domain when grouping by sender. |
| Gmail `CATEGORY_PROMOTIONS`, `CATEGORY_UPDATES`, `CATEGORY_SOCIAL`, `CATEGORY_FORUMS`, `CATEGORY_PERSONAL` | Gmail API system labels | Google's own classifier, free. Promotions is a strong marketing signal; Updates is mostly transactional. |
| Outlook `inferenceClassification` (`focused`, `other`) | Microsoft Graph message property | Coarser: "other" means less important, with no promotions versus receipts split. |
| Sender history | Our own data | Prior swipes and prior classifications for this sender. |

### Why rules first

- Zero marginal cost and sub-millisecond in Rust.
- Explainable, which the card and the Needs Attention flow both need.
- Gmail and Outlook expose different native labels. Our own header score gives one consistent meaning across providers; native labels become extra features rather than the source of truth.
- The `List-Unsubscribe` header is already parsed for the unsubscribe flow, so the work is shared.

Weak spot: rules separate bulk from personal well, but separate marketing from transactional poorly. A receipt from Apple and a promotion from Apple share sender, platform and often headers. That split is a v2 problem; layer 2 and 3 handle it.

To verify in a spike: whether Gmail applies `CATEGORY_*` labels when the user has inbox tabs turned off. I could not confirm this from Google's documentation.

## Layer 2: local embeddings and per-user nearest neighbour

### How it works

1. At ingest, build a short text: sender name and domain, subject, first 200 to 300 characters of the snippet.
2. Embed it with a small model running in the backend through the `fastembed` Rust crate (ONNX runtime). Candidates: `BAAI/bge-small-en-v1.5` (default), `sentence-transformers/all-MiniLM-L6-v2` (384 dimensions), or `intfloat/multilingual-e5-small` if non-English mail matters. Quantised variants exist.
3. Store the vector alongside the message metadata (Postgres with `pgvector` fits the managed Postgres default in the open-questions doc).
4. **Up-swipe:** find the user's nearest filed messages. Same sender filed before is the strongest prior; otherwise vote among the top neighbours. Show the best guess plus two alternates, as the stories require.
5. **New type:** if the nearest neighbour is below a similarity threshold, ask the user to name the category (story 2). Tune the threshold on James's mail.
6. **Keep-learning:** count right swipes per sender and per tight cluster. After a threshold (for example 3 keeps of similar mail), queue a passive filing suggestion.
7. **Confidence over time:** as a category gathers examples, raise its confidence and move to one-tap confirm. Never auto-file silently.

### Why this fits

- Per-user by design. Categories are the user's own, so a global model would need retraining every time someone names a category. Nearest neighbour needs no training; adding an example is an insert.
- Private. Mail content stays on our server, which avoids a third-party transfer under Google's policy.
- Fast. A small embedding model on CPU and a vector lookup over one user's few thousand filed messages are both well inside the latency budget. I have no published benchmark for this exact setup; measure it in the spike.
- Undo-friendly. Undoing a swipe deletes one example; nothing to retrain.

### Cold start

A new user has no filed examples, so the first up-swipes always ask for a category name. That is what story 2 asks for. Sender history makes it improve quickly: most filing decisions repeat per sender.

## Layer 3: hosted LLM by exception

Use cases worth an LLM call:

- Proposing a category name the first time a new type appears, so the user confirms rather than types.
- v2 marketing versus transactional calls where rules and embeddings disagree or are low confidence, before any message goes to trash.

Rules for the call:

- Send the minimum: sender, subject, snippet. Never the full body, never attachments.
- Use structured output with a fixed label set and a confidence field.
- Batch v2 work through a batch API (50% cheaper where offered; latency does not matter).
- Behind a user consent toggle, disclosed in the privacy policy.

Model choice: any small model works for this. On the Anthropic API, Claude Haiku 4.5 at USD 1 input and USD 5 output per million tokens is the cheapest current option; API inputs are not used for training under commercial terms and are deleted within 30 days by default, with zero data retention available by agreement. Gemini 2.5 Flash-Lite and GPT-5 nano are 10 to 20 times cheaper per token. Pick on data terms first, price second.

## Cost

Assumptions (inferred, to check against James's mailbox): 100 messages per user per day; an LLM call uses about 500 input tokens and 30 output tokens. Prices in USD from the sources below.

### Per message

| Approach | Cost per message | Notes |
| --- | --- | --- |
| Header rules | about 0 | CPU only |
| Local embedding plus lookup | about 0 | CPU only; no per-call fee |
| OpenAI embeddings API (for comparison) | about 0.00001 | Sends content to a third party |
| GPT-5 nano (USD 0.05 / 0.40 per M) | about 0.00004 | |
| Gemini 2.5 Flash-Lite (USD 0.10 / 0.40 per M) | about 0.00006 | |
| Claude Haiku 4.5 (USD 1 / 5 per M) | about 0.00065 | about 0.00033 through the Batch API |

### Per month

| Scale | Messages per month | Haiku 4.5 on every message | Cascade (LLM on about 2% of messages) |
| --- | --- | --- | --- |
| James | 3,000 | about USD 2 | under USD 0.10 |
| Friends trial (20 users) | 60,000 | about USD 39 | under USD 1 |
| 100,000 users | 300 million | about USD 195,000 | about USD 3,900 |

The 2% figure is an assumption for the cascade, not a measurement. Sender caching should push it lower; the spike should measure it.

The cascade also has a fixed cost: the embedding model adds memory (tens to low hundreds of megabytes) to the backend container and some CPU at ingest. At trial scale this fits in the small container host already proposed.

## Privacy and policy constraints

- **Google forbids using Workspace API data for generalised models.** Google's Workspace API policy states that transfers of data for generalised AI or ML models are prohibited, and developers must not retain user data to develop, improve or train non-personalised AI or ML models. Gmail is a Workspace API. Consequence: we cannot train one shared marketing versus transactional model on all users' Gmail content. Per-user nearest neighbour (layer 2) is personalised and fits. A shared v2 classifier would need training data from outside user mailboxes (public datasets, synthetic mail) or must rely on header rules plus an LLM that does not train on our inputs.
- **Third-party transfer needs consent and disclosure.** Google's Limited Use rules allow transfers to provide user-facing features only with the user's consent. Sending snippets to an LLM provider is such a transfer. Layer 3 must be opt-in and named in the privacy policy, and the CASA assessment (needed before production) will look at it.
- **No human reads mail** without the user's affirmative agreement for specific messages. That rules out labelling users' mail by hand to build training sets; James labelling his own mail is fine.
- **Microsoft Graph** has no equivalent AI clause, but the "treat mail as sensitive" principle in `CONTEXT.md` applies equally. Use one policy for both providers.

## Options compared

| Option | Cost | Latency | Privacy | Fit for v1 |
| --- | --- | --- | --- | --- |
| Provider labels only | Free | Instant | Best | Gmail only; Outlook too coarse. Use as a feature. |
| Header rules | Free | Instant | Best | Strong for bulk score. Weak for filing. |
| Classic ML (naive Bayes, logistic regression) per user | Free | Instant | Best | Needs retraining per category change; weaker than embeddings on short text. Not recommended. |
| Global trained model | Free to run | Instant | Policy risk | Blocked for Gmail data by Google's AI clause. |
| Local embeddings plus nearest neighbour | Near free | Fast | Best | Recommended for filing and keep-learning. |
| Hosted embeddings API | Very cheap | Network round trip | Third-party transfer | No advantage over local at our scale. |
| Small LLM on every message | Cheap now, expensive at scale | 0.5 to 2 s | Third-party transfer | Not recommended as default. |
| Small LLM by exception | Negligible | Off hot path | Opt-in transfer | Recommended for naming and v2 edge cases. |

## Validation spike before build

1. Export about 500 of James's recent messages (headers, subject, snippet) from Gmail and Outlook, kept locally, never committed.
2. James labels them: personal, transactional, marketing, plus a filing category for the ones he would file.
3. Measure header-rule precision and recall for bulk versus personal. Target: under 1% of personal mail scored as bulk, since that is what makes a left swipe dangerous.
4. Measure top-3 filing suggestion accuracy with leave-one-out nearest neighbour.
5. Time embedding and lookup on the target container size.
6. For v2 only: measure marketing versus transactional precision. Trash decisions need very high precision on the "marketing" label, because a lost tax invoice costs more than a missed promotion.

## Sources

- Gmail API, manage labels (system and category labels): https://developers.google.com/workspace/gmail/api/guides/labels
- Microsoft Graph message resource (`inferenceClassification`, `internetMessageHeaders`): https://github.com/microsoftgraph/microsoft-graph-docs-contrib/blob/main/api-reference/beta/resources/message.md
- Microsoft Graph, manage Focused Inbox: https://learn.microsoft.com/en-us/graph/api/resources/manage-focused-inbox?view=graph-rest-1.0
- Google Workspace API policy protections for generative AI: https://workspace.google.com/blog/ai-and-machine-learning/api-policy-protections
- Google API Services User Data Policy (Limited Use, human access, restricted scope assessment): https://developers.google.com/terms/api-services-user-data-policy
- Gmail and Yahoo bulk sender requirements (summary): https://www.mailgun.com/state-of-email-deliverability/chapter/yahoogle-bulk-senders/
- Google email sender guidelines: https://support.google.com/a/answer/81126?hl=en
- RFC 2369 (List-* headers): https://www.rfc-editor.org/rfc/rfc2369
- RFC 8058 (one-click unsubscribe): https://www.rfc-editor.org/rfc/rfc8058
- RFC 2919 (List-Id): https://www.rfc-editor.org/rfc/rfc2919
- RFC 3834 (Auto-Submitted): https://www.rfc-editor.org/rfc/rfc3834
- fastembed-rs (local ONNX embeddings in Rust): https://github.com/Anush008/fastembed-rs
- Gemini 2.5 Flash-Lite and GPT-5 nano pricing: https://openrouter.ai/compare/google/gemini-2.5-flash-lite/openai/gpt-5-nano
- Claude Haiku 4.5 pricing: Anthropic API pricing, cached in the Claude API reference as of 25 September 2026 (USD 1 input, USD 5 output per million tokens; Batch API 50% off).
- Claude API data retention and training terms (secondary summary): https://www.getvoibe.com/resources/claude-api-data-retention/
- CASA assessment for Gmail restricted scopes: https://deepstrike.io/blog/google-casa-security-assessment-2025
