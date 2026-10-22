# ADR-0003 · monorepo 布局，Flutter 降级为设置界面

**状态**：部分已取代 · **日期**：M0

monorepo 与跨平台 Rust 内核的决定仍有效；下文的 Flutter 设置界面方案已由 Windows Rust/egui 与 Android Kotlin/Compose 取代。旧工程已移除，品牌资源迁至 `assets/`。当前结构见 [ARCHITECTURE.md](../../ARCHITECTURE.md) 与 [设置程序说明](../../apps/settings-egui/README.md)；以下保留原决策记录。

## 背景

仓库原本是 Flutter 应用脚手架（`lib/` + `windows/` + `android/` 在根目录）。但 Windows 输入法无法用 Flutter 实现（见 [ADR-0001](./0001-full-rust-core-and-tsf.md)），根目录布局会造成「看起来主项目是 Flutter」的误导。

## 决定

改造为 monorepo，Flutter 迁到 `apps/settings`，只承担**设置界面**：

```text
retype/
├─ ARCHITECTURE.md          架构总纲
├─ Cargo.toml               Rust workspace 根
├─ docs/                    roadmap / windows-tsf / dict / adr
├─ core/                    跨平台内核（Rust crates）
│  ├─ types/ pinyin/ dict/ context/ cloud/ engine/ ffi/
├─ platforms/
│  ├─ windows/  tsf/ candidate-ui/ diag/ installer/
│  └─ android/  （M5 启动）
├─ apps/settings/           Flutter 设置界面（原根目录内容）
├─ data/dict/               词库源数据
├─ tools/dict-build/        词库构建工具
└─ test.md                  参考架构原文
```

## 理由

- **目录即架构**：`core/` 不许依赖 `platforms/`，这条铁律靠目录结构一眼可见，也便于 CI 用 `cargo deny`/依赖图检查。
- **单仓库多产物**（TIP DLL、host exe、诊断 exe、词库工具、Flutter 设置界面、Android APK）共享版本号与 CI，跨仓同步成本远高于收益。
- Flutter 保留在 `apps/settings` 而非删除：设置界面（词库管理、热键、隐私白名单、日志查看）用 Flutter 写一份能同时给 Windows 和 Android 用，这是它在本项目里唯一合适的位置。

## 代价

- Flutter 命令必须在 `apps/settings` 下执行（`flutter run -d windows`），已写入根 `README.md`。
- 根目录同时有 Cargo workspace 和 Flutter 项目，IDE（Android Studio / VS Code）需要分别打开子目录才有完整补全。
