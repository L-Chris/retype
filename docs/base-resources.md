# Downloadable base dictionaries

Windows and Android installers contain the engine and a small trusted resource
catalog, without the Chinese or English vocabulary. In Settings → Dictionary,
download the base dictionaries once. Subsequent typing is completely offline.
Fresh installations keep ordinary direct input available while resources are absent.
Android also offers a download entry beside the keyboard logo.

The panel disappears when both resources are installed. It has progress, cancel,
retry and local import; there is no base-dictionary enable switch. Optional subject
dictionaries retain their existing separate switches. Chinese and English are
separate downloads, totaling approximately 31.7 MiB with the current catalog.

Upgrades validate and reuse compatible local files, independent of app version.
Windows migrates earlier installer dictionaries into the user's resource directory;
Android migrates earlier extracted assets. Already downloaded files from an older
catalog are reused in place while an updated resource is unavailable. Android can
complete resources for an existing installation on an unmetered network; a fresh
installation starts downloading only after the user requests it.

## Installation and recovery

Windows stores immutable images in `%LOCALAPPDATA%\retype\dictionaries`; the user
enrollment script grants AppContainers read access and publishes `BaseDictionaryRoot`
in `HKCU\Software\retype`. Personal learning and credentials remain private.
Android uses its private `files/dictionaries` directory. Files are named by content
hash, not application version, and remain read-only for the memory-mapped engine.

The catalog pins the compressed and raw SHA-256, sizes and binary format. Installers
bound download/decompression sizes, verify hashes and validate the image before
publishing it atomically. They never truncate a mapped file. A cancelled or failed
installation retains valid dictionaries. A process lock prevents concurrent writers.
Windows HTTP downloads resume a partial file only after a valid range response;
Android uses DownloadManager and WorkManager to survive backgrounding or network
interruptions. A user cancellation stops the task; on Android it also removes that
system download. English snapshots and Windows base dictionaries switch between
compositions. Android starts a missing session after the base resource arrives.

No network or resource installation happens on the input thread. Cloud configuration
and learning snapshots can still synchronize without a base dictionary; the learning
priors are rebuilt once a real dictionary becomes available.

For offline installation, download `retype-base.retype-resources` from the same
application release, transfer it to the device, and select **Import from file**.
Imports use the same catalog checks and do not accept archive-supplied paths.

## Building and publishing

```powershell
cargo run --locked --release -p retype-dict-build -- --out data/dict/retype-dict.tsv
# Regenerate data/dict/resources.json only when vocabulary data/format changes:
cargo run --locked --release -p retype-resource-build -- --out dist/dictionaries
# CI checks that committed metadata matches freshly built images:
cargo run --locked --release -p retype-resource-build -- --out dist/dictionaries --check
```

The Windows payload job builds deterministic gzip assets, `resources.json` and the
import bundle. The release workflow attaches them to the **same app release**, along
with the installers. Do not create a dictionary-only release: older Windows updaters
query the latest app release. Hash-named `.bin.gz` assets are the runtime download
targets. The intermediate `english.bin` is for diagnostics and is not a release asset.

By default the resource URL uses the application version; an optional per-component
`release` can pin a previously published `vX.Y.Z`. A local development build cannot
download newly generated assets until that application release has been published;
use local import for testing. Do not remove immutable assets used by old catalogs.

Production English vocabulary is read from a validated external mapping. The
`test-data` feature reads source fixtures on the host at test runtime only and must
not be enabled for a production build. Instrumentation fixtures belong exclusively
to the Android **test APK**, never the app APK.

Before Android instrumentation builds, generate the test bundle explicitly:

```powershell
cargo run --locked -p retype-resource-build -- --out platforms/android/build/test-resources --check
```

Resource regression tests cover corrupt/cancelled imports, old-resource migration,
cross-catalog reuse without duplicate images, immutable installation, concurrent
writer exclusion, valid resume/restart responses and invalid HTTP ranges. Android
instrumentation checks absence of bundled vocabulary, bounded import, Chinese input
and English completion from external resources.
