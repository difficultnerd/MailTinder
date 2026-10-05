/// Date and count formatting helpers. No `intl` package.
library;

/// Formats [n] with commas every three digits, e.g. 12431 -> "12,431".
String formatCount(int n) {
  final digits = n.toString();
  final buffer = StringBuffer();
  for (var i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 == 0) {
      buffer.write(',');
    }
    buffer.write(digits[i]);
  }
  return buffer.toString();
}

const List<String> _months = [
  'Jan',
  'Feb',
  'Mar',
  'Apr',
  'May',
  'Jun',
  'Jul',
  'Aug',
  'Sep',
  'Oct',
  'Nov',
  'Dec',
];

/// Formats [utc] as a local date, e.g. "3 Oct 2026".
String formatDate(DateTime utc) {
  final local = utc.toLocal();
  return '${local.day} ${_months[local.month - 1]} ${local.year}';
}

/// Formats [utc] as a local date and 12-hour time, e.g. "3 Oct 2026, 2:09 pm".
String formatDateTime(DateTime utc) {
  final local = utc.toLocal();
  final hour = local.hour % 12 == 0 ? 12 : local.hour % 12;
  final minute = local.minute.toString().padLeft(2, '0');
  final period = local.hour < 12 ? 'am' : 'pm';
  return '${formatDate(utc)}, $hour:$minute $period';
}
