//! `retype-updater.exe` —— 独立的更新检查 / 下载工具。
//!
//! ## 为什么是一个独立的 exe
//!
//! TIP DLL 被注入到**每一个**宿主进程。在里面发 HTTP 请求意味着：
//! Chrome、Word、记事本各自替我们打一遍网络流量，各自持有一条连接，
//! 而且更新检查一旦发生在线程上就违反 P1。所以更新永远是进程外的事，
//! 由设置界面（Flutter）或计划任务调起这个 exe。
//!
//! ## 它做什么、不做什么
//!
//! - ✅ `check`：查 GitHub `releases/latest`，比对版本，给出 release 页与产物信息
//! - ✅ `download`：下载产物 + 其 sha256 旁文件，**校验通过后才落盘**
//! - ❌ 不解压、不替换 DLL、不改注册表
//!
//! 最后一条是刻意的：TIP 正被所有进程加载着，直接覆盖会失败或造成半更新状态。
//! 原子替换（改名 + 重启后清理，或走 MSI）属于 M5 的安装器，见 `docs/auto-update.md`。
//!
//! ## 退出码（给脚本/计划任务用）
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 已是最新（或下载+校验成功） |
//! | 1 | 参数错误 |
//! | 2 | 网络 / GitHub API 错误 |
//! | 3 | 有新版但没有可安装的产物（缺 zip 或缺 sha256） |
//! | 4 | sha256 校验失败（产物已删除，绝不安装） |
//! | 10 | `check`：发现有可用更新 |

use retype_updater::{
    expected_for, resolve_repo, verify_sha256, Channel, HttpFetcher, Platform, Response,
    UpdateChecker, UpdateError, UpdateStatus,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 与参考实现（torto-app）一致的 15s：更新检查通常发生在设置界面打开时，
/// 卡住会让整个界面失去响应。
const DEFAULT_TIMEOUT_SECS: u64 = 15;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 1;
const EXIT_NETWORK: i32 = 2;
const EXIT_NO_ASSET: i32 = 3;
const EXIT_CHECKSUM: i32 = 4;
const EXIT_UPDATE_AVAILABLE: i32 = 10;

// ─────────────────────────── 真实 HTTP 实现 ───────────────────────────

struct UreqFetcher {
    agent: ureq::Agent,
    token: Option<String>,
}

impl UreqFetcher {
    fn new(timeout: Duration, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            // 4xx/5xx 也当成正常响应返回，由上层按状态码分支处理
            // （404 = 还没发布过任何 release，是正常状态而不是错误）
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { agent, token }
    }
}

impl HttpFetcher for UreqFetcher {
    fn get(&self, url: &str) -> Result<Response, UpdateError> {
        let mut req = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(
                "User-Agent",
                concat!("retype-updater/", env!("CARGO_PKG_VERSION")),
            );
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        match req.call() {
            Ok(resp) => {
                // ureq 3 返回 http::StatusCode，我们的 Response 只存 u16
                let status = resp.status().as_u16();
                let body = resp
                    .into_body()
                    .read_to_vec()
                    .map_err(|e| UpdateError::Transport(format!("读取响应失败: {e}")))?;
                Ok(Response { status, body })
            }
            Err(e) => Err(UpdateError::Transport(e.to_string())),
        }
    }
}

// ─────────────────────────── CLI ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Check,
    Download,
    /// 离线校验一个已下载的包。
    /// 存在的理由不只是给用户用：它让「package.ps1 生成的 .sha256 格式」
    /// 和「core/updater 的解析器」之间的约定可以被真正跑一遍验证，
    /// 而不是只靠两边各自读同一份文档。
    Verify,
}

struct Args {
    cmd: Command,
    repo: Option<String>,
    current: String,
    platform: Platform,
    channel: Channel,
    timeout: Duration,
    token: Option<String>,
    out: PathBuf,
    json: bool,
    file: Option<PathBuf>,
    sum: Option<PathBuf>,
}

