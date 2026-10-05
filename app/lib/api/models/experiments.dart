/// S7 API-EXP-1 response (the `classifier_bakeoff` object).
library;

class MyExperiments {
  const MyExperiments({
    required this.available,
    required this.optedIn,
    required this.consentVersion,
    required this.currentConsentVersion,
    required this.optedInAt,
  });

  factory MyExperiments.fromJson(Map<String, Object?> json) {
    final raw = json['classifier_bakeoff'];
    final m = raw is Map<String, Object?> ? raw : json;
    final at = m['opted_in_at'] as String?;
    return MyExperiments(
      available: (m['available'] as bool?) ?? false,
      optedIn: (m['opted_in'] as bool?) ?? false,
      consentVersion: m['consent_version'] as String?,
      currentConsentVersion: (m['current_consent_version'] as String?) ?? '',
      optedInAt: at == null ? null : DateTime.parse(at).toUtc(),
    );
  }

  final bool available;
  final bool optedIn;
  final String? consentVersion;
  final String currentConsentVersion;
  final DateTime? optedInAt;
}
