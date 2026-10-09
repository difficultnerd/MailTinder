# T-1111: Screenshot (golden) and accessibility regression tests for the main screens

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 350 lines of code plus tests | T-1001a, T-1002b, T-1003, T-1005, T-1006a |

**Read only these spec sections:** S9 (screens), S10 section on Flutter tests, `app/test/` existing widget tests (the style to copy). Nothing else is needed.

## Goal

A machine notices when a screen stops looking or behaving right: golden screenshots at three phone widths for the main screens, and automated accessibility guideline checks. A human reviews only the image diff when a golden changes.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/test/golden/screens_golden_test.dart` | Golden tests, tag `golden`, for: sign-in, invite, Feed (with the corpus), filing sheet, Needs Attention, Settings, at widths 360, 390 and 430 logical pixels |
| Create | `app/test/golden/a11y_test.dart` | For each of those screens: `meetsGuideline(androidTapTargetGuideline)`, `iOSTapTargetGuideline`, `labeledTapTargetGuideline`, `textContrastGuideline`; every interactive element has a semantics label |
| Create | `app/test/golden/goldens/` | The committed PNGs |
| Create | `tools/update_goldens.sh` | `flutter test --update-goldens --tags golden`, then prints the list of changed images |
| Create | `docs/golden-tests.md` | How to review a golden diff, when to update, the fonts decision below |

## Behaviour

1. Use seeded fake data and a fixed clock so screens are deterministic; no network.
2. **Fonts:** flutter test renders text with a placeholder font unless fonts are loaded. Load the app's bundled fonts in a test helper (`loadAppFonts`) so goldens show real text; if that is not possible, say so in `docs/golden-tests.md` and state that the goldens are layout-only.
3. Goldens are generated and verified on Linux only; pin the Flutter version already pinned for the repo. A golden mismatch fails the test and writes the diff images where the CI artifact step already looks, or to `app/test/golden/failures/`.
4. The a11y test fails on any violation; no blanket skips. If a screen genuinely violates a guideline, fix the screen in this PR.
5. Update `tools/ac_coverage_enforced.txt` only if the existing tracker requires it for the new tests.

## Acceptance criteria

- `golden_screens_match_at_three_widths` (18 images).
- `a11y_main_screens_meet_tap_target_label_and_contrast_guidelines`.
- `golden_test_fails_when_a_widget_changes` (shown once in the PR body: change a colour locally, show the failure output, revert).

## Out of scope

Dark mode, tablet layouts, animations, device farms.

## Done when

The tests pass, the belt is green, the PR body shows one example failing diff and lists every a11y fix; definition of done in S10 10.4. Do not edit `.github/workflows/`.
