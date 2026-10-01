# Candidate window rendering

The Windows candidate window now uses egui 0.36.2 for text measurement, wrapping,
rounded backgrounds and tessellation. A small CPU backend rasterizes its meshes
and font atlas into a top-down BGRX bitmap. Win32 presents the finished bitmap.
This follows egui's [custom integration model](https://github.com/emilk/egui/tree/0.36.2#integrating-with-egui),
without adding eframe, winit, an OpenGL context or a GPU device to the TSF DLL.

## Interaction and appearance

- The native window retains `WS_EX_NOACTIVATE`, `WS_EX_TOOLWINDOW`, topmost
  positioning, rounded clipping and its existing shadow.
- TSF still owns keyboard routing, selection, generation checks and composition.
  Number keys, Space, arrows and `-`/`=` retain their existing behavior.
- The UIElement Show/Hide negotiation and candidate enumeration are unchanged;
  hosts that draw their own candidates do not get a second popup.
- Candidate text uses light-mode font coverage to match the light background
  and avoid dark-mode compensation thickening text edges.
- Candidate text uses a 16px font, numbers 11px, with the existing colors,
  selected state, 4px outer padding and 2px cell gaps, scaled with DPI.
- The kernel continues to pack at most eight candidates into a 480 logical pixel
  maximum width. Widths now come from the same egui font layout used to paint.
  Ordinary phrases stay on one line; a candidate wider than the available area
  wraps without ellipsis. Composition letters, branding and page counts are not
  painted in the popup.
- Native mouse hit testing uses the exact physical cell rectangles produced by
  the renderer. Click targets are copied before requesting a reentrant TSF edit.

## Lifecycle and costs

Each TSF apartment lazily retains one egui context, font cache and atlas across
compositions. It renders only when candidate state changes. `WM_PAINT` copies the
cached bitmap, and there is no repaint timer or background render loop.
The font backend uses Skrifa and vello_cpu with font hinting enabled by default.
The font is loaded from Windows' Fonts directory (Microsoft YaHei preferred,
then Chinese fallback fonts). Microsoft fonts are not bundled in the installer.
Unlike the old GDI path, this keeps a font file and atlas in host memory, so this
change is not a claim of lower memory use. An unavailable CJK font leaves egui's
default fonts, which cannot display all Chinese characters.

On the development machine, a release benchmark using the previous 15px font with eight two-character
candidates and 200 warm updates per DPI scale measured:

| Scale | Median | P95 |
| --- | ---: | ---: |
| 100% | 1.63ms | 2.47ms |
| 150% | 3.37ms | 3.88ms |
| 200% | 5.48ms | 5.56ms |

The first font load, measure and render took about 34ms with ordinary OS file
caches. These timings cover CPU measurement and bitmap rendering only, not
dictionary lookup, TSF edits, window presentation or end-to-end key latency.
They are local observations, not performance assertions in tests.

## Validation

```powershell
cargo test -p retype-tsf -p retype-candidate-ui --features retype-candidate-ui/egui
cargo clippy -p retype-tsf -p retype-candidate-ui --features retype-candidate-ui/egui --all-targets -- -D warnings
cargo run --release -p retype-candidate-ui --features egui --example candidate-benchmark

$env:RETYPE_CANDIDATE_FIXTURES = "$PWD\target\candidate-egui-fixtures"
cargo test -p retype-candidate-ui --features egui export_visual_fixtures_when_requested
Remove-Item Env:RETYPE_CANDIDATE_FIXTURES
```

Regression tests cover 100–200% scaling (including 125% and 175%), complete long
phrases, later-page selection and hit testing, font atlas reuse, clipping and
shared triangle edges. Native tests verify no-activation flags, foreground
preservation, TSF commit/cancel/passthrough and search candidate enumeration.
Generated fixtures cover short characters, phrases, mixed Chinese/Latin text
and wrapping. Installed Windows Search and multiple real applications still
need manual acceptance; automated interface tests do not establish that result.

The settings application uses egui in a separate process; see
[its implementation and validation](../apps/settings-egui/README.md).
