import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:retype/main.dart';
import 'package:retype/settings_repository.dart';
import 'package:retype/statistics_store.dart';
import 'package:retype/dictionary_store.dart';

class FakeSettingsRepository implements SettingsRepository {
  PinyinScheme scheme = PinyinScheme.full;
  bool autoCheck = true;
  bool checkedUpdates = false;
  bool downloadedUpdate = false;
  bool installedUpdate = false;
  bool verifiedInstallation = false;
  String? skippedVersion;
  bool closed = false;
  bool statisticsEnabled = true;
  bool statisticsCleared = false;
  int statisticsLoads = 0;
  final Set<String> installedPacks = {};
  final Set<String> enabledPacks = {};

  @override
  Future<List<DictionaryPackState>> loadDictionaryPacks() async => [
    for (final pack in dictionaryPacks)
      DictionaryPackState(
        pack: pack,
        installed: installedPacks.contains(pack.id),
        enabled: enabledPacks.contains(pack.id),
      ),
  ];
  @override
  Future<void> setDictionaryPack(
    DictionaryPack pack,
    bool enabled, {
    void Function(double)? progress,
    PackDownloadControl? control,
  }) async {
    if (enabled) {
      installedPacks.add(pack.id);
      enabledPacks.add(pack.id);
      progress?.call(1);
    } else {
      enabledPacks.remove(pack.id);
    }
  }

  @override
  Future<void> deleteDictionaryPack(DictionaryPack pack) async {
    enabledPacks.remove(pack.id);
    installedPacks.remove(pack.id);
  }

  @override
  Future<SettingsSnapshot> load() async =>
      SettingsSnapshot(scheme: scheme, autoCheck: autoCheck, version: '0.2.1');
  @override
  Future<void> setScheme(PinyinScheme value) async => scheme = value;
  @override
  Future<void> setAutoCheck(bool value) async => autoCheck = value;
  @override
  Future<StatisticsSnapshot> loadStatistics() async {
    statisticsLoads++;
    return StatisticsSnapshot(
      enabled: statisticsEnabled,
      todayChinese: statisticsCleared ? 0 : 12,
      todayEnglish: statisticsCleared ? 0 : 7,
      totalChinese: statisticsCleared ? 0 : 30,
      totalEnglish: statisticsCleared ? 0 : 20,
      chinesePerMinute: null,
      englishPerMinute: null,
      week: List.generate(
        7,
        (index) => DailyStatistics(DateTime(2026, 9, index + 20), 0, 0),
      ),
      speedHistory: {
        for (final period in SpeedPeriod.values)
          period: [
            PeriodStatistics(DateTime(2026, 9, 27), 20, 20, 20000, 20000),
            PeriodStatistics(
              DateTime(2026, 9, 28),
              30 + period.index * 10,
              30,
              20000,
              20000,
            ),
          ],
      },
    );
  }

  @override
  Future<void> setStatisticsEnabled(bool value) async =>
      statisticsEnabled = value;
  @override
  Future<void> clearStatistics() async => statisticsCleared = true;
  @override
  Future<UpdateOffer> checkUpdates(String currentVersion) async {
    checkedUpdates = true;
    return const UpdateOffer(
      available: true,
      installable: true,
      version: '0.2.1',
      releasePage: 'https://github.com/L-Chris/retype/releases/tag/v0.2.1',
    );
  }

  @override
  Future<String> downloadUpdate(String expectedVersion) async {
    downloadedUpdate = true;
    return 'installer.exe';
  }

  @override
  Future<int> installUpdate(String path) async {
    installedUpdate = true;
    return 0;
  }

  @override
  Future<void> verifyInstallation(String expectedVersion) async =>
      verifiedInstallation = true;

  @override
  Future<void> skipUpdate(String version) async => skippedVersion = version;
  @override
  Future<void> openReleaseNotes(String url) async {}
  @override
  Future<void> openFeedback() async {}
  @override
  Future<void> openLicense() async {}
  @override
  Future<void> openNotice() async {}
  @override
  Future<void> startDrag() async {}
  @override
  Future<void> closeWindow() async => closed = true;
}

