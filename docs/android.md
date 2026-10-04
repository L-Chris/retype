# Android

Android milestones A1–A3 live in `platforms/android`: Kotlin,
`InputMethodService`, Jetpack Compose, and a small JNI adapter to the same Rust
engine used on Windows. Android 8.0 (API 26) or newer is required. Signed APKs are
published alongside Windows installers on the GitHub Releases page.

## Available behavior

- Offline Full Pinyin and Xiaohe Shuangpin, including incomplete syllables.
- English completion and spelling suggestions; tapping a word adds a space.
- A QWERTY keyboard, uppercase toggle, numeric/symbol layout, repeat backspace,
  language switching, and the system input-method picker.
- Single-line scrollable candidates and a scrollable expanded grid replacing the keyboard. Words
  stay intact; the desktop's 480 px limit does not apply to a phone keyboard.
- Chinese punctuation in Chinese mode and ordinary punctuation in English.
- Search, send, next, done and newline actions supplied by the current editor.
- Local SQLite personal learning, restored across sessions. Learning is only
  released after the current editor accepts the commit.
- Password fields use direct input without prediction or learning; editors
  requesting no personalized learning do not load or update personal words.
- Email/URL fields start in English; numeric fields use direct numeric input.
- Settings for Input, Dictionaries, Statistics, AI providers, Translation,
  Cloud sync and About, with an enable/select wizard and test editor.
- Optional Wanxiang dictionaries download only when enabled, verify a pinned
  SHA-256, compile the upstream word readings and publish an immutable binary.
- Chinese/English character counts and active-time weighted typing speeds,
  separately reported for the last five minutes and day/week/month/year.
- AI provider model discovery and translation using Compatible, Anthropic,
  Gemini or Ollama APIs, with the desktop reasoning settings (default: none).
- On-screen buttons for translation and Chinese/English switching.
- Shared WebDAV snapshots for settings, personal learning and statistics,
  including cstcloud compatibility, initial merge confirmation and conflict choices.
- Android Keystore encryption for credentials; API keys and WebDAV passwords
  stay local, matching the desktop credential exclusion.

Tablet-specific layouts belong to A4. About now includes Android update checks,
release notes, streamed downloads and a system installer entry point.
Network access is used for explicitly enabled downloads, translation and sync;
the app does not request accessibility or external-storage access.

## A3 behavior and boundaries

Translation reads a full extracted editor snapshot only on demand, rejects
password fields, partial snapshots and text over 65,536 UTF-16 units, and never
logs input text or credentials. Replacement requires the same editor, unchanged
text and unchanged selection. A successful translation that cannot be replaced
is copied with a short keyboard notice. Changing editors cancels the result.
Editors that refuse full text extraction cannot use whole-field translation;
browser compatibility depends on the browser's InputConnection implementation.
The loading indicator and result notices live inside the keyboard; notices expire.

Statistics store only numeric minute buckets, without input text. Password fields
are excluded. Gaps above 15 seconds do not count as active time. Speeds need at
least ten characters and one second of activity. A period compares its current
partial totals with the preceding complete calendar period in the device's zone.
Synced buckets merge by device, stream and minute using cumulative maxima, so
repeated downloads cannot count the same typing twice.

Enabling an optional dictionary downloads and verifies it; disabling retains its
local file. Dictionaries and input options apply on the next editor session.
The download manifest and tone stripping are shared with the desktop builder.

Cloud sync has manual connection testing and synchronization, plus hourly
WorkManager jobs while enabled and connected. Android may defer background jobs.
All portable settings, learning and statistics sync by default; APKs, dictionary
binaries, logs and credentials do not. Dictionary selections trigger independent
verified downloads. WebDAV uses the desktop `retype/v1` content-addressed format
and retains version history. The first import requires confirmation; concurrent
settings remain local until explicitly resolved. No cloud account is required
for normal offline typing.

## Updates and release signing

About checks stable GitHub releases for `retype-<version>-android.apk` and its
`.sha256` file. Windows-only releases and prereleases are ignored. Automatic
checks run at most once per day when settings open, plus a connected daily
WorkManager job. Android may defer that job. Checks never download or install
without a button click, and a release without an Android APK is reported explicitly.

Downloads stream into private cache with bounded size and a progress indicator.
Before exposing the APK to the system installer, retype checks the SHA-256,
package name, increasing version code and the installed app's signing certificate.
The installer receives a temporary read grant through a provider restricted to
the update cache. If needed, the user grants retype permission to install apps
and returns to continue the already requested installation. There is no silent
installation. Cancelled or failed downloads remove their temporary file.

