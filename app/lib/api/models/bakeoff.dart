/// S7 API-ADM-8 to API-ADM-14 bodies (the bake-off report, S9 7.7).
///
/// Suppressed cells arrive as null and stay null: nothing here computes or
/// fills in a value (CL-04 AC4).
library;

typedef Json = Map<String, Object?>;

Json _obj(Object? raw) => raw is Json ? raw : const <String, Object?>{};

List<Json> _list(Object? raw) =>
    raw is List<Object?> ? raw.whereType<Json>().toList() : const <Json>[];

double? _num(Object? raw) => raw is num ? raw.toDouble() : null;

int? _int(Object? raw) => raw is num ? raw.toInt() : null;

String _two(int n) => n.toString().padLeft(2, '0');

String _date(DateTime d) => '${d.year}-${_two(d.month)}-${_two(d.day)}';

/// A rate with its Wilson interval; every field is null when suppressed.
class Ci {
  const Ci({this.value, this.lower, this.upper, this.n});

  factory Ci.fromJson(Object? raw) {
    final j = _obj(raw);
    return Ci(
      value: _num(j['value']),
      lower: _num(j['lower']),
      upper: _num(j['upper']),
      n: _int(j['n']),
    );
  }

  final double? value;
  final double? lower;
  final double? upper;
  final int? n;

  bool get suppressed => value == null;
}

class BakeoffQuery {
  const BakeoffQuery({
    required this.from,
    required this.to,
    this.inputVersion,
    this.questionVersion,
    this.priceVersion,
    this.poolVersions = false,
  });

  factory BakeoffQuery.fromJson(Object? raw) {
    final j = _obj(raw);
    return BakeoffQuery(
      from: DateTime.tryParse((j['from'] as String?) ?? '') ?? DateTime.utc(0),
      to: DateTime.tryParse((j['to'] as String?) ?? '') ?? DateTime.utc(0),
      inputVersion: j['input_version'] as String?,
      questionVersion: j['question_version'] as String?,
      priceVersion: j['price_version'] as String?,
      poolVersions: (j['pool_versions'] as bool?) ?? false,
    );
  }

  final DateTime from;
  final DateTime to;
  final String? inputVersion;
  final String? questionVersion;
  final String? priceVersion;
  final bool poolVersions;

  BakeoffQuery copyWith({
    DateTime? from,
    DateTime? to,
    String? inputVersion,
    String? questionVersion,
    bool clearInput = false,
    bool clearQuestion = false,
  }) => BakeoffQuery(
    from: from ?? this.from,
    to: to ?? this.to,
    inputVersion: clearInput ? null : (inputVersion ?? this.inputVersion),
    questionVersion: clearQuestion
        ? null
        : (questionVersion ?? this.questionVersion),
    priceVersion: priceVersion,
    poolVersions: poolVersions,
  );

  /// GET query parameters (API-ADM-10).
  Map<String, String> toQuery() => {
    'from': _date(from),
    'to': _date(to),
    'input_version': ?inputVersion,
    'question_version': ?questionVersion,
    'price_version': ?priceVersion,
    'pool_versions': poolVersions.toString(),
  };

  /// Body of API-ADM-11.
  Map<String, Object?> toJson() => {
    'from': _date(from),
    'to': _date(to),
    'input_version': ?inputVersion,
    'question_version': ?questionVersion,
    'price_version': ?priceVersion,
    'pool_versions': poolVersions,
  };
}

class CalibrationBin {
  const CalibrationBin({
    required this.from,
    required this.to,
    required this.count,
    required this.observedAccuracy,
  });

  factory CalibrationBin.fromJson(Json j) => CalibrationBin(
    from: _num(j['confidence_from']) ?? 0,
    to: _num(j['confidence_to']) ?? 0,
    count: _int(j['count']),
    observedAccuracy: _num(j['observed_accuracy']),
  );

  final double from;
  final double to;
  final int? count;
  final double? observedAccuracy;
}

class LatencyBin {
  const LatencyBin({
    required this.fromMs,
    required this.toMs,
    required this.count,
  });

  factory LatencyBin.fromJson(Json j) => LatencyBin(
    fromMs: _int(j['from_ms']) ?? 0,
    toMs: _int(j['to_ms']) ?? 0,
    count: _int(j['count']),
  );

  final int fromMs;
  final int toMs;
  final int? count;
}

class ModelResult {
  const ModelResult({
    required this.model,
    required this.classifierId,
    required this.cards,
    required this.validAnswers,
    required this.accuracy,
    required this.junkPrecision,
    required this.junkRecall,
    required this.junkF1,
    required this.falseJunkRate,
    required this.ece,
    required this.calibrationBins,
    required this.p50Ms,
    required this.p95Ms,
    required this.histogram,
    required this.timeoutRate,
    required this.errorRate,
    required this.inputTokens,
    required this.costPer1000,
  });

