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
