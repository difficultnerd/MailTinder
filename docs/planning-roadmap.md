# Mail Tinder: Planning Roadmap to Spec-Ready

Last updated: 3 October 2026 (S5, S6 and ASVS register drafted; S7 and S10 running in parallel threads; spec audit applied)

## Purpose

This file defines what "planning complete" means for Mail Tinder: the point at which coding agents can pick up a work item, read a spec, write code and tests against it, and have CI prove the result without asking a human what was meant. It lists the spec artefacts still to produce, the decisions still open, and an estimate of progress.

Inputs: `CONTEXT.md`, `mail-tinder-v1-user-stories.md`, `plan-open-questions.md`, `research/unsubscribe-feasibility.md`, `research/cheap-email-classification.md`, `research/data-handling-and-in-account-ai.md`, and the `difficultnerd/MailTinder` repo template.

## Definition of spec-ready

Planning is complete when all of the following hold:

1. Every v1 user story has testable acceptance criteria, and every criterion maps to at least one named test.
2. Every external behaviour (API endpoint, provider call, background job, state transition) has a written contract that a test can check.
3. Every ASVS Level 2 requirement that applies to the app is mapped to a design control and a verification method (automated test, CI check or manual review).
4. The data inventory states, for every field, where it lives, why, for how long, and how it is deleted.
5. The backlog is split into small vertical slices with dependencies, each with a spec link and a definition of done.
6. The specs live in the repo, where coding agents read them, and `CLAUDE.md` tells agents how to use them.

## Spec artefacts

Each row is a document or set of documents in the repo. "Status" is today's state.

| # | Artefact | Contents | Depends on | Status |
| --- | --- | --- | --- | --- |
| S1 | Product context | `CONTEXT.md`: vision, decisions, principles | None | Done, kept current |
| S2 | User stories with acceptance criteria | Each v1 story rewritten with Given/When/Then criteria, edge cases and out-of-scope notes | S1, decisions D1 to D6 | Draft: `docs/specs/S2-v1-acceptance-criteria.md`, awaiting James's review |
| S3 | Domain model and state machines | Entities (user, mailbox, message reference, sender, swipe, sort rule, unsubscribe job, Needs Attention item, audit event) and their state transitions, including undo and the unsubscribe delay window | S2 | Draft: `docs/specs/S3-domain-model.md`, awaiting James's review |
| S4 | Architecture and ADRs | Component view, provider adapter trait, ingestion, job queue, unsubscribe pipeline, classification cascade, hosting. One ADR per significant choice | Research threads, D7 to D12 | Draft: `docs/specs/S4-architecture.md` (Google Cloud serverless), awaiting James's review |
| S5 | Data inventory and retention spec | Every stored field, its classification, purpose, retention and deletion path. Zero-retention design | Data handling research thread | Draft: `docs/specs/S5-data-inventory.md`, awaiting James's review |
| S6 | Security spec | Threat model (STRIDE over S4), ASVS Level 2 requirement map, authentication and session design, token handling, unsubscribe browser isolation (server-side request forgery controls), content security policy for Flutter web | S4, S5 | Draft: `docs/specs/S6-security.md` plus `docs/security/asvs-l2-register.md` (253 ASVS 5.0 L1 and L2 requirements mapped) |
| S7 | API contract | OpenAPI document for Flutter to Rust traffic, including error model and pagination | S3 | Draft: `docs/specs/S7-api-contract.md`, awaiting James's review |
| S8 | Provider adapter contracts | Rust trait plus Gmail mappings (Microsoft Graph in v2): scopes, calls, rate limits, label versus folder behaviour, failure codes. The trait stays provider-neutral so a new provider is a new adapter | S4 | Partly covered in research. Prerequisite: draft before S12 (spec audit, 3 October 2026), since S2, S7 and S10 point at it for scopes, labels and calls |
| S9 | UX spec | Screen list, card layout, gesture and button behaviour, filing flow, Needs Attention, Settings, empty and error states, copy, accessibility | S2 | Draft: `docs/specs/S9-functional-screens.md` (functional only; visuals later) |
| S10 | Test strategy | Test levels per component, provider fakes, synthetic mail fixture corpus (no real mail in the repo), local unsubscribe test sites, end-to-end harness for Flutter web, security tests tied to S6, coverage and CI gates | S3, S6, S7, S8 | Draft: `docs/specs/S10-test-strategy.md`, awaiting James's review |
| S11 | Operations spec | Environments, secrets handling, deploy pipeline, logging and monitoring that carry no mail content, backup. Must also carry the ASVS rows no other spec holds: V13.1 configuration documentation, V15.1.1 remediation windows, V16.4 log protection, alerting, incident handling and notifiable data breach handling | S4, S5 | Not started. Prerequisite: draft before S12 (spec audit, 3 October 2026) |
| S12 | Delivery backlog | Milestones, vertical slices, dependency order, each slice linked to its specs and tests | All above; S8 and S11 must be drafted first | Not started |
| S13 | Agent working agreement | Spec location and template, ADR format, definition of done, how agents claim work, updated `CLAUDE.md` | S12 | Not started |
| S14 | Privacy notice and Google verification pack | Privacy notice, including the Australian Privacy Principle 8 disclosure that server data is stored in the United States (S5), and the Google Limited Use statement; the pack Google OAuth verification needs. The bake-off consent depends on it | S5, S6, CR-01 | Not started (added 3 October 2026, spec audit) |

