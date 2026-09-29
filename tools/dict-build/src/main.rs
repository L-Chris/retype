//! 词库构建工具（docs/dict.md）。
//!
//! 输入：万象拼音 Base Rime 词库。
//! 输出：去声调的已注音词库 `词\t拼音\t权重`。
//!
//! 保留上游逐词注音与权重；运行时不再猜多音字读音。
//!
//! 用法：
//! ```text
//! cargo run -p retype-dict-build --release -- \
//!     --out data/dict/retype-dict.tsv
//! ```

use retype_dict::annotate;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone)]
struct Args {
    inputs: Vec<PathBuf>,
    output: PathBuf,
    min_weight: f64,
    max_word_len: usize,
    verify: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            inputs: ["zi", "jichu", "lianxiang", "duoyin"]
                .into_iter()
                .map(|name| PathBuf::from(format!("data/dict/raw/wanxiang-base/{name}.dict.yaml")))
                .collect(),
            output: PathBuf::from("data/dict/retype-dict.tsv"),
            min_weight: 1.0,
            max_word_len: 8,
            verify: true,
        }
    }
}

fn usage() -> String {
    "\
retype-dict-build —— 构建已注音词库

用法:
  retype-dict-build [--in <词库.yaml>]... [--out <输出.tsv>]
                    [--min-weight <n>] [--max-word-len <n>] [--no-verify]

参数:
  --in             万象 Base Rime 词库，可重复指定；默认单字、基础词、联想词、多音词
  --out            输出已注音词库，格式：`词<TAB>拼音<TAB>权重`
                   默认 data/dict/retype-dict.tsv（已在 .gitignore 中）
  --min-weight     权重下限，低于此值的词丢弃（默认 1）
  --max-word-len   词长上限（字符数，默认 8，与解码器上限一致）
  --no-verify      跳过构建后的回读校验
"
    .to_string()
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut custom_inputs = false;
    let mut i = 0;
    while i < argv.len() {
        let k = argv[i].as_str();
        let next = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{k} 缺少参数值"))
        };
        match k {
            "-h" | "--help" => return Err(String::new()),
            "--in" => {
                if !custom_inputs {
                    a.inputs.clear();
                    custom_inputs = true;
                }
                a.inputs.push(PathBuf::from(next(&mut i)?));
            }
            "--out" => a.output = PathBuf::from(next(&mut i)?),
            "--min-weight" => {
                a.min_weight = next(&mut i)?
                    .parse()
                    .map_err(|_| "--min-weight 需要数字".to_string())?
            }
            "--max-word-len" => {
                a.max_word_len = next(&mut i)?
                    .parse()
                    .map_err(|_| "--max-word-len 需要整数".to_string())?
            }
            "--no-verify" => a.verify = false,
            other => return Err(format!("未知参数: {other}\n\n{}", usage())),
        }
        i += 1;
    }
    if a.inputs.is_empty() || !a.min_weight.is_finite() || a.min_weight <= 0.0 {
        return Err("输入词库和权重下限必须有效".into());
    }
    Ok(a)
}

#[derive(Debug, Default)]
struct Stats {
    lines: usize,
    emitted: usize,
    words: usize,
    skipped_malformed: usize,
    skipped_low_weight: usize,
    skipped_long: usize,
    skipped_non_han: usize,
    skipped_no_pinyin: usize,
}

