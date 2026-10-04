# T-006: Flutter web CSP with Trusted Types spike

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M0 | strong | about 250 lines of config, script and tests | T-007 |

**Read only these spec sections:** S6 3 row T2 (`docs/specs/S6-security.md`); S10 7.2 rows "Headers" and "CSP compatibility" and S10 3.3 last bullet (`docs/specs/S10-test-strategy.md`); S7 2 rows "Base path" and "Security headers" (`docs/specs/S7-api-contract.md`); ASVS register section V3.4 (rows V3.4.1 to V3.4.6) and V3.7 (`docs/security/asvs-l2-register.md`); `docs/planning-roadmap.md` "Backlog seeds for S12" last bullet. Nothing else is needed.

## Goal

Prove that the Flutter web build runs under a strict Content Security Policy with Trusted Types, and ship that policy. After this task `firebase.json` exists with the Hosting headers (CSP, HSTS, `nosniff`, `Referrer-Policy`, `Permissions-Policy`, `frame-ancestors 'none'`) and the `/api/**` rewrite, a script loads the built app in headless Chrome under exactly those headers and fails on any CSP violation, and a short decision note records what the policy needed and why. The e2e harness (T-1101) later reuses the same headers to complete a swipe under the shipped CSP.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `firebase.json` | Hosting config: public folder, rewrites, headers |
| Change | `app/web/index.html` | Title and description "Mail Tinder"; no inline script or style; keep `flutter_bootstrap.js` |
| Change | `app/web/manifest.json` | Name and short name "Mail Tinder"; description |
| Create | `app/assets/fonts/` plus `app/pubspec.yaml` `fonts:` entry | Self-hosted text font (Roboto, Apache-2.0) so no font is fetched from a CDN at start-up |
| Change | `app/lib/app.dart` | `ThemeData(fontFamily: 'Roboto')` pointing at the bundled font |
| Create | `scripts/csp_serve.py` | Stdlib static server that applies `firebase.json` headers and collects CSP reports |
| Create | `scripts/csp_check.sh` | Builds the app, serves it, loads it in headless Chrome, fails on violations |
| Create | `app/test/firebase_headers_test.dart` | Asserts the shipped header values |
| Create | `docs/decisions/0001-flutter-web-csp.md` | One-page result of the spike |
| Change | `.github/workflows/ci.yml` | New job `csp-smoke` (not required yet; T-1101's `e2e` is the required gate) |

## Types and signatures

`firebase.json` starting point (the spike may only tighten it, or loosen it with a recorded reason):

```json
{
  "hosting": {
    "public": "app/build/web",
    "ignore": ["firebase.json", "**/.*"],
    "cleanUrls": false,
    "rewrites": [
      { "source": "/api/**", "run": { "serviceId": "api", "region": "us-central1" } },
      { "source": "**", "destination": "/index.html" }
    ],
    "headers": [
      {
        "source": "**",
        "headers": [
          { "key": "Content-Security-Policy", "value": "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; worker-src 'self' blob:; manifest-src 'self'; base-uri 'none'; object-src 'none'; form-action 'none'; frame-ancestors 'none'; require-trusted-types-for 'script'; trusted-types flutter-js flutter-engine" },
          { "key": "Strict-Transport-Security", "value": "max-age=31536000; includeSubDomains" },
          { "key": "X-Content-Type-Options", "value": "nosniff" },
          { "key": "Referrer-Policy", "value": "no-referrer" },
          { "key": "Permissions-Policy", "value": "camera=(), microphone=(), geolocation=(), payment=(), usb=(), interest-cohort=()" },
          { "key": "Cross-Origin-Opener-Policy", "value": "same-origin-allow-popups" }
        ]
      },
      {
        "source": "/index.html",
        "headers": [ { "key": "Cache-Control", "value": "no-cache" } ]
      }
    ]
  }
}
```

```python
# scripts/csp_serve.py (stdlib only: http.server, json, fnmatch, threading)
# Usage: python3 scripts/csp_serve.py --root app/build/web --config firebase.json --port 0 --reports <file>
# Serves files under --root; unknown paths serve index.html (the "**" rewrite); applies every header whose
# "source" glob matches; appends "; report-uri /__csp-report" to Content-Security-Policy;
# POST /__csp-report appends the body as one JSON line to --reports; prints "PORT <n>" on stdout once bound.
```

```bash
# scripts/csp_check.sh
# 1. flutter build web --release --no-web-resources-cdn --pwa-strategy=none  (in app/)
# 2. start csp_serve.py on port 0, read the port
# 3. google-chrome --headless=new --disable-gpu --no-sandbox --virtual-time-budget=20000 --dump-dom http://127.0.0.1:<port>/ > dom.html
# 4. fail if the reports file has any line; fail if dom.html has neither "flutter-view" nor "flt-glass-pane"
# 5. kill the server; print a one-line result
```

## Algorithm

1. Write `firebase.json` as above. Write the Dart header test so the policy is pinned before you start changing it.
2. Self-host everything: `--no-web-resources-cdn` bundles CanvasKit with the app (no `gstatic.com` script), and the bundled Roboto font replaces the default font download. Set `ThemeData(fontFamily: 'Roboto')`.
3. Run `scripts/csp_check.sh`. For each violation report, find which Flutter feature needs it and choose the narrowest fix, in this order of preference: change the build or bootstrap config; add a named Trusted Types policy to `trusted-types`; add a single source to one directive. Never add `'unsafe-eval'` to `script-src`, never add a remote host to `script-src`, never drop `require-trusted-types-for 'script'`.
4. Investigate in particular: whether the engine and `flutter.js` create Trusted Types policies under the names `flutter-js` and `flutter-engine` (and whether `'allow-duplicates'` is needed after a hot restart, which does not happen in release); whether `style-src 'unsafe-inline'` can be dropped (the engine injects `<style>` elements; if it cannot, record why: inline styles cannot run script, and `script-src` stays strict); whether CanvasKit needs `worker-src blob:`; whether fallback fonts for non-Latin text are fetched from `fonts.gstatic.com`.
5. **Fallback fonts** `[DEFAULT]`: if glyphs outside Roboto (for example subjects in other scripts, which the hostile corpus has) trigger a fetch from `https://fonts.gstatic.com/`, allow exactly that origin in `font-src` and `connect-src` and record it in the decision note. Reason: Google is already the user's mail provider, and self-hosting the whole Noto set costs tens of megabytes. If `fontFallbackBaseUrl` in the engine configuration can point at self-hosted files cheaply, prefer that and say so.
6. When `csp_check.sh` passes with zero reports and a rendered app, write `docs/decisions/0001-flutter-web-csp.md`: the final policy, each non-obvious directive with one line of reason, what was tried and dropped, the Flutter version used, and what to re-check on a Flutter upgrade.
7. Add the CI job (ubuntu runners ship Chrome):

```yaml
  csp-smoke:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
      - uses: subosito/flutter-action@1a449444c387b1966244ae4d4f8c696479add0b2 # v2
        with:
          channel: stable
          cache: true
      - run: flutter pub get
        working-directory: app
      - run: scripts/csp_check.sh
```

8. Header test assertions (`app/test/firebase_headers_test.dart`, reading `../firebase.json` from the `app/` working directory): CSP contains `object-src 'none'`, `base-uri 'none'`, `frame-ancestors 'none'`, `require-trusted-types-for 'script'`, a `trusted-types` directive, `default-src 'none'`; `script-src` has no `'unsafe-eval'`, no `'unsafe-inline'`, no `http:` or `https:` source and no `*`; HSTS `max-age` is at least 31536000 and has `includeSubDomains`; `X-Content-Type-Options` is `nosniff`; `Referrer-Policy` is `no-referrer`; the `/api/**` rewrite targets service `api` in `us-central1` and comes before the `**` rewrite.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| V3.4.1 | Hosting sends HSTS with a one-year `max-age` and `includeSubDomains` (the `api` half is T-500) |
| V3.4.3 | Hosting sends a CSP with `object-src 'none'`, `base-uri 'none'` and Trusted Types, and the app runs under it |
| V3.4.4 | Hosting sends `X-Content-Type-Options: nosniff` |
| V3.4.5 | Hosting sends `Referrer-Policy: no-referrer` |
| V3.4.6 | Hosting sends CSP `frame-ancestors 'none'` |

## Tests that must pass

- `'ASVS V3.4.1 hosting sends hsts one year with subdomains'` (unit, Dart)
- `'ASVS V3.4.3 hosting csp is strict with trusted types'` (unit, Dart)
- `'ASVS V3.4.3 script-src has no unsafe-eval inline or remote source'` (unit, Dart)
- `'ASVS V3.4.4 hosting sends nosniff'` (unit, Dart)
- `'ASVS V3.4.5 hosting sends no-referrer'` (unit, Dart)
- `'ASVS V3.4.6 hosting csp forbids framing'` (unit, Dart)
- `'api rewrite precedes spa rewrite'` (unit, Dart)
- `scripts/csp_check.sh` exits 0: the release build renders under the shipped headers with zero CSP reports (CI `csp-smoke`).

## Security review checklist

The reviewer confirms each item before merge:

- [ ] `script-src` is `'self'` plus `'wasm-unsafe-eval'` only: no `'unsafe-eval'`, `'unsafe-inline'`, nonce-less inline script, `data:`, `blob:` or remote host.
- [ ] `require-trusted-types-for 'script'` is present and every name in `trusted-types` is one Flutter itself creates (checked in the built `main.dart.js` or `flutter.js`), not a catch-all `*`.
- [ ] `object-src 'none'`, `base-uri 'none'`, `frame-ancestors 'none'`, `form-action 'none'` and `default-src 'none'` are present.
- [ ] Any remote origin in `font-src` or `connect-src` is exactly `https://fonts.gstatic.com` with a recorded reason, or absent; `connect-src` does not allow any API host other than `'self'`.
- [ ] `index.html` has no inline `<script>`, no inline event handler and no inline `<style>`.
- [ ] `csp_serve.py` adds `report-uri` only in the test server; `firebase.json` has no report endpoint (none exists in production).
- [ ] The `/api/**` rewrite targets the `api` service in `us-central1` and is listed before the SPA fallback, so API paths can never return `index.html`.
- [ ] HSTS `max-age=31536000; includeSubDomains`, `nosniff`, `no-referrer` and `Permissions-Policy` are on `**`.
- [ ] The decision note lists every directive that is looser than the starting point and why.
- [ ] `csp_check.sh` really fails on a violation: the pull request shows one run with a deliberately broken policy (for example `script-src 'none'`) failing.

## Edge cases and traps

- `require-trusted-types-for` blocks any `innerHTML` or script URL assignment without a policy. If Flutter fails to start, read the console error for the policy name it tried to create; do not guess names.
- `--no-web-resources-cdn` is required; without it CanvasKit loads from `www.gstatic.com`, which `script-src 'self'` blocks.
- Do not use `--wasm` (skwasm) in this spike unless CanvasKit fails; it adds cross-origin isolation headers to the problem. Note in the decision record if you tried it.
- `--pwa-strategy=none` avoids a service worker, which would cache app files and complicate updates and CSP.
- Use `127.0.0.1`, not `localhost`, so Chrome does not treat the page differently; HSTS is ignored on plain http loopback, which is expected in the smoke test.
- `--virtual-time-budget` lets the page finish starting without a fixed sleep; do not add `sleep` to the script.
- `csp_serve.py` must serve `.wasm` as `application/wasm` and `.js` as `text/javascript` (with `nosniff`, a wrong type blocks loading). Use `mimetypes.add_type`.
- Python only in `scripts/`; standard library only. Shell scripts start with `set -euo pipefail` and clean up the server with a `trap`.
- Roboto is Apache-2.0; keep its licence file beside the font files.
- The `api` service also sets its own CSP on `run.app` responses (S7 2); that is T-500, not this task.

## Out of scope

- Completing a swipe under the CSP and failing on violation reports during the e2e journeys: T-1101.
- The `api` response headers: T-500. Deploying Hosting: T-1104.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The security review checklist is pasted into the pull request with every item ticked or explained.
- `docs/decisions/0001-flutter-web-csp.md` is merged with the final policy.
