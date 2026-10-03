//! `retype-diag` —— 终端调试台。
//!
//! 调 TSF 最痛的是「改一行 → 重新注册 → 注销重登 → 打开记事本试」。
//! 这个工具把内核从 TSF 里剥出来跑在终端上：**90% 的迭代应该在这里完成**，
//! 只有验证「某个具体应用里的兼容性」时才需要真的注册 DLL。
//!
//! 它跑的是**同一个内核、同一份词库、同一条首刷/二刷链路**，
//! 只是把候选窗换成了文本渲染，把 TSF 事件换成了 stdin。
//!
//! ```text
//! cargo run -p retype-diag --release -- --dict data/dict/retype-dict.tsv
//! ```

use retype_candidate_ui::format_state;
use retype_cloud::MockConfig;
use retype_dict::{AsyncDict, LayeredDict, Learner, UserDict, DEFAULT_USER_BOOST};
use retype_engine::{
    BackendOptions, InlineBackend, Kernel, KernelBackend, KernelConfig, LocalBackend,
};
use retype_pinyin::Lexicon;
use retype_types::{
    AsrEvent, ContextSnapshot, InputEvent, InputSource, KernelAction, Key, LearningStore,
    Modifiers, PinyinScheme, PrivacyLevel, RenderState, VoiceEvent,
};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_DICT: &str = "data/dict/retype-dict.tsv";

struct Cli {
    dict: String,
    inline: bool,
    bench: bool,
    no_cloud: bool,
    context: Option<String>,
    /// 覆盖 `DecodeOptions::word_bonus`，用来现场调参
    word_bonus: Option<f32>,
    /// 覆盖 k-best 宽度
    kbest: Option<usize>,
    scheme: PinyinScheme,
    /// `--explain <拼音>`：打印候选的音节切分与得分后退出
    explain: Option<String>,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            dict: DEFAULT_DICT.to_string(),
            inline: false,
            bench: false,
            no_cloud: false,
            context: None,
            word_bonus: None,
            kbest: None,
            scheme: PinyinScheme::Full,
            explain: None,
        }
    }
}

fn usage() {
    print!(
        "\
retype-diag —— retype 输入内核的终端调试台

用法:
  retype-diag [--dict <path>] [--inline] [--bench] [--no-cloud] [--context <文本>]

  --dict      已注音词库 .tsv 或 .bin（tools/dict-build 的产物）。默认 {DEFAULT_DICT}
              文件不存在时会退化成「全量单字」模式，正好用来验证降级路径
  --inline    同步执行二刷（结果确定，便于断言）；默认走异步 LocalBackend
  --bench     跑一遍首刷延迟基准后退出
  --no-cloud  完全关闭云端，模拟断网
  --context   预设光标前文，用来观察二刷的重排效果
  --wb <f>    覆盖每词加分 word_bonus（默认取 DecodeOptions 的值）
              unigram 模型每多切一个词就白扣一次 ln(总词频)≈17.9，
              word_bonus 是这个偏差的部分补偿：太小 → 垃圾长词条压过常用词，
              太大 → 退化成全单字。用 --explain 观察逐边得分来调。
  --k <n>     覆盖 k-best 宽度
  --scheme <full|flypy>  选择全拼或小鹤双拼（默认 full）

REPL 命令:
  <字母>        当成拼音输入，例如  nihaomashijie
  1..9          选第 N 个候选
  :bs           退格
  :enter        回车（上屏原始字母）
  :esc          取消组字
  :pgup/:pgdn   候选翻页
  :ctx <文本>   设置光标前文（隐私等级 = Cloud，会参与二刷）
  :nctx         清空上下文
  :voice <文本> 模拟一次完整语音输入（Interim → Stable → Stop → Final）
  :mode         中英切换
  :stat         打印词库/代次/云端状态
  :help         本帮助
  :q            退出
"
    );
}

/// 把 CLI 覆盖项套到解码参数上，便于现场调参而不用重新编译。
fn decode_options(cli: &Cli) -> retype_pinyin::DecodeOptions {
    let mut o = retype_pinyin::DecodeOptions::default();
    if let Some(wb) = cli.word_bonus {
        o.word_bonus = wb;
    }
    if let Some(k) = cli.kbest {
        o.k = k;
    }
    o
}

