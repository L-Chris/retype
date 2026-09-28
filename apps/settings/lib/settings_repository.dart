import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';

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

abstract class SettingsRepository {
  Future<SettingsSnapshot> load();
  Future<void> setScheme(PinyinScheme scheme);
  Future<void> setAutoCheck(bool value);
  Future<void> openUpdates();
  Future<void> openFeedback();
  Future<void> openLicense();
  Future<void> openNotice();
  Future<void> startDrag();
  Future<void> closeWindow();
}

class WindowsSettingsRepository implements SettingsRepository {
  const WindowsSettingsRepository();
  static const _channel = MethodChannel('retype/settings');

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
  @override
  Future<void> openUpdates() => _channel.invokeMethod('openUpdates');
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
