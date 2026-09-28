import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:retype/main.dart';
import 'package:retype/settings_repository.dart';

class FakeSettingsRepository implements SettingsRepository {
  PinyinScheme scheme = PinyinScheme.full;
  bool autoCheck = true;
  bool openedUpdates = false;
  bool closed = false;

  @override
  Future<SettingsSnapshot> load() async =>
      SettingsSnapshot(scheme: scheme, autoCheck: autoCheck, version: '0.2.0');
  @override
  Future<void> setScheme(PinyinScheme value) async => scheme = value;
  @override
  Future<void> setAutoCheck(bool value) async => autoCheck = value;
  @override
  Future<void> openUpdates() async => openedUpdates = true;
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
    expect(find.text('0.2.0'), findsOneWidget);
    await tester.tap(find.byType(SwitchListTile));
    await tester.pumpAndSettle();
    expect(repository.autoCheck, false);
    await tester.tap(find.text('检查更新'));
    await tester.pumpAndSettle();
    expect(repository.openedUpdates, true);
    await tester.tap(find.byTooltip('关闭设置'));
    await tester.pumpAndSettle();
    expect(repository.closed, true);
  });
}
