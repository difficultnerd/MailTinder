# T-708: unsub-testbed local sites

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | sonnet | about 350 lines of code plus tests | T-001 |

**Read only these spec sections:** S10 6.1 and 6.2 (one-click and SSRF rows only; page rows are v2) and S10 3.3 bullet "`unsub-testbed`" (`docs/specs/S10-test-strategy.md`), S2 UN-02 AC1, AC3, AC4 (`docs/specs/S2-v1-acceptance-criteria.md`). Nothing else is needed.

## Goal

A small `axum` server, usable both as a library inside tests and as a binary for `scripts/e2e.sh`, that stands in for unsubscribe endpoints. Each route is one scenario (success codes, failures, retries, timeouts, redirects) and records exactly what it received so tests can assert the one-click request shape. It also exports the SSRF host table that T-702 uses with a fake resolver. Page-handler routes are v2 and not built.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/unsub-testbed/Cargo.toml` | Library plus binary; dev and test use only |
| Create | `backend/crates/unsub-testbed/src/lib.rs` | `start`, `Testbed`, `Recorded`, routes |
| Create | `backend/crates/unsub-testbed/src/ssrf_table.rs` | `SSRF_CASES` |
| Create | `backend/crates/unsub-testbed/src/main.rs` | Binary: binds `TESTBED_ADDR` (default `127.0.0.1:0`), prints the bound address |
| Create | `backend/crates/unsub-testbed/tests/routes.rs` | Tests of the testbed itself |
| Change | `backend/Cargo.toml` | Workspace member if T-001 did not add it |

## Types and signatures

```rust
// unsub-testbed/src/lib.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub method: String, pub path: String,           // path without query
    pub headers: Vec<(String, String)>,             // lower-case names, in arrival order
    pub body: Vec<u8>,
    pub cookies_present: bool,
}

pub struct Testbed { pub addr: std::net::SocketAddr, /* private: shutdown, shared state */ }
impl Testbed {
    pub fn url(&self, path: &str) -> url::Url;                // http://127.0.0.1:<port><path>; https variant below
    pub fn https_url(&self, path: &str) -> url::Url;          // https on the TLS listener, self-signed test CA
    pub fn test_ca_pem(&self) -> &'static [u8];               // trust root for the egress test client
    pub fn requests(&self) -> Vec<Recorded>;                  // everything, in order
    pub fn requests_to(&self, path: &str) -> Vec<Recorded>;
    pub fn release_hanging(&self);                            // lets /oneclick/hang requests finish
    pub async fn shutdown(self);
}
/// Binds port 0 on 127.0.0.1 for plain HTTP and a second port for HTTPS; returns once both listen.
pub async fn start() -> std::io::Result<Testbed>;
/// Same, but the HTTPS listener uses a certificate that the test CA did not sign (for the TLS-failure test).
pub async fn start_with_untrusted_cert() -> std::io::Result<Testbed>;

// unsub-testbed/src/ssrf_table.rs
pub struct SsrfCase { pub host: &'static str, pub resolves_to: &'static [std::net::IpAddr], pub note: &'static str }
pub const SSRF_CASES: &[SsrfCase];   // host names under .test mapped to forbidden addresses (see Algorithm)
```

Routes (all under both listeners; one-click routes accept `POST` only, other methods return 405 and are still recorded):

| Route | Behaviour |
| --- | --- |
| `POST /oneclick/200`, `/oneclick/202`, `/oneclick/204` | That status, empty body |
| `POST /oneclick/400` | 400 |
| `POST /oneclick/500` | 500 every time |
| `POST /oneclick/500-then-200/{key}` | 500 on the first request for `key`, 200 after |
| `POST /oneclick/fail-n/{n}/{key}` | 500 for the first `n` requests for `key`, then 200 |
| `POST /oneclick/hang` | Never answers until `release_hanging` or 30 s, whichever first |
| `POST /oneclick/redirect/{code}` | `code` in 301, 302, 303, 307, 308 with `Location: /landed` |
| `ANY /landed` | 200; tests assert it was never reached |
| `GET /__testbed/requests` | JSON list of `Recorded` (binary mode only, for `scripts/e2e.sh`) |
| `POST /__testbed/reset` | Clears records and counters (binary mode only) |

## Algorithm

1. `start()` builds one shared `Arc<Mutex<State>>` (records, per-key counters, a `tokio::sync::Notify` for hanging requests).
2. Every handler first records the request (method, path, lower-cased header names and values, body bytes, `cookies_present = headers contain "cookie"`), then applies its scenario.
3. HTTPS: generate a test CA and a leaf certificate for `127.0.0.1` and `localhost` at start-up with `rcgen` `[DEFAULT: dev-dependency only]`, serve with `axum-server` plus `rustls`. `start_with_untrusted_cert` uses a second, unrelated self-signed leaf.
4. `SSRF_CASES` (S10 6.2), each host under `.test`:
   `loopback4.test` 127.0.0.1; `private10.test` 10.0.0.1; `private172.test` 172.16.0.1; `private192.test` 192.168.1.1; `cgnat.test` 100.64.0.1; `linklocal.test` 169.254.1.1; `metadata-ip.test` 169.254.169.254; `loopback6.test` ::1; `linklocal6.test` fe80::1; `ula6.test` fd00::1; `mapped6.test` ::ffff:127.0.0.1; `mixed.test` 93.184.216.34 then 10.0.0.1 (two answers, one private). `metadata.google.internal` is tested by name in T-702 without this table.
5. The binary prints `TESTBED_ADDR=<addr> TESTBED_HTTPS_ADDR=<addr>` on one line so `scripts/e2e.sh` can read it.
6. The `/__testbed/*` control routes exist only in the binary's router; `start()` used in-process exposes the same data through `Testbed` methods.

## Acceptance criteria

This task proves no AC by itself. It is the fixture T-702 uses to prove UN-02 AC1, AC3 and AC4.

## Tests that must pass

- `testbed_records_method_headers_body_cookies` (integration: a `reqwest` POST with a cookie is recorded exactly)
- `testbed_status_routes_return_their_status` (integration: 200, 202, 204, 400, 500)
- `testbed_500_then_200_per_key` (integration: two keys are independent)
- `testbed_fail_n_then_succeeds` (integration)
- `testbed_redirect_sets_location_and_landed_unreached` (integration with a client that does not follow redirects)
- `testbed_hang_released_on_demand` (integration with `tokio::time::pause` not needed: `release_hanging` returns the response)
- `testbed_https_trusted_with_test_ca` and `testbed_untrusted_cert_fails_tls` (integration)
- `testbed_ssrf_table_covers_s10_ranges` (unit: every range named in S10 6.2 has at least one case)
- `testbed_binds_port_zero_in_parallel` (integration: two testbeds at once)

## Edge cases and traps

- Bind only to `127.0.0.1`, never `0.0.0.0`.
- Never sleep for real in tests; the hang route waits on `Notify` and a 30 s ceiling only protects a forgotten test.
- Record before responding, so a client timeout still leaves a record.
- Do not depend on `unsub`, `egress` or `api`; this crate must build first.
- Domains in any fixture are reserved names only (`.test`, `example.com`); `93.184.216.34` is used only as a "public" answer and is never contacted (the resolver is fake).
- `rcgen`, `axum-server` and `rustls` are dev or testbed dependencies; check `cargo deny` licences pass.

## Out of scope

- Page, stop-and-ask and hostile page routes (S10 6.2): v2 with the page handler.
- The fake resolver itself and the egress policies: T-306.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `cargo run -p unsub-testbed` prints both addresses and serves `/oneclick/200`.
