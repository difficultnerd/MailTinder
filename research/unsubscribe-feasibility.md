# Mail Tinder: Unsubscribe Feasibility Research

3 October 2026. Answers `docs/mail-tinder-unsubscribe-agent-research-brief.md`. Research only; no code.

## Recommendation for version 1

Ship a staged pipeline, with the fully autonomous browser agent behind a feature flag and off by default for the friends trial.

1. **Header path (always on).** RFC 8058 one-click POST when the header is DKIM-signed; otherwise `mailto` sent from the user's own account.
2. **Deterministic page handler (always on).** For a header `https` link without one-click, or a body link: fetch it in an isolated headless browser, recognise the common single-page patterns (confirmation button, "enter your email" form, "unsubscribe from all" checkbox) with fixed rules, and use one cheap LLM call only to classify the resulting page as success, failure or unclear.
3. **Everything else goes to Needs Attention** with the link, the reason it stopped, and a one-tap "open in browser" action.
4. **Two safety nets for every path:** an app-side "rejected sender" rule that moves future mail from that sender to trash, and a delivery check that raises a Needs Attention item if mail still arrives after the legal grace period.
5. **Never unsubscribe from suspected spam or phishing.** Left swipe on unauthenticated or provider-flagged spam blocks and reports it instead.

The open-ended agent (LLM choosing every click) is a v2 fast-follow, switched on per user once the trial produces a labelled set of real fallback cases to measure it against.

The reasons: the fallback population in v1 is small and skewed towards senders the app should not click at all; nobody publishes an unsubscribe-specific success rate, so a v1 promise of autonomy would be untested; and the safety nets give the user the outcome they want (no more mail from that sender) even when the unsubscribe itself fails.

## Why the fallback case is smaller than the brief assumes

