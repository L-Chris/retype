# Input and backspace latency investigation

Measured on 2026-10-08 from the v0.7.1 checkout, using the release profile and
the 1,622,791-entry compiled dictionary. This investigation does not change
input behavior or the installed input method.

## Reproduction and measurement

The reported case is Xiaohe Shuangpin `wdsssssssssss`, followed by backspaces.
The diagnostic types the entire string and deletes one key at a time, using
the Windows session's two dictionary layers, eight-candidate page limit and
480-DIP maximum width. It measures kernel processing, two state snapshots,
all-candidate width measurement, pagination and CPU bitmap rendering separately.
It uses empty personal and optional dictionaries, no network requests, and
does not read or modify real user data.

```powershell
cargo run --release -p retype-diag --features candidate-benchmark --example typing-latency -- wdsssssssssss 50 flypy
cargo run --release -p retype-diag --features candidate-benchmark --example typing-latency -- woui 50 flypy
```

The first cycle at each scale is reported separately as `cold`. Subsequent
50 cycles produce per-step median and estimated P99 measurements. With only
50 samples, the reported P99 is the largest sample, not a stable tail-latency
guarantee. Local scheduling affects the tails; an earlier run overlapping
compilation was discarded. Stage medians need not add up to the total median.

## Results

Deleting the last `s` to return to `wd` produces 14 candidates, eight visible.
Times below are milliseconds; totals exclude host COM calls and presentation.

| Scale | Kernel median | Width measurement median | Bitmap rendering median | Total median | Total estimated P99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 100% | 0.017 | 0.004 | 1.078 | 1.107 | 2.829 |
| 150% | 0.030 | 0.009 | 2.276 | 2.328 | 8.039 |
| 200% | 0.032 | 0.009 | 3.716 | 3.776 | 5.627 |

Deleting once more to `w` produces 512 candidates, still eight visible:

| Scale | Kernel median | Width measurement median | Bitmap rendering median | Total median | Total estimated P99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 100% | 1.026 | 3.356 | 1.155 | 6.161 | 13.234 |
| 150% | 1.000 | 3.502 | 2.301 | 7.734 | 18.343 |
| 200% | 0.984 | 3.440 | 3.616 | 8.849 | 20.373 |

Other deletion steps in the long input also pay for rendering: at 200%,
returning to `wdsssssssss` has a kernel median of 1.050 ms and rendering median
of 6.007 ms. First-cycle typing of `w` spends 23–31 ms measuring 512 candidates;
its total is 30–41 ms. The `woui` control similarly reaches an estimated P99
of 22.267 ms when deleting back to `w` at 200%.

There is no isolated decoder spike at `wd` in these measurements. The reported
application-visible stall at that exact boundary has not yet been reproduced
or fully explained. These numbers identify measurable bottlenecks; they do
not establish a comparison with WeChat Input and do not measure real-host
edit-lock waiting, composition writes, caret layout, native window management,
message-loop painting or compositor presentation.

## Source findings

- `core/engine/src/kernel.rs`: `on_backspace` redecodes the entire remaining
  buffer. There is no reuse of the previously decoded shorter prefix.
- `platforms/windows/candidate-ui/src/surface.rs`: `measure` lays out every
  candidate, including invisible candidates. The current font layout cache
  retains only recently used frames, so returning to an older candidate list
  can repeat text layout. Font atlas reuse alone does not cache candidate widths.
- The same file's `render` and `rasterize` rebuild the bitmap on the calling
  thread using CPU triangle rasterization. High DPI increases pixel work.
- `platforms/windows/tsf/src/edit.rs`: key handling writes the host composition
  and refreshes the popup in the same edit transaction. `OnLayoutChange` can
  request another refresh after that transaction; it has no content/layout
  equality check. Actual duplicate-refresh frequency remains unmeasured.
- `popup.rs` and `candidate.rs`: refresh clones candidate data several times,
  recreates the window region and calls positioning/show APIs even without
  a visual change. Snapshot/pagination cost is smaller than rendering in this
  benchmark; it is not the first optimization priority.
