# Development notes

The main README covers installation and basic contribution steps. This document keeps the diagnostic workflow and pitfalls discovered while building the Windows input method.

## Architecture and layout

The platform-independent Rust engine lives under `core/`. `platforms/windows/tsf` is the in-process TSF adapter, and `platforms/windows/updater` performs network work in a separate process. `apps/settings` is a Flutter settings shell, not the TSF input component. See [ARCHITECTURE.md](../ARCHITECTURE.md) and the [ADRs](adr/) for design decisions.

The input thread must not perform network calls, file I/O, dictionary loading, or wait on locks across calls. The local first pass remains usable when optional external services fail; asynchronous candidate updates must not remove text already displayed.

## Terminal diagnostic tool

`retype-diag` runs the same input engine and dictionary as TSF, with text output and stdin instead of a candidate window and keyboard callbacks. It is the fastest way to inspect candidate ordering before testing in a desktop host.

```powershell
cargo run -p retype-diag --release -- --dict data/dict/retype-dict.tsv
cargo run -p retype-diag --release -- --dict data/dict/retype-dict.tsv --explain nihaomashijie
cargo run -p retype-diag --release -- --bench --dict data/dict/retype-dict.tsv
```

Use `:help` in the tool for its interactive commands. The dictionary build command is:

```powershell
cargo run -p retype-dict-build --release -- --out data/dict/retype-dict.tsv
```

## Pitfalls recorded during development

- The `windows` 0.62 crate does not expose an `implement` feature. The `#[implement]` macro needs `windows-core` as a direct dependency.
- The feature is `Win32_UI_Input_Ime`, not `Win32_UI_Input_Methods`.
- `AdviseKeyEventSink` belongs to `ITfKeystrokeMgr`, which must be queried from the thread manager.
- In this version of the bindings, use `windows_core::BOOL`; `Param<T, InterfaceType>` accepts a borrow, for example `AdviseKeyEventSink(tid, &sink, true)`.
- The `pinyin` crate can return `lü`; normalize it to `lv` before matching the syllable table.
- K-best traceback cannot rely on `(position, slot index)` because top-k insertion and truncation change indexes. The engine uses an `Rc` chain and a regression test.
- Unigram scoring can favor implausible segmentations. The current formula and its limits are in [dictionary notes](dict.md); historical tuning is in [performance notes](performance.md).
- Inno Setup scripts containing Chinese must be saved with a UTF-8 BOM. Without it, the compiler can silently write garbled profile names.
- A TSF DLL may remain loaded in another application during an upgrade. Install into a new version directory, then update registration; see [automatic updates](auto-update.md).

The editable app mark is [assets/logo.svg](../assets/logo.svg). After changing its geometry, update the matching drawing commands in `tools/scripts/generate-logo.ps1` and run that script to regenerate the Windows icon, Flutter PNG, and Android launcher icons.
