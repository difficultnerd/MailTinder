# T-1010: On-device category name proposal

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 150 lines of code plus tests | T-1003 |

**Read only these spec sections:** S2 FL-02 (`docs/specs/S2-v1-acceptance-criteria.md`); S9 section 4, "New category" control (`docs/specs/S9-functional-screens.md`); CONTEXT decision D9 (header rules, then Gemini Nano on device). Nothing else is needed.

## Goal

When the browser offers an on-device language model, the filing sheet's "New category" field is pre-filled with a short proposed name, which the user can edit. When no model is available the field stays empty, exactly as T-1003 built it. Nothing leaves the device.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/platform/on_device_model.dart` | Conditional export of the web and stub implementations |
| Create | `app/lib/platform/on_device_model_web.dart` | Feature detection and one prompt through the browser's built-in model |
| Create | `app/lib/platform/on_device_model_stub.dart` | Always unavailable (tests, non-web) |
| Create | `app/lib/state/name_proposer.dart` | `OnDeviceNameProposer` implementing T-1003's `NameProposer` |
| Change | `app/lib/screens/filing/filing_sheet.dart` | Use `OnDeviceNameProposer` in place of the null proposer |
| Create | `app/test/state/name_proposer_test.dart` | Unit tests with a fake model |
| Change | `app/test/screens/filing/filing_sheet_test.dart` | Widget tests for AC2 |

## Types and signatures

```dart
/// Thin wrapper over the browser's on-device model. Web: the built-in Prompt API
/// (`LanguageModel` global in current Chrome) [ASSUMES]; stub elsewhere.
abstract class OnDeviceModel {
  Future<bool> isAvailable();                 // true only when the model is ready without a download prompt
  Future<String?> prompt(String text, {Duration timeout});
}

class OnDeviceNameProposer implements NameProposer {
  OnDeviceNameProposer(this.model, {this.timeout = const Duration(milliseconds: 800)});
  final OnDeviceModel model;
  final Duration timeout;
  @override
  Future<String?> propose({required String senderDisplay, required String subject, required List<String> existing});
}
```

## Algorithm

1. `isAvailable()`: feature-detect the global; call its availability check; return true only for the "available" state. Any exception returns false. Never trigger a model download `[DEFAULT]`: a download is a user-visible cost the spec does not ask for.
2. `propose`: if unavailable, return null. Build a fixed prompt: "Suggest a folder name of one or two words for an email from {senderDisplay} with subject {subject}. Existing folders: {existing}. Reply with the name only." Truncate subject to 120 characters.
3. Call `prompt` with the 800 ms timeout `[DEFAULT]` (the filing sheet's 200 ms suggestion target applies to the server suggestion, not this field; the field fills in when ready).
4. Clean the reply: first line only, strip quotes and punctuation at the ends, collapse spaces, at most 30 characters, title case. Reject empty results and anything containing `@`, `http` or digits only. If the cleaned name matches an existing category (case-insensitive), return that existing name.
5. The filing sheet shows the field empty at once and fills it only if the user has not typed yet.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FL-02 AC2 | With an on-device model available, "New category" is pre-filled with an editable proposed name |

## Tests that must pass

- `'FL-02 AC2 proposed name is pre-filled and editable'` (widget, fake model returns "Receipts")
- `'FL-02 AC2 no model leaves the field empty'` (widget, fake model unavailable)
- `'FL-02 AC2 typing before the proposal arrives is never overwritten'` (widget)
- `'FL-02 AC2 reply is cleaned and capped at 30 characters'` (unit)
- `'FL-02 AC2 model error or timeout returns null'` (unit)

## Edge cases and traps

- Do not send the prompt anywhere but the on-device model; no network call, no logging of sender or subject.
- Do not store the proposal or the prompt in browser storage.
- The stub must compile under `flutter test` (conditional import on `dart.library.js_interop`).
- Do not block the sheet on the model; the field must be usable at once.

## Out of scope

- Using the on-device model for the card badge or filing suggestion (later, after the bake-off).
- Downloading a model.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Manually checked once in a Chrome build with the on-device model enabled; the pull request notes the Chrome version, or says it could not be checked.
