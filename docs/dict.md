# 词库数据管线

## 目标

给本地拼音引擎（首刷）提供：**词 → 拼音串 → 权重** 的查询能力，且

- 加载不阻塞输入线程（P1）→ mmap 只读二进制，首次激活异步预热
- 损坏可降级（P2）→ 校验失败退化为「单字模式」
- 体积可控 → 目标 `dict.bin` ≤ 30MB（TIP 会被注入每个进程，但词库文件是共享 mmap，只占一份物理内存）

---

## 数据来源

### 1. 词频表：jieba 词典（MIT）

`data/dict/raw/jieba-dict.txt`，来自 [jieba-rs](https://github.com/messense/jieba-rs)（MIT，词表继承自 [jieba](https://github.com/fxsjy/jieba)，MIT）。

格式：`词 词频 词性`，一行一条，UTF-8，约 349,045 条。

```text
中国 120442 ns
人工智能 3450 n
的 12588906 uj
```

> 只取「词 + 词频」，词性用于过滤（如过滤纯数字/字母条目、量词降权）。

### 2. 字音表：`pinyin` crate（离线全量）

[`pinyin`](https://crates.io/crates/pinyin) 0.10 提供：

- `ToPinyin`：单字 → 首选读音（`han.to_pinyin().plain()`）
- `ToPinyinMulti`：单字 → **全部**读音（多音字），用于生成多音字条目的所有可能拼音键

这样即使词频表里没收录某个词，也能用单字读音拼出候选（降级路径）。

单字的源词频不代表每种读音的使用次数。构建器使用 Unicode 15.1.0
[`kHanyuPinlu`](https://www.unicode.org/reports/tr38/) 的逐读音频率分配词频；
该数据未记录的读音保留极小权重。首选读音维持原来的分配量，
其他读音按相对出现次数折减，避免单字组词压过完整词；未覆盖的字把次读音降到首选的 1%。
原词表中有少数罕见单字词频异常高；对未出现在频率表中的单字把源词频封顶为 100，
仍保留其候选。词级覆盖表另行处理“没收”“南无”这类只有在词中才能确定的读音。

### 3. 用户词库（M2）

运行时生成，SQLite，`%APPDATA%/retype/user.db`，不入 git。

---

## 构建流程

```text
data/dict/raw/jieba-dict.txt          pinyin crate（字音表）
            │                                  │
            └──────────────┬───────────────────┘
                           ▼
                 tools/dict-build（Rust bin）
                 ├ 过滤：非汉字条目、单字词性噪声、超长词(>8字)
                 ├ 注音：逐字取读音；多音字 → 该词的所有合法拼音键
                 │        （多音字消歧先用「词级读音表」兜底：
                 │          常见词硬编码，如 重庆=chongqing、长大=zhangda）
                 ├ 归一：词频 → log 概率（-log10(freq/total)），压到 u16
                 └ 排序：按拼音键字典序，写出二进制
                           ▼
                 data/dict/retype-dict.bin   （gitignore，构建产物）
```

命令：`cargo run -p retype-dict-build -- --in data/dict/raw/jieba-dict.txt --out data/dict/retype-dict.bin`

---

## `retype-dict.bin` 格式

小端序。设计目标：**给定拼音键序列，O(log n) 定位候选区间，顺序读出，无需解压**。

```text
Header (32B)
  magic      [u8; 8]  = b"RETYPED1"
  version    u32      = 1
  entry_count u32
  syllable_count u32
  checksum   u32      = crc32(Header 之后的全部字节)
  reserved   u32

SyllableTable
  每个音节: id u16, len u8, bytes  （"zhuang" 等，按字典序）
  → 音节 id 是拼音键的原子单位，避免在条目里重复存字符串

Entries（按拼音键字典序排列，便于二分）
  每条:
    key_len    u8                 # 音节数（1..=8）
    syllables  [u16; key_len]     # 音节 id 序列
    text_len   u8
    text       [u8; text_len]     # UTF-8 汉字
    logp       u16                # 归一化词频，越大越常见
    flags      u8                 # bit0=多音字歧义 bit1=专有名词 bit2=热词可云端更新

Index
  以「音节 id 序列」为 key 的有序数组，二分查找 → Entries 区间偏移
```

> M1 用「排序数组 + 二分」足够（35 万条，查询 < 100µs）。若后续要做前缀模糊匹配（`nihaom...` 整句搜索），再升级为 DAWG/FST，格式版本 +1 即可，`magic` 里的 `D1` 就是留给这件事的。

---

## 打分模型：unigram 的固有偏差与 `word_bonus`

这是 M0 阶段最重要的一个实测结论，直接决定候选质量。

词库里存的是 `logp(w) = ln(freq_w / T)`（T = 全库词频总和，实测约 5.8e7）。
一条 k 个词的路径总分是：

```text
Σ ln(f_i / T) = Σ ln f_i − k · ln T
                                    ↑
                        每多切一个词就白扣 ln T ≈ 17.9
```

这个 `−k·ln T` 是**模型的 artifact，不是语言事实**：它让「词数少」的路径无条件占优。
后果在真实词库上非常刺眼 —— jieba 里那个词频≈3 的垃圾词条 `妳好吗`（logp −16.8）
会压过 `你好(−11.3) + 吗(−7.9)`：

```text
妳好吗 + 世界        Σ logp = −24.28, k=2
你好 + 吗 + 世界     Σ logp = −26.74, k=3
```

所以 `DecodeOptions::word_bonus` 是**每词加分**（不是惩罚），用来部分补偿这个偏差：

```text
score = Σ ( logp(w_i) + word_bonus )
```

- 理论上界是 `ln T ≈ 17.9`（完全补偿，等价于假设任意词都能自由相接 → 退化成全单字）
- 下界是 0（不补偿 → 垃圾长词条压过常用词）

### 实测扫描（352,357 条真实词库，`retype-diag --wb <β> --explain <拼音>`）

| β | nihaomashijie | shanghai | rengongzhineng | jintiantianqibucuo | nihaoma |
|---|---|---|---|---|---|
| 1.0 | 你号码世界 ✗ | 上海 ✓ | 人工智能 ✓ | 今天天气不错 ✓ | 你号码 ✗ |
| 2.0 | 你号码世界 ✗ | 上海 ✓ | 人工智能 ✓ | 今天天气不错 ✓ | 你号码 ✗ |
| 3.0 | 你号码世界 ✗ | 上海 ✓ | 人工智能 ✓ | 今天天气不错 ✓ | 你号码 ✗ |
| **3.5** | **你好吗世界 ✓** | **上海 ✓** | **人工智能 ✓** | **今天天气不错 ✓** | **你好吗 ✓** |
| 4.0 | 你好吗世界 ✓ | 上海 ✓ | 人工智能 ✓ | 今天天气不错 ✓ | 你好吗 ✓ |
| 5.0 | 你好吗是她 ✗ | 上还 ✗ | 人共只而 ✗ | 今天天起不最 ✗ | 你好吗 ✓ |
| 6.0 | 你好吗是她 ✗ | 上还 ✗ | 人共只而 ✗ | 今天是按起不最 ✗ | 你好吗 ✓ |

**默认值取 3.5**（区间 3.5~4.0 都可接受，5.0 起明显过度切分）。

复现命令：

```powershell
target\release\retype-diag.exe --dict data\dict\retype-dict.tsv --wb 3.5 --explain nihaomashijie
```

`--explain` 会打印**逐边分解**（每个词的 logp、消费字符数、该步得分）。
没有这个视图就没法调参：候选文字相同的两条路径可能来自完全不同的切分
（`你好 + 吗` vs `你 + 好吗`），光看结果是猜不出原因的。

### 已知残留问题（M2 解决）

- `woxiangchifan` 首选是「我**向**吃饭」而不是「我**想**吃饭」。
  向/想 的词频差在 unigram 下无法靠上下文纠正 —— 这正是 test.md 第三节说的
  「只有音频/拼音，没有上下文，模型就只能猜」。M2 的本地上下文打分 +
  M3 的云端重排就是冲这个来的。
- β 是个全局常数，对长短输入并非同时最优。真正的解法是 bigram：
  `logp(w_i | w_{i-1})` 天然消除 `−k·ln T` 偏差，届时 `word_bonus` 应退化成很小的微调量。

---

## 实测性能（release，x86_64，352,357 词条）

| 指标 | 实测 | 预算 |
|---|---|---|
| `dict-build` 全量构建 | 1.2 s | — |
| 产物体积（TSV） | 8.7 MB | — |
| 词库加载（解析 + 建 trie） | 0.65 ~ 1.1 s | **必须异步**，见 `AsyncDict` |
| trie 节点数 | 357,163 | — |
| 首刷按键延迟 P50 | 445 µs | — |
| 首刷按键延迟 P95 | 2.18 ms | — |
| 首刷按键延迟 P99 | 3.35 ms | 5 ms ✓ |
| 首刷按键延迟 max | 22 ms | 首次按键含惰性初始化，M1 用预热消除 |

复现：

```powershell
cargo run -p retype-diag --release -- --bench --dict data/dict/retype-dict.tsv
```

词库加载 0.65~1.1s 是**绝对不能**放在 `ActivateEx` 里的（那跑在宿主应用的 UI 线程上，
等于让用户的记事本卡一秒）。`retype_dict::AsyncDict` + `spawn_loader` 就是为此存在：
激活时先装空词库（降级成原样字母），后台线程加载完再热替换，键盘全程不卡。

---

## 音节表

标准汉语拼音音节约 410 个（不带声调）。表由 `retype-pinyin` 提供**唯一真源**（`core/pinyin/src/syllables.rs`），`dict-build` 与运行时共用，避免两边不一致。

切分歧义样例（必须有单元测试）：

| 输入 | 可能切分 | 期望首选 |
|---|---|---|
| `xian` | `xi'an` / `xian` | 先/现（`xian`） |
| `shanghai` | `shang'hai` / `shan'g'hai` | 上海 |
| `jian` | `ji'an` / `jian` | 见/建 |
| `changan` | `chang'an` / `chan'gan` | 长安 |
| `nihao` | `ni'hao` | 你好 |

规则：**音节边界歧义由词库解决，不由切分器猜**。切分器产出所有合法切分构成词格（lattice），由 Viterbi 结合词频选最优路径 —— 这正是 §2 首刷的做法。

---

## 模糊音 / 简拼

- **模糊音**（M2，可在设置里开关）：`zh↔z`、`ch↔c`、`sh↔s`、`n↔l`、`f↔h`、`an↔ang`、`en↔eng`、`in↔ing`。实现方式：查询时对音节 id 做等价类扩展，**不改词库**。
- **简拼**（M1）：`nhm` → 首字母匹配。实现方式：索引里额外维护「首字母序列 → 条目」的次级索引，或查询时退化为逐音节首字母过滤。

---

## 授权

- jieba / jieba-rs 词表：**MIT**。`data/dict/raw/LICENSE-jieba` 保留原始版权声明，发行包中需附带。
- `pinyin` crate：**MIT**。
- 自建的用户词库：用户数据，不上传（除用户显式开启云同步）。
