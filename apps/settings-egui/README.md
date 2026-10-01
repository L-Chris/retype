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
  speeds by day/week/month/year, history charts, collection switch and confirmed
  clearing. Existing numeric logs and reset markers remain compatible.
- About: installed version, daily update reminders, manual checks, release notes,
  verified download and elevated installation, installation verification,
  version skipping, feedback and license links.
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
.\tools\scripts\benchmark-settings-egui.ps1 -Renderer glow -Rounds 3
```

The runtime smoke test captures all pages and verifies window reuse and install
path isolation. It opens the update page and checks for a release, but never
installs an update or changes input/dictionary preferences.
The optional network test downloads one pack to an isolated temporary directory:

```powershell
$env:RETYPE_TEST_DICT_BUILDER = "$PWD\target\release\retype-dict-build.exe"
cargo test -p retype-settings-egui download_and_convert_without_changing_user_preferences -- --ignored
```

Production uses OpenGL (`glow`). An optional comparison build enables wgpu:
`cargo build --release -p retype-settings-egui --features wgpu`, then
`--renderer=wgpu`. eframe is pinned to 0.31.1 to support Rust 1.85. Chinese fonts
come from Windows (Microsoft YaHei preferred); Microsoft fonts are not bundled.

Diagnostics support `--page=input|dictionary|statistics|about`, `--updates`,
`--timing-file=<path>`, `--screenshot=<path>`, `--opened-at=<Windows tick>`,
`--exit-after-ms=<milliseconds>` `--idle-exit-ms=<milliseconds>` and `--hide-after-ms=<milliseconds>`. Failures write
`%TEMP%\retype-settings-egui-error.txt` and display a native error message.

Startup benchmarks launch fresh processes with normal OS/driver caches.
`open_ms` includes process loading and ends at the next UI update after the first
render; `first_ui_ms` alone does not measure presentation. Memory is sampled
500ms later, not at its peak. A three-run production measurement on the development machine averaged 505ms
(437–625ms), with a 5.49 MiB executable and 96 MiB average working set.
The earlier Flutter baseline averaged about 2422ms. These results depend on
the machine and cache state; use the script above to measure your environment.