fn usage() -> String {
    format!(
        "\
retype-updater {} —— 检查 / 下载更新

用法:
  retype-updater check    [选项]
  retype-updater download [选项] --out <目录>
  retype-updater verify   --file <产物> [--sum <sha256文件>]

选项:
  --repo <owner/name>   GitHub 仓库。优先级：本参数 > 环境变量 RETYPE_GITHUB_REPO
                        > 编译期烘入值（CI 用 github.repository 注入）
  --current <ver>       当前版本，默认取本 exe 的编译版本 ({})
  --platform <p>        windows-x64（默认）/ windows-x86 / android-arm64 / ...
  --channel <c>         stable（默认，跳过预发布）/ beta
  --timeout <秒>        单次请求超时，默认 {DEFAULT_TIMEOUT_SECS}
  --token <t>           GitHub token，用于提高 API 限额；也可用环境变量 GITHUB_TOKEN
  --out <目录>          download 的落盘目录
  --file <路径>         verify 的待校验产物
  --sum <路径>          verify 的 sha256 文件，默认 <产物>.sha256
  --json                以 JSON 输出（给设置界面调用）

退出码: 0 已最新/成功/校验通过 · 1 参数错误 · 2 网络错误 · 3 无可安装产物 ·
        4 校验失败 · 10 有可用更新
",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_VERSION")
    )
}

fn parse_platform(s: &str) -> Option<Platform> {
    Some(match s {
        "windows-x64" => Platform::WindowsX64,
        "windows-x86" => Platform::WindowsX86,
        "android-arm64" => Platform::AndroidArm64,
        "android-arm32" => Platform::AndroidArm32,
        "android-x64" => Platform::AndroidX64,
        _ => return None,
    })
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut cmd = None;
    let mut a = Args {
        cmd: Command::Check,
        repo: None,
        current: env!("CARGO_PKG_VERSION").to_owned(),
        platform: Platform::current().unwrap_or(Platform::WindowsX64),
        channel: Channel::Stable,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        token: None,
        out: PathBuf::from("."),
        json: false,
        file: None,
        sum: None,
    };

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
            "-h" | "--help" => return Err(String::new()),
            "check" => cmd = Some(Command::Check),
            "download" => cmd = Some(Command::Download),
            "verify" => cmd = Some(Command::Verify),
            "--repo" => a.repo = Some(val(&mut i)?),
            "--current" => a.current = val(&mut i)?,
            "--platform" => {
                a.platform = parse_platform(&val(&mut i)?)
                    .ok_or_else(|| "--platform 取值非法（见 --help）".to_string())?
            }
            "--channel" => match val(&mut i)?.as_str() {
                "stable" => a.channel = Channel::Stable,
                "beta" => a.channel = Channel::Beta,
                other => return Err(format!("--channel 取值非法: {other}")),
            },
            "--timeout" => {
                let secs: u64 = val(&mut i)?
                    .parse()
                    .map_err(|_| "--timeout 需要整数秒".to_string())?;
                a.timeout = Duration::from_secs(secs.clamp(1, 300));
            }
            "--token" => a.token = Some(val(&mut i)?),
            "--out" => a.out = PathBuf::from(val(&mut i)?),
            "--file" => a.file = Some(PathBuf::from(val(&mut i)?)),
            "--sum" => a.sum = Some(PathBuf::from(val(&mut i)?)),
            "--json" => a.json = true,
            other => return Err(format!("未知参数: {other}")),
        }
        i += 1;
    }

    let Some(cmd) = cmd else {
        return Err("需要指定子命令：check 或 download".into());
    };
    a.cmd = cmd;
    if a.cmd == Command::Download && a.out.as_os_str().is_empty() {
        return Err("download 需要 --out <目录>".into());
    }
    if a.cmd == Command::Verify {
        let Some(f) = a.file.clone() else {
            return Err("verify 需要 --file <产物路径>".into());
        };
        if a.sum.is_none() {
            // 约定：校验文件就叫 <产物>.sha256（package.ps1 就是这么生成的）
            let mut s = f.clone().into_os_string();
            s.push(".sha256");
            a.sum = Some(PathBuf::from(s));
        }
    }
    if a.token.is_none() {
        a.token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());
    }
    Ok(a)
}

fn status_to_json(s: &UpdateStatus, platform: Platform) -> String {
    let latest = s.latest.as_ref().map(|r| {
        serde_json::json!({
            "tag": r.tag,
            "version": r.version.to_string(),
            "published_at": r.published_at,
            "prerelease": r.prerelease,
            "asset_count": r.assets.len(),
        })
    });
    serde_json::json!({
        "current": s.current.to_string(),
        "update_available": s.update_available,
        "installable": s.is_installable(),
        "platform": platform.asset_suffix(),
        "release_page": s.release_page,
        "latest": latest,
        "asset": s.asset.as_ref().map(|a| serde_json::json!({
            "name": a.name, "url": a.url, "size": a.size,
        })),
        "checksum_asset": s.checksum_asset.as_ref().map(|a| serde_json::json!({
            "name": a.name, "url": a.url, "size": a.size,
        })),
    })
    .to_string()
}

