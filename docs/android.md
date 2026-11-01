# Android

Android milestones A1–A3 live in `platforms/android`: Kotlin,
`InputMethodService`, Jetpack Compose, and a small JNI adapter to the same Rust
engine used on Windows. Android 8.0 (API 26) or newer is required. Signed APKs are
published alongside Windows installers on the GitHub Releases page.

## Available behavior

- Offline Full Pinyin and Xiaohe Shuangpin, including incomplete syllables.
- Separate Chinese and English system keyboard entries with English completion and spelling suggestions; tapping a word adds a space.
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

In Settings → Input, use **Manage Chinese and English entries** to enable both
`retype 中文` and `retype English` in Android's language settings. The keyboard's
language button selects the other retype entry. English completion and spelling
suggestions remain active in the English entry; changing editors preserves the
selected entry.

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

## Clipboard devices

The Cross-device page can associate multiple computers. Each device has its own
encrypted credential, certificate pin, saved routes, connection and retry loop.
Adding, cancelling or removing one association leaves the other connections
running. Existing single-device associations migrate automatically.

Text and supported images copied on the phone are sent to every connected device.
Incoming clipboard events share one deduplication journal; received content is
not reissued as a new phone copy. Clipboard access still requires retype to be the
default keyboard, and sensitive clipboard items are not sent.

Android can both initiate and accept connections, including connections from
Windows, macOS and another Android device. All three platforms default to TCP
port `50206`. The page shows the phone's IPv4 listening endpoints as `IP:port`,
with an editable listening port and an explicit error if the port cannot be
bound. A computer can enter that phone endpoint to pair manually; local service
discovery advertises the same listener and the current pairing mode.

The selected port, per-install TLS identity and per-device credentials survive
restarts. Either device may display the matching code while the other enters it.
Pairing uses the desktop certificate-bound proof protocol; already-associated
devices can reverse connection roles with fresh token-bound proofs rather than
trusting an unknown certificate. Incoming and outgoing connections share the
same device list and duplicate-connection handling. Disabling sync closes the
listener and peer sockets and withdraws its discovery advertisement.

Enabled sync uses an Android connected-device foreground service so closing the
settings activity does not close the listener; its notification can disable sync.
Each connected device also shows its actual local and remote socket endpoints;
an outgoing socket's temporary local port is distinct from the listening port.
Address lists on Android, Windows and macOS hide IPv6 entries.

## Updates and release signing

About checks stable GitHub releases for `retype-<version>-android-<abi>.apk`
and its `.sha256` file, preferring the device's supported ABI order. It falls
back to `retype-<version>-android.apk` for older releases. The universal APK
keeps that name so previous app versions can still update. Windows-only
releases and prereleases are ignored. Automatic
checks run at most once per day when settings open, plus a connected daily
WorkManager job. Android may defer that job. Checks never download or install
without a button click, and a release without an Android APK is reported explicitly.

Downloads stream into private cache with bounded size and a progress indicator.
APK and checksum downloads use GitHub's official release-asset API with binary
content negotiation, supporting direct responses and redirects without using
browser download links. Legacy cached browser links are discarded and refreshed
on the next allowed update check.
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
`RETYPE_ANDROID_KEY_PASSWORD`, then run
`gradlew -PretypeSplitApks=true :app:assembleRelease`.
Release builds enable R8 optimization and resource shrinking, with JNI classes
explicitly retained. The workflow publishes arm64-v8a, x86_64 and universal APKs.
Without the split property, local builds retain the single `app-release.apk` path.
Debug previews use the local development key and cannot be overwritten by a
formal release signed with a different key; the updater reports this mismatch.
The GitHub APK channel is intended for direct APK distribution, not Play Store
distribution, which would need its own store update mechanism.

## Build on Windows

Install JDK 17, Android SDK platform 35, build-tools 36.0.0 and NDK r28 or newer.
Set `JAVA_HOME` and `ANDROID_HOME`, then run from the repository root:

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
./platforms/android/build.ps1
# Also include the emulator ABI:
./platforms/android/build.ps1 -Abi arm64-v8a,x86_64
```

The debug-signed APK is `platforms/android/app/build/outputs/apk/debug/app-debug.apk`.
The app APK contains no base vocabulary. In Settings → Dictionary, download it once
or import the matching release bundle; offline typing is available afterward.
See [base resources](base-resources.md) for publishing, migration and recovery.
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
  cargo ndk -t arm64-v8a -t x86_64 --platform 26 -o platforms/android/build/native \
  build --locked --release -p retype-android
cd platforms/android
./gradlew :app:assembleDebug :app:lintDebug
```

## Validation

