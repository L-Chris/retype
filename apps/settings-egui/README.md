# Windows settings

The production Windows settings application uses egui/eframe in its own process.
The installer places it at `settings/retype.exe`, preserving older input-method
launchers and update shortcuts. Flutter is no longer part of the Windows package.
The original `apps/settings` sources remain as a reference and asset source.

## Features

- Input: Full Pinyin and Xiaohe Shuangpin, sharing the existing registry settings.
- Dictionary: seven optional Wanxiang packs, download progress and cancellation,
  pinned SHA-256 verification and local conversion. Each pack has an on/off switch;
  turning on an undownloaded pack downloads and enables it automatically.
- Statistics: separate Chinese/English counts and live speeds, weighted average
  speeds by day/week/month/year and history charts. Collection is always enabled;
  existing numeric logs and legacy reset markers remain compatible.
- About: installed version, daily update reminders, manual checks, release notes,
  verified download and elevated installation, installation verification,
  version skipping, feedback and license links.
- Cloud sync: WebDAV connection tests, background automatic/manual sync, first-merge
  summaries and conflict resolution. Settings, personal learning and statistics
  synchronize together; credentials remain local. See [cloud sync](../../docs/cloud-sync.md).
- Borderless, centered, draggable window with no header row and a close button
  at the top right of the content area. Closing
  with that button retains a hidden window for ten minutes; native close exits
  so upgrade scripts can stop an obsolete instance. Hidden windows do not poll
  statistics. Executable-path-specific instance routing keeps versions separate.

Network work, dictionary conversion, hashing and statistics reads run in a
background worker. Update downloads reuse `retype-updater.exe`; elevated setup
and post-install verification preserve the existing update contract. The latter
continues to use `update-verify.ps1`, including its profile checks. Opening
Settings does not launch PowerShell.

## Build and validate

```powershell
cargo build --release -p retype-settings-egui
.\target\release\retype-settings-egui.exe
cargo test -p retype-settings-egui
cargo clippy -p retype-settings-egui --all-targets -- -D warnings
.\tools\scripts\test-settings-egui.ps1
```

The optional manual runtime smoke test captures all pages and verifies window reuse and install
path isolation. It opens the update page and checks for a release, but never
installs an update or changes input/dictionary preferences.
The optional network test downloads one pack to an isolated temporary directory:

```powershell
$env:RETYPE_TEST_DICT_BUILDER = "$PWD\target\release\retype-dict-build.exe"
cargo test -p retype-settings-egui download_and_convert_without_changing_user_preferences -- --ignored
```

Production uses OpenGL (`glow`). An optional comparison build enables wgpu:
`cargo build --release -p retype-settings-egui --features wgpu`, then
`--renderer=wgpu`. egui/eframe are pinned to 0.36.2 and require Rust 1.95 or newer. Chinese fonts
come from Windows (Microsoft YaHei preferred); Microsoft fonts are not bundled.

Manual preview options support `--page=input|dictionary|statistics|about`,
`--updates`, `--screenshot=<path>`, `--exit-after-ms=<milliseconds>`,
`--idle-exit-ms=<milliseconds>` and `--hide-after-ms=<milliseconds>`.
Failures write `%TEMP%\retype-settings-egui-error.txt` and display a native message.

Startup benchmarking and timing logs have been removed. Functional checks remain
available on demand; ordinary launches do not run a benchmark or UI test sequence.
Legacy timing arguments from older input-method launchers are accepted and ignored.
