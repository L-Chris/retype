# Isolated Windows memory profiler

This developer tool is outside the release workspace. It reports process private bytes, working set, peak working set and live/peak Rust allocations at component boundaries. Run each case in a fresh process at least three times, compare deltas against its baseline, and do not add working sets across processes as if they were all private memory.

```powershell
cargo build --release --manifest-path tools/memory-profile/Cargo.toml
$profiler = '.\tools\memory-profile\target\release\retype-memory-profile.exe'
& $profiler dict 'C:\path\to\retype-dict.bin'
& $profiler candidate
& $profiler compile 'C:\path\to\retype-dict.tsv' "$env:TEMP\retype-profile-new.bin"
& $profiler mapped "$env:TEMP\retype-profile-new.bin"
& $profiler mapped-queries "$env:TEMP\retype-profile-new.bin" 'C:\path\to\retype-dict.tsv'
& $profiler verify 'C:\path\to\legacy-retype-dict.bin' "$env:TEMP\retype-profile-new.bin" 'C:\path\to\retype-dict.tsv'
& $profiler broker 'C:\path\to\retype-dict.bin' "$env:LOCALAPPDATA\retype\learning\user.db" "$env:TEMP\retype-profile-new.db"
```

The broker case opens the source database read-only and makes a consistent SQLite backup to a **new** scratch file before opening the learning store. It never migrates the live database, connects to the installed learning pipe, registers a TIP or modifies editor text. Only numeric counts/sizes are printed. On successful completion it deletes the scratch database. If interrupted, delete only that specifically named scratch database and its journal files; it may contain personal learned words.

`dict` retains and then releases the dictionary, followed by a second load. Its allocation breakdown is enabled only through the `retype-dict/memory-profile` feature. String allocation sizes include an estimated Arc header and alignment; process counters include allocator overhead, SQLite allocations and committed memory that the Rust allocation tracker does not see.

`candidate` measures font loading, first measurement, a first bitmap, 500 and 10,000 updates using 2,000 distinct synthetic CJK characters, and a second independent surface. It displays no desktop windows and starts no timers.

`mapped` measures read-only loading and confirms two references reuse one dictionary object. `hold-map` keeps that process alive for fifteen seconds for concurrent-process sampling. `mapped-queries` runs 100,000 source-row queries to exercise bounded caches. `verify` compares all source-row lookups, order, float score bits, flags and every prefix against the old dictionary. `compile` refuses to overwrite an existing destination.

This measures the components used in TIP hosts, not the entire incremental footprint of a browser. Measuring that requires a controlled host with equivalent typing sequences, dictionary selections and DPI settings; the browser's own memory cannot be attributed to retype.
