# Settings startup and window reuse

Windows settings now use egui in a separate process. The installer contains a
single `settings/retype.exe`; no Flutter engine is loaded. Input, Dictionary,
Statistics and About are migrated, and existing registry preferences, downloaded
packs and statistics logs are retained. See [settings development](../apps/settings-egui/README.md).

The custom close button saves preferences and exits by default; it remains hidden
only while pending dictionary downloads or updates finish. Native close exits
for upgrade scripts. Hidden windows stop statistics polling. Each install path
has its own single-instance lock and native message window. Both direct launches
and the input method route `--updates` to About in an existing same-version
process. New windows start centered, without a Windows title bar.

Startup benchmarking, timing arguments and normal-launch timing logs have been
removed from the production app. `tools/scripts/test-settings-egui.ps1` is an
optional manual check of pages, window reuse and install-path isolation; it does
not measure startup speed. Ordinary launches never run a UI test sequence.

The older Flutter diagnostics below describe the archived reference app only;
they do not apply to the production Windows package.

# Historical Flutter startup diagnostics

The settings window stays in its own process. Closing it with the custom close
button hides it for ten minutes, allowing subsequent visits to reuse the Flutter
engine. Statistics polling stops while hidden. The hidden process exits after
the idle period; a normal Windows close still exits immediately for installation
and shutdown.

Both the input method launcher and the settings runner validate the executable
path before reusing a window. Path comparison ignores Windows casing differences.
The single-instance mutex is specific to the executable path so a newly installed
version cannot forward its request to a retained window from an older directory.

## Logs

The runner writes `%LOCALAPPDATA%\retype\logs\settings-startup.log`. The current
file rotates to `.previous` at approximately 256 KiB. The input method also emits
launcher events through Windows debug output, without writing files in the host.
Logs contain only UTC timestamps, process IDs, event names and elapsed times.

- `process_ms`: time since runner initialization.
- `open_ms`: time since the settings click, including process startup before the
  runner entry point; resets for each reused-window activation. Direct launches
  without a click timestamp measure from runner initialization instead.
- `launcher.*`: why a new process was needed, when started by the input method.
- `engine.begin`, `engine.ready`, `engine.channels_ready`: engine initialization
  and platform-channel setup.
- `frame.native_first`, `window.shown`, `frame.dart_first`: first-frame and
  initial window-display milestones.
- `preferences.begin`, `preferences.ready`, `settings.interactive`: preference
  reading and the Dart frame after settings become available.
- `reuse.activated`, `reuse.forwarded`, `reuse.version_mismatch`: activation and
  instance-selection results.
- `window.hidden`, `process.idle_exit`: retention lifecycle.

## Verification

Use a release build and compare a fresh launch, reopening immediately, reopening
after more than one minute, and reopening after ten minutes. Check both
`window.shown` and `settings.interactive`: an earlier frame is not necessarily an
interactive window. Cold launches still initialize Flutter; no engine preloading
or login task is installed.

When testing an upgrade, keep an older settings window hidden, then launch the
settings executable from the new installation directory. The new directory must
get its own process, and another request for that version must raise its window.