fn parse_args(argv: &[String]) -> Result<Cli, String> {
    let mut c = Cli::default();
    let mut i = 0;
    while i < argv.len() {
        let k = argv[i].as_str();
        let val = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{k} 缺少参数值"))
        };
        match k {
            "-h" | "--help" => {
                usage();
                std::process::exit(0);
            }
            "--dict" => c.dict = val(&mut i)?,
            "--inline" => c.inline = true,
            "--bench" => c.bench = true,
            "--no-cloud" => c.no_cloud = true,
            "--context" => c.context = Some(val(&mut i)?),
            "--wb" => {
                c.word_bonus = Some(
                    val(&mut i)?
                        .parse()
                        .map_err(|_| "--wb 需要浮点数".to_string())?,
                )
            }
            "--k" => {
                c.kbest = Some(
                    val(&mut i)?
                        .parse()
                        .map_err(|_| "--k 需要整数".to_string())?,
                )
            }
            "--explain" => c.explain = Some(val(&mut i)?),
            "--scheme" => {
                c.scheme = match val(&mut i)?.as_str() {
                    "full" => PinyinScheme::Full,
                    "flypy" => PinyinScheme::Flypy,
                    _ => return Err("--scheme 只接受 full 或 flypy".to_string()),
                }
            }
            other => return Err(format!("未知参数: {other}")),
        }
        i += 1;
    }
    Ok(c)
}

/// 加载词库。失败不致命 —— 降级成单字模式，正好演示 §7 的兜底路径。
fn load_dict(path: &str) -> Arc<AsyncDict> {
    let holder = AsyncDict::empty();
    let p = std::path::Path::new(path);
    if !p.exists() {
        eprintln!(
            "[警告] 词库 {path} 不存在，降级为「全量单字」模式。\n\
             先构建完整词库：\n  \
             cargo run -p retype-dict-build --release -- --out {DEFAULT_DICT}"
        );
        holder.install(Arc::new(retype_dict::single_char_fallback()));
        return holder;
    }
    let started = Instant::now();
    let f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[警告] 打不开词库 {path}: {e}，降级为单字模式");
            holder.install(Arc::new(retype_dict::single_char_fallback()));
            return holder;
        }
    };
    let loaded = if p.extension().is_some_and(|ext| ext == "bin") {
        retype_dict::binary::open_shared(p).map(|d| {
            let stats = retype_dict::LoadStats {
                lines: d.len(),
                accepted: d.len(),
                ..Default::default()
            };
            (d, stats)
        })
    } else {
        retype_dict::load_annotated(std::io::BufReader::with_capacity(1 << 16, f))
            .map(|(d, stats)| (Arc::new(retype_dict::binary::Dictionary::from(d)), stats))
    };
    match loaded {
        Ok((d, stats)) => {
            eprintln!(
                "[词库] {} 词条 / {} 行 / {} trie 节点，加载耗时 {:?}",
                d.len(),
                stats.lines,
                d.node_count(),
                started.elapsed()
            );
            holder.install_binary(d);
        }
        Err(e) => {
            eprintln!("[警告] 词库解析失败: {e}，降级为单字模式");
            holder.install(Arc::new(retype_dict::single_char_fallback()));
        }
    }
    holder
}

struct App {
    backend: Arc<dyn KernelBackend>,
    dict: Arc<AsyncDict>,
    user: Arc<UserDict>,
    /// 仅用于 :stat 展示。内核里也有一份，但 `KernelBackend` 是 dyn-compatible 的
    /// trait，不暴露内核内部状态，所以调试台自己记一份。
    last_context: std::sync::Mutex<String>,
}

