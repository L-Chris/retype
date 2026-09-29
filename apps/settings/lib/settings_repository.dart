import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';

import 'statistics_store.dart';

enum PinyinScheme { full, xiaohe }

class SettingsSnapshot {
  const SettingsSnapshot({
    required this.scheme,
    required this.autoCheck,
    required this.version,
  });
  final PinyinScheme scheme;
  final bool autoCheck;
  final String version;
}

class UpdateOffer {
  const UpdateOffer({
    required this.available,
    required this.installable,
    this.version,
    this.releasePage,
  });
  final bool available;
  final bool installable;
  final String? version;
  final String? releasePage;
}

abstract class SettingsRepository {
  Future<SettingsSnapshot> load();
  Future<void> setScheme(PinyinScheme scheme);
  Future<void> setAutoCheck(bool value);
  Future<StatisticsSnapshot> loadStatistics();
  Future<void> setStatisticsEnabled(bool value);
  Future<void> clearStatistics();
  Future<UpdateOffer> checkUpdates(String currentVersion);
  Future<String> downloadUpdate(String expectedVersion);
  Future<int> installUpdate(String path);
  Future<void> verifyInstallation(String expectedVersion);
  Future<void> skipUpdate(String version);
  Future<void> openReleaseNotes(String url);
  Future<void> openFeedback();
  Future<void> openLicense();
  Future<void> openNotice();
  Future<void> startDrag();
  Future<void> closeWindow();
}

class WindowsSettingsRepository implements SettingsRepository {
  const WindowsSettingsRepository();
  static const _channel = MethodChannel('retype/settings');
  static StatisticsStore? _statisticsStoreInstance;

  Future<String> _installationDirectory() async {
    final values = await _channel.invokeMapMethod<String, Object?>(
      'getSettings',
    );
    final directory = values?['directory'] as String?;
    if (directory == null || directory.isEmpty) {
      throw StateError('未找到已安装的 retype');
    }
    return directory;
  }

  Future<Map<String, dynamic>> _runUpdater(List<String> arguments) async {
    final directory = await _installationDirectory();
    final result = await Process.run(
      '$directory\\retype-updater.exe',
      arguments,
      stdoutEncoding: utf8,
      stderrEncoding: utf8,
    );
    Map<String, dynamic>? payload;
    try {
      final decoded = jsonDecode(result.stdout as String);
      if (decoded is Map<String, dynamic>) payload = decoded;
    } on FormatException {
      // Prefer the updater's own diagnostic below.
    }
    if (result.exitCode != 0 && result.exitCode != 10) {
      throw StateError(
        payload?['error'] as String? ??
            (result.stderr as String).trim().ifEmpty(
              '更新器退出码 ${result.exitCode}',
            ),
      );
    }
    if (payload == null) throw StateError('更新器没有返回有效结果');
    return payload;
  }

  @override
  Future<SettingsSnapshot> load() async {
    final values = await _channel.invokeMapMethod<String, Object?>(
      'getSettings',
    );
    if (values == null) throw StateError('无法读取设置');
    bool? autoCheck = values['autoCheck'] as bool?;
    if (autoCheck == null) {
      autoCheck = await _legacyAutoCheck() ?? true;
      await setAutoCheck(autoCheck);
    }
    return SettingsSnapshot(
      scheme: values['scheme'] == 1 ? PinyinScheme.xiaohe : PinyinScheme.full,
      autoCheck: autoCheck,
      version: values['version'] as String? ?? '开发版本',
    );
  }

  Future<bool?> _legacyAutoCheck() async {
    final local = Platform.environment['LOCALAPPDATA'];
    if (local == null) return null;
    try {
      final file = File('$local\\retype\\updates\\state.json');
      final state = jsonDecode(await file.readAsString());
      return state is Map && state['AutoCheck'] is bool
          ? state['AutoCheck'] as bool
          : null;
    } on FileSystemException {
      return null;
    } on FormatException {
      return null;
    }
  }

