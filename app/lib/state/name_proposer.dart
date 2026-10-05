import '../api/models/category.dart';
import '../api/models/feed.dart';
import '../platform/on_device_model.dart';

/// Proposes a short folder name through the on-device model (FL-02 AC2).
/// Nothing leaves the device and nothing is stored or logged.
class OnDeviceNameProposer {
  OnDeviceNameProposer(
    this.model, {
    this.timeout = const Duration(milliseconds: 800),
  });

  final OnDeviceModel model;
  final Duration timeout;

  /// The proposed name, or null when there is no model, no usable reply, an
  /// error or a timeout.
  Future<String?> propose({
    required String senderDisplay,
    required String subject,
    required List<String> existing,
  }) async {
    try {
      if (!await model.isAvailable()) return null;
      final cut = subject.length > 120 ? subject.substring(0, 120) : subject;
      final reply = await model.prompt(
        'Suggest a folder name of one or two words for an email from '
        '$senderDisplay with subject $cut. '
        'Existing folders: ${existing.join(', ')}. Reply with the name only.',
        timeout: timeout,
      );
      return reply == null ? null : _clean(reply, existing);
    } on Object {
      return null;
    }
  }

  /// A proposer for the filing sheet that reads existing names from
  /// [categories] at call time.
  Future<String?> Function(FeedCard card) forSheet(
    List<Category>? Function() categories,
  ) {
    return (card) => propose(
      senderDisplay: card.senderName.isNotEmpty
          ? card.senderName
          : card.senderAddress,
      subject: card.subject,
      existing: [for (final c in categories() ?? const <Category>[]) c.name],
    );
  }

  static final _edges = RegExp(r'''^[\s"'`.,;:!?*_-]+|[\s"'`.,;:!?*_-]+$''');

  static String? _clean(String reply, List<String> existing) {
    final firstLine = reply
        .split('\n')
        .map((l) => l.trim())
        .firstWhere((l) => l.isNotEmpty, orElse: () => '');
    var name = firstLine.replaceAll(_edges, '').replaceAll(RegExp(r'\s+'), ' ');
    if (name.length > 30) name = name.substring(0, 30).trimRight();
    if (name.isEmpty) return null;
    final lower = name.toLowerCase();
    if (name.contains('@') ||
        lower.contains('http') ||
        RegExp(r'^\d+$').hasMatch(name)) {
      return null;
    }
    final titled = name
        .split(' ')
        .map(
          (w) => w.isEmpty
              ? w
              : '${w[0].toUpperCase()}${w.substring(1).toLowerCase()}',
        )
        .join(' ');
    for (final e in existing) {
      if (e.toLowerCase() == titled.toLowerCase()) return e;
    }
    return titled;
  }
}
