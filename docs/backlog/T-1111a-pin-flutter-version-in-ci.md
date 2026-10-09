# T-1111a: Pin the Flutter version CI uses

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small | T-1111 |

**Follow-up from:** T-1111's review finding F2.

## Goal

The golden screenshots in `app/test/golden/goldens/` are captured with Flutter
3.47.6 on Linux, the version recorded in
`docs/decisions/0001-flutter-web-csp.md`. CI runs `flutter test` on whatever
`channel: stable` resolves to, so a stable-channel bump can change rasterisation
and fail every pull request — and the usual response, re-baselining the goldens,
weakens the control. Make CI use the pinned version so a golden failure means a
real UI change.

## Files

| Action | Path | What |
| --- | --- | --- |
| Edit | `.github/workflows/ci.yml` | Add `flutter-version: 3.47.6` to the `subosito/flutter-action` step in the `dart` job (and any other job that runs `flutter test`) |
| Edit | `docs/golden-tests.md` | Update "Platform and version" once CI pins the version |

## Behaviour

1. The pinned value matches `docs/decisions/0001-flutter-web-csp.md`; if that
   decision changes, both move together.
2. When the pin and the goldens disagree, prefer regenerating the goldens on
   the pinned version, not on whatever stable resolves to.

## Acceptance criteria

- `ci_pins_the_flutter_version_recorded_in_the_decision`.

## Out of scope

Bumping the Flutter version, re-baselining the goldens, any other CI job.

## Done when

CI pins the version, the belt is green, and `docs/golden-tests.md` no longer
says the version is unpinned.