fn unaccent(c: char) -> Option<char> {
    match c {
        'ā' | 'á' | 'ǎ' | 'à' => Some('a'),
        'ē' | 'é' | 'ě' | 'è' | 'ê' => Some('e'),
        'ī' | 'í' | 'ǐ' | 'ì' => Some('i'),
        'ō' | 'ó' | 'ǒ' | 'ò' => Some('o'),
        'ū' | 'ú' | 'ǔ' | 'ù' => Some('u'),
        'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => Some('v'),
        'ń' | 'ň' | 'ǹ' => Some('n'),
        'ḿ' => Some('m'),
        c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// Base 词库保留逐词带调拼音；当前引擎使用去声调后的音节。
fn plain_pinyin(raw: &str, word_len: usize) -> Option<String> {
    let mut out = String::new();
    let mut count = 0;
    for item in raw.split_whitespace() {
        if count > 0 {
            out.push(' ');
        }
        for c in item.chars() {
            if matches!(c, '\u{0300}'..='\u{036f}') {
                continue;
            }
            out.push(unaccent(c)?);
        }
        count += 1;
    }
    if count != word_len || annotate::parse_pinyin(&out)?.len() != count {
        return None;
    }
    Some(out)
}

fn build(args: &Args) -> Result<Stats, String> {
    let started = Instant::now();
    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建不出目录 {}: {e}", parent.display()))?;
        }
    }
    let outfile = std::fs::File::create(&args.output)
        .map_err(|e| format!("写不了 {}: {e}", args.output.display()))?;

    let mut w = BufWriter::with_capacity(1 << 20, outfile);
    let mut line = String::new();
    let mut stats = Stats::default();
    for input in &args.inputs {
        let infile =
            std::fs::File::open(input).map_err(|e| format!("打不开 {}: {e}", input.display()))?;
        let mut reader = BufReader::with_capacity(1 << 16, infile);
        let mut in_body = false;
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .map_err(|e| format!("读取 {} 失败: {e}", input.display()))?;
            if n == 0 {
                break;
            }
            stats.lines += 1;
            let t = line.trim();
            if !in_body {
                in_body = t == "...";
                continue;
            }
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let mut cols = t.split('\t');
            let (Some(word), Some(raw_pinyin), Some(weight_s)) =
                (cols.next(), cols.next(), cols.next())
            else {
                stats.skipped_malformed += 1;
                continue;
            };
            let Ok(weight) = weight_s.parse::<f64>() else {
                stats.skipped_malformed += 1;
                continue;
            };
            if !weight.is_finite() || weight <= 0.0 {
                stats.skipped_malformed += 1;
                continue;
            }
            if weight < args.min_weight {
                stats.skipped_low_weight += 1;
                continue;
            }
            let word_len = word.chars().count();
            if word_len > args.max_word_len {
                stats.skipped_long += 1;
                continue;
            }
            if !annotate::all_han(word) {
                stats.skipped_non_han += 1;
                continue;
            }
            let Some(pinyin) = plain_pinyin(raw_pinyin, word_len) else {
                stats.skipped_no_pinyin += 1;
                continue;
            };
            writeln!(w, "{word}\t{pinyin}\t{weight}").map_err(|e| format!("写输出失败: {e}"))?;
            stats.emitted += 1;
            stats.words += 1;
        }
        if !in_body {
            return Err(format!("{} 缺少 Rime 词库正文标记 ...", input.display()));
        }
        eprintln!("已处理 {}，累计产出 {} 条", input.display(), stats.emitted);
    }
    if stats.emitted == 0 {
        return Err("词库没有可用词条".into());
    }
    w.flush().map_err(|e| format!("flush 失败: {e}"))?;
    let binary_path = args.output.with_extension("bin");
    let input = std::fs::File::open(&args.output).map_err(|e| e.to_string())?;
    let output = std::fs::File::create(&binary_path).map_err(|e| e.to_string())?;
    let count = retype_dict::binary::compile(BufReader::new(input), BufWriter::new(output))
        .map_err(|e| e.to_string())?;
    let (binary, _) =
        retype_dict::binary::load(std::fs::File::open(&binary_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if retype_pinyin::Lexicon::len(&binary) == 0 || retype_pinyin::Lexicon::len(&binary) > count {
        return Err("二进制词库回读数量无效".into());
    }
    eprintln!(
        "构建完成: {} 词 → {} 条，耗时 {:?}",
        stats.words,
        stats.emitted,
        started.elapsed()
    );
    Ok(stats)
}

/// 回读校验：确认产物能被运行时加载，并且常用词查得到。
fn verify(args: &Args) -> Result<(), String> {
    let started = Instant::now();
    let f = std::fs::File::open(&args.output)
        .map_err(|e| format!("打不开产物 {}: {e}", args.output.display()))?;
    let (dict, stats) = retype_dict::load_annotated(BufReader::with_capacity(1 << 16, f))
        .map_err(|e| format!("回读校验失败: {e}"))?;
    eprintln!(
        "回读校验: {} 行 → {} 词条, {} trie 节点, 耗时 {:?}",
        stats.lines,
        dict.len(),
        dict.node_count(),
        started.elapsed()
    );
    if dict.len() == 0 {
        return Err("产物为空".into());
    }

    use retype_pinyin::Lexicon;
    for (word, py) in [
        ("你好", "ni hao"),
        ("中国", "zhong guo"),
        ("世界", "shi jie"),
        ("输入法", "shu ru fa"),
        ("没收", "mo shou"),
        ("南无", "na mo"),
        ("奇数", "ji shu"),
        ("快捷键", "kuai jie jian"),
    ] {
        let Some(ids) = annotate::parse_pinyin(py) else {
            return Err(format!("测试拼音非法: {py}"));
        };
        let mut out = Vec::new();
        dict.lookup(&ids, &mut out);
        if !out.iter().any(|e| &*e.text == word) {
            return Err(format!("回读校验：{py} 查不到「{word}」"));
        }
    }
    eprintln!("回读校验通过：常用词均可查得");
    Ok(())
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            // 空消息 == --help，正常退出
            if msg.is_empty() {
                print!("{}", usage());
                return;
            }
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    for input in &args.inputs {
        eprintln!("输入: {}", input.display());
    }
    eprintln!("输出: {}", args.output.display());
    let stats = match build(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "过滤明细: 低权重 {} / 超长 {} / 非汉字 {} / 无法注音 {} / 格式错误 {}",
        stats.skipped_low_weight,
        stats.skipped_long,
        stats.skipped_non_han,
        stats.skipped_no_pinyin,
        stats.skipped_malformed
    );
    if args.verify {
        if let Err(e) = verify(&args) {
            eprintln!("错误: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_readings_keep_word_pronunciation_without_tone() {
        assert_eq!(plain_pinyin("jī shù", 2).as_deref(), Some("ji shu"));
        assert_eq!(plain_pinyin("jī shù", 2).as_deref(), Some("ji shu"));
        assert_eq!(plain_pinyin("lǜ sè", 2).as_deref(), Some("lv se"));
        assert_eq!(
            plain_pinyin("kuài jié jiàn", 3).as_deref(),
            Some("kuai jie jian")
        );
        assert_eq!(plain_pinyin("qí shù", 3), None);
        assert_eq!(plain_pinyin("jī;dk shù;mw", 2), None);
    }
}
