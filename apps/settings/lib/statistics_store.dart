import 'dart:async';
import 'dart:io';

class DailyStatistics {
  const DailyStatistics(this.date, this.chinese, this.english);
  final DateTime date;
  final int chinese;
  final int english;
  int get total => chinese + english;
}

enum SpeedPeriod { day, week, month, year }

class PeriodStatistics {
  const PeriodStatistics(
    this.start,
    this.chinese,
    this.english,
    this.chineseActiveMs,
    this.englishActiveMs,
  );

  final DateTime start;
  final int chinese;
  final int english;
  final int chineseActiveMs;
  final int englishActiveMs;
  int? get chinesePerMinute => _averageSpeed(chinese, chineseActiveMs);
  int? get englishPerMinute => _averageSpeed(english, englishActiveMs);
}

int? _averageSpeed(int count, int activeMs) => count >= 10 && activeMs >= 10000
    ? (count * 60000 / activeMs).round()
    : null;

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
    required this.speedHistory,
  });

  final bool enabled;
  final int todayChinese;
  final int todayEnglish;
  final int totalChinese;
  final int totalEnglish;
  final int? chinesePerMinute;
  final int? englishPerMinute;
  final List<DailyStatistics> week;
  final Map<SpeedPeriod, List<PeriodStatistics>> speedHistory;
  int get todayTotal => todayChinese + todayEnglish;
  int get total => totalChinese + totalEnglish;
}

class _Counts {
  int chinese = 0;
  int english = 0;
  int chineseMs = 0;
  int englishMs = 0;

  _Counts copy() => _Counts()
    ..chinese = chinese
    ..english = english
    ..chineseMs = chineseMs
    ..englishMs = englishMs;

  void add(_Counts other) {
    chinese += other.chinese;
    english += other.english;
    chineseMs += other.chineseMs;
    englishMs += other.englishMs;
  }
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
  const _ParsedFile(
    this.size,
    this.modified,
    this.offset,
    this.days,
    this.recent,
  );
  final int size;
  final DateTime modified;
  final int offset;
  final Map<DateTime, _Counts> days;
  final List<_RecentEntry> recent;
}

class StatisticsStore {
  StatisticsStore(this.directory, {DateTime Function()? clock})
    : _clock = clock ?? DateTime.now;
  final Directory directory;
  final DateTime Function() _clock;
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
    final now = _clock();
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
              cached.modified != stat.modified ||
              cached.offset < stat.size) {
            _files[entry.path] = await _parseFile(
              entry,
              stat,
              resetMs,
              now,
              cached,
            );
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
        counts.add(entry.value);
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
      speedHistory: {
        for (final period in SpeedPeriod.values)
          period: _history(days, today, period),
      },
    );
  }

  Future<_ParsedFile> _parseFile(
    File file,
    FileStat stat,
    int resetMs,
    DateTime now,
    _ParsedFile? cached,
  ) async {
    final append =
        cached != null &&
        stat.size >= cached.size &&
        (stat.size > cached.size || stat.modified == cached.modified);
    final start = append ? cached.offset : 0;
    final days = append
        ? {
            for (final entry in cached.days.entries)
              entry.key: entry.value.copy(),
          }
        : <DateTime, _Counts>{};
    final recent = append
        ? [
            for (final entry in cached.recent)
              if (entry.timestampMs >= now.millisecondsSinceEpoch - 300000)
                entry,
          ]
        : <_RecentEntry>[];
    void addLine(String line) {
      final parts = line.split(',');
      if (parts.length != 5) return;
      final values = parts.map(int.tryParse).toList();
      if (values.any((value) => value == null || value < 0)) return;
      final timestampMs = values[0]! * 1000;
      if (timestampMs < resetMs || timestampMs > now.millisecondsSinceEpoch) {
        return;
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
      counts.chineseMs += chineseMs;
      counts.englishMs += englishMs;
      if (timestampMs >= now.millisecondsSinceEpoch - 300000) {
        recent.add(
          _RecentEntry(timestampMs, chinese, english, chineseMs, englishMs),
        );
      }
    }

    // Logs are append-only and numeric. Keep the final unfinished line for
    // the next refresh rather than counting a partial writer flush.
    final pending = <int>[];
    var overflow = false;
    var consumed = start;
    var offset = start;
    await for (final chunk in file.openRead(start, stat.size)) {
      for (final byte in chunk) {
        consumed++;
        if (byte == 10) {
          if (!overflow) addLine(String.fromCharCodes(pending).trim());
          pending.clear();
          overflow = false;
          offset = consumed;
        } else if (!overflow) {
          if (pending.length < 4096) {
            pending.add(byte);
          } else {
            overflow = true;
          }
        }
      }
    }
    return _ParsedFile(stat.size, stat.modified, offset, days, recent);
  }

  static int? _speed(int count, int activeMs, int lastMs, int nowMs) {
    if (nowMs - lastMs > 30000) return null;
    return _averageSpeed(count, activeMs);
  }

  static DateTime _periodStart(DateTime date, SpeedPeriod period) {
    switch (period) {
      case SpeedPeriod.day:
        return DateTime(date.year, date.month, date.day);
      case SpeedPeriod.week:
        return DateTime(date.year, date.month, date.day - date.weekday + 1);
      case SpeedPeriod.month:
        return DateTime(date.year, date.month);
      case SpeedPeriod.year:
        return DateTime(date.year);
    }
  }

  static DateTime _shiftPeriod(DateTime start, SpeedPeriod period, int offset) {
    switch (period) {
      case SpeedPeriod.day:
        return DateTime(start.year, start.month, start.day + offset);
      case SpeedPeriod.week:
        return DateTime(start.year, start.month, start.day + offset * 7);
      case SpeedPeriod.month:
        return DateTime(start.year, start.month + offset);
      case SpeedPeriod.year:
        return DateTime(start.year + offset);
    }
  }

  static List<PeriodStatistics> _history(
    Map<DateTime, _Counts> days,
    DateTime today,
    SpeedPeriod period,
  ) {
    final length = switch (period) {
      SpeedPeriod.day => 7,
      SpeedPeriod.week => 8,
      SpeedPeriod.month => 12,
      SpeedPeriod.year => 5,
    };
    final current = _periodStart(today, period);
    final totals = <DateTime, _Counts>{
      for (var i = 1 - length; i <= 0; i++)
        _shiftPeriod(current, period, i): _Counts(),
    };
    for (final entry in days.entries) {
      totals[_periodStart(entry.key, period)]?.add(entry.value);
    }
    return List.generate(length, (index) {
      final start = _shiftPeriod(current, period, index + 1 - length);
      final sum = totals[start]!;
      return PeriodStatistics(
        start,
        sum.chinese,
        sum.english,
        sum.chineseMs,
        sum.englishMs,
      );
    });
  }

  Future<void> clear() => _serial(_clear);

  Future<void> _clear() async {
    await directory.create(recursive: true);
    final reset = File('${directory.path}${Platform.pathSeparator}reset.txt');
    await reset.writeAsString('${_clock().millisecondsSinceEpoch}');
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