- `popup.rs` invalidates the window and waits for `WM_PAINT` in the host's
  message loop. Windows normally delivers paint after higher-priority queued
  work. Repeated input/edit/layout work could therefore delay visible updates
  beyond bitmap computation; this is a scheduling hypothesis, not a measured
  cause of the reported stall. See [Microsoft's WM_PAINT documentation](https://learn.microsoft.com/en-us/windows/win32/gdi/wm-paint).

The existing 6-ms diagnostic budget measures the local engine only. It does
not include candidate measurement or drawing and cannot establish an end-to-end
input latency guarantee.

## Recommended order

1. Add asynchronous, count-only host timing for edit-session queue waiting,
   kernel processing, composition writes, caret geometry, popup measurement,
   rasterization and paint dispatch. Include work kind, generation, pending
   count and DPI, without logging typed text or candidates. Reproduce the exact
   stall in the affected application before attributing the unexplained portion.
2. Cache widths by text/font/scale with a bounded cache. Measure the current page
   first and subsequent pages when needed, while preserving accurate widths,
   numbering, full TSF enumeration and variable page sizes.
3. Separate anchor updates from content rendering; reuse the bitmap when visible
   candidates, selection and scale are unchanged. Coalesce redundant layout
   refreshes, but never discard backspace/input events or composition writes.
4. Reduce CPU rendering cost, including repeated background/triangle work;
   benchmark a native accelerated text/drawing path if necessary. Move avoidable
   UI work outside the host write transaction without moving COM objects across
   apartments. Preserve the current UI and mouse interaction.
5. Consider a small, bounded prefix-decode cache for backspace only after UI work
   is addressed; invalidate on dictionary, learning, scheme and relevant context
   changes. Do not change candidate ranking to meet a timing target.

Acceptance should include first-use and warm typing, the reported deletion
sequence, `woui`, returning to single-letter previews, continuous backspace,
100/150/200% DPI, and real Notepad/Edge/Windows Search hosts. Report local CPU
time separately from edit waiting and visible-presentation latency.

## Implemented optimization and matched follow-up

The follow-up implements bounded text-width caching (4,096 entries and 128 KiB
of text keys, cleared on DPI change), reuse of the last bitmap when all visual
inputs match, and scanline filling for opaque constant-color triangles. Cached
widths are unclamped, so a smaller monitor/viewport still clips accurately.
Bitmap reuse ignores generation and composition metadata, but the native popup
always replaces its click generation, context and page index with current values.
It also skips unchanged window regions, placement, visibility and accessibility
labels. Redundant pending layout refreshes are coalesced within a focus epoch;
normal input/backspace events remain separate and are never discarded. Completed
refreshes release their tokens even when a host retains their COM objects.

The original executable and optimized executable were run consecutively with
the same dictionary, input and 50 rounds, after build/test activity stopped.
Times are milliseconds; this still excludes host scheduling/COM/presentation.

| Scale | Return to | Before total median | After total median | Before estimated P99 | After estimated P99 |
| --- | --- | ---: | ---: | ---: | ---: |
| 100% | `wd` | 1.109 | 0.665 | 3.459 | 2.191 |
| 150% | `wd` | 2.295 | 1.130 | 5.804 | 1.978 |
| 200% | `wd` | 3.747 | 1.706 | 10.691 | 2.521 |
| 100% | `w` | 5.777 | 2.010 | 16.686 | 7.664 |
| 150% | `w` | 6.920 | 2.401 | 19.969 | 4.266 |
| 200% | `w` | 8.509 | 2.891 | 22.015 | 7.171 |

At 200%, returning to `w` reduces the width-measurement median from 3.455 to
0.029 ms and rasterization from 3.769 to 1.701 ms. Returning to `wdsssssssss`
reduces total median from 7.538 to 4.245 ms. Tail variability remains, including
some totals above 6 ms; the engine-only target is not an end-to-end guarantee.
First-use measurement still lays out all candidates to preserve complete TSF
page enumeration. Lazy page measurement and prefix decoding reuse remain future
work; the first cold `w` measurement still takes roughly 25–30 ms here.

Validation covers width reuse/eviction, viewport and DPI changes, metadata-only
bitmap reuse, visual changes, English single-row candidates, long phrases,
later-page selection, exact scanline coverage, pending-refresh coalescing and
actual native-popup mouse selection after a generation change. Four before/after
PNG fixtures (short, phrases, mixed and long) are byte-identical. Candidate UI,
TSF and engine tests and strict Clippy checks pass.

## Host diagnostics

Slow operations now emit numeric `edit_timing` / `paint_timing` records through
the existing nonblocking logging worker, normally in
`%LOCALAPPDATA%\retype\logs\settings-*.jsonl` (or the configured `SettingsLogPath`).
Records have `role=input`; the log file prefix may reflect the first component
that started the shared logging worker. No typed letters, text, candidates or
credentials are logged. Normal mode records operations taking at least 16 ms,
limited to one edit record and one paint record per 250 ms per input thread.

For a targeted session, start the host from a process with
`RETYPE_PROFILE_INPUT=1` to record every operation. This is opt-in per host
process and is read once; changing it does not affect an already running host.

`edit_timing` includes work kind, generation, input byte length, candidate and
pending counts, HRESULT and microsecond durations for edit-lock queue waiting,
kernel processing, composition writes, caret geometry, candidate measurement,
pagination, UI updating and bitmap rasterization, plus DPI and bitmap reuse.
`raster_us` is included in `ui_us`, so those fields must not be summed together.
`paint_timing` measures time from bitmap invalidation to `WM_PAINT` and the GDI
presentation call; it does not measure the compositor's final screen refresh.
These records can identify the remaining application-specific stall after
deploying the optimized build and reproducing it in the affected host.

## Candidate enumeration follow-up

After installing the first optimization on 2026-10-08, slow-operation records
for backspace returning to one input byte and 512 candidates showed about
85–119 ms total, including 84–113 ms in UI updating. Kernel processing was
about 1–7 ms and bitmap rendering below 1 ms. The sampled slow records do not
establish the user's precise two-letter reproduction: diagnostics omit most
operations below 16 ms and do not record typed text.

The TSF candidate getters previously cloned the entire `RenderState` for each
read, including every candidate's strings and syllable vectors. Enumerating
512 candidate strings could therefore copy 262,144 candidate objects. This
cost was absent from the standalone rendering benchmark above.

Candidate getters now borrow the published state while returning scalar
fields or copying just the requested string into its caller-owned BSTR. No
candidate-data lock is held across a COM edit request: Finalize copies only
the selected index and generation before releasing the read lock. A new UI
element has its own data, selection and counters; retained old elements do
not start reading a newly created element's data. Changing TSF contexts ends
the previous element and binds a new one to the correct document.

Notifications compare exposed candidate text, count, effective selection,
page boundaries and current page. Generation, score, consumption or
composition-only changes still publish fresh data but do not request text
enumeration. A new element advertises all fields. Caret-only refreshes retain
the last notification's flags for hosts which query them after notification.
Failed notifications retain their change mask and retry on the next update.
Sorting, candidate count, complete enumeration and variable-width pagination
are preserved. Lazy page measurement and prefix-decode caching remain later
optimizations; this follow-up targets the newly measured host-facing cost.

Numeric input diagnostics additionally include `notify_us` (Begin/Update UI
element calls), `getter_calls` (candidate-state/flags/document reads inside
those calls), `string_calls` (GetString calls), and `updated_flags` (the change
mask for this update). `notify_us` and `raster_us` are both parts of `ui_us`,
not additional stages to sum. These counts cover synchronous notification
callbacks, not later asynchronous enumeration.
`notify_result` records the UpdateUIElement HRESULT (zero on success).

The real in-memory TSF host test now enumerates all candidates when text or
count changes. It checks 512 entries, the last and invalid indexes, all page
boundaries, metadata-only updates, selection reset on generation changes,
page-only updates without text reads, retained element isolation and
synchronous Finalize reentry. A microbenchmark is explicitly opt-in:

```powershell
$env:RETYPE_BENCH_CANDIDATES = '1'
cargo test --release -p retype-tsf real_tsf_composition_commit_cancel_and_passthrough -- --nocapture
Remove-Item Env:RETYPE_BENCH_CANDIDATES
```

It compares full-state cloning plus BSTR allocation with the production COM
GetString interface, using 512 synthetic candidates with text, comments and
syllable metadata. It reports the median of 11 complete enumerations and does
not measure real Edge/Search scheduling or final screen presentation.

On this machine, the release-build median was 56.797 ms with full-state
cloning and 0.121 ms with indexed production COM getters. This directly
measures removal of the quadratic enumeration cost, not the final latency
of the user's deletion sequence in a real application.
