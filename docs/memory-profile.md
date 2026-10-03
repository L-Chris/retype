# Memory profile and implemented optimizations

Measured on Windows x64 on 2026-10-03. The isolated profiler uses current 0.6.3 source, release optimization, and the installed Wanxiang Base binary from 0.6.2. That binary is 44.3 MiB and contains 1,622,791 records; its in-memory trie has 2,076,004 nodes. Three fresh processes were used per component. All numbers below are MiB (1,048,576 bytes), rounded.

## Component results

| Component / stage | Process private memory | Live Rust heap | Interpretation |
| --- | ---: | ---: | --- |
| Dictionary baseline | 0.64 | <0.01 | Subtract this baseline for incremental usage |
| Dictionary loaded | 364.51 | 292.44 | Range 364.25–364.68 across three runs |
| Dictionary build peak | — | 443.20 | Includes transient source bytes and builder data |
| Dictionary released | 2.73 | <0.01 | Most memory is returned when ownership ends |
| Candidate baseline | 0.62 | <0.01 | Independent fresh process |
| Candidate font loaded | 19.48 | 18.81 | Full CJK font collection bytes |
| Candidate first measurement / bitmap | 39.15 | 38.25 | Bitmap itself adds only about 0.09 MiB of Rust heap |
| Candidate 500 updates, 2,000 glyphs | 47.16 | 45.93 | Glyph and layout cache growth under this bounded workload |
| Candidate second simultaneous surface | 85.38 | 84.10 | Another surface adds about 38.2 MiB private |
| Candidate surfaces released | 1.04 | <0.01 | No retained heap in this isolated lifecycle |
| Learning store, 442 local entries | 1.46 | 0.37 | Incremental private memory about 0.83 MiB |
| Learning store plus dictionary | 365.52 | 292.80 | Almost all growth comes from the base dictionary |

The installed 0.6.2 learning helper separately showed about 366 MiB private memory, consistent with the isolated result. This is not a claim that the browser itself consumes only this amount or that every host always has exactly one dictionary/surface.

## Dictionary allocation breakdown

| Storage | Allocated / estimated MiB |
| --- | ---: |
| Entry array | 37.14 |
| Arc strings, including estimated headers/alignment | 48.57 |
| Trie node array capacity | 148.57 |
| Trie node array actual populated size | 95.03 |
| Per-node edge arrays | 36.33 |
| Per-node terminal ID arrays | 21.82 |

The node array reserves 3,245,584 slots for 2,076,004 nodes, leaving **53.54 MiB** unused. Edge and terminal arrays also overallocate. The live Rust heap is about 292 MiB, while process private memory is about 365 MiB; the difference includes allocation granularity and heap overhead, not another independently identified dictionary.

The old RTDICT01 binary format stores records rather than a directly searchable index: loading parses all records and rebuilds the trie. Before this change, each session allocated a dictionary, and the learning helper loaded another copy. Optional dictionaries follow the same representation and would add to these figures; they were excluded from this baseline.

## Settings lifecycle

The installed 0.6.2 settings executable showed about 128 MiB while open and 133 MiB after hiding. With an explicit eight-second retention override, the test process remained alive during the hidden interval. The changed 0.6.3 development build was about 128 MiB while open and exited before the first one-second sample after its scheduled close. These are two separate process builds, not a renderer comparison.

Settings now saves configuration and exits on close by default. It remains hidden only while an outstanding dictionary download or update finishes, then exits. Cloud settings are flushed before closing; the independent cloud-sync helper continues normally. Explicit `--idle-exit-ms` overrides remain available for diagnostics.

## Implemented changes

