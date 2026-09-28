# 词库数据管线

## 默认数据

retype 的默认词库来自[万象拼音 Base v18.0.14](https://github.com/amzxyz/rime-wanxiang/releases/tag/v18.0.14)。
仓库保存了上游的单字、基础词、联想词和多音词四份原始 Rime 词典；
来源、校验和及许可见 [SOURCE.md](../data/dict/raw/wanxiang-base/SOURCE.md)。
构建器直接使用上游逐词注音与权重，不再由单字读音推测整个词的发音。
因此「奇数」保留 `jī shù`，小鹤双拼输入 `jiuu` 能找到它。

Base 词典按词保存带调拼音，例如 `jī shù`；构建时只去掉声调。
当前 retype 不支持万象 Pro 的辅码筛选、Lua 功能或语法模型。
全拼和小鹤双拼共用同一份去声调词库。

构建器只纳入汉字词和最多五字的词条，与解码器当前的
`max_word_syllables = 5` 一致。万象的专业分类词库和六字以上的长词
尚未纳入默认词库；这些数据可以在扩展引擎能力后单独评估。

## 构建与检查

仓库中的原始词典可离线构建；产物位于 `data/dict/`，不提交 Git：

```powershell
cargo run -p retype-dict-build --release -- --out data/dict/retype-dict.tsv
cargo run -p retype-diag --release -- --dict data/dict/retype-dict.bin --scheme flypy --explain jiuu
cargo run -p retype-diag --release -- --dict data/dict/retype-dict.bin --scheme flypy --bench
```

构建器读取 `词<TAB>带调拼音<TAB>权重`，产出
`词<TAB>无声调拼音<TAB>权重` TSV，再编译成 `RTDICT01` 二进制词库。
无效音节、非汉字词条、超过五字的词条会跳过；构建结束后回读二进制，
核查「奇数」「快捷键」「没收」「南无」等词的读音。
`--in` 可重复指定其他 Rime 词典；`--min-weight` 和
`--max-word-len` 可调整过滤阈值。

这次转换产出约 155 万条有效词条；TSV 约 44.5 MB，二进制约 43.5 MB。
词库在后台线程加载，不阻塞 TSF 激活线程。词库大小高于旧版
Jieba 词库，后续若要压缩，应优化二进制编码或有依据地裁剪词条，
避免直接降低常用词覆盖率。

## 候选排序

词典权重由上游维护，**不是与旧版 Jieba 频次可直接相加的语料计数**。
加载器按 `logp(w) = ln(weight_w / Σweight)` 归一化。
一条切词路径的得分为 `Σ(logp(w_i) + word_bonus)`；
当前 `word_bonus` 默认 3.5，解码器保留前 24 条路径，
再补入未入选的完整词条以便翻页选择。
因此仅更换词库不能保证所有候选顺序正确，仍需针对真实输入验收排序。

## 授权

retype 代码为 MIT；万象 Base 原始词典及其转换产物保留
[CC BY 4.0](../data/dict/raw/wanxiang-base/LICENSE) 数据许可。
发行包附带 `NOTICE.txt`、`LICENSE-wanxiang` 和来源、修改说明。
