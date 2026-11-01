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