/// 下载产物 + 其 sha256 旁文件，**校验通过后才落盘**。
///
/// 顺序是刻意的：先拿校验值再拿产物，这样任何一个环节失败都不会在磁盘上
/// 留下一个「看起来像是对的」的文件。
fn download(a: &Args, fetcher: &UreqFetcher, s: &UpdateStatus) -> Result<PathBuf, (i32, String)> {
    let (Some(asset), Some(checksum)) = (&s.asset, &s.checksum_asset) else {
        return Err((
            EXIT_NO_ASSET,
            "这个 release 没有当前平台的产物或缺少 .sha256 校验文件，拒绝安装".into(),
        ));
    };

    let sum_resp = fetcher
        .get(&checksum.url)
        .map_err(|e| (EXIT_NETWORK, format!("下载校验文件失败: {e}")))?;
    if !(200..300).contains(&sum_resp.status) {
        return Err((
            EXIT_NETWORK,
            format!("校验文件返回 HTTP {}", sum_resp.status),
        ));
    }
    let sum_text = String::from_utf8_lossy(&sum_resp.body).into_owned();
    let expected = expected_for(&sum_text, &asset.name).ok_or_else(|| {
        (
            EXIT_NO_ASSET,
            format!("校验文件里没有 {} 对应的条目", asset.name),
        )
    })?;

    let resp = fetcher
        .get(&asset.url)
        .map_err(|e| (EXIT_NETWORK, format!("下载产物失败: {e}")))?;
    if !(200..300).contains(&resp.status) {
        return Err((EXIT_NETWORK, format!("产物返回 HTTP {}", resp.status)));
    }

    // 先校验，后落盘：磁盘上永远不会出现未经验证的产物
    if let Err(e) = verify_sha256(&resp.body, &expected) {
        let msg = match e {
            UpdateError::ChecksumMismatch { expected, actual } => {
                format!("sha256 不匹配！期望 {expected}，实际 {actual}。产物已丢弃，未写入磁盘。")
            }
            other => format!("校验失败: {other}"),
        };
        return Err((EXIT_CHECKSUM, msg));
    }

    std::fs::create_dir_all(&a.out).map_err(|e| (EXIT_USAGE, format!("建不出目录: {e}")))?;
    let dest = a.out.join(&asset.name);
    std::fs::write(&dest, &resp.body).map_err(|e| (EXIT_NETWORK, format!("写文件失败: {e}")))?;
    std::fs::write(
        a.out.join(format!("{}.sha256", asset.name)),
        sum_text.as_bytes(),
    )
    .map_err(|e| (EXIT_NETWORK, format!("写校验文件失败: {e}")))?;

    if !a.json {
        println!("sha256 校验通过: {expected}");
        println!("已下载到: {}", dest.display());
        println!("大小: {} 字节", resp.body.len());
        println!();
        println!("下一步：静默运行安装器完成升级（Restart Manager 会处理 DLL 被占用）：");
        println!(
            "  & \"{}\" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART",
            dest.display()
        );
        println!("/NORESTART 是刻意的：由你决定何时重启，而不是让安装器在用户打字时重启机器。");
        println!("详见 docs/auto-update.md §4。");
    }
    Ok(dest)
}

