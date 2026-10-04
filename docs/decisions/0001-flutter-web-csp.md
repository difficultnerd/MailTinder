# ADR 0001: Flutter web Content Security Policy with Trusted Types

Status: accepted (spike T-006)
Date: 4 October 2026
Flutter: 3.47.6 stable, web target, CanvasKit renderer

## Result

The Flutter web release build renders under a strict Content Security Policy
with Trusted Types enforced. The policy ships in `firebase.json` and is
verified by the `csp-smoke` CI job, which loads the built app in headless
Chrome under exactly the shipped headers and fails on any violation report.

## Final policy

```
default-src 'none';
script-src 'self' 'wasm-unsafe-eval';
style-src 'self' 'unsafe-inline';
img-src 'self' data: blob:;
font-src 'self' https://fonts.gstatic.com;
connect-src 'self' https://fonts.gstatic.com;
worker-src 'self' blob:;
manifest-src 'self';
base-uri 'none';
object-src 'none';
form-action 'none';
frame-ancestors 'none';
require-trusted-types-for 'script';
trusted-types flutter-js flutter-engine
```

## Why each non-obvious directive

- `script-src 'self' 'wasm-unsafe-eval'`: only the app's own scripts load.
  `'wasm-unsafe-eval'` is required by the CanvasKit engine's WebAssembly
  module. `'unsafe-eval'` (JS eval) is never allowed, and no remote host is
  allowed in `script-src`.
- `style-src 'self' 'unsafe-inline'`: the Flutter engine injects `<style>`
  elements at runtime. Inline styles cannot execute script, and `script-src`
  stays strict, so this is safe. Dropping it would break rendering.
- `worker-src 'self' blob:`: CanvasKit may spawn a worker from a blob URL.
- `require-trusted-types-for 'script'` with `trusted-types flutter-js
  flutter-engine`: the engine and `flutter.js` create Trusted Types policies
  under these names. No catch-all `*` is used.
- `font-src 'self' https://fonts.gstatic.com`: the Roboto font is bundled, so no
  font is fetched at start-up; the engine falls back to Noto Sans (loaded from
  `fonts.gstatic.com`) for glyphs Roboto lacks (e.g. CJK/emoji). Exactly that
  origin is allowed as the documented fallback-font decision below.
- `connect-src 'self' https://fonts.gstatic.com`: the app talks to its own
  origin (`/api/**`) plus exactly the fallback font CDN above.
- `frame-ancestors 'none'`, `object-src 'none'`, `base-uri 'none'`,
  `form-action 'none'`, `default-src 'none'`: deny-by-default for framing,
  plugins, base URL and forms. The Flutter `index.html` template's default
  `<base href="/">` element is REMOVED from the template, so `base-uri 'none'`
  produces no violation; the app is served at the root, where omitting the base
  element is equivalent.

## What was tried and dropped

- `--wasm` (skwasm) renderer: not used. It adds cross-origin isolation headers
  to the problem; CanvasKit works under the policy above.
- Dropping `style-src 'unsafe-inline'`: the engine injects `<style>` elements,
  so it cannot be dropped in this Flutter version.
- `'allow-duplicates'` in `trusted-types`: not needed in a release build (no
  hot restart).

## Fallback fonts

Triggered by the release-build smoke test (14k+ Noto Sans / Noto Sans SC
glyphs fetched from `fonts.gstatic.com` at runtime). The decision above is in
force: exactly `https://fonts.gstatic.com` is allowed in `font-src` and
`connect-src`. It is safe because Google is already the user's mail provider,
and self-hosting the whole Noto set costs tens of megabytes. If it ever
becomes cheap, prefer `fontFallbackBaseUrl` pointing at self-hosted files and
drop the origin from both directives.

## Re-check on Flutter upgrade

- Whether the engine still creates Trusted Types policies named `flutter-js`
  and `flutter-engine`.
- Whether `style-src 'unsafe-inline'` can be dropped.
- Whether CanvasKit still needs `worker-src blob:`.
- Whether `--no-web-resources-cdn` still bundles CanvasKit (no `gstatic.com`).
