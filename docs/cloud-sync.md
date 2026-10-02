# Cloud sync

Open **Settings → Cloud sync**, configure a WebDAV provider, username and application password, and enable sync. Input preferences, optional dictionary selections, shortcuts, AI/translation settings, personal learning and typing history are always synchronized together; there are no per-category switches.

Presets include Jianguoyun, cstcloud, InfiniCLOUD, Koofr, HiDrive and Yandex Disk; custom WebDAV endpoints are supported. InfiniCLOUD requires the connection URL shown in the account's My Page. **Test connection** checks directory listing, writing, reading and removing a synthetic test file inside `retype/v1/tmp/`.

## Behavior

- The per-user `retype-sync-host.exe` runs independently of Settings and input host applications. The learning broker starts it at login if sync is enabled; enabling sync or selecting **Sync now** also launches it.
- Automatic sync runs immediately on startup and every five minutes thereafter. Failed requests retry after 30 seconds, two minutes and then ten minutes. Each HTTP request has a 30-second timeout.
- Disabling sync stops the helper without deleting local or remote data. Uninstallation requests helper shutdown while retaining connection metadata and user data.
- On first connection to an existing cloud folder, the page shows an initial summary and asks to merge. The first merge adopts cloud settings, retains local learning and statistics, and adds their evidence to the cloud.
- Concurrent edits to the same setting appear in the page with **Keep local** / **Use cloud** actions. Related shortcut bindings are one field and validated together; translation provider/model selection is also one field. Removed AI providers retain a deletion marker so another device does not resurrect them.
- After a successful sync, Settings refreshes saved preferences without replacing an active AI configuration draft. Enabled optional dictionaries are downloaded separately by the existing dictionary downloader; they are not uploaded or bundled in cloud snapshots.

## Data and merging

Learning is exported and imported through the authenticated learning broker, the sole SQLite writer. Existing counts receive a one-time seed origin; new selections and corrections use a stable local origin. Each origin has absolute counters, merged by maximum before summing different origins. Repeated downloads, round trips and process restarts do not duplicate evidence. The local recency clock and lexical priors are retained or recalculated locally, never added to a remote device's clock.

Statistics keep absolute counts and active milliseconds per device, writer stream and UTC minute. Only complete log lines are exported. Imported history is stored separately from local writer logs; it is included in day/week/month/year totals and weighted average speeds. The live five-minute speed remains local to the current device. Downloads merge each source once rather than adding the same totals again.

AI provider addresses, names and selected models are synchronized. API keys, cloud passwords, cloud connection settings, device identity, application paths, update files, logs, translation input and translation output are not part of snapshots. Keys/passwords remain in Windows Credential Manager; another computer must enter its own credentials. Personal words and corrections are stored in cloud JSON, so use an account and endpoint you trust. This version does not encrypt cloud snapshots independently of HTTPS.

## Storage and failure handling

Local connection metadata, account state and validated download caches live under `%LOCALAPPDATA%\retype\sync` with a user/SYSTEM-only directory ACL. Cloud application passwords use account-scoped Windows credentials; the JSON contains no password. The pre-merge portable settings are retained in `before-merge.json` for recovery.

Validated object caches are bounded to 512 MiB per account. Connection changes save automatically, including when closing Settings.

The cloud layout is `retype/v1/devices/<device-id>.json` plus immutable `objects/<sha256>.json`. A new object is uploaded and read back for SHA256 verification before the device index is replaced. Interrupted uploads leave the previous index intact. Three previous complete versions per device are retained; deletion of older objects is best effort and checks device ownership. Unchanged snapshots are not republished; unchanged downloaded objects use a validated local cache.

Snapshots are versioned and capped at 32 MiB, with at most 128 cloud devices. Invalid schemas, hashes, source IDs, oversized records or invalid shortcut combinations report an error instead of silently clearing data. Read/write errors remain in the Cloud sync page and do not open dialogs or block typing. Cross-origin redirects are rejected so cloud credentials are not forwarded elsewhere; configure the final WebDAV URL.

## Validation

Tests use a local in-memory WebDAV server and temporary learning databases rather than real accounts. They cover interrupted publication, version retention, corrupted downloads, concurrent settings edits, repeated statistics imports, partial log lines, learning round trips and restart persistence. Provider-specific authentication and real multi-PC operation still require account testing.
