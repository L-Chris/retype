import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:retype/statistics_store.dart';

void main() {
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
