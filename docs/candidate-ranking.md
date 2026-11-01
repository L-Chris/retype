# Candidate coverage and unfinished Shuangpin syllables

Investigation: 2026-10-31, Windows Xiaohe Shuangpin, reported input
`hduiyzdmm` (`hd'ui'yz'dm'm`).

## Reproduction

The 1,622,791-entry dictionary reproduces the reported order with empty personal
history: `还是`, `还是有点`, `还是有`, `还是优点`, `还是有点吗`. The final `m`
is searched: `还是有点慢` existed at position 8. The leading `还是` consumes only
four of the nine input bytes. Average per-syllable score was compared before
coverage, so a frequent short prefix could hide full-input candidates.

Reproduce without installing a TIP or reading personal learning:

```sh
cargo run --locked --release -p retype-diag -- --dict data/dict/retype-dict.bin --scheme flypy --explain hduiyzdmm
```

## Open-source approaches reviewed

These are source observations, not claims about WeChat Input's internal model.

| Project | Relevant approach | Application here |
| --- | --- | --- |
| [Rime script translator](https://github.com/rime/librime/blob/56737cd29a4d393a104a2098ece3697aa2810465/src/rime/gear/script_translator.cc#L502) | Builds sentences when a reliable full phrase is absent, and offers those sentences before partial phrase streams. | Keep exact words and full-input sentences separate from shorter selectable prefixes. |
| [libime Pinyin context](https://github.com/fcitx/libime/blob/6fec3054d8a65fc8a6942a467a42f2c0dac20eb8/src/libime/pinyin/pinyincontext.cpp#L709) and [decoder](https://github.com/fcitx/libime/blob/6fec3054d8a65fc8a6942a467a42f2c0dac20eb8/src/libime/core/decoder.cpp#L226) | Emits n-best sentences first, scores transitions using a language model, and adjusts partial candidates by distance to the input end. | Coverage and match evidence must enter ranking; isolated word frequency is insufficient for sentence quality. |
| [Mozc dictionary predictor](https://github.com/google/mozc/blob/e5f1494923a0a624d01f19c50d57bbdfa03ea5ba/src/prediction/dictionary_predictor.cc#L1078) | Scores a partially consumed candidate with the cost of decoding its remaining suffix plus an explicit partial-candidate penalty. | A short prefix must not compete as though the remaining input cost were zero. |

A real contextual language model is a broader improvement than this fix. The
current dictionary is a unigram lexicon, not a trained adjacent-word model;
these changes cannot promise identical sentence ordering to another IME.

## Implemented policy

- Preserve exact, fully typed dictionary-word priority.
- For an unfinished final Shuangpin syllable, rank plausible candidates by input
  coverage before average score. Retain shorter candidates for partial selection.
- Reserve a bounded second search beam for paths whose final incomplete syllable
  completes an existing dictionary phrase. This keeps `有点慢` / `有点忙` available
  even when frequent isolated `m` characters fill the ordinary beam.
- Within the same coverage and completion-evidence group, retain dictionary and
  personalized scores. A normalized score window of 3.0 keeps very weak previews
  from displacing a strong short match, covered by the `woe` / `我` regression.
  This is a heuristic score window, not a calibrated probability.
- Expand dictionary lattices only at reachable positions. This removes work on
  word interiors that cannot contribute to a candidate, while retaining raw
  fallback edges and explicit separators.

With the full dictionary, the first candidate is now `还是有点慢`, consumes all
nine input bytes and records five syllables. Prefix selection remains available;
selecting the complete candidate includes the final syllable in learning.

## Window presentation and measurement limits

The v0.7.2 width/bitmap caches and indexed TSF candidate getters remain in place.
The later v0.7.5 surface change fixes font-atlas recreation; it does not remove
those caches. The new Windows change presents a ready, positioned popup with
[UpdateWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-updatewindow),
which sends paint directly instead of waiting for a low-priority queued paint.
The paint handler only copies the existing bitmap; it requests no TSF edits.
UI-less hosts still render their own candidate list, and unchanged bitmaps remain
reusable. A real TSF-host fixture changes visible pixels without dispatching the
application queue and verifies that presentation drains the pending paint.

CPU benchmarks do not include real application edit-lock waits or compositor
latency. The local macOS runner also lacks Windows' CJK font, so its bitmap
numbers cannot establish Windows visual latency or a comparison with WeChat.

A matched release-binary CPU check used the same dictionary and 60 typing /
backspace cycles on the local macOS runner, before/after executables run
consecutively after builds stopped. At 200% scale:

| Input | Before decoder median | After decoder median | Before decoder estimated P99 | After decoder estimated P99 |
| --- | ---: | ---: | ---: | ---: |
| `h` | 0.324 ms | 0.311 ms | 0.371 ms | 0.332 ms |
| `hduiyzdmm` | 0.285 ms | 0.270 ms | 0.301 ms | 0.292 ms |

The target input now retains 102 candidates rather than 94. Including local
bitmap work, its median is 0.546 ms before and 0.562 ms after; displayed text
also changes, so that is not a rendering-only comparison. Sixty samples give
only an estimated tail, and these numbers cannot explain or establish a fix
for the reported Windows application-visible delay.

## Native macOS measurement

`bash platforms/macos/test-input-latency.sh` builds an isolated Cocoa harness
using the current `RetypeInputController`, a real `NSTextView`, the native
candidate panel and the full dictionary. It uses `RETYPE_TEST_ROOT` and does
not install, enable or select an input source or access personal learning.
The main thread requests interactive QoS and prevents App Nap during the test.
It records the initial pass separately, then measures 40 typing/backspace
cycles for `hduiyzdmm`, `wdsssssssssss` and `woui`. JSON stage timings and actual
editor/candidate PNGs are written under `target/macos-input-latency/current`.
`RETYPE_LATENCY_OUTPUT` and `RETYPE_LATENCY_ROUNDS` override those defaults.

The input controller and panel both request widths for every candidate. Native
CJK text measurement dominated the measured macOS handler time. The panel now
caches scalar widths at its fixed 16-point font, with NSCache count/cost limits;
it retains no glyph or bitmap layouts. Keys are immutable copies, and smoke
checks cover repeated/mutated strings and native CJK/Latin/emoji metrics.

In consecutive before/after runs after builds stopped, the target input's
width-computation P99 fell from 9.068 ms to 0.244 ms (720 warm keystrokes per
build). A reverse-order repeat measured 9.001 ms versus 0.245 ms. Candidate
PNGs were byte-identical, with `还是有点慢` first. Whole-frame submission did
**not** show a stable improvement: the first pair measured P99 10.970 ms before
and 11.887 ms after, with substantially different costs in unrelated command
and drawing stages. This establishes reduced width work, not an overall
latency improvement or a comparison with WeChat.

The harness calls the input controller directly and submits AppKit drawing; it
does not include hardware event delivery, system InputMethodKit cross-process
calls, other applications' edit behavior or compositor/display latency. Native
macOS results cannot establish Windows application-visible latency.

For Windows, run the read-only diagnostic after reproducing:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/scripts/diagnose-input-latency.ps1
```

It reads existing input timing records and prints numeric stage summaries, version,
PID and DPI; it never exports raw log details or typed text. Default logs normally
contain only operations of at least 16 ms, so these summaries are sampled slow
operations, not all-key latency percentiles. Full capture uses
`$env:RETYPE_PROFILE_INPUT = '1'` before launching a fresh target application.
An application reusing an old process will not pick up the new environment value.
See [input latency investigation](input-latency.md) for stage definitions and the
limitations of paint timing versus final screen presentation.
