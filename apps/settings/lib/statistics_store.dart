import 'dart:async';
import 'dart:convert';
import 'dart:io';

class DailyStatistics {
  const DailyStatistics(this.date, this.chinese, this.english);
  final DateTime date;
  final int chinese;
  final int english;
  int get total => chinese + english;
}

class StatisticsSnapshot {
  const StatisticsSnapshot({
    required this.enabled,
    required this.todayChinese,
    required this.todayEnglish,
    required this.totalChinese,
    required this.totalEnglish,
    required this.chinesePerMinute,
    required this.englishPerMinute,
    required this.week,
  });

  final bool enabled;
  final int todayChinese;
  final int todayEnglish;
  final int totalChinese;
  final int totalEnglish;
  final int? chinesePerMinute;
  final int? englishPerMinute;
  final List<DailyStatistics> week;
  int get todayTotal => todayChinese + todayEnglish;
  int get total => totalChinese + totalEnglish;
}

class _Counts {
  int chinese = 0;
  int english = 0;
}

class _RecentEntry {
  const _RecentEntry(
    this.timestampMs,
    this.chinese,
    this.english,
    this.chineseMs,
    this.englishMs,
  );
  final int timestampMs;
  final int chinese;
  final int english;
  final int chineseMs;
  final int englishMs;
}

class _ParsedFile {
  const _ParsedFile(this.size, this.modified, this.days, this.recent);
  final int size;
  final DateTime modified;
  final Map<DateTime, _Counts> days;
  final List<_RecentEntry> recent;
}

class StatisticsStore {
  StatisticsStore(this.directory);
  final Directory directory;
  final _files = <String, _ParsedFile>{};
  int? _lastResetMs;
  Future<void> _tail = Future.value();

  Future<T> _serial<T>(Future<T> Function() task) async {
    final previous = _tail;
    final done = Completer<void>();
    _tail = done.future;
    await previous;
    try {
      return await task();
    } finally {
      done.complete();
    }
  }

  Future<StatisticsSnapshot> load({required bool enabled}) =>
      _serial(() => _load(enabled: enabled));

  Future<StatisticsSnapshot> _load({required bool enabled}) async {
    final now = DateTime.now();
    final today = DateTime(now.year, now.month, now.day);
    var resetMs = 0;
    try {
      resetMs =
          int.tryParse(
            (await File(
              '${directory.path}${Platform.pathSeparator}reset.txt',
            ).readAsString()).trim(),
          ) ??
          0;
    } on FileSystemException {
      // No reset marker yet.
    }
    if (_lastResetMs != resetMs) {
      _files.clear();
      _lastResetMs = resetMs;
    }
    final seen = <String>{};
    if (await directory.exists()) {
      await for (final entry in directory.list(followLinks: false)) {
        if (entry is! File || !entry.path.endsWith('.log')) continue;
        seen.add(entry.path);
        try {
          final stat = await entry.stat();
          final cached = _files[entry.path];
          if (cached == null ||
              cached.size != stat.size ||
              cached.modified != stat.modified) {
            _files[entry.path] = await _parseFile(entry, stat, resetMs, now);
          }
        } on FileSystemException {
          // An app may be replacing or removing its log during a refresh.
        } on FormatException {
          // Ignore a partial trailing line after an interrupted write.
        }
      }
    }
    _files.removeWhere((path, _) => !seen.contains(path));
    final days = <DateTime, _Counts>{};
    var recentChinese = 0;
    var recentEnglish = 0;
    var recentChineseMs = 0;
    var recentEnglishMs = 0;
    var lastChineseMs = 0;
    var lastEnglishMs = 0;
    for (final file in _files.values) {
      for (final entry in file.days.entries) {
        final counts = days.putIfAbsent(entry.key, _Counts.new);
        counts.chinese += entry.value.chinese;
        counts.english += entry.value.english;
      }
      for (final recent in file.recent) {
        if (recent.timestampMs < now.millisecondsSinceEpoch - 300000) {
          continue;
        }
        recentChinese += recent.chinese;
        recentEnglish += recent.english;
        recentChineseMs += recent.chineseMs;
        recentEnglishMs += recent.englishMs;
        if (recent.chinese > 0 && recent.timestampMs > lastChineseMs) {
          lastChineseMs = recent.timestampMs;
        }
        if (recent.english > 0 && recent.timestampMs > lastEnglishMs) {
          lastEnglishMs = recent.timestampMs;
        }
      }
    }
    var totalChinese = 0;
    var totalEnglish = 0;
    for (final counts in days.values) {
      totalChinese += counts.chinese;
      totalEnglish += counts.english;
    }
    final week = List.generate(7, (index) {
      final date = DateTime(today.year, today.month, today.day - 6 + index);
      final counts = days[date];
      return DailyStatistics(date, counts?.chinese ?? 0, counts?.english ?? 0);
    });
    return StatisticsSnapshot(
      enabled: enabled,
      todayChinese: days[today]?.chinese ?? 0,
      todayEnglish: days[today]?.english ?? 0,
      totalChinese: totalChinese,
      totalEnglish: totalEnglish,
      chinesePerMinute: enabled
          ? _speed(
              recentChinese,
              recentChineseMs,
              lastChineseMs,
              now.millisecondsSinceEpoch,
            )
          : null,
      englishPerMinute: enabled
          ? _speed(
              recentEnglish,
              recentEnglishMs,
              lastEnglishMs,
              now.millisecondsSinceEpoch,
            )
          : null,
      week: week,
    );
  }

  Future<_ParsedFile> _parseFile(
    File file,
    FileStat stat,
    int resetMs,
    DateTime now,
  ) async {
    final days = <DateTime, _Counts>{};
    final recent = <_RecentEntry>[];
    await for (final line
        in file
            .openRead()
            .transform(utf8.decoder)
            .transform(const LineSplitter())) {
      final parts = line.split(',');
      if (parts.length != 5) continue;
      final values = parts.map(int.tryParse).toList();
      if (values.any((value) => value == null || value < 0)) continue;
      final timestampMs = values[0]! * 1000;
      if (timestampMs < resetMs || timestampMs > now.millisecondsSinceEpoch) {
        continue;
      }
      final chinese = values[1]!;
      final english = values[2]!;
      final chineseMs = values[3]!;
      final englishMs = values[4]!;
      final time = DateTime.fromMillisecondsSinceEpoch(timestampMs);
      final date = DateTime(time.year, time.month, time.day);
      final counts = days.putIfAbsent(date, _Counts.new);
      counts.chinese += chinese;
      counts.english += english;
      if (timestampMs >= now.millisecondsSinceEpoch - 300000) {
        recent.add(
          _RecentEntry(timestampMs, chinese, english, chineseMs, englishMs),
        );
      }
    }
    return _ParsedFile(stat.size, stat.modified, days, recent);
  }

  static int? _speed(int count, int activeMs, int lastMs, int nowMs) {
    if (count < 10 || activeMs < 10000 || nowMs - lastMs > 30000) {
      return null;
    }
    return (count * 60000 / activeMs).round();
  }

  Future<void> clear() => _serial(_clear);

  Future<void> _clear() async {
    await directory.create(recursive: true);
    final reset = File('${directory.path}${Platform.pathSeparator}reset.txt');
    await reset.writeAsString('${DateTime.now().millisecondsSinceEpoch}');
    _files.clear();
    _lastResetMs = null;
    await for (final entry in directory.list(followLinks: false)) {
      if (entry is File && entry.path.endsWith('.log')) {
        try {
          await entry.delete();
        } on FileSystemException {
          // The reset marker still excludes records held open by another host.
        }
      }
    }
  }
}