impl App {
    fn new(cli: &Cli) -> Self {
        let dict = load_dict(&cli.dict);
        let user = Arc::new(UserDict::new());
        let sys: Arc<dyn Lexicon> = Arc::clone(&dict) as Arc<dyn Lexicon>;
        let learner: Arc<dyn LearningStore> =
            Arc::new(Learner::with_system(Arc::clone(&user), Arc::clone(&sys)));
        let layered: Arc<dyn Lexicon> = Arc::new(LayeredDict::with_system_and_user(
            sys,
            Arc::clone(&user),
            DEFAULT_USER_BOOST,
        ));

        let cloud = if cli.no_cloud {
            retype_engine::offline_cloud(Duration::from_millis(50))
        } else {
            // Mock 的「假 AI」：上下文里出现过的词会被提前，
            // 足以在终端里观察 test.md 图 4 的二刷效果
            retype_engine::mock_cloud(MockConfig::default(), Duration::from_millis(1500))
        };

        let kernel = Kernel::new(
            KernelConfig {
                rerank_enabled: !cli.no_cloud,
                decode: decode_options(cli),
                pinyin_scheme: cli.scheme,
                ..Default::default()
            },
            layered,
            learner,
            Arc::clone(&cloud),
        );

        let backend: Arc<dyn KernelBackend> = if cli.inline {
            Arc::new(InlineBackend::new(kernel, cloud))
        } else {
            LocalBackend::with_options(
                kernel,
                cloud,
                BackendOptions {
                    rerank_debounce: Duration::from_millis(150),
                },
            )
        };

        let app = Self {
            backend,
            dict,
            user,
            last_context: std::sync::Mutex::new(String::new()),
        };
        if let Some(ctx) = &cli.context {
            app.set_context(ctx);
        }
        app
    }

    fn submit(&self, ev: InputEvent) -> Vec<KernelAction> {
        let mut acts = self.backend.submit(ev);
        acts.extend(self.drain());
        acts
    }

    /// 收干异步队列（二刷完成后的重渲染等）。
    fn drain(&self) -> Vec<KernelAction> {
        let mut out = Vec::new();
        while let Some(a) = self.backend.poll_action() {
            out.push(a);
        }
        out
    }

    /// 等二刷落地。终端里用户敲完一行就回车，天然有一个停顿，
    /// 这里给它一点时间把异步结果收回来，否则看不到二刷效果。
    fn settle(&self, wait: Duration) {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if !self.drain().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn render(&self) {
        let state = self.backend.render();
        print_render(&state);
    }

    fn set_context(&self, text: &str) {
        self.submit(InputEvent::ContextUpdated(ContextSnapshot {
            text_before: text.to_string(),
            privacy: PrivacyLevel::Cloud,
            ..Default::default()
        }));
        if let Ok(mut g) = self.last_context.lock() {
            *g = text.to_string();
        }
        println!("[上下文] 已设置为「{text}」（隐私等级 Cloud，会参与二刷）");
    }

    fn type_pinyin(&self, s: &str) {
        for c in s.chars() {
            self.submit(InputEvent::Key {
                key: Key::Char(c),
                mods: Modifiers::NONE,
                source: InputSource::Keyboard,
            });
        }
        self.settle(Duration::from_millis(600));
        self.render();
    }

    fn simulate_voice(&self, text: &str) {
        // 完整走一遍 test.md 图 2 的三段式：Interim → Stable → Stop → Final
        println!("── 语音会话开始（三段式）──");
        self.submit(InputEvent::Voice(VoiceEvent::Start));

        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let cut1 = (n / 2).max(1).min(n);
        let interim: String = chars[..cut1].iter().collect();
        self.submit(InputEvent::Voice(VoiceEvent::Asr(AsrEvent::Interim(
            interim.clone(),
        ))));
        println!("[第一遍 interim] {interim}");
        show_composition(&self.backend);

        self.submit(InputEvent::Voice(VoiceEvent::Asr(AsrEvent::Stable(
            text.to_string(),
        ))));
        println!("[第二遍 stable ] {text}");
        show_composition(&self.backend);

        self.submit(InputEvent::Voice(VoiceEvent::Stop));
        println!("[松手] 音频发送完毕，等待 final pass —— 状态应为「识别优化中」");
        show_composition(&self.backend);

        let polished = format!("{text}。");
        let acts = self.submit(InputEvent::Voice(VoiceEvent::Asr(AsrEvent::Final(
            polished.clone(),
        ))));
        println!("[第三遍 final  ] {polished}");
        report(&acts);
        println!("── 语音会话结束 ──");
    }

    fn stat(&self) {
        let s = self.backend.render();
        let ctx = self
            .last_context
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| String::new());
        let voice = if s
            .status
            .contains(retype_types::StatusFlags::VOICE_RECORDING)
        {
            "录音中"
        } else if s
            .status
            .contains(retype_types::StatusFlags::VOICE_OPTIMIZING)
        {
            "识别优化中"
        } else {
            "空闲"
        };
        println!(
            "[状态] 代次={} 词库={}条(已加载={}) 用户词={}条 候选={} 语音={voice} 上下文={}",
            s.gen,
            self.dict.len(),
            self.dict.is_loaded(),
            self.user.entry_count(),
            s.candidates.len(),
            if ctx.is_empty() {
                "(空)"
            } else {
                ctx.as_str()
            }
        );
    }
}

