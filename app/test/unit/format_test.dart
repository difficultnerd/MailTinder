import 'package:app/format.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('formatCount and formatDate produce the expected text', () {
    expect(formatCount(0), '0');
    expect(formatCount(5), '5');
    expect(formatCount(999), '999');
    expect(formatCount(1000), '1,000');
    expect(formatCount(12431), '12,431');
    expect(formatCount(1234567), '1,234,567');
    expect(formatCount(100000000), '100,000,000');

    // formatDate uses the local date of the given UTC instant.
    final utc = DateTime.utc(2026, 10, 3, 14, 9, 18);
    final local = utc.toLocal();
    final expected = '${local.day} ${_monthAbbr(local.month)} ${local.year}';
    expect(formatDate(utc), expected);
  });

  test('formatDateTime uses 12-hour time with lower-case am/pm', () {
    final utc = DateTime.utc(2026, 10, 3, 14, 9, 18);
    final local = utc.toLocal();
    final hour = local.hour % 12 == 0 ? 12 : local.hour % 12;
    final minute = local.minute.toString().padLeft(2, '0');
    final period = local.hour < 12 ? 'am' : 'pm';
    expect(formatDateTime(utc), '${formatDate(utc)}, $hour:$minute $period');
  });
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

String _monthAbbr(int month) => _months[month - 1];