```powershell
cargo test --locked -p retype-android
cargo test --locked -p retype-learning --features broker
cargo run --locked --release -p retype-dict-build -- --out data/dict/retype-dict.tsv
cargo run --locked -p retype-resource-build -- --out platforms/android/build/test-resources --check
cd platforms/android
./gradlew.bat :app:lintDebug :app:connectedDebugAndroidTest
```

Set `ANDROID_SERIAL` if multiple devices are connected. The connected tests need
the APK and test APK installation confirmed on phones that restrict ADB installs.
Native tests exercise external test dictionaries, both Pinyin schemes, English
learning persistence and private sessions. The UI test types only into isolated
test editors, verifies touch commits and field switching, and restores the
previous default input method afterward.

The opt-in `LanMultiDeviceTest` connects an emulator to two independent TLS desktop
fixtures. Start `cargo test -p retype-sync android_emulator_multiple_devices --
--ignored --nocapture` on macOS, then pass the ports and IDs from
`retype-lan-multi-ports.json` in the OS temporary directory as the instrumentation
arguments `multiA`, `multiB`, `multiAId` and `multiBId`. Use the validation application
ID (`-PretypeValidationBuild=true`) with that APK selected as the emulator's keyboard.
This checks independent pairing, phone clipboard fan-out, incoming original JPEG,
offline/reconnect behavior, disable/re-enable and removing just one peer.

`LanInboundTest` exercises real TLS 1.2/default TLS connections to the Android
listener: incoming pairing, fresh resume proofs, two simultaneous devices,
clipboard relay, individual unlink, disabled listeners, stable certificates,
port conflicts, cancelled offers, unauthenticated messages and rapid foreground
service restarts. `LanPageTest` checks the visible IPv4 listening endpoints and
port editor. Use the validation application ID to keep these fixtures isolated.

For a production desktop-client acceptance test, forward an emulator listener
with `adb -s emulator-5554 forward tcp:51206 tcp:50206`. Run
`LanDesktopInboundTest` with instrumentation argument `desktopInbound=true` and
the validation APK selected as that emulator's keyboard. In parallel, set
`RETYPE_ANDROID_LISTENER=127.0.0.1:51206` and run
`cargo test -p retype-sync android_listener_accepts_production_desktop_pairing_and_resume -- --ignored --nocapture`.
This exercises desktop-initiated pairing, bidirectional text, original PNG
reception, persistent certificate identity and desktop token-bound resume proofs.

Discovery acceptance uses an independent desktop mDNS browser. Run
`LanDiscoveryHostTest` with `discoveryHost=true`, read its synthetic device ID
from `cache/lan-discovery-id.txt` via `adb shell run-as` for the validation app,
then set `RETYPE_ANDROID_DISCOVERY_ID` to that ID and run
`cargo test -p retype-sync android_listener_announces_modes_to_desktop_browser -- --ignored --nocapture`.
The browser checks the shared port, Android platform/dial attributes, show/join/idle
mode transitions and withdrawal. The discovery host never selects an input method
or writes the system clipboard.

`LanPhoneToPhoneTest` runs on two isolated Android environments with the
validation APK selected as each environment's keyboard. Start one with
`phoneRole=server`; start the other with `phoneRole=client` and an explicit
`phoneAddress`. Both listeners retain port `50206`; emulator port forwarding can
provide the explicit route. The test exchanges text in both directions and
checks that a PNG roundtrip preserves the original bytes.

TLS keys authorize pre-hashed ECDSA signing as required by the
[Android Keystore TLS guidance](https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec.Builder#setDigests(java.lang.String...)).
The enabled listener uses Android's
[connected-device foreground service type](https://developer.android.com/develop/background-work/services/fgs/service-types#connected-device)
and [NSD registration](https://developer.android.com/develop/connectivity/wifi/use-nsd).

Listener validation on 2026-11-01 passed nine Android 15 emulator regressions,
the production Windows client's incoming-pair/resume/text/PNG acceptance test,
the independent desktop discovery-mode/withdrawal test and a two-emulator Android
text/byte-identical PNG roundtrip. ARM64 and x86_64 JNI builds, Android unit tests,
debug/test APK builds, lint and scoped strict Clippy passed. The three affected
Rust library suites passed 33 tests when run serially. These new listener checks
used isolated validation applications; no physical phone's input-method selection
or production pairing records were changed.

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
Normal input uses typed JNI commands and updates, with at most eight preview
candidates. Unchanged candidate text reuses the existing Kotlin list. Expanded
candidates are fetched in pages of at most 64, guarded by generation and selected
by absolute index. Feature operations and compatibility tests retain JSON.
See [Android performance validation](android-performance.md) for measurement
commands and release-size results. A P99 of 6 ms remains a measurement target,
not an established Android performance claim.

The Rust library is linked for 16 KiB ELF alignment. Locally verified builds also
passed checks of the packaged Compose native library and APK zip alignment. A 16 KiB-page device still
needs its own runtime compatibility test before declaring that device supported.
