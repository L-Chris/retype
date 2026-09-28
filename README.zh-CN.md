<p align="center"><img src="assets/logo.svg" width="88" height="88" alt="retype 标志"></p>

# retype

基于共享 Rust 输入内核的中文输入法。Windows 桌面输入已有预览版，Android 输入法仍在规划中。

[English](README.md) · [最新版本](https://github.com/L-Chris/retype/releases/latest) · [路线图](docs/roadmap.md)

## 当前功能

- 支持 Windows 64 位和 32 位桌面应用的 TSF 输入法。
- 全拼和小鹤双拼，可在语言栏切换；双拼未完成的音节也会预览匹配的字词。
- 内置万象 Base 词库，使用逐词注音和词组权重。
- 紧凑横排候选窗，支持键盘和鼠标选词。
- 点击语言栏图标切换中文和英文。
- 设置的「关于」页支持每日检查、跳过版本、校验下载，以及用户确认后安装。

M1 桌面预览已通过独立 RichEdit 输入测试。常用应用及现代 Windows 应用环境的兼容性仍在验收，详见 [验证记录](docs/m1-validation.md)。

## 安装与使用

从 [最新发布页面](https://github.com/L-Chris/retype/releases/latest)下载安装包，内含 64 位和 32 位输入组件。如果旧安装器留下待重启操作，请先完成重启再升级。

按 `Win+Space` 选择 **retype**。全拼输入 `nihao`，或用小鹤双拼输入 `nihc`，再按空格上屏“你好”。单按 Shift，或点击品牌图标左侧的模式图标，可切换中英文；组字时按 `-`、`=` 翻候选页，按 `1`–`8` 选择当前页候选。右键模式图标选择“设置”，即可切换拼音方案或检查更新。更新后重新打开正在使用输入法的应用，即可加载新版组件。

## 从源码构建

Windows 开发需要 Rust stable、MSVC C++ 构建工具和 `i686-pc-windows-msvc` 目标；生成安装包还需要 Inno Setup。

```powershell
cargo test --workspace
.\platforms\windows\installer\build.ps1
.\platforms\windows\installer\build.ps1 -Installer -SkipDict -NoTest
```

终端调试方法与构建踩坑见 [开发笔记](docs/development-notes.md)。Rust 内核、TSF 适配层、词库和更新流程分别见 [架构文档](ARCHITECTURE.md)、[Windows TSF](docs/windows-tsf.md)、[词库](docs/dict.md) 和 [自动更新](docs/auto-update.md)。

## 里程碑

| 阶段 | 目标 | 状态 |
| --- | --- | --- |
| M0 | 共享内核、词库、TSF 基础和调试台 | 已完成 |
| M1 | 在 Windows 应用中输入中文 | 桌面预览可用，应用兼容性验收中 |
| M2 | 上下文和个人词库 | 计划中 |
| M3 | 云端辅助候选优化 | 计划中 |
| M4 | 流式语音输入 | 计划中 |
| M5 | 产品完善与 Android 输入法 | 进行中；安装器、更新器和小鹤双拼已可用 |

详细验收标准和未完成项见 [路线图](docs/roadmap.md)，历史测量数据见 [性能记录](docs/performance.md)。

## 许可

代码采用 MIT 许可；随附的万象 Base 词库采用 CC BY 4.0。来源与署名见 [NOTICE.txt](NOTICE.txt) 和 [词库许可](data/dict/raw/wanxiang-base/LICENSE)。
