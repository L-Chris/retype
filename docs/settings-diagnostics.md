# Settings diagnostics

Settings lifecycle logs are enabled in builds containing this change. Installing an older release does not enable them.

## Collecting logs

After a failed attempt to open Settings, record the time and the application from which Settings was opened. Collect `settings-*.jsonl` from `%LOCALAPPDATA%\retype\logs`. If that directory cannot be written, check `%TEMP%\retype-settings-logs` in the affected process's environment. The existing startup error report remains at `%TEMP%\retype-settings-egui-error.txt`.

Launcher and Settings records share a numeric `request` identifier. Each record includes the timestamp in Unix milliseconds, process ID, version, architecture, event and diagnostic details.

## Reading the sequence

- `open_requested`, `active_dir_lookup`, `target_ready`: the input method received the command and resolved the installed Settings executable.
- `reuse_post`, `show_received`, `show_dispatch`: a retained Settings process was asked to show its window and handled the request.
- `spawn_begin`, `spawn_ok`, `process_start`: a new Settings process was launched.
- `fonts_ready`, `preferences_ready`, `ai_preferences_ready`, `ui_frame_complete`: initialization and the first UI frame completed.
- `window_state`: visibility, minimized state, foreground state and window bounds.
- `startup_failed`, `panic`, `child_exit`: startup failure, panic location or process termination.
- `translation_key_registration`: registration of the translation shortcut with TSF.
- `translation_shortcut`, `translation_preserved_shortcut`: whether a translation edit request was accepted.
- `translation_context`: text-store capability and read-only flags; no text is recorded.

Compare records with the same request ID to find the last completed stage. A posted message alone does not establish that the window became visible.

## Storage and privacy

Logs contain lifecycle metadata, error codes and timings. They do not record typed text, API keys, provider configuration, raw command-line arguments or panic payloads. Writes run on a background thread through a bounded queue; input callbacks do not wait for disk writes. Logging is best effort, so records can be dropped if the queue is full or storage is unavailable.

Each process keeps a log up to 2 MiB and one previous file. Matching diagnostic files older than seven days are removed when the logger starts. The installer grants packaged applications write access to this dedicated log directory.