## Evidence to gather before specs firm up

These are read-only spikes on James's own mail. They produce numbers that size the build. Scripts would live in `tools/` and keep data local, never committed.

| # | Spike | Answers | Source |
| --- | --- | --- | --- |
| E1 | Header coverage count over a sample of James's Gmail | How much mail lands on the one-click path, `mailto` only, `https` only, or no header. Sizes the fallback work. Re-run on older backlog senders before the trial (James, 3 October 2026, spec audit Q3) | Unsubscribe research |
| E2 | Label about 500 recent messages (personal, transactional, marketing, filing category) | Header-rule precision for bulk versus personal; top-3 filing accuracy | Classification research |
| E4 | Gemini versus Jev bake-off on every consenting user's cards, judged by swipes; both also run over the E2 labelled set | Classifier accuracy, calibration, latency and cost; publishable aggregates | CR-01 |
| E3 | Embedding and lookup timing on the target container size | No longer needed (3 October 2026, spec audit): v1 has no server-side embedding layer (D9) | Classification research |

## Decisions still open

Grouped by the artefact they block. Each has a recommended default; most come from earlier documents.

### Blocking the stories and domain model (S2, S3)

| # | Decision | Recommended default | Source |
| --- | --- | --- | --- |
| D1 | Left swipe on suspected spam or phishing | Decided: block and report; never unsubscribe | Unsubscribe research |
| D2 | Rejected-sender rule | Decided: trash future mail matching sender plus List-Id only, shown in History, switchable off | Unsubscribe research |
| D3 | Card score label | Decided in principle: graded playful scale (poop icons to friend icon) with a reason on tap; visuals deferred to S9 | Gap list A4, classification research |
| D4 | Skip behaviour | Decided: returns later in the queue (end or random), at most twice | Gap list A1 |
| D5 | Identity versus mailboxes, and invite enforcement | Decided: invite by email or approved invite request; OAuth email must match the invite, and redemption needs the invite email's single-use token (spec audit, 3 October 2026) | Gap list B4, B5 |
| D6 | Merged Feed | Decided: many mailboxes per user, several per provider allowed, one merged Feed with an account badge | Gap list B6 |

### Blocking architecture, data and security (S4, S5, S6)

| # | Decision | Recommended default | Source |
| --- | --- | --- | --- |
| D7 | Open-ended unsubscribe agent | Decided: v2, feature flag, off for the trial | Unsubscribe research |
| D8 | `mailto` unsubscribe | Decided: send from the user's mailbox; adds send scope | Unsubscribe research, gap list B3 |
| D9 | Where AI runs and what leaves the server | Decided (James, 3 October 2026): header rules (no Gmail category labels), then Gemini Nano on-device; no server-side model in v1 except the opt-in Gemini versus Jev bake-off (CR-01, James 3 October 2026) | Data handling research, CR-01 |
| D10 | What the server stores | Decided (James, 3 October 2026): no mail content at rest; user state (rules, categories, History) in one primary app folder per user, in the Drive of the first linked Gmail mailbox, encrypted (OneDrive app folder in v2 with Microsoft); server keeps only job, Needs Attention and security log records with TTLs | Data handling research |
| D10a | Stay signed in or sign in each session | Superseded (James, 3 October 2026): passkey lock dropped for the trial; Google OAuth sign-in; refresh tokens under the KMS-wrapped per-user key; passkey lock moves to v2 pre-CASA hardening | Data handling research |
| D11 | Hosting and region | Decided: serverless on Google Cloud, `us-central1` (Sydney pin dropped 3 October 2026), Google Cloud Well-Architected Framework | Gap list C |
| D12 | Mail ingestion | Decided: check on open and refresh | Gap list C |

### Blocking test and delivery planning (S10, S12, S13)

