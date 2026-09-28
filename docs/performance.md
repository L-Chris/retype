# Performance notes

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