void main() {
  testWidgets('optional pack downloads on first enable and can be removed', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(960, 640);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final repository = FakeSettingsRepository();
    await tester.pumpWidget(RetypeApp(repository: repository));
    await tester.pumpAndSettle();
    await tester.tap(find.text('词库'));
    await tester.pumpAndSettle();
    expect(find.text('基础词库'), findsOneWidget);
    expect(find.text('名人'), findsOneWidget);
    await tester.tap(find.byType(Switch).first);
    await tester.pumpAndSettle();
    expect(repository.enabledPacks, contains('mingren'));
    await tester.tap(find.byType(Switch).first);
    await tester.pumpAndSettle();
    expect(repository.installedPacks, contains('mingren'));
    await tester.tap(find.byTooltip('删除名人词库'));
    await tester.pumpAndSettle();
    expect(repository.installedPacks, isEmpty);
  });
  testWidgets('switches scheme and exposes About update controls', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(960, 640);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final repository = FakeSettingsRepository();
    await tester.pumpWidget(RetypeApp(repository: repository));
    await tester.pumpAndSettle();

    expect(find.text('全拼'), findsOneWidget);
    await tester.tap(find.text('小鹤双拼'));
    await tester.pumpAndSettle();
    expect(repository.scheme, PinyinScheme.xiaohe);

    await tester.tap(find.text('关于'));
    await tester.pumpAndSettle();
    expect(find.text('0.2.1'), findsOneWidget);
    await tester.ensureVisible(find.byType(SwitchListTile));
    await tester.tap(find.byType(SwitchListTile));
    await tester.pumpAndSettle();
    expect(repository.autoCheck, false);
    await tester.tap(find.text('检查更新'));
    await tester.pumpAndSettle();
    expect(repository.checkedUpdates, true);
    expect(find.text('发现新版本 0.2.1。'), findsOneWidget);
    await tester.tap(find.text('跳过此版本'));
    await tester.pumpAndSettle();
    expect(repository.skippedVersion, '0.2.1');
    await tester.tap(find.text('下载并安装'));
    await tester.pumpAndSettle();
    expect(repository.downloadedUpdate, true);
    expect(repository.installedUpdate, true);
    expect(repository.verifiedInstallation, true);
    await tester.tap(find.byTooltip('关闭设置'));
    await tester.pumpAndSettle();
    expect(repository.closed, true);
  });

  testWidgets('statistics separates Chinese and English and can be cleared', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(960, 640);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final repository = FakeSettingsRepository();
    await tester.pumpWidget(RetypeApp(repository: repository));
    await tester.pumpAndSettle();
    await tester.tap(find.text('统计'));
    await tester.pumpAndSettle();
    expect(find.text('中文 12  ·  英文 7'), findsOneWidget);
    expect(find.text('中文 30  ·  英文 20'), findsOneWidget);
    await tester.ensureVisible(find.text('平均速度'));
    expect(find.text('较上期 +50%'), findsNWidgets(2));
    await tester.tap(find.text('月'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<SegmentedButton<SpeedPeriod>>(
            find.byType(SegmentedButton<SpeedPeriod>),
          )
          .selected,
      {SpeedPeriod.month},
    );
    expect(find.text('150'), findsOneWidget);
    await tester.ensureVisible(find.byType(SwitchListTile));
    await tester.tap(find.byType(SwitchListTile));
    await tester.pumpAndSettle();
    expect(repository.statisticsEnabled, false);
    await tester.ensureVisible(find.text('清空统计数据'));
    await tester.tap(find.text('清空统计数据'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('清空'));
    await tester.pumpAndSettle();
    expect(repository.statisticsCleared, true);
    expect(find.text('中文 0  ·  英文 0'), findsNWidgets(2));
  });

  testWidgets('hidden settings pauses statistics and resumes on reopening', (
    tester,
  ) async {
    final repository = FakeSettingsRepository();
    await tester.pumpWidget(RetypeApp(repository: repository));
    await tester.pumpAndSettle();
    await tester.tap(find.text('统计'));
    await tester.pumpAndSettle();
    final before = repository.statisticsLoads;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    await tester.pump(const Duration(minutes: 2));
    expect(repository.statisticsLoads, before);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();
    expect(repository.statisticsLoads, greaterThan(before));
    final resumed = repository.statisticsLoads;
    await tester.pump(const Duration(seconds: 5));
    expect(repository.statisticsLoads, greaterThan(resumed));
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('update entry still opens About after adding Statistics', (
    tester,
  ) async {
    final repository = FakeSettingsRepository();
    await tester.pumpWidget(
      RetypeApp(repository: repository, openUpdates: true),
    );
    await tester.pumpAndSettle();
    expect(repository.checkedUpdates, true);
    expect(find.text('0.2.1'), findsOneWidget);
  });
}
