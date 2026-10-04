# T-702: One-click unsubscribe sender

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | strong | about 250 lines of code plus about 400 lines of tests | T-306, T-406, T-701, T-708 |

**Read only these spec sections:** S2 UN-01 AC5, UN-02 AC1, AC3, AC4 (`docs/specs/S2-v1-acceptance-criteria.md`), S6 6 bullet "One-click" and threats T3, T11 (`docs/specs/S6-security.md`), S4 3.3 steps 5 and 6, S4 5.7 first bullet (`docs/specs/S4-architecture.md`), S7 5.8 `reason_code` list (`docs/specs/S7-api-contract.md`), S10 6.1, 6.2 rows "One-click" and "SSRF on one-click targets" and the paragraph after the table (`docs/specs/S10-test-strategy.md`), ASVS register rows V1.3.6, V12.2.1, V13.2.4, V13.2.5, V15.3.2, V16.3.4 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

## Goal

Add the one-click sender to `unsub`: given a claimed job with method one-click, it sends exactly one RFC 8058 POST to the DKIM-covered https target through `HttpEgress::one_click_post` and turns the result into a `SendResult` (sent, retry, or Needs Attention). After this task a one-click job runs end to end against the local `unsub-testbed`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/unsub/src/one_click.rs` | `OneClickSender` and `map_one_click` |
| Change | `backend/crates/unsub/src/main.rs` | Register `OneClickSender` in `UnsubState::senders` |
| Change | `backend/crates/unsub/src/lib.rs` | `pub mod one_click;` |
| Create | `backend/crates/unsub/tests/one_click.rs` | Service integration tests against `unsub-testbed` |
| Create | `backend/crates/unsub/tests/one_click_ssrf.rs` | SSRF table with the production policy and a fake resolver |

## Types and signatures

```rust
// unsub/src/one_click.rs
pub struct OneClickSender;   // stateless; egress comes from Ports

#[async_trait]
impl UnsubSender for OneClickSender {
    fn method(&self) -> JobMethod { JobMethod::OneClick }
    async fn send(&self, ports: &Ports, job: &ClaimedJob) -> SendResult;
}

/// Pure mapping from the egress result to the runner's result. Unit tested on its own.
pub fn map_one_click(result: &Result<OneClickOutcome, EgressError>) -> SendResult;

/// Parses and re-checks the decrypted target before any network call.
pub fn parse_target(raw: &str) -> Result<url::Url, TargetRejected>;
pub enum TargetRejected { NotUrl, NotHttps, HasUserinfo, NoHost }
```

`OneClickOutcome` and `EgressError` are defined in `ports::egress` (T-201a) and implemented by T-306. `map_one_click` is exactly this table (no `_` arm on either enum):

| Egress result | `SendResult` |
| --- | --- |
| `Ok(Accepted { .. })` (2xx) | `Sent { code: OneClickAccepted }` |
| `Ok(Redirected { .. })` (3xx, not followed) | `NeedsAttention { reason: OneClickRedirect, code: Redirected }` |
| `Ok(Rejected { .. })` (4xx or 5xx) | `Retryable { code: HttpRejected }` (S2 UN-02 AC3: any other response retries) |
| `Ok(TimedOut)`, `Err(Timeout)` | `Retryable { code: TimedOut }` |
| `Err(AddressRefused(_))`, `Err(IpLiteralHost)` | `NeedsAttention { reason: OneClickAddressRefused, code: AddressRefused }` |
| `Err(SchemeNotAllowed)`, `Err(CredentialsInUrl)`, `Err(PortNotAllowed)`, `Err(HostNotAllowed)`, `Err(NotPermitted)` | `NeedsAttention { reason: OneClickAddressRefused, code: Refused }` |
| `Err(DnsFailed)`, `Err(Connect)`, `Err(ResponseTooLarge)` | `Retryable { code: HttpRejected }` |
| `Err(Tls)` | `Retryable { code: HttpRejected }` plus a security event `egress_tls_failure` |
| `Err(PermanentDeleteRefused)` | cannot happen on this path; `NeedsAttention { reason: UnsubscribeFailed, code: Refused }` and an error log |

`IpLiteralHost`: T-306 refuses IP-literal hosts on one-click targets outright, so `https://2130706433/` and `https://0177.0.0.1/` land here. That is the intended refusal.

## Algorithm

1. `parse_target(job.target)`: parse with `url::Url::parse`; scheme must be `https`; no username or password; host present. Any failure: return `NeedsAttention { reason: OneClickAddressRefused, code: Refused }` and make no network call.
2. Call `ports.egress.one_click_post(&url)`. Do not mint an access token and do not touch `MailProvider` (UN-01 AC5).
3. Return `map_one_click(&result)`. On TLS failure also write the security event (route template `unsub.one_click`, outcome `tls_failure`; never the URL or host).
4. The runner (T-701) owns retries, final-attempt handling, outcome recording and the Needs Attention item. This sender sends at most one request per call.

What T-306 guarantees and these tests confirm end to end: method `POST`, body exactly `List-Unsubscribe=One-Click`, `Content-Type: application/x-www-form-urlencoded`, no `Cookie`, no `Authorization`, no `Referer`, a fixed `User-Agent` `[DEFAULT: "MailTinder-Unsubscribe/1"]`, no redirects followed, a 10-second timeout, the connection made to the address resolved and checked first (pinned).

Test set-up (S10 6.2, last paragraph):

