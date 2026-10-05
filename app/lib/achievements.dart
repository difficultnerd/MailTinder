/// The achievements catalogue. IDs match the backend `AchievementId` wire
/// strings (T-109).
library;

class AchievementInfo {
  const AchievementInfo(this.id, this.title);

  final String id;
  final String title;
}

const List<AchievementInfo> kAchievements = [
  AchievementInfo('first_unsubscribe', 'First unsubscribe'),
  AchievementInfo('senders_silenced_100', '100 senders silenced'),
  AchievementInfo('year_cleared', 'A year cleared'),
  AchievementInfo('cleared_1000', '1,000 cleared'),
  AchievementInfo('first_filing_category', 'First filing category'),
  AchievementInfo('first_blocked_person', 'First blocked person'),
  AchievementInfo('ten_unsubscribes_in_round', 'Ten unsubscribes in a round'),
];

/// The title for [id], or null for an unknown ID.
String? achievementTitle(String id) {
  for (final a in kAchievements) {
    if (a.id == id) {
      return a.title;
    }
  }
  return null;
}
