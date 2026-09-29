import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:retype/statistics_store.dart';

void main() {
  String row(
    DateTime time,
    int chinese,
    int english,
    int chineseMs,
    int englishMs,
  ) =>
      '${time.millisecondsSinceEpoch ~/ 1000},$chinese,$english,$chineseMs,$englishMs\n';

  test('calendar averages use total characters and active time', () async {
    final root = await Directory.systemTemp.createTemp('retype-speed-test-');
    addTearDown(() => root.delete(recursive: true));
    final now = DateTime(2026, 9, 29, 12);
    final log = File('${root.path}${Platform.pathSeparator}speed.log');
    await log.writeAsString(
      row(DateTime(2025, 9, 29, 11), 40, 0, 20000, 0) +
          row(DateTime(2026, 8, 20, 11), 40, 0, 20000, 0) +
          row(DateTime(2026, 9, 23, 11), 20, 0, 20000, 0) +
          row(DateTime(2026, 9, 28, 11), 10, 40, 10000, 20000) +
          row(DateTime(2026, 9, 29, 11, 59, 59), 120, 30, 60000, 30000),
    );
    final store = StatisticsStore(root, clock: () => now);
    final first = await store.load(enabled: true);
    final days = first.speedHistory[SpeedPeriod.day]!;
    expect(days.last.chinesePerMinute, 120);
    expect(days.last.englishPerMinute, 60);
    expect(days[days.length - 2].chinesePerMinute, 60);
    final weeks = first.speedHistory[SpeedPeriod.week]!;
    expect(weeks.last.start, DateTime(2026, 9, 28));
    expect(weeks.last.chinesePerMinute, 111); // 130 chars / 70 active seconds.
    expect(weeks[weeks.length - 2].chinesePerMinute, 60);
    expect(first.speedHistory[SpeedPeriod.month]!.last.chinesePerMinute, 100);
    expect(first.speedHistory[SpeedPeriod.year]!.last.chinesePerMinute, 104);
    expect(first.speedHistory[SpeedPeriod.year]![3].chinesePerMinute, 120);

    // A half-written final line is not counted or duplicated on later refresh.
    final extra = row(DateTime(2026, 9, 29, 11, 59, 59), 30, 0, 30000, 0);
    await log.writeAsString(
      extra.substring(0, extra.length - 1),
      mode: FileMode.append,
    );
    final partial = await store.load(enabled: true);
    expect(partial.speedHistory[SpeedPeriod.day]!.last.chinesePerMinute, 120);
    await log.writeAsString('\n', mode: FileMode.append);
    final refreshed = await store.load(enabled: true);
    expect(refreshed.speedHistory[SpeedPeriod.day]!.last.chinesePerMinute, 100);
    expect(refreshed.todayChinese, 150);
    final paused = await store.load(enabled: false);
    expect(paused.chinesePerMinute, null);
    expect(paused.speedHistory[SpeedPeriod.day]!.last.chinesePerMinute, 100);
  });

  test('week and year boundaries follow local calendar dates', () async {
    final root = await Directory.systemTemp.createTemp('retype-speed-year-');
    addTearDown(() => root.delete(recursive: true));
    final now = DateTime(2027, 1, 1, 12);
    final log = File('${root.path}${Platform.pathSeparator}speed.log');
    await log.writeAsString(
      row(DateTime(2026, 12, 31, 10), 20, 0, 20000, 0) +
          row(DateTime(2027, 1, 1, 10), 30, 0, 20000, 0),
    );
    final snapshot = await StatisticsStore(
      root,
      clock: () => now,
    ).load(enabled: true);
    final week = snapshot.speedHistory[SpeedPeriod.week]!.last;
    expect(week.start, DateTime(2026, 12, 28));
    expect(week.chinesePerMinute, 75);
    expect(snapshot.speedHistory[SpeedPeriod.year]!.last.chinese, 30);
    expect(snapshot.speedHistory[SpeedPeriod.year]![3].chinese, 20);
  });

  test(
    'aggregates Chinese and English separately and clears prior records',
    () async {
      final root = await Directory.systemTemp.createTemp('retype-stats-test-');
      addTearDown(() => root.delete(recursive: true));
      final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      final log = File('${root.path}${Platform.pathSeparator}sample.log');
      await log.writeAsString('$now,12,7,12000,10000\n$now,3,2,0,0\n');
      final store = StatisticsStore(root);
      final before = await store.load(enabled: true);
      expect(before.todayChinese, 15);
      expect(before.todayEnglish, 9);
      expect(before.total, 24);
      expect(before.chinesePerMinute, 75);
      expect(before.englishPerMinute, null);
      await log.writeAsString('$now,2,4,0,0\n', mode: FileMode.append);
      final refreshed = await store.load(enabled: true);
      expect(refreshed.todayChinese, 17);
      expect(refreshed.todayEnglish, 13);
      await store.clear();
      final after = await store.load(enabled: true);
      expect(after.total, 0);
    },
  );
}
