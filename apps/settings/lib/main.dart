import 'package:flutter/material.dart';

import 'settings_repository.dart';

void main() => runApp(const RetypeApp());

const _ink = Color(0xFF142B3B);
const _accent = Color(0xFF138F96);
const _muted = Color(0xFF657781);
const _border = Color(0xFFE3E9EC);

class RetypeApp extends StatefulWidget {
  const RetypeApp({super.key, this.repository});
  final SettingsRepository? repository;

  @override
  State<RetypeApp> createState() => _RetypeAppState();
}

class _RetypeAppState extends State<RetypeApp> with WidgetsBindingObserver {
  late final SettingsRepository repository =
      widget.repository ?? const WindowsSettingsRepository();
  SettingsSnapshot? settings;
  String? error;
  bool saving = false;
  int page = 0;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _load();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed && !saving) _load();
  }

  Future<void> _load() async {
    try {
      final value = await repository.load();
      if (mounted) {
        setState(() {
          settings = value;
          error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => error = '读取设置失败：$e');
    }
  }

  Future<void> _saveScheme(PinyinScheme scheme) async {
    final current = settings;
    if (saving || current == null || current.scheme == scheme) return;
    setState(() {
      saving = true;
      error = null;
    });
    try {
      await repository.setScheme(scheme);
      if (mounted) {
        setState(
          () => settings = SettingsSnapshot(
            scheme: scheme,
            autoCheck: current.autoCheck,
            version: current.version,
          ),
        );
      }
    } catch (e) {
      if (mounted) setState(() => error = '保存拼音方案失败：$e');
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  Future<void> _saveAutoCheck(bool value) async {
    final current = settings;
    if (saving || current == null) return;
    setState(() {
      saving = true;
      error = null;
    });
    try {
      await repository.setAutoCheck(value);
      if (mounted) {
        setState(
          () => settings = SettingsSnapshot(
            scheme: current.scheme,
            autoCheck: value,
            version: current.version,
          ),
        );
      }
    } catch (e) {
      if (mounted) setState(() => error = '保存更新设置失败：$e');
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  Future<void> _open(Future<void> Function() action) async {
    try {
      await action();
    } catch (e) {
      if (mounted) setState(() => error = '打开失败：$e');
    }
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'retype 设置',
    debugShowCheckedModeBanner: false,
    theme: ThemeData(
      useMaterial3: true,
      colorScheme: ColorScheme.fromSeed(seedColor: _accent),
      fontFamily: 'Microsoft YaHei UI',
      textTheme: ThemeData.light().textTheme.apply(
        bodyColor: _ink,
        displayColor: _ink,
      ),
    ),
    home: Scaffold(
      backgroundColor: const Color(0xFFF7F9FA),
      body: Stack(
        children: [
          Row(
            children: [
              _sidebar(),
              Expanded(
                child: Column(
                  children: [
                    if (error != null)
                      MaterialBanner(
                        content: Text(error!),
                        actions: [
                          TextButton(
                            onPressed: () => setState(() => error = null),
                            child: const Text('关闭'),
                          ),
                        ],
                      ),
                    Expanded(
                      child: settings == null
                          ? Center(
                              child: error == null
                                  ? const CircularProgressIndicator()
                                  : TextButton(
                                      onPressed: _load,
                                      child: const Text('重试'),
                                    ),
                            )
                          : page == 0
                          ? _inputPage(settings!)
                          : _aboutPage(settings!),
                    ),
                  ],
                ),
              ),
            ],
          ),
          Positioned(
            top: 0,
            left: 0,
            right: 58,
            height: 30,
            child: Listener(
              behavior: HitTestBehavior.opaque,
              onPointerDown: (_) => _open(repository.startDrag),
              child: Semantics(label: '拖动设置窗口'),
            ),
          ),
          Positioned(
            top: 8,
            right: 8,
            child: IconButton(
              tooltip: '关闭设置',
              onPressed: () => _open(repository.closeWindow),
              icon: const Icon(Icons.close_rounded, color: _muted),
            ),
          ),
        ],
      ),
    ),
  );

  Widget _sidebar() => Container(
    width: 238,
    decoration: const BoxDecoration(
      gradient: LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [Color(0xFFDDF6F8), Color(0xFFEAF6FC)],
      ),
    ),
    child: SafeArea(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 34, 16, 48),
            child: Row(
              children: [
                Image.asset('assets/logo.png', width: 38, height: 38),
                const SizedBox(width: 10),
                const Expanded(
                  child: Text(
                    'retype 输入法',
                    style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.w700,
                      color: _ink,
                    ),
                  ),
                ),
              ],
            ),
          ),
          _navItem(0, Icons.keyboard_outlined, '输入'),
          const SizedBox(height: 8),
          _navItem(1, Icons.info_outline_rounded, '关于'),
          const Spacer(),
          const Padding(
            padding: EdgeInsets.fromLTRB(26, 0, 0, 24),
            child: Text(
              '简单、专注地输入',
              style: TextStyle(fontSize: 12, color: _muted),
            ),
          ),
        ],
      ),
    ),
  );

  Widget _navItem(int index, IconData icon, String label) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 12),
    child: Material(
      color: page == index
          ? Colors.white.withValues(alpha: .82)
          : Colors.transparent,
      borderRadius: BorderRadius.circular(12),
      child: InkWell(
        borderRadius: BorderRadius.circular(12),
        onTap: () => setState(() => page = index),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 17, vertical: 14),
          child: Row(
            children: [
              Icon(icon, size: 22, color: page == index ? _accent : _ink),
              const SizedBox(width: 14),
              Text(
                label,
                style: TextStyle(
                  fontSize: 16,
                  fontWeight: page == index ? FontWeight.w700 : FontWeight.w500,
                ),
              ),
            ],
          ),
        ),
      ),
    ),
  );

  Widget _inputPage(SettingsSnapshot state) => SingleChildScrollView(
    padding: const EdgeInsets.symmetric(horizontal: 46, vertical: 46),
    child: Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 650),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              '输入',
              style: TextStyle(fontSize: 28, fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 10),
            const Text('选择适合你的拼音输入方式。', style: TextStyle(color: _muted)),
            const SizedBox(height: 34),
            const Text(
              '拼音方案',
              style: TextStyle(fontSize: 16, fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 14),
            _card(
              Column(
                children: [
                  _schemeRow(
                    PinyinScheme.full,
                    '全拼',
                    '输入 nihao，空格上屏“你好”',
                    state.scheme,
                  ),
                  const Divider(height: 1, color: _border),
                  _schemeRow(
                    PinyinScheme.xiaohe,
                    '小鹤双拼',
                    '输入 nihc，空格上屏“你好”',
                    state.scheme,
                  ),
                ],
              ),
            ),
            const SizedBox(height: 20),
            const Text(
              '单按 Shift 切换中英文；组字时用 - 和 + 翻候选页。',
              style: TextStyle(fontSize: 13, color: _muted),
            ),
          ],
        ),
      ),
    ),
  );

  Widget _schemeRow(
    PinyinScheme value,
    String title,
    String subtitle,
    PinyinScheme selected,
  ) => InkWell(
    onTap: saving ? null : () => _saveScheme(value),
    child: Padding(
      padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 20),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: const TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 5),
                Text(
                  subtitle,
                  style: const TextStyle(fontSize: 13, color: _muted),
                ),
              ],
            ),
          ),
          Icon(
            selected == value
                ? Icons.radio_button_checked
                : Icons.radio_button_unchecked,
            color: selected == value ? _accent : _muted,
          ),
        ],
      ),
    ),
  );

  Widget _aboutPage(SettingsSnapshot state) => SingleChildScrollView(
    padding: const EdgeInsets.symmetric(horizontal: 46, vertical: 38),
    child: Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 610),
        child: Column(
          children: [
            const SizedBox(height: 12),
            Container(
              padding: const EdgeInsets.all(14),
              decoration: BoxDecoration(
                color: Colors.white,
                borderRadius: BorderRadius.circular(22),
                border: Border.all(color: _border),
              ),
              child: Image.asset('assets/logo.png', width: 86, height: 86),
            ),
            const SizedBox(height: 21),
            const Text(
              'retype 输入法',
              style: TextStyle(fontSize: 28, fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 5),
            Text(
              state.version,
              style: const TextStyle(fontSize: 16, color: _muted),
            ),
            const SizedBox(height: 34),
            _card(
              Column(
                children: [
                  SwitchListTile.adaptive(
                    value: state.autoCheck,
                    onChanged: saving ? null : _saveAutoCheck,
                    title: const Text(
                      '每天自动检查并提醒',
                      style: TextStyle(
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    subtitle: const Text(
                      '发现新版本时提醒，不会自动下载安装',
                      style: TextStyle(fontSize: 12),
                    ),
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: 22,
                      vertical: 5,
                    ),
                  ),
                  const Divider(
                    height: 1,
                    indent: 22,
                    endIndent: 22,
                    color: _border,
                  ),
                  _actionRow(
                    Icons.system_update_alt,
                    '检查更新',
                    () => _open(repository.openUpdates),
                  ),
                  const Divider(
                    height: 1,
                    indent: 22,
                    endIndent: 22,
                    color: _border,
                  ),
                  _actionRow(
                    Icons.chat_bubble_outline,
                    '反馈问题',
                    () => _open(repository.openFeedback),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 38),
            const Text(
              '© 2026 retype · MIT License',
              style: TextStyle(fontSize: 12, color: _muted),
            ),
            Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                TextButton(
                  onPressed: () => _open(repository.openLicense),
                  child: const Text('开源许可'),
                ),
                const Text('·', style: TextStyle(color: _muted)),
                TextButton(
                  onPressed: () => _open(repository.openNotice),
                  child: const Text('第三方署名'),
                ),
              ],
            ),
          ],
        ),
      ),
    ),
  );

  Widget _actionRow(IconData icon, String label, VoidCallback onTap) => InkWell(
    onTap: onTap,
    child: Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 17),
      child: Row(
        children: [
          Icon(icon, color: _accent, size: 21),
          const SizedBox(width: 14),
          Expanded(
            child: Text(
              label,
              style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w600),
            ),
          ),
          const Icon(Icons.chevron_right, color: _muted),
        ],
      ),
    ),
  );

  Widget _card(Widget child) => Material(
    color: Colors.white,
    elevation: 3,
    shadowColor: const Color(0x20142B3B),
    shape: RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(18),
      side: const BorderSide(color: _border),
    ),
    clipBehavior: Clip.antiAlias,
    child: child,
  );
}
