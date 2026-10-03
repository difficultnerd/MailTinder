# Spike E1: Gmail Header Coverage (first run)

3 October 2026. Read-only, via the Gmail connector. Aggregates only; no subjects, bodies or addresses recorded. Raw tool outputs were deleted after parsing.

## Result

Sample: all 11 distinct senders in the last 60 days (55 threads) of the connected Gmail account. The connected account appears to be a low-traffic secondary mailbox, not James's main one, so these numbers are indicative only.

| Bucket | Senders |
| --- | --- |
| One-click (RFC 8058), DKIM covers both headers, dkim=pass | 6 of 6 list senders |
| Mailto only | 0 |
| HTTPS without one-click | 0 |
| No header, looks bulk | 0 |
| No header, account or security notices (Google, Apple, Microsoft, Steam) | 5 |

- Every mailing list sender supported one-click unsubscribe with a valid DKIM signature. No case needed the fallback page handler or agent.
- The five headerless senders were account and security notices. These belong in the "never unsubscribe" class and should not reach an agent.
- No message carried a Gmail `CATEGORY_*` label (inbox tabs appear to be off for this account). This confirms open item 5 in the data handling research: category labels cannot be relied on as a classification input.

## Next

Re-run on James's main, high-volume Gmail account for a sample of 100 or more senders.

## Second run: main account (3 October 2026)

Read-only, via the Gmail connector, on the owner's main, high-volume Gmail account. Sample: the 40 most recent distinct senders, one message each. Headers only were parsed; raw outputs and the parser were deleted afterwards.

| Bucket | Senders |
| --- | --- |
| One-click (RFC 8058) https, DKIM `h=` covers both headers, dkim=pass | 33 of 40 (83%) |
| of which also offer mailto | 19 |
| Mailto only | 0 |
| HTTPS without one-click | 0 |
| No header | 7 of 40 (18%) |

- **Headerless mail is transactional or relayed.** Six are account, billing or security notices (cloud and developer platforms, financial institutions, a one-time sign-in code). The seventh arrived through iCloud Hide My Email, which rewrites the sender to an `icloud.com` address and carried no List-Unsubscribe on arrival. None looked like bulk marketing.
- **Some one-click mail is transactional.** A cloud signup verification and a bill-splitting balance summary both carry one-click headers. A header alone must not mark a sender as marketing.
- **DMARC can fail on legitimate one-click mail.** 2 of 33 one-click senders (two small retailers) failed DMARC because the passing DKIM signature belonged only to the ESP (`shared.klaviyomail.com`, `mlsend.com`). The List-Unsubscribe headers were still covered by a passing signature.
- **ESPs seen:** Salesforce Marketing Cloud, Klaviyo, SendGrid, Mailgun, Amazon SES, Maropost, MailerLite, Oracle Email Delivery, plus large in-house senders.
- **Other signals:** Feedback-ID on 25 of 40, List-Id on 3, `Precedence: bulk` on 3. List-Id is too sparse to be the main rule key; From address remains the fallback.
- **No `CATEGORY_*` labels** on any message, consistent with the first run. Category labels cannot be a classification input.
- The account shows a prior bulk unsubscribe tool has been used. Senders already unsubscribed that way will not appear in recent mail, which may slightly flatter coverage.

### What this means for the unsubscribe plan

1. One-click POST covers every list sender sampled. The page handler and agent fallback are edge cases for v1, as the feasibility research assumed.
2. Headerless senders belong in the "never unsubscribe" class; a left swipe on them should trash and set a sender rule only.
3. Trust check: verify a passing DKIM signature whose `h=` covers both List-Unsubscribe headers. Do not require DMARC pass, or about 6% of legitimate one-click senders would be rejected.
4. Classification must not treat "has one-click" as "is marketing", since some transactional senders include it.

Sample limits: 40 senders, most recent only, one mailbox. Older backlog senders may have lower one-click adoption (RFC 8058 became a Gmail bulk sender requirement in 2024).
