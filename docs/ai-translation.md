# AI translation

The settings window includes **AI providers**, **Translation**, and **Shortcuts**. Configure a provider, select one or more models, and choose a translation model and target language. Source languages are detected automatically. Translation is explicitly triggered, rather than continuously sent while typing.

## Using translation

1. Add a provider and its API address and key. Presets include OpenAI-compatible services, Anthropic, Gemini, and Ollama. The custom provider supports all four interface types.
2. Refresh the model list or add a model ID manually. Connection testing sends a short synthetic phrase, not text from an application.
3. Under Translation, select a **provider / model** and target language. Choose automatic replacement or preview before replacement. Optional instructions and a 5–180 second timeout are available; compatible supported model families also expose reasoning effort.
4. Select retype in the target application and press **Ctrl+Alt+0** (the number-row zero key). Both Chinese and English modes support translation. The shortcut can be changed or cleared under Shortcuts; the default Chinese/English toggle remains a Shift tap. Existing custom shortcuts are retained.
5. A compact, non-activating status strip shows a spinning indicator while translating and can be closed to cancel. Completed translations briefly show a success message before disappearing. Preview mode shows the translated text with replacement and close controls; long previews support the mouse wheel. A test area in the Translation settings page can try the configured model without changing an application.

Changes save automatically, including when closing the settings window. Shortcut changes reach already-running input components through their shared preferences cache within approximately 250 ms.

Ctrl/Alt/Shift translation combinations are registered as TSF preserved keys while retype is active, including in English mode. Windows-key combinations use the ordinary key-event path because the preserved-key API does not represent that modifier. Shortcut and text-store metadata can be collected using [Settings diagnostics](settings-diagnostics.md).

## Text and replacement boundaries

The Windows adapter reads a complete TSF context range from its start to end, in bounded UTF-16 chunks. The range must expose ACP boundaries starting at zero, and the returned text length must equal the complete range length. Private/password/PIN input scopes, read-only contexts, embedded objects, invalid UTF-16, and unsupported or partial text stores are rejected. The optional NOHIDDENTEXT capability is required for automatic learning, but its absence does not deny an explicit translation request. Windows Search's composition-only fallback is not a full-text translation source.

The host application defines its TSF context; compatibility must be verified for each editor. A document editor may expose an entire document, whereas a browser may expose one edit control. Unsupported contexts do not fall back to simulated Ctrl+A/C or silently translate a prefix. The local limit is 65,536 UTF-16 units, and provider model limits can be smaller. Empty, truncated, blocked, or oversized model output is never automatically written back.

An active composition is committed before translation. Before replacement, retype checks the input method activation epoch, focused TSF context, foreground owner window, absence of a new composition, privacy/read-only status, and exact equality with the captured original text. A mismatch leaves the current text intact. A matching result replaces the complete range in one write transaction and attempts to move the caret to its end. Undo behavior depends on the host editor. Translation output is not recorded as typing activity or learned word selections.

## Storage and background service

- Metadata lives in `%LOCALAPPDATA%\retype\ai\settings.json`, outside versioned installations, using atomic replacement and a user/SYSTEM-only directory ACL.
- API keys live in Windows Credential Manager under `retype/ai/<provider-id>`; JSON configuration, IPC, and diagnostics do not contain keys.
- Shortcut preferences are stored in `HKCU\Software\retype`, readable by AppContainer input hosts. They contain key codes and modifier bits only.
- The native `retype-ai-host.exe` starts on demand, handles network requests outside application processes, and exits after ten idle minutes. It yields to the active installation between jobs after an upgrade.
- AppContainer clients that cannot start a full-trust child ask the installed user learning broker to launch the fixed AI helper beside its executable; callers cannot supply an executable path or shell arguments. If that broker is unavailable, opening settings or logging in again restores a desktop launch path.
- The authenticated per-user named pipe reuses the learning service transport, including user SID authentication, owner verification, remote-client rejection, and bounded overlapped IO. Models/tests accept only a provider configuration saved by the settings UI, preventing IPC callers from redirecting stored credentials to another address.
- At most four network workers run concurrently, with bounded request time, response size and retained jobs. Cancellation invalidates the result immediately; an already-sent HTTP request may continue until completion or timeout.
- The selected provider receives the entire captured text only on a translation request. Original and translated text remain in memory; they are not written to history, statistics, personal learning, or logs. Provider-side retention is outside retype's control.
- HTTP errors are summarized without echoing provider response bodies. Redirects are disabled so authorization headers are not forwarded elsewhere.

## Validation

Tests cover full multi-chunk TSF reads with emoji and paragraphs, stale snapshots, rejected writes, hidden contexts, invalid/embedded text, model output truncation, reasoning-text exclusion, local HTTP request formats, model listing, sanitized errors, shortcut validation/recording, and normal/minimum-width egui layouts. The existing real AppContainer named-pipe tests exercise the reused transport. Real provider credentials and compatibility across additional editors still require user testing.

```powershell
cargo test --workspace --features retype-learning/broker,retype-ai/service
cargo test --target i686-pc-windows-msvc -p retype-tsf
cargo build -p retype-ai --features service
```