  factory ModelResult.fromJson(Json j) {
    final calibration = j['calibration'] is Json
        ? _obj(j['calibration'])
        : null;
    final latency = _obj(j['latency']);
    return ModelResult(
      model: (j['model'] as String?) ?? '',
      classifierId: (j['classifier_id'] as String?) ?? '',
      cards: _int(j['cards']),
      validAnswers: _int(j['valid_answers']),
      accuracy: Ci.fromJson(j['accuracy']),
      junkPrecision: Ci.fromJson(j['junk_precision']),
      junkRecall: Ci.fromJson(j['junk_recall']),
      junkF1: _num(j['junk_f1']),
      falseJunkRate: Ci.fromJson(j['false_junk_rate']),
      ece: calibration == null ? null : _num(calibration['ece']),
      calibrationBins: calibration == null
          ? const []
          : _list(calibration['bins']).map(CalibrationBin.fromJson).toList(),
      p50Ms: _int(latency['p50_ms']),
      p95Ms: _int(latency['p95_ms']),
      histogram: _list(latency['histogram']).map(LatencyBin.fromJson).toList(),
      timeoutRate: Ci.fromJson(j['timeout_rate']),
      errorRate: Ci.fromJson(j['error_rate']),
      inputTokens: _int(j['input_tokens']),
      costPer1000: _num(j['cost_per_1000_messages_usd']),
    );
  }

  final String model;
  final String classifierId;
  final int? cards;
  final int? validAnswers;
  final Ci accuracy;
  final Ci junkPrecision;
  final Ci junkRecall;
  final double? junkF1;
  final Ci falseJunkRate;
  final double? ece;
  final List<CalibrationBin> calibrationBins;
  final int? p50Ms;
  final int? p95Ms;
  final List<LatencyBin> histogram;
  final Ci timeoutRate;
  final Ci errorRate;
  final int? inputTokens;
  final double? costPer1000;
}

class PairedResult {
  const PairedResult({
    required this.modelA,
    required this.modelB,
    required this.nPaired,
    required this.onlyACorrect,
    required this.onlyBCorrect,
    required this.bothCorrect,
    required this.neitherCorrect,
    required this.mcnemarP,
    required this.difference,
    required this.bootstrapMethod,
  });

  factory PairedResult.fromJson(Json j) => PairedResult(
    modelA: (j['model_a'] as String?) ?? '',
    modelB: (j['model_b'] as String?) ?? '',
    nPaired: _int(j['n_paired']),
    onlyACorrect: _int(j['only_a_correct']),
    onlyBCorrect: _int(j['only_b_correct']),
    bothCorrect: _int(j['both_correct']),
    neitherCorrect: _int(j['neither_correct']),
    mcnemarP: _num(j['mcnemar_exact_p']),
    difference: Ci.fromJson(j['accuracy_difference']),
    bootstrapMethod: (_obj(j['bootstrap'])['method'] as String?) ?? '',
  );

  final String modelA;
  final String modelB;
  final int? nPaired;
  final int? onlyACorrect;
  final int? onlyBCorrect;
  final int? bothCorrect;
  final int? neitherCorrect;
  final double? mcnemarP;

  /// The difference interval; `n` is unused.
  final Ci difference;
  final String bootstrapMethod;
}

class AgreementRow {
  const AgreementRow({
    required this.modelA,
    required this.modelB,
    required this.rate,
  });

  factory AgreementRow.fromJson(Json j) => AgreementRow(
    modelA: (j['model_a'] as String?) ?? '',
    modelB: (j['model_b'] as String?) ?? '',
    rate: Ci.fromJson(j['rate']),
  );

  final String modelA;
  final String modelB;
  final Ci rate;
}

class SegmentRow {
  const SegmentRow({
    required this.dimension,
    required this.value,
    required this.model,
    required this.accuracy,
    required this.falseJunkRate,
  });

  factory SegmentRow.fromJson(Json j) => SegmentRow(
    dimension: (j['dimension'] as String?) ?? '',
    value: (j['value'] as String?) ?? '',
    model: (j['model'] as String?) ?? '',
    accuracy: Ci.fromJson(j['accuracy']),
    falseJunkRate: Ci.fromJson(j['false_junk_rate']),
  );

  final String dimension;
  final String value;
  final String model;
  final Ci accuracy;
  final Ci falseJunkRate;
}

class ConfusionRow {
  const ConfusionRow({
    required this.model,
    required this.cls,
    required this.label,
    required this.count,
  });

  factory ConfusionRow.fromJson(Json j) => ConfusionRow(
    model: (j['model'] as String?) ?? '',
    cls: (j['class'] as String?) ?? '',
    label: (j['label'] as String?) ?? '',
    count: _int(j['count']),
  );

  final String model;
  final String cls;
  final String label;
  final int? count;
}

