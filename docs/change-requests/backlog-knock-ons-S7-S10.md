# Backlog knock-ons for S7 and S10

From the product plan thread, 4 October 2026. Writing the S12 task files (`docs/backlog/`) turned up these contradictions or gaps in S7 and S10. The task files already follow the resolution given here, marked `[DEFAULT]` where it was a choice.

## S7 (API contract)

1. **Step-up `max_age`.** S7 3.6, API-AUTH-1 and the OpenAPI `intent` text say `max_age=0`; S2, S4 3.1, S6 4 and ASVS V6.8.4 say `max_age=300`. Use 300. `recent_auth_at` is set from the ID token's `auth_time`, not the server clock.
2. **No access token on jobs.** S7 2.1 and 3.5 say a job holds an access token minted at queue time. S3, S5 JOB-1, S6 5 and S2 UN-01 AC4 say none is stored; `unsub` mints one at run time through `svc-common`. Rewrite 3.5.
3. **Needs Attention reason for a revoked token.** Add `sign_in_required` to the S7 5.8 table and the OpenAPI `reason_code` enum (S2 UN-01 AC6, S9 6).
4. **History outcome.** The History `outcome` enum lacks `needs_attention`. Applied: S7 added it, and T-602b, T-609 and T-1006b use it.
5. **Sealed token from an earlier session (S7 5.5).** AES-GCM cannot tell it apart from a tampered token. Tasks: a wrong token type gives `400`; any other open failure on a classification token lets the swipe proceed with no eval record. Reword 5.5.
6. **API-INT-1 non-queued jobs.** "A job not in `queued` returns 200" blocks Cloud Tasks retries of a job left `running`. T-701 lets a delivery with a higher retry count reclaim it. Reword 5.12.
7. **Card unsubscribe method.** The optimistic reject toast cannot be right for mailto lists without an `unsubscribe_method` (`one_click`, `mailto`, `manual`, `none`) field on the card. Add it.
8. **Mailbox IDs.** Tasks derive mailbox IDs as UUID v5 of provider plus subject, so uniqueness needs no extra collection. S7 2 says server IDs are UUID v4; add this exception.
9. **Invite errors.** No code for re-sending or revoking a used or revoked invite; tasks return `404`. No `405`; unmatched methods return a `404` problem.
10. **`Mailbox.linked_at`** is required in the prose but not in the OpenAPI schema.
11. **API-ADM-16 `email_address`** is the earliest linked mailbox still present, since the joining mailbox may be disconnected.
12. **Copy and references.** Where S7 3.4 copy differs from S9 (`not_registered`, `invite_invalid`, `step_up_wrong_account`), S9 wins. The consent text reference "S9 7.4a" is now S9 7.8.
13. **Bake-off report details.** `ClassifierId` format is `gemini@<model>` (S3, S4), not `gemini-flash-lite@<version>` (ADM-8). Latency bins should end below the 2,000 ms timeout, with timeouts counted separately. "Agreement" is five-class agreement. Header-rules confidence of `bulk_score / 100` is the probability of bulk, not confidence in the predicted class; say so in 5.13.

## S10 (test strategy)

1. **DKIM in the corpus (S10 5).** T-406 trusts Gmail's own Authentication-Results header (`mx.google.com`) and parses the DKIM-Signature `h=` tag; it does not verify signatures. The corpus needs those headers, not test keys and a fake DNS resolver.
2. **End-to-end driver (S10 3.2, 3.3).** `integration_test` cannot survive full-page OAuth redirects. T-1101a drives Chrome over WebDriver (`fantoccini`) from a Rust `e2e` crate, finding controls by semantics labels.
3. **One key (S10 6.3).** "Both keys crypto-shredded" is left over from the passkey design; there is one `data_key`.
4. **Plain http unsubscribe links (S10 5).** S6 6 drops non-https links, so the http corpus row produces no Needs Attention link. Update the row.
5. **Bulk look-alike row.** To class as `bulk_no_header`, that corpus message needs a header signal such as `Precedence: bulk`.
6. **Delivery check grace (S10 6.3, S2 UN-06, ST-02).** Tasks take the literal reading: mail inside the 5-business-day grace blocks "confirmed working". Business days are Monday to Friday in UTC, with no holiday list `[DEFAULT]`.
7. **Slow models (S10 9.1).** Read as "the Feed waits no longer than the 2-second model timeout".
8. **Test names.** S10's `asvs_v8_<row>_` example should read `asvs_<row>_`.
9. **Coverage job.** If it runs with `--all-features`, it needs the Firestore emulator steps that T-301 adds to the `rust` job.
