<p align="center"><img src="assets/logo.svg" width="88" height="88" alt="retype logo"></p>

# retype

This project was developed by Qwen3.8 Max.

A Chinese input method built around a shared Rust input engine. Windows desktop input is available as an early preview; Android support is planned.

[简体中文](README.zh-CN.md) · [Latest release](https://github.com/L-Chris/retype/releases/latest) · [Roadmap](docs/roadmap.md)

## Features

- Windows TSF input method for 64-bit and 32-bit desktop applications.
- Full Pinyin and Xiaohe Shuangpin, selectable from the language bar; incomplete Shuangpin codes preview matching characters and phrases.
- A locally bundled Wanxiang Base dictionary with word-specific pronunciations and phrase weights.
- A compact horizontal candidate window with keyboard and mouse selection.
- Chinese/English mode switching from the language bar.
- Offline English word completion and spelling suggestions, with case preservation and persistent personal vocabulary; see [English input](docs/english-input.md).
- Configurable AI providers and models, with a shortcut to translate and replace supported input fields or preview the result first.
- Personal learning saved locally and shared across applications, Full Pinyin, and Xiaohe Shuangpin, with frequency-based ranking and decaying recent preferences.
- Local typing statistics in Settings, with separate Chinese and English counts, speeds, and a seven-day trend; input text is never saved.
- Update controls in Settings > About, with daily checks, version skipping, verified downloads, and installation after user confirmation.
- Optional [WebDAV cloud sync](docs/cloud-sync.md) for settings, personal learning, and typing history across computers.

The M1 desktop preview has passed a dedicated RichEdit input test. Compatibility across common applications and modern Windows app environments is still being evaluated; see the [validation record](docs/m1-validation.md).

## Install and use

Download the Windows installer from the [latest release](https://github.com/L-Chris/retype/releases/latest). It includes both 64-bit and 32-bit input components. If an earlier installer has a pending restart, complete that restart before upgrading.

Select **retype** with `Win+Space`. Type `nihao` in Full Pinyin or `nihc` in Xiaohe Shuangpin, then press Space to commit “你好”. Tap Shift alone to switch between Chinese and English, or click the mode icon on the left of the branded language indicator. While composing, use `-` and `=` to turn candidate pages; keys `1`–`8` select from the current page. Right-click the mode icon and choose **Settings** to change the Pinyin scheme, view typing statistics, or check for updates. Reopen applications that were already running after an update so they load the new input component.

In English mode, Space commits what you typed; Tab or clicking a candidate accepts it with a trailing space. Use Up/Down to select explicitly, then Space to accept it with a trailing space. Numbers and punctuation remain ordinary input. Settings > Input controls English completion and spelling suggestions.

## Build from source

Windows development requires stable Rust, the MSVC C++ build tools, and the `i686-pc-windows-msvc` target. Inno Setup is needed to build an installer.

```powershell
cargo test --workspace --features retype-learning/broker,retype-ai/service
.\platforms\windows\installer\build.ps1
.\platforms\windows\installer\build.ps1 -Installer -SkipDict -NoTest
```

The [developer notes](docs/development-notes.md) cover the terminal input tool and build pitfalls. The Rust engine, TSF adapter, dictionary pipeline, typing statistics, and update design are documented in [ARCHITECTURE.md](ARCHITECTURE.md), [Windows TSF](docs/windows-tsf.md), [dictionary](docs/dict.md), [typing statistics](docs/typing-statistics.md), and [automatic updates](docs/auto-update.md).

Personal learning storage, sharing, and recovery boundaries are described in [user learning](docs/user-learning.md).
Provider configuration, translation shortcuts, and editor compatibility boundaries are described in [AI translation](docs/ai-translation.md).

## Milestones

| Milestone | Goal | Status |
| --- | --- | --- |
| M0 | Shared engine, dictionary, TSF foundation, and diagnostic tool | Complete |
| M1 | Chinese input in Windows applications | Desktop preview; application compatibility review in progress |
| M2 | Context and personal dictionary | Persistent shared learning available; context learning planned |
| M3 | Cloud-assisted candidate refinement | Planned |
| M4 | Streaming voice input | Planned |
| M5 | Product polish and Android input method | In progress; installer, updater, and Xiaohe Shuangpin are available |

Detailed acceptance criteria and current gaps are in the [roadmap](docs/roadmap.md). Historical measurements are in [performance notes](docs/performance.md).

## License

Code is MIT licensed. The bundled Wanxiang Base dictionary is CC BY 4.0; its source and attribution are in [NOTICE.txt](NOTICE.txt) and [its license](data/dict/raw/wanxiang-base/LICENSE).