/// 离线校验一个已下载的产物。不发任何网络请求。
fn run_verify(a: &Args) -> i32 {
    let (Some(file), Some(sum)) = (&a.file, &a.sum) else {
        eprintln!("verify 需要 --file（--sum 可选，默认 <产物>.sha256）");
        return EXIT_USAGE;
    };
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("读不了 {}: {e}", file.display());
            return EXIT_USAGE;
        }
    };
    let sum_text = match std::fs::read_to_string(sum) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("读不了校验文件 {}: {e}", sum.display());
            return EXIT_USAGE;
        }
    };
    let name = file
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(expected) = expected_for(&sum_text, &name) else {
        eprintln!("校验文件里没有 {name} 对应的条目");
        return EXIT_NO_ASSET;
    };
    match verify_sha256(&bytes, &expected) {
        Ok(()) => {
            if a.json {
                println!(
                    "{}",
                    serde_json::json!({ "file": name, "sha256": expected, "size": bytes.len(), "ok": true })
                );
            } else {
                println!("校验通过: {name}");
                println!("  sha256 = {expected}");
                println!("  大小   = {} 字节", bytes.len());
            }
            EXIT_OK
        }
        Err(e) => {
            if a.json {
                println!(
                    "{}",
                    serde_json::json!({ "file": name, "ok": false, "error": e.to_string() })
                );
            } else {
                // UpdateError 的 Display 已经带了「校验失败:」前缀，这里不要重复
                eprintln!("{e}");
                eprintln!("不要安装这个产物。");
            }
            EXIT_CHECKSUM
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            if msg.is_empty() {
                print!("{}", usage());
                std::process::exit(EXIT_OK);
            }
            eprintln!("{msg}\n");
            eprint!("{}", usage());
            std::process::exit(EXIT_USAGE);
        }
    };

    // verify 是纯离线操作：不需要仓库地址，也不该发任何网络请求
    if a.cmd == Command::Verify {
        std::process::exit(run_verify(&a));
    }

    let Some(repo) = resolve_repo(a.repo.as_deref()) else {
        eprintln!(
            "错误: 仓库未配置。请用 --repo owner/name，或设置环境变量 RETYPE_GITHUB_REPO。\n\
             （CI 发布的 exe 会把仓库烘进二进制，开发构建需要手动指定）"
        );
        std::process::exit(EXIT_USAGE);
    };

    let fetcher = Arc::new(UreqFetcher::new(a.timeout, a.token.clone()));
    let checker = UpdateChecker::new(repo.clone(), Arc::clone(&fetcher) as Arc<dyn HttpFetcher>)
        .with_channel(a.channel);

    let status = match checker.check(&a.current, a.platform) {
        Ok(s) => s,
        Err(e) => {
            // 更新检查失败绝不能被当成致命错误：断网时用户照样要能打字
            if a.json {
                println!(
                    "{}",
                    serde_json::json!({ "error": e.to_string(), "current": a.current })
                );
            } else {
                eprintln!("更新检查失败（不影响输入法使用）: {e}");
            }
            std::process::exit(EXIT_NETWORK);
        }
    };

    match a.cmd {
        // verify 已在网络流程之前处理并退出，这里只为穷尽匹配
        Command::Verify => std::process::exit(EXIT_OK),
        Command::Check => {
            if a.json {
                println!("{}", status_to_json(&status, a.platform));
            } else {
                println!("仓库      : {repo}");
                println!("当前版本  : {}", status.current);
                match &status.latest {
                    Some(r) => {
                        println!("最新版本  : {} ({})", r.version, r.tag);
                        if let Some(p) = &r.published_at {
                            println!("发布时间  : {p}");
                        }
                        println!("发布页    : {}", r.html_url);
                    }
                    None => println!("最新版本  : (仓库还没有任何 release)"),
                }
                println!(
                    "结论      : {}",
                    if status.update_available {
                        "有可用更新"
                    } else {
                        "已是最新"
                    }
                );
                if status.update_available {
                    println!(
                        "可安装    : {}{}",
                        status.is_installable(),
                        if status.is_installable() {
                            String::new()
                        } else {
                            "（缺产物或缺 sha256，只能引导用户去发布页）".to_string()
                        }
                    );
                }
            }
            std::process::exit(if status.update_available {
                EXIT_UPDATE_AVAILABLE
            } else {
                EXIT_OK
            });
        }
        Command::Download => {
            if !status.update_available {
                if a.json {
                    println!("{}", status_to_json(&status, a.platform));
                } else {
                    println!("已是最新（{}），无需下载", status.current);
                }
                std::process::exit(EXIT_OK);
            }
            match download(&a, &fetcher, &status) {
                Ok(path) => {
                    if a.json {
                        let status_json: serde_json::Value =
                            serde_json::from_str(&status_to_json(&status, a.platform))
                                .unwrap_or(serde_json::Value::Null);
                        println!(
                            "{}",
                            serde_json::json!({
                                "downloaded": path.to_string_lossy(),
                                "status": status_json,
                            })
                        );
                    }
                    std::process::exit(EXIT_OK);
                }
                Err((code, msg)) => {
                    if a.json {
                        println!("{}", serde_json::json!({ "error": msg }));
                    } else {
                        eprintln!("{msg}");
                    }
                    std::process::exit(code);
                }
            }
        }
    }
}
