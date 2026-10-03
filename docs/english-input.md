# English input

This first stage adds local word completion, one-edit spelling suggestions and personal vocabulary to English mode. Phrase prediction, next-word prediction, grammar correction and AI writing are not included yet.

## Interaction

- Type letters to preview prefix matches, ordered by word frequency and personal usage. Initial capitals and all-caps input retain their casing; internal apostrophes are supported.
- Space commits the original word unless you explicitly selected a candidate. No automatic spelling replacement occurs.
- Tab or clicking a candidate accepts it and inserts one trailing space. Up/Down explicitly selects a candidate; Space then accepts it and inserts a space. Enter accepts an explicitly selected candidate without a trailing space and retains the application's Enter action.
- Enter commits the word and reaches the application normally, allowing search submission and newlines. Escape keeps the typed word and dismisses completion.
- Candidates show numbered labels. Press `1–8` to accept the corresponding candidate on the current page and insert one trailing space. Digits with no matching visible candidate remain literal input; `0` and `9` always remain literal.
- To type a word containing digits, such as `python3`, press Escape after `python` to keep the original text and dismiss completion, then type `3`. Punctuation, `-` and `=` remain literal input. Ctrl/Alt/Windows shortcuts first finish the word and then reach the application.
- Disable English completion or spelling suggestions under Settings > Input.

Spelling suggestions cover one insertion, deletion, substitution or adjacent transposition, such as `hellp → hello` and `teh → the`. They arrive asynchronously after prefix matches; existing candidates retain their positions while typing. Suggestions never replace your text without selection.

## Data and privacy

The bundled vocabulary is derived from SymSpell's 82,765-entry frequency file, pinned in `data/dict/raw/english/SOURCE.txt`. The index is compiled into shared, read-only module data, with no runtime vocabulary download or heap-sized dictionary construction. This uses the upstream frequency data, not the SymSpell correction algorithm. Notices and licenses are included in the installer.

Successfully committed English words accumulate local usage counts separately from Chinese learning. A word becomes a personal suggestion after at least two observations; one-off input is not immediately promoted. Personal vocabulary is bounded to 10,000 words and persists through the existing learning broker. Existing WebDAV sync also carries English usage and input preferences; repeated synchronization merges per-device counts without double counting.

Password, hidden and private input scopes do not produce suggestions or learning. Unknown input scopes also disable learning. No input is sent to an AI provider by English completion.

## Validation

```powershell
cargo test --workspace --features retype-learning/broker,retype-ai/service
cargo clippy --workspace --all-targets --features retype-learning/broker,retype-ai/service -- -D warnings
cargo run -p retype-diag --release -- --english --bench --no-cloud
```

The first-pass benchmark excludes asynchronous spelling work and native popup rendering. Application compatibility still needs desktop testing beyond the real TSF test host.

## 中文说明

第一阶段支持离线单词补全、单次编辑纠错建议、大小写和单词内撇号，以及独立的个人英文词汇学习；暂未加入短语补全、下一个词预测、语法纠正和 AI 写作。

空格默认上屏原文，Tab 或鼠标选词后自动补一个空格；上下键显式选中后，空格接受候选并添加空格。Enter 选词不补空格，并保留应用原有行为；Escape 保留输入并结束补全。候选显示编号，按 1–8 选用当前页对应候选并补一个空格；没有对应可见候选时数字正常输入，0 和 9 始终直接输入。输入 python3 等字母数字组合时，先在 python 后按 Escape 保留原文并结束补全，再输入 3。标点和快捷键保持原有用途。

成功上屏的英文单词保存在本机；至少使用两次才作为个人候选，最多保留 10,000 个词，并沿用现有云同步机制。密码、隐藏与隐私输入范围不提供建议，也不学习；未知输入范围不学习。英文补全不调用 AI 模型。