1. **Compact and precompile the base and optional dictionaries.** Replace per-node vectors and per-word allocations with contiguous node/edge/terminal tables and a UTF-8 string arena. Compile indexes at dictionary build/download time rather than rebuilding them inside each host. Keep weights, pronunciations and lookup results equivalent.
2. **Map the compiled tables read-only.** Let Windows share file-backed pages across hosts, including the learning helper. Allocate candidate strings only for query results. Report both private memory and shared file-backed residency so shared pages are not counted repeatedly. Keep a validated fallback for old record binaries during migration.
3. **Reuse dictionaries within a process.** Cache by canonical dictionary path/file generation with weak ownership; share immutable dictionaries across TSF sessions while keeping composition, user state and COM apartment objects separate. Release old generations once no session uses them.
4. **Reduce candidate font duplication.** Share immutable font data within a process and use alpha-only storage for monochrome textures. Verify rendering and measurement at multiple DPIs. The baseline showed a roughly 38 MiB per-surface cost; it did not prove a font leak or unlimited growth.
5. **Keep Settings transient.** Already implemented and verified, saving around 133 MiB while closed on this machine. This does not remove the much larger per-host dictionary cost.

The implementation changes storage and sharing together instead of only shrinking vector capacity.

## Validation scope

- Repeat these isolated cases before/after with the identical dictionary and learning snapshot, at least three fresh processes each.
- Compare dictionary lookup results, scores and prefix answers for every record plus absent keys; verify corrupt/truncated files are rejected before use.
- Measure private-memory scaling across one, three and five controlled hosts, with and without optional packs, at 100%, 150% and 200% DPI.
- Record typing latency and dictionary-load peaks alongside memory; confirm full Pinyin and odd/even Xiaohe Shuangpin behavior is unchanged.
- Exercise 10,000 candidate updates and repeated activate/deactivate cycles, confirming bounded retained memory and release of old dictionary generations.

## Before / after results

All five storage, rendering and lifecycle changes above are implemented in the development build. RTDICT02 contains validated contiguous tables and a deduplicated UTF-8 arena. Windows mappings retain a handle that prevents in-place writes/truncation while permitting atomic replacement. Weak references reuse the same file generation within a process.

Query caches are bounded by 512 nodes, 16,384 entries and 2 MiB estimated payload; prefix caching is bounded by 8,192 keys of at most 32 syllables. A first/two-syllable accelerator is capped at 1 MiB. Fonts use one read-only mapping per process. Monochrome textures retain one-byte alpha instead of four-byte RGBA; colored patches promote the texture while preserving coverage.

The same Wanxiang Base source compiled to a 94.75 MiB RTDICT02 file. The final build includes the bounded caches and accelerators. Private memory below is total isolated process memory, including its baseline, averaged over three fresh processes.

| Component / stage | Before private MiB | After private MiB |
| --- | ---: | ---: |
| Dictionary loaded | 364.51 | 1.48 |
| Learning store plus dictionary | 365.52 | 2.32 |
| Candidate first bitmap | 39.15 | 1.35 |
| Candidate 500 updates, 2,000 glyphs | 47.16 | 6.61 |
| Candidate second simultaneous surface | 85.38 | 7.09 |

The dictionary still uses about **95 MiB of file-backed data**, and the mapped font also has physical residency. Windows can share these pages across processes; private-memory savings do not mean total physical usage is 1.48 MiB. The dictionary process working set was about 99 MiB immediately after validation. Optional packs and the browser's full footprint were excluded.

After 100,000 dictionary queries, private memory was 1.88 MiB in the stress process. After 10,000 candidate updates using the same 2,000 glyphs, live Rust allocations remained 5,608,442 bytes, equal to the 500-update stage. This bounded workload does not establish a universal limit for arbitrary glyph sets. Releasing surfaces returned live heap to about zero.

Full-source equivalence checked **1,624,491 source rows**, including duplicates, against the old dictionary: lookup order, float score bits, flags and every prefix matched. Tests also cover malformed/truncated files, cache eviction, weak reuse, atomic replacement, write exclusion, alpha texture equivalence and multiple DPI layouts.