  @override
  Future<void> setScheme(PinyinScheme scheme) =>
      _channel.invokeMethod('setScheme', scheme.index);
  @override
  Future<void> setAutoCheck(bool value) =>
      _channel.invokeMethod('setAutoCheck', value);
  StatisticsStore _statisticsStore() {
    final root = Platform.environment['LOCALAPPDATA'];
    if (root == null || root.isEmpty) {
      throw StateError('无法找到本地统计目录');
    }
    return _statisticsStoreInstance ??= StatisticsStore(
      Directory('$root\\retype\\statistics'),
    );
  }

  @override
  Future<StatisticsSnapshot> loadStatistics() async {
    final values = await _channel.invokeMapMethod<String, Object?>(
      'getSettings',
    );
    return _statisticsStore().load(
      enabled: values?['statisticsEnabled'] != false,
    );
  }

  @override
  Future<void> setStatisticsEnabled(bool value) =>
      _channel.invokeMethod('setStatisticsEnabled', value);

  @override
  Future<void> clearStatistics() => _statisticsStore().clear();
  @override
  Future<UpdateOffer> checkUpdates(String currentVersion) async {
    final result = await _runUpdater([
      'check',
      '--repo',
      'L-Chris/retype',
      '--current',
      currentVersion,
      '--json',
    ]);
    final latest = result['latest'];
    return UpdateOffer(
      available: result['update_available'] == true,
      installable: result['installable'] == true,
      version: latest is Map ? latest['version'] as String? : null,
      releasePage: result['release_page'] as String?,
    );
  }

  @override
  Future<String> downloadUpdate(String expectedVersion) async {
    final root = Platform.environment['LOCALAPPDATA'];
    if (root == null) throw StateError('无法找到本地更新目录');
    final directory = await Directory(
      '$root\\retype\\updates\\${DateTime.now().microsecondsSinceEpoch}',
    ).create(recursive: true);
    final result = await _runUpdater([
      'download',
      '--repo',
      'L-Chris/retype',
      '--json',
      '--timeout',
      '300',
      '--expected-version',
      expectedVersion,
      '--out',
      directory.path,
    ]);
    final path = result['downloaded'] as String?;
    if (path == null || path.isEmpty) throw StateError('下载结果没有安装包路径');
    return path;
  }

  @override
  Future<int> installUpdate(String path) async {
    await _channel.invokeMethod('installUpdate', path);
    for (var attempt = 0; attempt < 1200; attempt++) {
      await Future<void>.delayed(const Duration(milliseconds: 500));
      final code = await _channel.invokeMethod<int>('installerStatus');
      if (code != null) return code;
    }
    throw StateError('安装等待超时，请检查安装日志');
  }

  @override
  Future<void> verifyInstallation(String expectedVersion) async {
    final directory = await _installationDirectory();
    final result = await Process.run('powershell.exe', [
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      '$directory\\update-verify.ps1',
      '-ExpectedVersion',
      expectedVersion,
    ]);
    if (result.exitCode != 0) {
      throw StateError((result.stderr as String).trim().ifEmpty('安装后验证失败'));
    }
  }

  @override
  Future<void> skipUpdate(String version) async {
    final root = Platform.environment['LOCALAPPDATA'];
    if (root == null) throw StateError('无法保存跳过版本');
    final file = File('$root\\retype\\updates\\state.json');
    Map<String, dynamic> state = {};
    try {
      final decoded = jsonDecode(await file.readAsString());
      if (decoded is Map<String, dynamic>) state = decoded;
    } on FileSystemException {
      // First update check may not have written a state file yet.
    } on FormatException {
      // A damaged cache should not block the preference.
    }
    state['SkippedVersion'] = version;
    await file.parent.create(recursive: true);
    await file.writeAsString(jsonEncode(state));
  }

  @override
  Future<void> openReleaseNotes(String url) =>
      _channel.invokeMethod('openReleaseNotes', url);
  @override
  Future<void> openFeedback() => _channel.invokeMethod('openFeedback');
  @override
  Future<void> openLicense() => _channel.invokeMethod('openLicense');
  @override
  Future<void> openNotice() => _channel.invokeMethod('openNotice');

  @override
  Future<void> startDrag() => _channel.invokeMethod('startDrag');

  @override
  Future<void> closeWindow() => _channel.invokeMethod('closeWindow');
}

extension on String {
  String ifEmpty(String fallback) => isEmpty ? fallback : this;
}
