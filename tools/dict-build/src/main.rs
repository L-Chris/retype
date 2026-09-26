//! 词库构建工具（docs/dict.md）。
//!
//! 输入：jieba 词频表 `词 词频 [词性]`
//! 输出：已注音词库 `词\t拼音\t词频`，可被 `retype_dict::load_annotated` 直接加载。
//!
//! 注音在**构建期**完成，运行时不需要再跑一遍全量汉字注音 —— 这是
//! 「词典加载不能阻塞输入线程」（P1）能在 M1 用 mmap 二进制实现的前提。
//!
//! 用法：
//! ```text
//! cargo run -p retype-dict-build --release -- \
//!     --in  data/dict/raw/jieba-dict.txt \
//!     --out data/dict/retype-dict.tsv
//! ```

use retype_dict::annotate;
use retype_pinyin::syllables;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    min_freq: f64,
    max_word_len: usize,
    verify: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            input: PathBuf::from("data/dict/raw/jieba-dict.txt"),
            output: PathBuf::from("data/dict/retype-dict.tsv"),
            min_freq: 1.0,
            max_word_len: 8,
            verify: true,
        }
    }
}

fn usage() -> String {
    "\
retype-dict-build —— 构建已注音词库

用法:
  retype-dict-build [--in <词频表>] [--out <输出.tsv>]
                    [--min-freq <n>] [--max-word-len <n>] [--no-verify]

参数:
  --in             输入词频表，jieba 格式：`词 词频 [词性]`
                   默认 data/dict/raw/jieba-dict.txt
  --out            输出已注音词库，格式：`词<TAB>拼音<TAB>词频`
                   默认 data/dict/retype-dict.tsv（已在 .gitignore 中）
  --min-freq       词频下限，低于此值的词丢弃（默认 1）
                   想要更小更快的词库可以设成 50，代价是生僻词打不出来
  --max-word-len   词长上限（字符数，默认 8）
  --no-verify      跳过构建后的回读校验
"
    .to_string()
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
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
            "--in" => a.input = PathBuf::from(next(&mut i)?),
            "--out" => a.output = PathBuf::from(next(&mut i)?),
            "--min-freq" => {
                a.min_freq = next(&mut i)?
                    .parse()
                    .map_err(|_| "--min-freq 需要数字".to_string())?
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
    Ok(a)
}

#[derive(Debug, Default)]
struct Stats {
    lines: usize,
    emitted: usize,
    words: usize,
    skipped_malformed: usize,
    skipped_low_freq: usize,
    skipped_long: usize,
    skipped_non_han: usize,
    skipped_no_pinyin: usize,
}

fn build(args: &Args) -> Result<Stats, String> {
    let started = Instant::now();
    let infile = std::fs::File::open(&args.input)
        .map_err(|e| format!("打不开 {}: {e}", args.input.display()))?;
    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建不出目录 {}: {e}", parent.display()))?;
        }
    }
    let outfile = std::fs::File::create(&args.output)
        .map_err(|e| format!("写不了 {}: {e}", args.output.display()))?;

    let mut reader = BufReader::with_capacity(1 << 16, infile);
    let mut w = BufWriter::with_capacity(1 << 20, outfile);
    let mut line = String::new();
    let mut stats = Stats::default();

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("读词频表失败: {e}"))?;
        if n == 0 {
            break;
        }
        stats.lines += 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut it = t.split_whitespace();
        let (Some(word), Some(freq_s)) = (it.next(), it.next()) else {
            stats.skipped_malformed += 1;
            continue;
        };
        let Ok(freq) = freq_s.parse::<f64>() else {
            stats.skipped_malformed += 1;
            continue;
        };
        if freq < args.min_freq {
            stats.skipped_low_freq += 1;
            continue;
        }
        if word.chars().count() > args.max_word_len {
            stats.skipped_long += 1;
            continue;
        }
        if !annotate::all_han(word) {
            stats.skipped_non_han += 1;
            continue;
        }
        let variants = annotate::annotate_variants(word);
        if variants.is_empty() {
            stats.skipped_no_pinyin += 1;
            continue;
        }
        let ambiguous = annotate::is_ambiguous(word);
        let per = freq / variants.len() as f64;
        for ids in variants {
            let py: Vec<&str> = ids
                .iter()
                .filter_map(|id| syllables::name_of(*id))
                .collect();
            if py.len() != ids.len() {
                stats.skipped_no_pinyin += 1;
                continue;
            }
            // flags 用第 4 列带上，加载方目前忽略它，留给 M2 的上下文打分
            writeln!(w, "{word}\t{}\t{per}\t{}", py.join(" "), ambiguous as u8)
                .map_err(|e| format!("写输出失败: {e}"))?;
            stats.emitted += 1;
        }
        stats.words += 1;

        if stats.lines % 50_000 == 0 {
            eprintln!("  ... 已处理 {} 行，产出 {} 条", stats.lines, stats.emitted);
        }
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
    if retype_pinyin::Lexicon::len(&binary) != count {
        return Err("二进制词库回读数量不一致".into());
    }
    eprintln!(
        "构建完成: {} 词 → {} 条（含多音字变体），耗时 {:?}",
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

    eprintln!("输入: {}", args.input.display());
    eprintln!("输出: {}", args.output.display());
    let stats = match build(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "过滤明细: 低词频 {} / 超长 {} / 非汉字 {} / 无法注音 {} / 格式错误 {}",
        stats.skipped_low_freq,
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
