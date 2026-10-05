import 'package:flutter/material.dart';

import '../../api/models/bakeoff.dart';
import '../../copy.dart';

/// Rates and counts as plain text. A suppressed value is never filled in
/// (CL-04 AC4).
String formatCount(int? n) {
  if (n == null) {
    return Copy.tooFewToShow;
  }
  final digits = n.abs().toString();
  final out = StringBuffer(n < 0 ? '-' : '');
  for (var i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 == 0) {
      out.write(',');
    }
    out.write(digits[i]);
  }
  return out.toString();
}

String formatRate(double? v) =>
    v == null ? Copy.tooFewToShow : v.toStringAsFixed(3);

String formatCost(double? v) =>
    v == null ? Copy.tooFewToShow : v.toStringAsFixed(2);

String formatMs(int? v) => v == null ? Copy.tooFewToShow : '$v ms';

/// `0.870 (0.863 to 0.877), n 8,312`; the value alone when the bounds are
/// null; "Too few to show" when suppressed.
String formatCi(Ci ci, {bool withN = true}) {
  final v = ci.value;
  if (v == null) {
    return Copy.tooFewToShow;
  }
  final out = StringBuffer(v.toStringAsFixed(3));
  final lo = ci.lower;
  final hi = ci.upper;
  if (lo != null && hi != null) {
    out.write(' (${lo.toStringAsFixed(3)} to ${hi.toStringAsFixed(3)})');
  }
  if (withN && ci.n != null) {
    out.write(', n ${formatCount(ci.n)}');
  }
  return out.toString();
}

/// Every section of the report as plain tables.
class BakeoffTables extends StatelessWidget {
  const BakeoffTables({super.key, required this.report});

  final BakeoffReport report;

  @override
  Widget build(BuildContext context) {
    final models = report.models;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _Section(
          title: 'Overview',
          headers: const ['Measure', 'Value'],
          rows: [
            ['Labelled swipes', formatCount(report.labelledSwipes)],
            ['Participants', formatCount(report.participants)],
            ['Top contributor share', formatRate(report.topContributorShare)],
          ],
        ),
        _Section(
          title: 'Methods',
          headers: [
            'Measure',
            for (final m in models) Copy.modelLabel(m.model),
          ],
          rows: [
            _row('Cards', models, (m) => formatCount(m.cards)),
            _row('Valid answers', models, (m) => formatCount(m.validAnswers)),
            _row('Accuracy', models, (m) => formatCi(m.accuracy)),
            _row('Junk precision', models, (m) => formatCi(m.junkPrecision)),
            _row('Junk recall', models, (m) => formatCi(m.junkRecall)),
            _row('Junk F1', models, (m) => formatRate(m.junkF1)),
            _row('False junk rate', models, (m) => formatCi(m.falseJunkRate)),
            _row('ECE', models, (m) => formatRate(m.ece)),
            _row('Latency p50', models, (m) => formatMs(m.p50Ms)),
            _row('Latency p95', models, (m) => formatMs(m.p95Ms)),
            _row('Timeout rate', models, (m) => formatCi(m.timeoutRate)),
            _row('Error rate', models, (m) => formatCi(m.errorRate)),
            _row(
              'Cost per 1,000 messages (USD)',
              models,
              (m) => formatCost(m.costPer1000),
            ),
          ],
        ),
        _Section(
          title: 'Paired comparison',
          headers: const [
            'Pair',
            'Only first correct',
            'Only second correct',
            'Both correct',
            'Neither correct',
            'Paired n',
            'McNemar p',
            'Accuracy difference',
            'Bootstrap',
          ],
          rows: [
            for (final p in report.paired)
              [
                '${Copy.modelLabel(p.modelA)} v ${Copy.modelLabel(p.modelB)}',
                formatCount(p.onlyACorrect),
                formatCount(p.onlyBCorrect),
                formatCount(p.bothCorrect),
                formatCount(p.neitherCorrect),
                formatCount(p.nPaired),
                formatRate(p.mcnemarP),
                formatCi(p.difference, withN: false),
                p.bootstrapMethod,
              ],
          ],
        ),
        _Section(
          title: 'Agreement',
          headers: const ['Pair', 'Same class'],
          rows: [
            for (final a in report.agreement)
              [
                '${Copy.modelLabel(a.modelA)} v ${Copy.modelLabel(a.modelB)}',
                formatCi(a.rate),
              ],
          ],
        ),
        _Section(
          title: 'Segments',
          headers: const ['Segment', 'Method', 'Accuracy', 'False junk rate'],
          rows: [
            for (final s in report.bySegment)
              [
                '${s.dimension}=${s.value}',
                Copy.modelLabel(s.model),
                formatCi(s.accuracy),
                formatCi(s.falseJunkRate),
              ],
          ],
        ),
        _Section(
          title: 'Confusion',
          headers: const ['Method', 'Class', 'Label', 'Count'],
          rows: [
            for (final c in report.confusion)
              [Copy.modelLabel(c.model), c.cls, c.label, formatCount(c.count)],
          ],
        ),
        _Section(
          title: 'Calibration',
          headers: const ['Method', 'Confidence', 'Count', 'Observed accuracy'],
          rows: [
            for (final m in models)
              for (final b in m.calibrationBins)
                [
                  Copy.modelLabel(m.model),
                  '${b.from.toStringAsFixed(1)} to ${b.to.toStringAsFixed(1)}',
                  formatCount(b.count),
                  formatRate(b.observedAccuracy),
                ],
          ],
        ),
        _Section(
          title: 'Latency histogram',
          headers: const ['Method', 'Milliseconds', 'Count'],
          rows: [
            for (final m in models)
              for (final b in m.histogram)
                [
                  Copy.modelLabel(m.model),
                  '${b.fromMs} to ${b.toMs}',
                  formatCount(b.count),
                ],
          ],
        ),
        _Section(
          title: 'Daily trend',
          headers: const ['Date', 'Method', 'Accuracy'],
          rows: [
            for (final t in report.trend)
              [t.date, Copy.modelLabel(t.model), formatCi(t.accuracy)],
          ],
        ),
      ],
    );
  }

  static List<String> _row(
    String label,
    List<ModelResult> models,
    String Function(ModelResult) cell,
  ) => [label, for (final m in models) cell(m)];
}

class _Section extends StatelessWidget {
  const _Section({
    required this.title,
    required this.headers,
    required this.rows,
  });

  final String title;
  final List<String> headers;
  final List<List<String>> rows;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Semantics(
            header: true,
            child: Text(title, style: Theme.of(context).textTheme.titleMedium),
          ),
          if (rows.isEmpty)
            const Text(Copy.adminNothingHere)
          else
            SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: DataTable(
                columns: [for (final h in headers) DataColumn(label: Text(h))],
                rows: [
                  for (final r in rows)
                    DataRow(cells: [for (final c in r) DataCell(Text(c))]),
                ],
              ),
            ),
        ],
      ),
    );
  }
}
