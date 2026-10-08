# Performance notes

The current local key-processing P99 budget is **6 ms** for both Full Pinyin and Xiaohe Shuangpin. CI measures the compiled binary dictionary. Current memory measurements and optimization results are in [the memory profile](memory-profile.md); the historical figures below retain their original budget.

The [input/backspace investigation](input-latency.md) measures candidate width calculation and bitmap rendering separately and documents their costs beyond the engine-only budget.

These are historical measurements from the 0.1.3 M1 desktop preview, not promises for later releases. They were measured with a release x86_64 build and a dictionary of 352,357 entries. The local input-engine benchmark excludes TSF host scheduling and candidate-window drawing.

| Measurement | 0.1.3 result | Target or context |
| --- | --- | --- |
| Local first-pass key latency P50 | 293 µs | — |
| Local first-pass key latency P99 | 2.22 ms | 5 ms budget |
| Dictionary load | 0.65–1.1 s | Must load asynchronously |
| Dictionary build | 1.2 s | — |
| TSF DLL size | 767.5 KiB x64 / 652 KiB x86 | ~300 KiB goal not yet met |
| Windows installer | ~8 MiB | Includes both DLL architectures and dictionary |

The compact pronunciation table is generated at build time, replacing the larger runtime `pinyin` data inside the TSF DLL. See the [M1 validation record](m1-validation.md) for the related tests and remaining compatibility work. To measure a current checkout, run:

```powershell
cargo run -p retype-diag --release -- --bench --dict data/dict/retype-dict.tsv
```