The release workflow includes signed Android APKs when all four repository
secrets are available: `ANDROID_KEYSTORE_BASE64`, `ANDROID_STORE_PASSWORD`,
`ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`. Keep an independent secure backup
of the signing key; future APKs must use the same certificate. No key is generated
or uploaded automatically, and no signing credentials are tracked in Git.
Without these secrets, Windows releases still work and Android publishing is
explicitly skipped. The workflow verifies APK signatures, 16 KB zip alignment,
and uploads the matching SHA-256 alongside the APK.

For a local signed release build, set `RETYPE_ANDROID_KEYSTORE` (absolute path),
`RETYPE_ANDROID_STORE_PASSWORD`, `RETYPE_ANDROID_KEY_ALIAS`, and
`RETYPE_ANDROID_KEY_PASSWORD`, then run `gradlew :app:assembleRelease`.
Debug previews use the local development key and cannot be overwritten by a
formal release signed with a different key; the updater reports this mismatch.
The GitHub APK channel is intended for direct APK distribution, not Play Store
distribution, which would need its own store update mechanism.

## Build on Windows

Install JDK 17, Android SDK platform 35, build-tools 36.0.0 and NDK r28 or newer.
Set `JAVA_HOME` and `ANDROID_HOME`, then run from the repository root:

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
# Generate the offline binary from the tracked Wanxiang sources, if absent:
cargo run --locked --release -p retype-dict-build -- --out data/dict/retype-dict.tsv
./platforms/android/build.ps1
# Also include the emulator ABI:
./platforms/android/build.ps1 -Abi arm64-v8a,x86_64
```

The debug-signed APK is `platforms/android/app/build/outputs/apk/debug/app-debug.apk`.
Enable retype in Android's system keyboard settings, then select it as the current
input method. In retype Settings > 输入 choose Full Pinyin or
Xiaohe; changes apply when entering an editor again. Try `nihao` or `nihc`, or
switch to English and try `hel`. For `python3`, use the numeric keyboard: its
digits are literal and never choose candidates.

Typing statistics are separated by platform: Android shows mobile input only,
and Windows shows desktop input only, including counts, speeds and history charts.
Cloud sync preserves both platforms' records without combining their statistics.

The bundled binary is about 95 MiB before APK compression. Installation prepares
a versioned read-only app-private file via atomic rename. Android maps that file
rather than copying the dictionary into the heap. Never overwrite, truncate or
chmod a mapped dictionary in place. Upgrades publish a new file/inode.

## Linux/macOS native build

With an Android SDK, NDK r28+ and JDK 17:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --locked
cargo run --locked --release -p retype-dict-build -- --out data/dict/retype-dict.tsv
RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384' \
  cargo ndk -t arm64-v8a -t x86_64 -p 26 -o platforms/android/build/native \
  build --locked --release -p retype-android
cd platforms/android
./gradlew :app:assembleDebug :app:lintDebug
```

## Validation

```powershell
cargo test --locked -p retype-android
cargo test --locked -p retype-learning --features broker
cd platforms/android
./gradlew.bat :app:lintDebug :app:connectedDebugAndroidTest
```

Set `ANDROID_SERIAL` if multiple devices are connected. The connected tests need
the APK and test APK installation confirmed on phones that restrict ADB installs.
Native tests exercise the bundled dictionary, both Pinyin schemes, English
learning persistence and private sessions. The UI test types only into isolated
test editors, verifies touch commits and field switching, and restores the
previous default input method afterward.

A3 has been exercised on an Android 15 ARM64 phone and an Android 15 x86_64
emulator: all 12 connected tests passed, including real editor translation,
transient notices, credential encryption, verified upstream dictionary download,
statistics deduplication and WebDAV conflict resolution. The HTTP fixtures use
isolated local servers rather than a user's AI key or cloud account. Android 8–14,
individual browsers and production cloud accounts still need their own checks.

The JNI ABI uses numeric handles, validates stale candidate generations, contains
Rust panics, and reports failures as Java exceptions. A serial engine dispatcher
queues keys and commits; main-thread editor calls are gated by an editor epoch
and the captured connection. SQLite writes run on a separate writer thread.
Small JSON commands/snapshots are the initial bridge protocol; engine latency
and allocation costs should be profiled on representative Android hardware before
changing that transport. A P99 of 6 ms remains a measurement target, not an
established Android performance claim.

The Rust library is linked for 16 KiB ELF alignment. Locally verified builds also
passed checks of the packaged Compose native library and APK zip alignment. A 16 KiB-page device still
needs its own runtime compatibility test before declaring that device supported.
