# Golden (screenshot) tests

T-1111 adds screenshot regression tests for the six main screens so a machine
notices when a screen stops looking right. They live in
`app/test/golden/`:

- `screens_golden_test.dart` — one golden per screen per phone width.
- `a11y_test.dart` — the accessibility guidelines for the same screens.
- `screens.dart` — the shared builders (seeded fake data, a fixed clock, the
  bundled fonts).
- `goldens/` — the committed PNGs, one per screen per width.
- `failures/` — where a mismatch writes its diff images.

## What is covered

Six screens — sign-in, Request invite, Feed (with the synthetic corpus), the
filing sheet, Needs Attention and Settings — at 360, 390 and 430 logical
pixels: 18 images.

Every screen is built from `FakeApiClient` seeded data and uses a fixed clock,
so the output is deterministic and no network is touched.

## Fonts

`flutter test` renders text with a placeholder font: `flutter_tester` runs with
`--use-test-fonts`, so every glyph is drawn as a filled square and a custom
font cannot be substituted. `loadAppFonts()` in `screens.dart` still attempts to
register the app's bundled Roboto faces through a `FontLoader`, but on this
engine it has no effect — the captured glyphs are the test font's squares. The
goldens are therefore **layout-only**: they prove structure, spacing, colours
and text lengths, not glyph shapes. Loading real fonts here would require a
binding that does not force the test font, which the repository does not use.

## Platform and version

Goldens are generated and verified on Linux only. They were captured with
Flutter 3.47.6 stable, the version recorded in
`docs/decisions/0001-flutter-web-csp.md` for this repository; a different
Flutter or a non-Linux host can shift glyph rasterisation and will report a
false mismatch. Regenerate only on Linux with that version.

CI does **not** pin that version: the `dart` job in
`.github/workflows/ci.yml` runs `subosito/flutter-action` with `channel: stable`
and no `flutter-version`, so a stable-channel bump can change rasterisation and
fail `flutter test` for every pull request. Pinning the CI version is tracked as
`docs/backlog/T-1111a-pin-flutter-version-in-ci.md`.

## How to review a golden diff

1. Run `tools/update_goldens.sh` to regenerate the images.
2. Open the changed files under `app/test/golden/goldens/` and look at each
   one. Confirm the change is what the code change intended and that nothing
   else moved.
3. Check `git diff --stat app/test/golden/goldens` — the list of images the
   script printed must match the change you meant to make. An unexpected image
   is a regression: investigate the screen, do not just re-baseline it.
4. Commit the changed PNGs in the same pull request as the code that caused
   them.

If a golden fails in CI it writes the master, test and diff images to
`app/test/golden/failures/`; the failing test names the screen and width.

## Proving the goldens are a control

Two things show the goldens catch a change, not merely record one.

`golden_test_fails_when_a_widget_changes` in `screens_golden_test.dart` renders
the sign-in screen at 360 logical pixels and runs the golden comparison twice:
once against the committed 360-pixel golden (the same rendering, which must
match) and once against the 390-pixel golden (a different rendering, which the
comparison must reject). A recording comparator captures the rejection so the
test can assert it while the run stays green. If the comparison ever stopped
detecting a difference, the test fails.

The same failure by hand, from a real run — change a colour in a screen (here
the scaffold background in the shared `_theme()`), run
`flutter test --tags golden`, and read the mismatch:

    Golden "goldens/sign_in_360.png": Pixel test failed, 82.94%, 238872px diff detected.
    Failure feedback can be found at app/test/golden/failures
    ...
    golden_screens_match_at_three_widths sign_in at 360px [E]
      Test failed. See exception logs above.

Revert the colour and the suite is green again. The master, test and diff images
are left in `app/test/golden/failures/` (gitignored) for inspection.

## When to update a golden

Update it when the visible change was intended and reviewed — a real copy,
layout or colour change. Never update one to silence a failure you have not
explained: a screenshot changing for no visible code reason is exactly the
regression these tests exist to catch.

## Accessibility

`a11y_test.dart` runs four guidelines over the same six screens:
`androidTapTargetGuideline`, `iOSTapTargetGuideline`,
`labeledTapTargetGuideline` and `textContrastGuideline`. Each screen at each
width is a separate test, so a run reports every violation rather than only the
first, and the semantics handle is released in a `finally` even when a
guideline fails. A violation fails the suite; there are no blanket skips, and a
genuine violation is fixed in the screen rather than waived.
