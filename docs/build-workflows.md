# Build workflows

CI and Release call `.github/workflows/windows.yml`. Windows quality checks, x64 payloads, x86 TIPs and the settings binary run concurrently; installer assembly waits for all four to succeed. Release additionally enables registration/unregistration tests on its disposable x64 runner.

Each build role has a stable cache key shared between CI and Release. Different roles have separate caches, so a concurrent job cannot save a partial dependency cache over another role's cache. Both workflows use explicit Windows targets and the same repository environment when compiling updater defaults.

Settings tests and Clippy are included in the workspace quality job once. TIP library tests are compiled once per architecture; the same test executable runs normal tests and, on a release runner, the ignored installed-TIP checks. The injected DLL is built separately from the helpers' broker/service feature union. Dictionary building invokes the already-built executable.

Release still runs its own quality gate, rather than trusting an arbitrary previous CI result. Reusing a previous result would require verifying the exact source commit and workflow definition; it is not implemented here. Release LTO and binary optimization settings are unchanged.

Manual releases check out the requested tag for metadata, source builds and release notes. Installer assembly validates asset naming and generates SHA-256 checksums. Release publishing requires the validated installer job to succeed.

Use Actions step timings to compare total duration and cache restoration after pushing these changes. Parallel jobs reduce elapsed time but may increase runner minutes; a cold cache is still expected to take longer.

Local payload validation (without registering the input method):

```powershell
$env:RETYPE_GITHUB_REPO = 'L-Chris/retype'
pwsh -File tools/ci/windows-payload.ps1 -Target i686-pc-windows-msvc
pwsh -File tools/ci/windows-payload.ps1 -Target x86_64-pc-windows-msvc
```

Only use `-RegisterTip` on a disposable CI machine: it writes and removes machine/user input-method registration.

The service runner verifies machine registration/unregistration, current-user enablement, profile enumeration, category registration, and loading of the installed DLL. It cannot switch to a Chinese desktop input language, so it does not run the interactive profile-activation probe. Ordinary TSF host tests still cover composition, commits, and key routing.

On a desktop with retype installed and Chinese input configured, run `cargo test -p retype-tsf --lib installed_tip_can_activate_in_test_process -- --ignored --test-threads=1` to verify profile activation. The probe initializes its private TSF host, selects Chinese within its process, and restores the input language during cleanup.