- **Bulk senders must already provide one-click.** Since 1 June 2024 Gmail requires senders of more than 5,000 messages a day to Gmail addresses to include RFC 8058 one-click unsubscribe on marketing mail and to honour it within 48 hours. Gmail does not accept a body link as a substitute ([Google sender FAQ](https://support.google.com/a/answer/14229414)). Yahoo has matching rules, and Outlook.com has rejected unauthenticated high-volume mail with `550 5.7.515` since 5 May 2025, with a functional unsubscribe expected of the same senders ([Mailtester summary](https://mailtester.com/blog/outlook-list-unsubscribe-requirement/), [Mailgun](https://www.mailgun.com/blog/deliverability/microsoft-sender-requirements/)).
- **Major email service providers add the headers automatically**, so small senders using Mailchimp, Klaviyo or similar platforms usually land on the header path too.
- **v1 ingests new mail only** (CONTEXT.md). Historical mail, with older templates and expired unsubscribe tokens, is the v2 retroactive feature.

So the v1 fallback set is mostly: small senders on custom or legacy mailers, transactional systems that send marketing on the side, and spam. The last group must not be clicked (see Question 5).

**Cheapest de-risking step:** before building anything, run a read-only script over a sample of James's Gmail and count, per distinct sender, the share with a one-click header, a `mailto` only, an `https` only, and no header. That number decides how much engineering the fallback deserves. It needs no LLM and fits in `tools/`.

## The header path (baseline)

### What a clean header looks like

| Case | Headers | Action | Classification |
| --- | --- | --- | --- |
| One-click | `List-Unsubscribe: <https://...>` plus `List-Unsubscribe-Post: List-Unsubscribe=One-Click`, both covered by a valid DKIM signature | `POST` the literal body `List-Unsubscribe=One-Click` | Clean |
| Mailto | `List-Unsubscribe: <mailto:...>` | Send the email from the user's account | Clean |
| HTTPS without one-click | `List-Unsubscribe: <https://...>` only | Opens a landing page | Page handler (not clean) |
| None | No header | Find a body link | Page handler or Needs Attention |

RFC 8058 rules the receiver must follow ([RFC 8058](https://www.rfc-editor.org/rfc/rfc8058)):

- The POST must carry no cookies, HTTP authorisation or other context.
- Body as `multipart/form-data` (preferred) or `application/x-www-form-urlencoded`.
- If the DKIM signature does not cover both headers, the receiver should not use one-click. Mail Tinder should treat that case as "HTTPS without one-click".
- Senders must not redirect. Do not follow redirects on the POST; treat a redirect as unclear.

### What the header path cannot tell you

A `200 OK` on the POST only proves the server accepted the request. RFC 8058 sets no response semantics. Gmail's own Manage Subscriptions view uses the same one-click mechanism and has the same blind spot ([Suped](https://www.suped.com/blog/gmail-launches-manage-subscriptions-directly-in-gmail-everything-you-need-to-know)). This is why the delivery check below applies to the header path too.

### Mailto specifics

- Needs a send permission: `gmail.send` (sensitive) or `gmail.modify` (restricted) on Gmail ([Gmail scopes](https://developers.google.com/workspace/gmail/api/auth/scopes)); `Mail.Send` on Microsoft Graph.
- The message lands in the user's Sent folder. Show it in History so the user is not surprised.
- Rate-limit per user (for example 30 an hour) to stay well inside consumer sending limits and avoid looking like bulk mail.
- Prefer one-click POST over `mailto` when both are present.

## Question 1: viable approaches

| Approach | Examples | Cost per attempt | Fit for Mail Tinder |
| --- | --- | --- | --- |
| A. Plain HTTP plus rules | Fetch, parse HTML, submit a known form | Near zero | Good for GET-to-unsubscribe links and simple forms. Breaks on JavaScript-rendered pages. |
| B. Headless browser plus rules, LLM as page judge | Chromium driven over the DevTools protocol; one small-model call to classify the final page | About US$0.001 to $0.01 | **Recommended v1.** Handles JavaScript pages, stays predictable, cheap. |
| C. Headless browser plus LLM choosing each action from the accessibility tree | browser-use (Python), Stagehand (TypeScript), or a custom loop | About US$0.02 to $0.15 | v2 agent. Strong on easy tasks. |
| D. Vision computer-use agents | Claude computer use, OpenAI Operator/CUA | Higher (screenshots each step) | Overkill for single-page flows. |
| E. Hosted browser agent platforms | Browserbase, Skyvern, browser-use cloud | Platform fee plus tokens | Quick to start; adds a processor that sees users' addresses and unsubscribe tokens. |

Stack constraint: the repo's language policy is Rust backend, Dart front end, Python only in `tools/` and `scripts/`. browser-use (Python) and Stagehand (TypeScript) cannot run in the backend without a policy exception. A Rust worker using a DevTools-protocol crate (such as `chromiumoxide`) with a small fixed action set (click, fill email, tick, submit, finish, give up) and direct calls to the Claude API is feasible and keeps the policy intact. The action space for unsubscribing is narrow enough that a general agent framework adds little.

The 2025 survey of production browser agents reaches the same view from the security side: narrow specialist agents with deterministic, code-enforced limits beat general agents, and a hybrid of accessibility tree plus selective screenshots beats pure vision ([arXiv 2511.19477](https://arxiv.org/html/2511.19477v1)).

## Question 2: realistic success rate

No public benchmark or vendor report measures autonomous unsubscribe specifically. The nearest evidence is general web-agent benchmarks:

- **Online-Mind2Web** (300 live tasks, 136 sites). On its April 2025 release the best agent (OpenAI Operator) scored 61.3% overall but 90.4% on easy tasks of five steps or fewer, dropping sharply as steps increase ([arXiv 2504.01382](https://arxiv.org/html/2504.01382v4)). The leaderboard as of 5 August 2026 shows top agents at 92.7% to 97.7% ([Yutori leaderboard](https://yutori.com/leaderboards/online-mind2web)). Leaderboard entries are self-submitted and run on tuned agents.
- The same paper found earlier benchmark figures (around 90% on WebVoyager) overstated real performance.

An unsubscribe is almost always an easy task by that definition (one to four steps). The estimates below are **inferences** from those benchmarks and from the legal constraints on senders, not measurements:

| Pattern | Rules plus judge (v1) | LLM agent (v2) | Notes |
| --- | --- | --- | --- |
| Link unsubscribes on load, or one confirm button | 85 to 95% | 90 to 97% | Most common legitimate pattern |
| Form asking for the email address | 80 to 90% | 90 to 95% | The app knows the address |
| Preference centre (many toggles, "unsubscribe from all") | 50 to 70% | 75 to 90% | Wrong-toggle risk; needs the judge |
| "Check your inbox to confirm" | 0% | 0% in v1; v2 can watch the inbox for the confirmation mail | |
| Login-gated | 0% by design | 0% by design | The agent must never use credentials |
| CAPTCHA or bot challenge | 0% by design | 0% by design | Do not use solver services |

Login-gated unsubscribe is rare among compliant senders because US law forbids requiring anything beyond an email address and a single web page ([FTC CAN-SPAM guide](https://www.ftc.gov/business-guidance/resources/can-spam-act-compliance-guide-business)).

Plan to measure, not trust these numbers: log every fallback attempt during the trial (outcome, stop reason, page type, never page content), and have James label a sample. That labelled set is the gate for turning on the v2 agent.

## Question 3: failure modes and detection

| Failure | Detection | Reliability |
| --- | --- | --- |
| CAPTCHA (reCAPTCHA, hCaptcha, Turnstile) | DOM and iframe signatures | High, deterministic |
| Bot challenge or block (Cloudflare interstitial, 403) | Status code, known challenge markup | High |
| Login wall | Password field present, redirect to a sign-in path | High |
| Broken link, expired token, 404, redirect loop | Status codes, redirect count cap | High |
| Infinite or looping flow | Hard caps: about 8 actions, 60 to 90 seconds, same-page detection | High (enforced in code) |
| Dark patterns ("pause instead", survey, "are you sure?") | LLM judge | Medium |
| Ambiguous final state | LLM judge with three outputs: success, failure, unclear | Medium |
| Page claims success but mail continues | Delivery check only | High, but delayed |

The weak point is the success judgement itself. The best automatic judge in the Online-Mind2Web study agreed with human raters 85.7% of the time ([arXiv 2504.01382](https://arxiv.org/html/2504.01382v4)). An agent's own "done" is therefore wrong roughly one time in seven. Two design consequences:

- Route "unclear" to Needs Attention, never to "done".
- Close the loop with the **delivery check**: record the unsubscribe date, then raise a Needs Attention item if new mail from the same sender (same From domain plus list ID) arrives after the grace period. Use 5 working days, the Australian Spam Act deadline ([ACMA fact sheet](https://www.acma.gov.au/sites/default/files/2024-05/Fact%20sheet%20-%20email%20and%20SMS%20unsubscribe%20rules.pdf)); Gmail expects 48 hours and US law allows 10 business days. This is the only signal that proves an unsubscribe worked, and it covers every path, including the header path.

## Question 4: cost and latency

Prices from Anthropic's published rates (cached 25 September 2026): Claude Haiku 4.5 at US$1 input and $5 output per million tokens; Claude Sonnet 5.5 at $2 and $10.

| Path | Tokens | LLM cost | Browser cost | Latency |
| --- | --- | --- | --- | --- |
| One-click POST or mailto | 0 | 0 | 0 | Under 2 seconds |
| v1 page handler, one judge call on Haiku 4.5 (about 3,000 tokens in, 100 out) | ~3k | ~US$0.004 | Self-hosted Chromium, negligible | 5 to 20 seconds |
| v2 agent, 3 to 6 steps at 5,000 to 9,000 tokens each, Haiku 4.5 | 15k to 55k | ~US$0.02 to $0.06 | Negligible self-hosted; Browserbase about $0.12 per browser hour ([pricing summary](https://scrapegraphai.com/blog/browserbase-pricing)) | 20 to 90 seconds |
| v2 agent on Sonnet 5.5 | as above | ~US$0.04 to $0.15 | as above | 20 to 90 seconds |

For comparison, the browser-agent survey measured a harder 205-second e-commerce task at US$0.145 ([arXiv 2511.19477](https://arxiv.org/html/2511.19477v1)).

Scale for one user: unsubscribe once per **sender**, not per email, so cost tracks new senders rather than rejects. A heavy day of 200 rejects might hold 50 new senders, of which perhaps 10% to 20% need the fallback. That is 5 to 10 runs: under US$0.05 a day in v1 and under US$1.50 a day on Sonnet in v2. For 20 trial users, cost is not a constraint. Latency does not matter either because the work runs in the background after the swipe.

## Question 5: legal, safety and provider policy

**Security (the most important flags)**

- **Clicking unsubscribe in spam is a known risk.** It confirms the address is live and can lead to malicious pages; DNSFilter reported 1 in 644 unsubscribe-link clicks led to a potentially malicious site ([Tom's Guide, June 2025](https://www.tomsguide.com/computing/online-security/that-unsubscribe-link-is-actually-a-hidden-security-risk-do-this-instead)). Gate every automated unsubscribe on: DKIM pass aligned with the From domain (or DMARC pass), message not in the provider's spam folder, and the link host matching or plausibly belonging to the sender. Fail any gate and the left swipe becomes "block and report spam".
- **Prompt injection.** Treat page content as data. Enforce limits in code, not in the prompt: the only text the agent may type is the user's own address; no passwords, no payment fields, no file downloads; navigation confined to the link's domain and its redirect chain; fixed action and time caps.
- **Isolation.** Fresh browser context per attempt, no stored cookies, run from the server (does not expose the user's IP). Unsubscribe URLs carry tokens that act like bearer credentials, so never log full URLs or page bodies (consistent with CONTEXT.md's proposed principles).

**Mailbox provider policy**

- **Gmail scopes are the real v1 blocker, and they apply to the whole app, not only unsubscribe.** Reading mail needs `gmail.readonly` or `gmail.modify`, both restricted. A production app with restricted scopes needs Google verification plus an annual third-party security assessment ([Google restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)). Without it, an unverified app is capped at 100 users and shows an "unverified app" warning ([Google user cap](https://support.google.com/cloud/answer/7454865)), and apps left in Testing status get short-lived refresh tokens (Google states 7 days), which would force trial users to sign in again weekly. For 20 friends, publish the unverified app to production status and accept the warning screen. Plan the assessment before any wider release or client demo on real accounts.
- Gmail's Limited Use policy also governs sending message content to an LLM provider. Allowed for a user-facing feature with disclosure; keep it to the minimum (headers and the unsubscribe page, not message bodies) for this feature.
- Outlook needs `Mail.Read`/`Mail.ReadWrite`, plus `Mail.Send` for mailto, and `MailboxSettings.ReadWrite` only if Mail Tinder writes native inbox rules ([Graph messageRules](https://learn.microsoft.com/en-us/graph/api/mailfolder-post-messagerules)).
- The app-side rejected-sender rule needs no extra scope if the app already holds modify access for filing. A native Gmail filter would need `gmail.settings.basic` (also restricted), so keep the rule in the app for v1.

**Legal**

- The law sits on the user's side. Australian senders must provide a functional unsubscribe and honour it within 5 working days (ACMA, above). US senders cannot charge, ask for anything beyond an email address and opt-out choices, or require more than a reply email or one web page (FTC, above).
- Automated access to third-party unsubscribe pages, at single-user volume and on the user's instruction, carries low terms-of-service risk. Do not use CAPTCHA solvers, residential proxies or fingerprint evasion: they turn a user's agent into a bot circumventing access controls, and they cost money.
- Link scanners already trigger unsubscribe links at scale (Microsoft Safe Links prefetching opted users out of lists; [Microsoft Tech Community](https://techcommunity.microsoft.com/discussions/microsoft-security/atp-safe-links-are-automatically-unsubscribing-users-from-email-lists/205278)), so senders already expect automated hits.

## Question 6: v1 or staged?

Stage it. Specifically:

| Capability | v1 | v2 |
| --- | --- | --- |
| One-click POST and mailto | Yes | |
| Spam and phishing gate (block and report instead) | Yes | |
| Rejected-sender rule (future mail to trash, recoverable) | Yes | Optional native provider filter |
| Delivery check after 5 working days | Yes | |
| Page handler: rules plus LLM judge | Yes | |
| Needs Attention with reason and "open in browser" | Yes | |
| LLM agent choosing each action | Behind a flag, off | On, gated on trial data |
| Confirm-by-email flows (agent watches the inbox) | | Yes |
| Retroactive historical senders (expired tokens) | | Yes, with the agent |

This keeps the user-visible promise in CONTEXT.md intact: every left swipe either unsubscribes or produces a clear Needs Attention item, nothing fails silently, and future mail from that sender is cleared regardless. Nothing in this plan blocks the friends trial. The Google restricted-scope verification is the one item with lead time, and it blocks wider release of the whole app rather than this feature.

## Sort rules: where unsubscribe fits

James's framing (3 October 2026): user actions in the app build sort rules that bulk-action mail. Under that framing the rule clears the mailbox and the unsubscribe only cuts future volume, which lowers the stakes on the fallback agent further.

- **Every swipe writes or strengthens a rule.** Left: trash future matching mail, plus an unsubscribe attempt. Repeated right: keep (or file, through keep-learning). Up: file to the chosen category. The rejected-sender rule in the recommendation above is the first of these.
- **Match key (proposed): `List-Id` header, else the exact From address.** A whole domain is too broad (one retailer domain sends both receipts and marketing). Sender plus content class (marketing or transactional) handles that case properly once the classifier exists; see `research/cheap-email-classification.md`.
- **v1 applies rules to new mail as it arrives**, recorded in History with undo. Applying them across historical mail is the v2 retroactive feature.
- **Rules outlive unsubscribes.** If the delivery check finds mail still arriving, the rule has already trashed it; the Needs Attention item tells the user the sender ignored the request.

## Decisions for James

1. Confirm the left-swipe-on-spam rule: block and report, never unsubscribe.
2. Confirm the rejected-sender rule moves future mail to trash automatically (recoverable, shown in History).
3. Confirm the open-ended agent is v2, flagged off for the trial.
4. Approve the read-only header coverage script over your own Gmail as the first build task.