fn print_render(state: &RenderState) {
    let mut lock = std::io::stdout().lock();
    let _ = write!(lock, "{}", format_state(state));
    let _ = lock.flush();
}

fn show_composition(b: &Arc<dyn KernelBackend>) {
    let s = b.render();
    println!("         组字区: {:?}", s.composition);
}

/// 把一次动作序列里的「上屏」和「直通」打印出来。
fn report(acts: &[KernelAction]) {
    for a in acts {
        match a {
            KernelAction::Commit(c) => match c {
                retype_types::CommitRequest::Text(t) => println!("★ 上屏(插入): {t}"),
                retype_types::CommitRequest::ReplaceComposition { text } => {
                    println!("★ 上屏(替换组字串): {text}")
                }
            },
            KernelAction::PassThrough => println!("→ 按键交回宿主（直通）"),
            KernelAction::Side(s) => println!("  (副作用: {:?})", side_name(s)),
            KernelAction::Render(_) => {}
        }
    }
}

fn side_name(s: &retype_types::SideEffect) -> &'static str {
    match s {
        retype_types::SideEffect::Rerank(_) => "二刷请求",
        retype_types::SideEffect::Learn(_) => "学习回写",
        retype_types::SideEffect::CollectContext => "采集上下文",
    }
}

fn read_line() -> Option<String> {
    // Windows 控制台可能不是 UTF-8，用 lossy 转换避免 read_line 直接报错
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut buf: Vec<u8> = Vec::new();
    let mut one = [0u8; 1];
    loop {
        match lock.read(&mut one) {
            Ok(0) => return None,
            Ok(_) => {
                if one[0] == b'\n' {
                    break;
                }
                if one[0] != b'\r' {
                    buf.push(one[0]);
                }
            }
            Err(_) => return None,
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn repl(app: &App) {
    println!("retype-diag —— 输入拼音回车即可看候选，:help 查看命令，:q 退出");
    println!("（Windows 控制台若显示乱码，先执行 chcp 65001）");
    loop {
        let mut out = std::io::stderr().lock();
        let _ = write!(out, "\n拼音> ");
        let _ = out.flush();
        let Some(raw) = read_line() else {
            println!("\n[EOF] 退出");
            break;
        };
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line == ":q" || line == ":quit" {
            break;
        }
        if line.starts_with(':') {
            let (cmd, rest) = match line.find(char::is_whitespace) {
                Some(i) => (&line[..i], line[i..].trim()),
                None => (line, ""),
            };
            match cmd {
                ":h" | ":help" => usage(),
                ":bs" => {
                    let acts = app.submit(key_ev(Key::Backspace));
                    report(&acts);
                    app.render();
                }
                ":enter" => {
                    let acts = app.submit(key_ev(Key::Enter));
                    report(&acts);
                    app.render();
                }
                ":esc" => {
                    let acts = app.submit(key_ev(Key::Escape));
                    report(&acts);
                    app.render();
                }
                ":pgup" => {
                    app.submit(InputEvent::CandidatePage { delta: -1 });
                    app.render();
                }
                ":pgdn" => {
                    app.submit(InputEvent::CandidatePage { delta: 1 });
                    app.render();
                }
                ":ctx" => {
                    if rest.is_empty() {
                        println!("用法: :ctx <光标前文>");
                    } else {
                        app.set_context(rest);
                    }
                }
                ":nctx" => app.set_context(""),
                ":voice" => {
                    if rest.is_empty() {
                        println!("用法: :voice <要识别的文字>");
                    } else {
                        app.simulate_voice(rest);
                    }
                }
                ":mode" => {
                    let acts = app.submit(InputEvent::ToggleChinese);
                    report(&acts);
                    app.render();
                }
                ":stat" => app.stat(),
                other => println!("未知命令: {other}（:help 看列表）"),
            }
            continue;
        }
        // 单个数字 = 选词
        if line.len() == 1 {
            let Some(d) = line.chars().next() else {
                continue;
            };
            if !d.is_ascii_digit() {
                app.type_pinyin(line);
                continue;
            }
            if d == '0' {
                println!("候选序号从 1 开始");
                continue;
            }
            let idx = d as usize - '1' as usize;
            let acts = app.submit(InputEvent::CandidateChosen { index: idx });
            report(&acts);
            app.settle(Duration::from_millis(400));
            app.render();
            continue;
        }
        app.type_pinyin(line);
    }
}

fn key_ev(k: Key) -> InputEvent {
    InputEvent::Key {
        key: k,
        mods: Modifiers::NONE,
        source: InputSource::Keyboard,
    }
}

/// 首刷延迟基准。这条线是 P1 的量化形式：
/// 按键 → 候选上屏的本地路径必须远小于一帧（16.7ms）。
fn bench(cli: &Cli) {
    const P99_BUDGET: Duration = Duration::from_millis(6);
    let inputs: &[&str] = match cli.scheme {
        PinyinScheme::Full => &[
            "nihao",
            "nihaomashijie",
            "woxiangchifan",
            "shanghai",
            "xian",
            "zhongguorenmin",
            "jintiantianqibucuo",
            "rengongzhineng",
            "yuyanshurumodel",
            "mingtianwanshangwomenyiqichifanba",
        ],
        PinyinScheme::Flypy => &[
            "a", "m", "w", "y", "mwy", "woe", "nihc", "qiuu", "edu", "eedu", "edum",
        ],
    };

    // 基准要隔离二刷：关掉云端，只测本地首刷
    let dict = load_dict(&cli.dict);
    let user = Arc::new(UserDict::new());
    let sys: Arc<dyn Lexicon> = Arc::clone(&dict) as Arc<dyn Lexicon>;
    let learner: Arc<dyn LearningStore> = Arc::new(Learner::new(Arc::clone(&user)));
    let layered: Arc<dyn Lexicon> = Arc::new(LayeredDict::with_system_and_user(
        sys,
        Arc::clone(&user),
        DEFAULT_USER_BOOST,
    ));
    let kernel = Kernel::new(
        KernelConfig {
            pinyin_scheme: cli.scheme,
            rerank_enabled: false,
            decode: decode_options(cli),
            ..Default::default()
        },
        layered,
        learner,
        retype_engine::offline_cloud(Duration::from_millis(10)),
    );
    let backend = InlineBackend::new(
        kernel,
        retype_engine::offline_cloud(Duration::from_millis(10)),
    );

    let mut samples: Vec<Duration> = Vec::new();
    const ROUNDS: usize = 30;
    for _ in 0..ROUNDS {
        for s in inputs.iter() {
            for c in s.chars() {
                let t = Instant::now();
                backend.submit(key_ev(Key::Char(c)));
                samples.push(t.elapsed());
            }
            backend.submit(key_ev(Key::Escape));
        }
    }

    samples.sort();
    let pct = |p: f64| -> Duration {
        let idx = ((samples.len() as f64) * p).floor() as usize;
        samples[idx.min(samples.len().saturating_sub(1))]
    };
    println!(
        "\n── 首刷延迟基准（{} 次按键，本地路径，不含云端）──",
        samples.len()
    );
    println!("  P50  {:?}", pct(0.50));
    println!("  P95  {:?}", pct(0.95));
    println!("  P99  {:?}", pct(0.99));
    println!("  max  {:?}", samples.last().copied().unwrap_or_default());
    println!("  本地按键 P99 预算 {}ms/次", P99_BUDGET.as_millis());
    let p99 = pct(0.99);
    if p99 > P99_BUDGET {
        println!("  ✗ P99 超预算，需要优化词格构建或词库索引");
    } else {
        println!("  ✓ P99 在预算内");
    }

    // 顺带看几个真实解码结果，确认词库确实生效
    println!("\n── 抽样解码结果 ──");
    let examples: &[&str] = match cli.scheme {
        PinyinScheme::Full => &["nihaomashijie", "shanghai", "rengongzhineng", "xian"],
        PinyinScheme::Flypy => &["a", "mwy", "woe", "nihc", "qiuu"],
    };
    for s in examples {
        for c in s.chars() {
            backend.submit(key_ev(Key::Char(c)));
        }
        let texts: Vec<String> = backend
            .render()
            .candidates
            .iter()
            .take(5)
            .map(|c| c.text.clone())
            .collect();
        println!("  {s:24} → {}", texts.join(" / "));
        backend.submit(key_ev(Key::Escape));
    }
}

/// 打印每个候选的音节切分与得分，用于调参和定位排序问题。
///
/// 没有这个视图就没法回答「为什么 `妳` 排在 `你` 前面」——
/// 候选文字相同但切分不同的情况下，光看结果是猜不出原因的。
fn explain(cli: &Cli, input: &str) {
    let dict = load_dict(&cli.dict);
    let opts = decode_options(cli);
    println!(
        "\n── explain {:?}  (scheme={:?}, word_bonus={}, k={}, raw_penalty={}) ──",
        input, cli.scheme, opts.word_bonus, opts.k, opts.raw_penalty
    );

    use retype_pinyin::{normalize, Decoder};
    let out = match cli.scheme {
        PinyinScheme::Full => Decoder::with_options(opts.clone()).decode(input, dict.as_ref()),
        PinyinScheme::Flypy => retype_pinyin::shuangpin::decode(input, dict.as_ref(), &opts),
    };
    println!("音节切分(首选): {}", out.syllables.join("'"));
    println!(
        "已匹配音节数: {}  含原样字母: {}",
        out.matched_syllables, out.has_raw
    );
    // 表头必须和数据行用同一套列宽。绑成变量而不是直接写字面量，
    // 否则 clippy::print_literal 会要求把它们内联进格式串，列宽就对不齐了。
    let (h_idx, h_text, h_syl, h_score, h_src) = ("#", "候选", "音节", "得分", "来源");
    println!(
        "{:>3}  {:<20} {:<12} {:>10}  {}",
        h_idx, h_text, h_syl, h_score, h_src
    );
    for (i, c) in out.candidates.iter().enumerate() {
        println!(
            "{:>3}  {:<20} {:<12} {:>10.3}  {:?} 消费{}字符/{}音节",
            i + 1,
            c.text,
            c.comment,
            c.score,
            c.source,
            c.consumed,
            c.syllable_len
        );
    }
    if cli.scheme == PinyinScheme::Full {
        println!("\n切分歧义（前 12 种）:");
        for s in retype_pinyin::all_segmentations(&normalize(input), 12) {
            println!("  {}", s.join(" + "));
        }
    }

    // 单音节对照：直接看词库里这个音节下谁的分最高
    println!("\n单音节对照（词库里该音节的原始 logp）:");
    use retype_pinyin::Lexicon;
    for syl in out.syllables.iter().take(4) {
        let Some(id) = retype_pinyin::syllables::id_of(syl) else {
            continue;
        };
        let mut hits = Vec::new();
        dict.lookup(&[id], &mut hits);
        hits.sort_by(|a, b| {
            b.logp
                .partial_cmp(&a.logp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top: Vec<String> = hits
            .iter()
            .take(6)
            .map(|h| format!("{}({:.2})", h.text, h.logp))
            .collect();
        println!("  {syl:<6} → {}", top.join("  "));
    }

    if cli.scheme == PinyinScheme::Full {
        // 逐边分解：相同的候选文字可能来自完全不同的切分，
        // 只有拆开看每一步的 logp 才能解释排序
        println!("\n逐边分解（top {} 路径）:", out.candidates.len().min(6));
        for t in retype_pinyin::trace(input, dict.as_ref(), &opts)
            .iter()
            .take(6)
        {
            println!("  总分 {:>9.3}  {}", t.score, t.text);
            for s in &t.steps {
                let syl = if s.syllables.is_empty() {
                    "-".to_string()
                } else {
                    s.syllables.join("+")
                };
                let tag = if s.raw { " [原样字母]" } else { "" };
                println!(
                    "      {:<8} {:<16} logp={:>8.3}  step={:>8.3}{}{}",
                    s.text,
                    syl,
                    s.logp,
                    s.score,
                    if s.raw { "" } else { "  (-wp)" },
                    tag
                );
            }
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n");
            usage();
            std::process::exit(2);
        }
    };

    if let Some(input) = &cli.explain {
        explain(&cli, input);
        return;
    }

    if cli.bench {
        bench(&cli);
        return;
    }

    let app = App::new(&cli);
    app.stat();
    repl(&app);
}