- Testbed tests run `unsub` in process with a test egress policy that allows exactly the testbed's socket address and nothing else, and an egress timeout of 250 ms `[DEFAULT: keeps the timeout case fast; production stays 10 s]`.
- The SSRF table runs with the production policy and a fake resolver that maps test host names (for example `loopback.test`) to forbidden addresses.
- Jobs are created directly in the fake store with an encrypted target; the fake `JobScheduler` delivers attempts 0 to 3 by calling the route with `X-CloudTasks-TaskRetryCount`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-01 AC5 | One-click needs no mailbox token |
| UN-02 AC1 | One POST with body `List-Unsubscribe=One-Click` and no cookies or credentials |
| UN-02 AC3 | 2xx marks sent; other responses and timeouts retry up to 3 times, then Needs Attention |
| UN-02 AC4 | No redirects; private, loopback, link-local, CGNAT and metadata targets refused; resolved IP pinned |
| V1.3.6 | SSRF checks on the mail-supplied target |
| V12.2.1 | https only; `http` targets refused |
| V13.2.4 | `unsub` allowlist: Gmail, Google OAuth token endpoint, Google certs, checked one-click targets; never Drive |
| V13.2.5 | Any other host refused before connecting |
| V15.3.2 | Redirects are not followed |
| V16.3.4 | Backend TLS failures are logged |

## Tests that must pass

- `un_01_ac5_one_click_needs_no_token` (service integration: identity fake records zero refresh calls)
- `un_02_ac1_post_has_fixed_body_no_cookies_no_auth` (service integration: testbed record shows method, body, content type, no `Cookie`, `Authorization` or `Referer`)
- `un_02_ac3_2xx_marks_sent` (service integration, one case each for 200, 202, 204)
- `un_02_ac3_500_then_200_sent_on_retry` (service integration)
- `un_02_ac3_400_retries_then_needs_attention` (service integration)
- `un_02_ac3_four_failures_go_to_needs_attention` (service integration: reason `unsubscribe_failed`, testbed saw exactly 4 requests)
- `un_02_ac3_timeout_retries` (service integration)
- `un_02_ac4_3xx_not_followed_not_retried` (service integration, cases 301, 302, 303, 307, 308: reason `one_click_redirect`, testbed `/landed` saw zero requests, one request in total)
- `un_02_ac4_private_targets_refused` (service integration, table: `127.0.0.1`, `10.0.0.1`, `172.16.0.1`, `192.168.1.1`, `100.64.0.1`, `169.254.169.254`, `metadata.google.internal`, `::1`, `fe80::1`, `fd00::1`, `::ffff:127.0.0.1`, `https://2130706433/`, `https://0177.0.0.1/`; each: reason `one_click_address_refused`, zero connections)
- `un_02_ac4_dns_rebinding_uses_pinned_address` (service integration: resolver answers the allowed address then `169.254.169.254`; one lookup, connection to the first address)
- `asvs_v1_3_6_one_click_target_checked_before_connect` (unit on `parse_target` plus the table above)
- `asvs_v12_2_1_http_target_refused` (unit and service integration: `http://` target, zero connections)
- `asvs_v13_2_4_unsub_never_reaches_drive` (unit: the `unsub` allowlist constant has no Drive host)
- `asvs_v13_2_5_unsub_other_host_refused` (service integration with the production policy)
- `asvs_v15_3_2_redirect_not_followed` (service integration)
- `asvs_v16_3_4_tls_failure_logged` (service integration: testbed TLS listener with an untrusted certificate)
- `map_one_click_table` (unit: every row of the mapping table, one case per enum variant)

## Edge cases and traps

- Never build your own `reqwest` client here. Every request goes through `HttpEgress::one_click_post`, which owns resolution, pinning, redirects and the timeout.
- `url::Url` normalises `2130706433` and `0177.0.0.1` to `127.0.0.1`; that is what you want. Do not hand-parse hosts.
- A 3xx is never retried, even on attempt 0. A 4xx is retried (S2 UN-02 AC3), even though it rarely helps.
- Do not treat a 3xx as success, even 303 "See Other".
- Do not put the URL, host or resolved IP in any log, error or metric. The security event carries only the route template and an outcome code.
- The testbed listens on loopback, which the production policy refuses. Use the test policy for testbed tests and the production policy for the SSRF table; never weaken the production policy to make a test pass.
- Tests must not sleep. The timeout case relies on the 250 ms test timeout and the testbed's "never answer" route, or on `tokio::time::pause` if T-306 supports it.
- A target that fails `parse_target` goes straight to Needs Attention; it is not retried.

## Out of scope

- The SSRF policy, resolver and pinning themselves: T-306.
- Whether a one-click job is created at all (DKIM cover, UN-02 AC2): T-406 and T-605.
- The testbed routes: T-708.
- The v2 page handler.

## Security review checklist

- The only network call is `HttpEgress::one_click_post`; no other client exists in `unsub` for this path.
- The production `unsub` egress policy is used in production configuration; the test policy is compiled only under `cfg(test)` or the `testkit` feature.
- No redirect, retry or second request is made on a 3xx.
- The SSRF table covers every range in S10 6.2, including IPv6 and IPv4-mapped forms and decimal and octal IPv4.
- Nothing from the target appears in logs or error bodies.
- The fixed body, header set and no-credential rule are asserted from the testbed's record, not assumed.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The PR shows one reverted-change failure for `un_02_ac4_3xx_not_followed_not_retried`.
