# Voice input

## Setup

1. Add an audio-capable model under **AI Providers**. Supported transports are OpenAI-compatible audio chat, Gemini audio-file recognition, and Gemini Live transcription. Text-only models and other transcription protocols are not interchangeable.
2. Select the provider and model under **Voice Input**, then choose recognition mode and language. The official Gemini `gemini-3.5-transcribe-live` model defaults to Live; custom aliases can explicitly select Gemini Live. Text cleanup is off by default.
3. On Windows, select a microphone or keep the system default. On Android, grant microphone permission from this page.
4. Use **Start recording test** without inserting text into another app. Live models preview while recording; file models show text only after recognition completes. The provider connection test initializes a Live session for a Live model, without recording or sending a translation request.

## Input

- Windows: hold **Right Alt**, speak, then release to finish. Left Alt and Ctrl + Alt combinations retain their existing behavior. The language bar also exposes a microphone button that starts recording until **Finish recording**. Configure the hold shortcut under **Shortcuts**; Escape cancels.
- Android: tap the keyboard microphone or long-press Space. The voice panel replaces the letter keys. Tap **Done** to finish; **Cancel**, an upward swipe on the waveform, or hiding the keyboard cancels.
- Completed text is inserted once at the original selection. Changing the editor, typing on Windows, or moving the Android cursor cancels the session. If the app rejects insertion, the preview retains the text.

## Model behavior

File recognition records in memory and sends the complete recording once after Stop. There are no pause-triggered uploads, intermediate text previews, automatic retries, or partial-text fallbacks. Qwen3.8 Omni Flash accepts audio files over HTTP; SSE streams the response internally but does not make audio input realtime. Gemini file recognition uses native inline audio and requires a complete response.

Gemini Live uses one bidirectional WebSocket connection, sending PCM16 mono 16 kHz audio frames after setup completes. Manual activity start/end follows the recording controls. Interim hypotheses replace the current preview, and authoritative transcription replaces it with final text. Completion and transcription events may arrive in either order; unfinished previews are never inserted. Cleanup uses SMART transcription in the same session; otherwise VERBATIM is used.

The provider's address and credentials are reused; Google Live derives its WebSocket path from the Gemini API base. A gateway must implement Gemini Live itself: exposing a Live model in its model list is insufficient. Retype does not silently re-upload recordings through a file API when Live fails. Per-model recognition modes are stored with the provider and sync across devices.

The audio adapter sends a PCM16, mono, 16 kHz WAV data URI, requests text streaming, and disables thinking. The configured gateway model `qwen/Qwen-Ambassador/Qwen3.8-Omni-Flash` accepted this format in a public eight-second audio probe. Provider aliases may differ; select the name listed by your provider.

Audio stays in memory for the session and is uploaded to the selected provider only while recording or finalizing. Retype does not save audio files, log transcripts or keys, or upload surrounding editor text. Password and private fields are excluded. Completed voice text can contribute to the existing personal learning store only after insertion succeeds.

Recordings stop at two minutes. The frame queue holds at most 96 entries and each frame is bounded to one second. Buffer overflow, incomplete responses, and timeouts are reported explicitly. File failures never publish partial text. Live previews remain uncommitted until authoritative completion. Cancellation prevents late results from being inserted.

## Statistics and sync

Voice counts are separate from keyboard counts and never contribute to typing speed. The statistics page shows cumulative Chinese characters and English words for voice input. Desktop and mobile streams remain separate during cloud sync.

Provider/model selection, recognition language and text cleanup sync through the existing cloud settings. Microphone selection and permission remain local to each device. Both devices need a version that supports voice settings.

## Verification

Automated coverage exercises WAV framing, fragmented SSE, reasoning exclusion, truncated output, one complete upload for long/short file recordings, no file previews or retries, Live setup-only connection tests, PCM framing, replacement of interim text, late final events, early disconnects, old configuration compatibility, sanitized errors, cancellation, silence, selected-range insertion and privacy guards. Native Windows TSF tests use an isolated in-memory editor; they do not record the developer's microphone or modify installed input methods.

Manual acceptance should also cover Notepad, Windows Search, browser textareas/contenteditable fields, Android normal/private/password fields, microphone permission denial and revocation, device unplugging, and cancelling during a slow request. These app/device checks are distinct from the public-audio model probe.