The local first-result P99 budget is **6ms**, reflected in diagnostics and CI/release checks for both full Pinyin and Xiaohe Shuangpin using compiled dictionaries. Final three-run full Pinyin P99 values were 2.4222, 2.3662 and 2.3502ms; Shuangpin values were 5.0093, **8.9331** and 5.2334ms. One Shuangpin run exceeded the budget, so these results do not establish stable compliance. These measurements exclude UI painting, host scheduling and network latency. Earlier runs of the same query implementation were 4.88–5.11ms for Shuangpin; controlled host measurements should investigate the variance before claiming a release performance guarantee.

Multi-host, optional-pack and extended real-host measurements remain follow-up coverage. Old RTDICT01 files retain the old owned-memory representation until rebuilt/downloaded in the new format. No installed input method was replaced during measurement.

Reproduction steps are in [the profiler README](../tools/memory-profile/README.md). Raw local artifacts are under `E:\retype-memory-profile-results` and are not required for builds.

## Reference implementations reviewed

Reviewed upstream source on 2026-10-03; these are implementation patterns, not equivalent memory benchmarks across products.

| Project | Observed approach | Relevance to retype |
| --- | --- | --- |
| Rime dictionary | `Table::Load` opens a compiled table read-only and uses its stored index/string table; `MappedFile` uses file mappings. Dictionary components reuse table/prism objects through weak ownership. | Precompile searchable storage, map it, and reuse the same resource across local sessions. |
| Weasel (Windows Rime frontend) | TSF forwards key processing to a client; the handler processes Rime sessions and updates the UI. Candidate rendering uses a shared DirectWrite factory and text formats. | A single candidate/engine service is a valid alternative for reducing per-host duplication; it introduces IPC, focus and failure-handling work. |
| Mozc | Maps dictionary files, opens encoded key/value LOUDS tries and token arrays from stored sections, and decodes query tokens during iteration. | Compact serialized structures and on-demand result materialization, rather than one owned string/vector per dictionary record. |
| libime (Fcitx) | Uses a double-array trie, serializes its binary representation, and supports Zstandard-compressed dictionary input. | Compact indexes are useful; compression alone reduces disk size and does not demonstrate shared resident memory. |

Sources:

- Rime [mapped file implementation](https://github.com/rime/librime/blob/master/src/rime/dict/mapped_file.cc), [compiled table loading](https://github.com/rime/librime/blob/master/src/rime/dict/table.cc), [resource reuse and query iterator](https://github.com/rime/librime/blob/master/src/rime/dict/dictionary.cc).
- Weasel [TSF key forwarding](https://github.com/rime/weasel/blob/master/WeaselTSF/KeyEventSink.cpp), [session handler and UI updates](https://github.com/rime/weasel/blob/master/RimeWithWeasel/RimeWithWeasel.cpp), [DirectWrite resources](https://github.com/rime/weasel/blob/master/WeaselUI/DirectWriteResources.cpp).
- Mozc [dictionary mappings](https://github.com/google/mozc/blob/master/src/dictionary/file/dictionary_file.cc), [system dictionary layout and queries](https://github.com/google/mozc/blob/master/src/dictionary/system/system_dictionary.cc).
- libime [trie type](https://github.com/fcitx/libime/blob/master/src/libime/core/triedictionary.h), [double-array trie](https://github.com/fcitx/libime/blob/master/src/libime/core/datrie.h), [binary load/save](https://github.com/fcitx/libime/blob/master/src/libime/pinyin/pinyindictionary.cpp).

Compiled contiguous tables and read-only mappings are now implemented. An initial flat node/edge format can preserve existing syllable lookup semantics without immediately adopting LOUDS or moving every key event across IPC. Font-data sharing can preserve the current egui candidate interaction; DirectWrite or a single candidate renderer should be compared only after this step using the same workload and DPI. A full centralized input engine remains a separate architecture change, with measurable latency and recovery requirements.