class TrendRow {
  const TrendRow({
    required this.date,
    required this.model,
    required this.accuracy,
  });

  factory TrendRow.fromJson(Json j) => TrendRow(
    date: (j['date'] as String?) ?? '',
    model: (j['model'] as String?) ?? '',
    accuracy: Ci.fromJson(j['accuracy']),
  );

  final String date;
  final String model;
  final Ci accuracy;
}

class BakeoffReport {
  const BakeoffReport({
    required this.query,
    required this.versionsPresent,
    required this.minCellSize,
    required this.groundTruthVersion,
    required this.labelledSwipes,
    required this.participants,
    required this.topContributorShare,
    required this.models,
    required this.paired,
    required this.agreement,
    required this.bySegment,
    required this.confusion,
    required this.trend,
  });

  factory BakeoffReport.fromJson(Json j) {
    final participants = _obj(j['participants']);
    final share = participants['top_contributor_share'] is Json
        ? _obj(participants['top_contributor_share'])
        : null;
    return BakeoffReport(
      query: BakeoffQuery.fromJson(j['query']),
      versionsPresent: parseVersionsPresent(j['versions_present']),
      minCellSize: _int(j['min_cell_size']) ?? 1,
      groundTruthVersion: (j['ground_truth_version'] as String?) ?? '',
      labelledSwipes: _int(j['labelled_swipes']),
      participants: _int(participants['count']),
      topContributorShare: share == null ? null : _num(share['value']),
      models: _list(j['models']).map(ModelResult.fromJson).toList(),
      paired: _list(j['paired']).map(PairedResult.fromJson).toList(),
      agreement: _list(j['agreement']).map(AgreementRow.fromJson).toList(),
      bySegment: _list(j['by_segment']).map(SegmentRow.fromJson).toList(),
      confusion: _list(j['confusion']).map(ConfusionRow.fromJson).toList(),
      trend: _list(j['trend']).map(TrendRow.fromJson).toList(),
    );
  }

  final BakeoffQuery query;
  final Map<String, List<String>> versionsPresent;
  final int minCellSize;
  final String groundTruthVersion;
  final int? labelledSwipes;
  final int? participants;
  final double? topContributorShare;
  final List<ModelResult> models;
  final List<PairedResult> paired;
  final List<AgreementRow> agreement;
  final List<SegmentRow> bySegment;
  final List<ConfusionRow> confusion;
  final List<TrendRow> trend;
}

/// `{input, question, price}` lists; also the `versions_present` member of a
/// `409 versions_mixed` problem body.
Map<String, List<String>> parseVersionsPresent(Object? raw) {
  final j = _obj(raw);
  return {
    for (final key in const ['input', 'question', 'price'])
      key: [
        if (j[key] is List<Object?>)
          for (final v in j[key]! as List<Object?>)
            if (v is String) v,
      ],
  };
}

class SnapshotSummary {
  const SnapshotSummary({
    required this.snapshotId,
    required this.name,
    required this.createdAt,
    required this.labelledSwipes,
  });

  factory SnapshotSummary.fromJson(Json j) => SnapshotSummary(
    snapshotId: (j['snapshot_id'] as String?) ?? '',
    name: (j['name'] as String?) ?? '',
    createdAt:
        DateTime.tryParse((j['created_at'] as String?) ?? '')?.toUtc() ??
        DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
    labelledSwipes: _int(j['labelled_swipes']),
  );

  final String snapshotId;
  final String name;
  final DateTime createdAt;
  final int? labelledSwipes;
}

class Snapshot {
  const Snapshot({required this.summary, required this.report});

  factory Snapshot.fromJson(Json j) => Snapshot(
    summary: SnapshotSummary.fromJson(j),
    report: BakeoffReport.fromJson(_obj(j['report'])),
  );

  final SnapshotSummary summary;
  final BakeoffReport report;
}

class ModelSwitch {
  const ModelSwitch({
    required this.model,
    required this.classifierId,
    required this.enabled,
    required this.changedAt,
  });

  factory ModelSwitch.fromJson(Json j) => ModelSwitch(
    model: (j['model'] as String?) ?? '',
    classifierId: (j['classifier_id'] as String?) ?? '',
    enabled: (j['enabled'] as bool?) ?? false,
    changedAt:
        DateTime.tryParse((j['changed_at'] as String?) ?? '')?.toUtc() ??
        DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
  );

  final String model;
  final String classifierId;
  final bool enabled;
  final DateTime changedAt;
}

class ClassifierExperiment {
  const ClassifierExperiment({
    required this.models,
    required this.participants,
  });

  factory ClassifierExperiment.fromJson(Json j) => ClassifierExperiment(
    models: _list(j['models']).map(ModelSwitch.fromJson).toList(),
    participants: _int(j['participants']) ?? 0,
  );

  final List<ModelSwitch> models;
  final int participants;
}
