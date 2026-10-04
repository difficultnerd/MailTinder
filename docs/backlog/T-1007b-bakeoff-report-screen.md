# T-1007b: Bake-off report screen

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 450 lines of code plus tests | T-1007a |

**Read only these spec sections:** S9 section 7.7 (`docs/specs/S9-functional-screens.md`); S7 sections 4 (`versions_mixed`, `snapshot_limit`), 5.13 (API-ADM-8 to API-ADM-14, "Rules for every section", "CSV") (`docs/specs/S7-api-contract.md`) and the `BakeoffReport`, `Ci`, `SnapshotSummary`, `Snapshot`, `ClassifierExperiment`, `VersionsPresent` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 CL-04. Nothing else is needed.

## Goal

Admins see the bake-off report as plain tables (visual design comes later): per-method figures with intervals, the paired comparison, segments, the latency histogram and the daily trend, with suppressed cells shown as "Too few to show". They can filter, flip the two kill switches, save, list, open, download and delete snapshots, with writes behind Confirm it's you.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/api/models/bakeoff.dart` | `Ci`, `BakeoffQuery`, `BakeoffReport` and its parts, `SnapshotSummary`, `Snapshot`, `ClassifierExperiment` |
| Create | `app/lib/state/bakeoff_model.dart` | `BakeoffModel` |
| Create | `app/lib/screens/admin/bakeoff_screen.dart` | Report, filters, kill switches, snapshots |
| Create | `app/lib/screens/admin/bakeoff_tables.dart` | Table widgets |
| Change | `app/lib/platform/browser.dart` and both implementations | Add `saveFile` |
| Change | `app/lib/api/api_client.dart` (T-007's `ApiException`) | Add `final Map<String, dynamic>? problem;` holding the raw problem body |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | Methods below |
| Change | `app/lib/screens/settings/settings_entries.dart` | Bake-off report (`adminOnly`) |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/fixtures/bakeoff_report.json` | Synthetic report with suppressed cells |
| Create | `app/test/screens/bakeoff_test.dart` | Tests below |

## Types and signatures

```dart
class Ci { final double? value, lower, upper; final int? n; bool get suppressed => value == null; }
class BakeoffQuery { final DateTime from, to; final String? inputVersion, questionVersion, priceVersion; final bool poolVersions;
                     Map<String, String> toQuery(); Map<String, dynamic> toJson(); }
class BakeoffReport { /* every field in the OpenAPI BakeoffReport, nullable where the schema says so */
  factory BakeoffReport.fromJson(Map<String, dynamic> j); }
class SnapshotSummary { final String snapshotId, name; final DateTime createdAt; final int? labelledSwipes; }
class Snapshot { final SnapshotSummary summary; final BakeoffReport report; }
class ModelSwitch { final String model; final String classifierId; final bool enabled; final DateTime changedAt; }
class ClassifierExperiment { final List<ModelSwitch> models; final int participants; }

abstract class ApiClient {
  Future<BakeoffReport> getBakeoff(BakeoffQuery q);                          // ADM-10 JSON
  Future<List<int>> getBakeoffCsv(BakeoffQuery q);                           // ADM-10, Accept: text/csv
  Future<ClassifierExperiment> getClassifierExperiment();                    // ADM-8
  Future<ClassifierExperiment> setModelEnabled(String model, bool enabled);  // ADM-9 {models:[{model, enabled}]}
  Future<Snapshot> createSnapshot(String name, BakeoffQuery q);              // ADM-11
  Future<Paged<SnapshotSummary>> listSnapshots({String? cursor});            // ADM-13
  Future<Snapshot> getSnapshot(String id);                                   // ADM-12 JSON
  Future<List<int>> getSnapshotCsv(String id);                               // ADM-12, Accept: text/csv
  Future<void> deleteSnapshot(String id);                                    // ADM-14
}

abstract class Browser { /* added */ void saveFile(List<int> bytes, String fileName, String mimeType); }

enum BakeoffState { loading, ready, notEnoughData, versionsMixed, failed }
class BakeoffModel extends ChangeNotifier {
  BakeoffModel({required ApiClient api, DateTime Function()? now});
  BakeoffState get state; BakeoffReport? get report; Map<String, List<String>>? get versionsPresent;
  BakeoffQuery get query; set query(BakeoffQuery q);   // setting reloads
  String? get lastSavedName;                           // drives the "snapshot saved" state
}
```

## Algorithm

1. Default query: last 30 days to today `[DEFAULT]`, no version filters, `poolVersions` false. Load report, kill switches and the snapshot list in parallel.
2. States: `loading` spinner; `ready`; `notEnoughData` when `labelledSwipes` is null or below `minCellSize`, showing `Copy.notEnoughData` `[DEFAULT]` (filters still visible); `versionsMixed` on `409 versions_mixed`: read `problem['versions_present']`, show `Copy.pickOneVersion` (S7) with dropdowns for input and question version filled from it; choosing sets the filter and reloads (CL-04 AC5). `failed` shows `Copy.actionFailed`.
3. Filters: two date pickers (from, to), input version and question version dropdowns from `versionsPresent` plus "Any". Price version is not offered (it never blocks pooling).
4. Rendering, all as `Table` or `DataTable` with `Text` cells:
   - Header: labelled swipes, participants count and top contributor share.
   - Per method (header_rules, gemini, jev): cards, valid answers, accuracy, junk precision, junk recall, junk F1, false junk rate, ECE, p50 and p95 latency, timeout rate, error rate, cost per 1,000 messages.
   - Paired: each pair with the four counts, `n_paired`, McNemar p, accuracy difference with interval, bootstrap method.
   - Agreement, segments (`by_segment`), confusion, latency histogram bins, daily trend.
   - A `Ci` prints as `0.870 (0.863 to 0.877), n 8,312`; a suppressed `Ci`, null count or null rate prints `Copy.tooFewToShow` (CL-04 AC4). Null `lower`/`upper` with a value prints the value only.
5. Kill switches (CL-04 AC1, AC8): one switch per model from ADM-8. Changing one runs `stepUp.run(waitingActionLabel: Copy.stepUpKillSwitch(model, on), action: () => api.setModelEnabled(model, on))`; revert the switch if the result is null or an error.
6. Save snapshot (CL-04 AC6): name field (1 to 100 characters, plain text) and "Save snapshot" running `stepUp.run(waitingActionLabel: Copy.stepUpSaveSnapshot, action: () => api.createSnapshot(name, query))`. Success shows `Copy.snapshotSaved` (S9 state "snapshot saved") and refreshes the list. `409 snapshot_limit` shows `Copy.deleteSnapshotFirst` (S7). `409 versions_mixed` goes to the versions-mixed state.
7. Snapshot list: name, `formatDate(createdAt)`, card count. Tap opens the snapshot's report read-only (same tables). Each row has "Download CSV" and "Delete" (confirm `Copy.deleteSnapshotQuestion(name)`, step-up `Copy.stepUpDeleteSnapshot`).
8. Download CSV (CL-04 AC7): fetch bytes with `Accept: text/csv`, then `browser.saveFile(bytes, 'bakeoff-report.csv', 'text/csv')` for the live report and `'bakeoff-snapshot.csv'` for a snapshot. Web `saveFile`: `Blob` plus an `<a download>` element clicked and revoked.

Copy (S9 verbatim unless marked): `tooFewToShow` "Too few to show"; `notEnoughData` `[DEFAULT]` "Not enough data yet. Come back when more swipes are in."; `pickOneVersion` "Pick one version" (S7 section 4); `snapshotSaved` `[DEFAULT]` "Snapshot saved."; `deleteSnapshotFirst` "Delete a snapshot first" (S7 section 4); `saveSnapshot` "Save snapshot"; `downloadCsv` "Download CSV"; `deleteSnapshotQuestion(n)` `[DEFAULT]` "Delete the snapshot $n?"; `stepUpKillSwitch(m, on)` `[DEFAULT]` "to switch $m on" or "to switch $m off" (m shown as "Gemini" or "Jev"); `stepUpSaveSnapshot` `[DEFAULT]` "to save a snapshot"; `stepUpDeleteSnapshot` `[DEFAULT]` "to delete a snapshot"; `bakeoffReport` "Bake-off report".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-04 AC1 | A kill switch sends a change for that model only |
| CL-04 AC2 | The report shows per-method accuracy, confusion, calibration, junk precision and recall, false junk, agreement, latency and cost |
| CL-04 AC3 | Every rate shows its interval and the paired comparison shows McNemar p and the difference interval |
| CL-04 AC4 | Suppressed cells show "Too few to show" |
| CL-04 AC5 | `versions_mixed` prompts the admin to pick one version |
| CL-04 AC6 | Save snapshot sends the name and the current query |
| CL-04 AC7 | Download CSV saves the server's CSV with the fixed file name |
| CL-04 AC8 | Kill switches, saving and deleting snapshots go through Confirm it's you |

## Tests that must pass

- `'CL-04 AC1 kill switch sends PATCH for that model only'` (widget)
- `'CL-04 AC2 report shows per-method figures'` (widget, fixture)
- `'CL-04 AC3 rates show intervals and paired shows McNemar p'` (widget)
- `'CL-04 AC4 suppressed cells show Too few to show'` (widget)
- `'CL-04 AC5 versions_mixed prompts to pick one version'` (widget)
- `'CL-04 AC6 save snapshot sends name and query'` (widget)
- `'CL-04 AC7 Download CSV saves with the fixed file name'` (widget, `FakeBrowser.saveFile`)
- `'CL-04 AC8 kill switch without a fresh sign-in shows Confirm it\'s you'` (widget)
- `'s9_bakeoff_loading'` (widget)
- `'s9_bakeoff_not_enough_data'` (widget)
- `'s9_bakeoff_snapshot_saved'` (widget)
- `'s9_bakeoff_snapshot_limit shows Delete a snapshot first'` (widget)
- `'s9_bakeoff_delete_snapshot confirms'` (widget)
- `'BakeoffReport.fromJson accepts nulls wherever the schema allows'` (unit)
- `'XC-03 bake-off controls are labelled'` (widget)

## Edge cases and traps

- Never compute or fill in a suppressed value on the client (no subtracting from totals).
- Never build the CSV file name from the snapshot name or query (ASVS V5.4.1 on the server; the client keeps the same rule).
- Model predictions per card never reach the browser; this screen shows aggregates only.
- Numbers display with fixed decimals (3 for rates, 2 for cost) `[DEFAULT]`; no charts or chart packages.
- The CSV request must carry the session cookie and `Accept: text/csv`; do not open the URL in a new tab.

## Out of scope

- Charts and visual design (later); computing the report (T-907, T-908).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
