# Android performance validation

Android is already native Kotlin (`InputMethodService` and Jetpack Compose) with
the shared Rust engine over JNI. The old Flutter desktop settings project has
been removed; Android does not package Flutter. Keep the current settings and keyboard UI while
measuring packaging, transport and editor latency separately.

## Release packaging

Release builds enable R8 and resource shrinking. `NativeBridge` and
`NativeUpdate` are kept explicitly because Rust resolves JNI methods and the
update constructor by name. CI produces arm64-v8a, x86_64 and universal APKs,
each signed and accompanied by its own SHA-256. The universal asset keeps the
old name for existing updaters; new updaters prefer a matching architecture.

Use the normal release signing environment variables documented in
[android.md](android.md), then run from `platforms/android`:

```powershell
./gradlew.bat --no-daemon -PretypeSplitApks=true :app:assembleRelease :app:lintRelease :app:testDebugUnitTest
```

For isolated validation, `-PretypeValidationBuild=true` adds `.validation` to
the application ID, preventing tests from accessing the installed app's settings,
credentials and learning data. It still requires signing credentials; a local
development key can be used for this validation package. Do not publish it.
`-PretypeOptimize=false` disables shrinking for a size comparison, while omitting
`-PretypeSplitApks=true` produces the former universal `app-release.apk` path.

Local validation on 2026-10-22 (development-signed validation application,
same dictionary and native libraries):

| Package | Bytes | MiB |
| --- | ---: | ---: |
| Unoptimized universal | 56,104,411 | 53.51 |
| Optimized universal | 50,134,875 | 47.81 |
| Optimized arm64-v8a | 41,579,269 | 39.65 |
| Optimized x86_64 | 41,991,687 | 40.05 |

That ARM64 download was approximately 25.9% smaller than the unoptimized universal
package. Compressed DEX fell from 6,788,025 to 1,155,819 bytes. Its bundled
offline dictionary occupied 32,150,981 compressed bytes. All three optimized APKs pass signature and 16 KiB ZIP alignment
checks. These sizes describe APK downloads, not installed size or process RAM.

The subsequent [external dictionary change](base-resources.md) removes both the
Chinese asset and compiled-in English vocabulary from the application. The ARM64
validation APK is 6,495,377 bytes (6.19 MiB), versus 39.65 MiB above (about 84% smaller).
Chinese and English resources download separately once (approximately 31.7 MiB),
then remain fully offline. Test bundles belong only to instrumentation APKs.

## Input transport

The service keeps a serialized engine queue and performs editor changes on the
main thread after checking the captured connection and editor epoch. Normal
commands use typed JNI parameters and a `NativeUpdate` object, rather than
serializing and parsing JSON on every key and acknowledgement.

The Rust adapter retains the kernel's emitted render state, avoiding another
full render snapshot. Each normal update carries at most eight preview strings;
unchanged preview text is omitted and Kotlin keeps the existing list. Expanded
candidates load in pages of at most 64. The total count, ordering and absolute
indexes remain available. A new generation invalidates expanded pages, and
stale page requests or candidate taps cannot select a different result. Learning
still waits for a successful editor acknowledgement. No keys are dropped or
merged, including repeated backspace.

The adapter still exposes JSON for feature operations and compatibility tests.
Core candidate generation is unchanged; preview windowing reduces transfer and
allocation, rather than reducing the search space.

## Validation and measurement

Build both Rust ABIs before testing. On Windows:

```powershell
./platforms/android/build.ps1 -Abi @('arm64-v8a','x86_64') -NativeOnly
cargo test --locked -p retype-android --lib
cargo clippy --locked -p retype-android --all-targets -- -D warnings
```

Build an isolated, signed, optimized release plus its instrumentation APK:

```powershell
cd platforms/android
./gradlew.bat --no-daemon -PretypeValidationBuild=true -PretypeSplitApks=true -PretypeTestRelease=true :app:assembleRelease :app:assembleReleaseAndroidTest
```

Install the matching release APK and test APK on a test device, then run:

```powershell
adb shell am instrument -w -e retypeBenchmark true io.github.retype.ime.validation.test/io.github.retype.ime.NativeReleaseSmokeRunner
```

The release runner uses only framework APIs and the kept JNI ABI. It verifies
the actual optimized app without retaining otherwise unused app APIs for the
test framework. It covers both Pinyin schemes, preview bounds, full pages,
generation guards, selection beyond the preview and commit acknowledgement.
Normal debug instrumentation uses AndroidJUnitRunner; run `NativeCompactInputTest`,
`NativeSessionTest` and `AppUpdatesTest` there to also cover personal learning
and architecture-specific update selection.

Local validation passed six Rust adapter tests, nine Kotlin unit tests and
eleven emulator instrumentation tests (including touch composition, Chinese and
English expanded-candidate selection, and private-field switching). Debug and
release lint complete without errors; existing warnings remain. Both ARM64 and
x86_64 JNI libraries build successfully. Release JNI smoke validation also passes
on the x86_64 emulator; physical ARM64 latency and rendering still need measurement.

The release runner's transport benchmark is opt-in via `-e retypeBenchmark true`.
It alternates JSON and typed transport
on two independent sessions, disables learning, warms both paths for 20 cycles
and measures another 80. It includes candidate consumption and acknowledgement,
reporting P50/P95/P99 in microseconds. Both paths use the current Rust adapter,
so this does not measure the adapter's removed full-snapshot clone. Emulator
results do not establish physical-device input latency or power consumption.

On the local API 35 x86_64 emulator, the optimized release produced these
warm-session results (microseconds, including core dispatch and ACK):

| Scheme / samples | Typed P50 | JSON P50 | Typed P95 | JSON P95 | Typed P99 | JSON P99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Full / 800 | 101 | 126 | 292 | 385 | 618 | 674 |
| Xiaohe / 640 | 591 | 630 | 1,254 | 1,444 | 1,640 | 2,008 |

This is a transport comparison on a shared development host, not a claim about
touch-to-screen latency. Repeat on the target phone before choosing a different
keyboard renderer.

The service emits numeric `retype-input` startup and slow-input timings only:
queue, native, main-thread wait, editor application, total duration and candidate
count. Slow events are throttled to one per 250 ms. It never logs keys, candidate
text, editor text or credentials. Perfetto sections `retype.native_input` and
`retype.editor_apply` allow the stages to be distinguished when app tracing is
enabled. A 6 ms P99 remains a target requiring a representative ARM64 phone.

Before replacing Compose with a View/Canvas keyboard, compare optimized release
builds on the same phone: cold keyboard appearance, warm appearance, input to
visible candidate, repeat deletion, expanded-grid scrolling, process PSS and
frame timing. Retain the existing UI unless a prototype improves those results
without losing accessibility, touch behavior, candidate selection or editor
compatibility.