| # | Decision | Recommended default | Source |
| --- | --- | --- | --- |
| D13 | Where specs live | Decided: repo `docs/`, Markdown, backlog as small simple task files | New |
| D14 | Trial success criteria | Decided: unsubscribe success rate, undo rate on rejects, zero unrecoverable actions | Gap list E |
| D15 | Approve spikes E1 and E2 on James's Gmail | Decided: approved, read-only. Access route pending | Research threads |
| D17 | Work Microsoft 365 mailboxes | Moved to v2 with Microsoft support (James, 3 October 2026). For v2: tenant admins often block user consent to `Mail.ReadWrite` for unverified apps, and employer mail may be covered by workplace policy. Recommended: support them technically; each tenant's consent outcome is tested in the trial, and users are told the app reads that mail | New (multiple mailboxes) |
| J1 | Publish bake-off results, and where | Decided: yes, as a blog post (James, 3 October 2026). Headline benchmark on James's labelled mail (E2), aggregates only, from saved snapshots (CR-01a, approved 3 October 2026) | CR-01, CR-01a |
| T1 | Dedicated sandbox mailboxes (one Gmail, one Outlook.com) for a nightly live contract run; weekly Gmail re-authorisation in Testing mode | Decided: yes (James, 3 October 2026). The Outlook.com sandbox moves to v2 with Microsoft | S10 |
| T2 | Trial targets: unsubscribe success 90% or better; undo rate on rejects tracked with a 10% investigate flag; zero unrecoverable actions | Decided: yes (James, 3 October 2026) | S10 |
| T3 | Coverage floors: 90% for the domain crate, 75% elsewhere | Decided: yes (James, 3 October 2026) | S10 |
| T4 | Move the template's `optional/privacy` layer into the core now, and `optional/zap` once staging exists | Decided: yes (James, 3 October 2026) | S10 |
| T5 | A staging Google Cloud project for smoke tests and dynamic scanning | Decided: yes (James, 3 October 2026) | S10 |
| T6 | End-to-end tests as a required pull request check (about 10 minutes per run) | Decided: yes (James, 3 October 2026) | S10 |
| D16 | Working name for client demos | Defer until before demos | Gap list E |

Spec audit (3 October 2026): `docs/reviews/spec-audit.md`. James answered its questions Q1 to Q4 the same day: D9 and D10 confirmed (Q1); Microsoft moves to v2 and v1 builds Gmail only behind the provider-neutral adapter (Q2); mailto unsubscribe stays in v1 and the page handler moves to v2 with the open-ended agent, with https-only unsubscribe links going to Needs Attention (Q3); consenting friends' mail may go to Jev before a DPA, and the consent text says TypeSafe has no retention commitment, DPA or attestation yet (Q4).

Decided since the BA interview and not repeated here: swipe effects on the message, personal-mail blocking, the unsubscribe delay window, Feed scope, backlog order, web first, provider filters in v2, and OWASP ASVS Level 2 as a hard requirement.

## Progress estimate

This is a judgement, weighted by how much each area contributes to agents building without guesswork.

| Area | Weight | Done | Contribution |
| --- | --- | --- | --- |
| Product intent and scope (S1, decisions) | 15% | 90% | 13% |
| Research and evidence (including spike E1) | 15% | 95% | 14% |
| Testable requirements (S2, S3, S9) | 20% | 75% | 15% |
| Architecture, contracts and data (S4, S5, S7, S8) | 20% | 70% | 14% |
| Security and ASVS mapping (S6, register) | 15% | 65% | 10% |
| Test strategy, operations, backlog, agent rules (S10 to S13) | 15% | 30% | 5% |
| **Total** | **100%** | | **about 71%** |

What's done is the part that needed James: intent, scope and most product decisions. What's left is mostly specification work agents can draft, with James reviewing decisions and acceptance criteria.

## Proposed sequence

1. James closes D1 to D6 and D13 to D15 (mostly yes or no).
2. Run spikes E1 and E2 while the data handling research finishes.
3. Draft S2 and S3 together, then S9. James reviews the acceptance criteria.
4. Draft S4, S5 and S8 once D7 to D12 close (S5 starts from Part 1 of the data handling doc), then S6 and S7.
5. Draft S10 and S11, then cut the backlog (S12) and the agent working agreement (S13).
6. Move everything into the repo in one documentation pull request and update `CLAUDE.md`.

## Backlog seeds for S12

Tasks already known, to be cut into task files when S12 is drafted.

- Install the template's `optional/privacy` layer and add `privacy-checks` to branch protection (T4).
- Install `optional/zap` once the staging project exists (T4, T5).
- Make `ac-coverage`, `coverage` and `e2e` required pull request checks (T3, T6).
- Update the `CLAUDE.md` "Before finishing" line to match the new required checks.
- Terraform the staging project with its own state (T5); set up the Gmail sandbox mailbox (Outlook.com in v2) and its GitHub Actions secrets for the nightly live contract run (T1).
- CSP with Trusted Types spike for Flutter web as the first front-end task (CanvasKit needs `wasm-unsafe-eval` and self-hosted assets; spec audit L8).

## v2 pre-CASA hardening

To do before leaving Google Testing mode and the CASA assessment.

- Passkey lock: per-user key for refresh tokens unlocked by a passkey (WebAuthn PRF), lost-passkey recovery, passkey management (dropped for the trial, James, 3 October 2026). Also closes the accepted V6.3.3 multi-factor deviation.
