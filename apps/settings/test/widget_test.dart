import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:retype/main.dart';
import 'package:retype/settings_repository.dart';

class FakeSettingsRepository implements SettingsRepository {
  PinyinScheme scheme = PinyinScheme.full;
  bool autoCheck = true;
  bool checkedUpdates = false;
  bool downloadedUpdate = false;
  bool installedUpdate = false;
  bool verifiedInstallation = false;
  String? skippedVersion;
  bool closed = false;

  @override
  Future<SettingsSnapshot> load() async =>
      SettingsSnapshot(scheme: scheme, autoCheck: autoCheck, version: '0.2.1');
  @override
  Future<void> setScheme(PinyinScheme value) async => scheme = value;
  @override
  Future<void> setAutoCheck(bool value) async => autoCheck = value;
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
}
