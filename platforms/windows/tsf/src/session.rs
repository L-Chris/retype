//! 一次激活（activate）期间的运行时装配。
//!
//! **词库必须异步加载**：`ActivateEx` 跑在宿主应用的 UI 线程上，
//! 在那里同步读几百 MB 的词库等于让用户的记事本卡住几秒（P1）。
//! 所以这里先装一个空的 [`AsyncDict`]，后台线程加载完再热替换。
//! 加载完成前用户敲字会得到原样字母 —— 这是 §7 降级矩阵里最低的一档，
//! 但键盘始终是活的。

use retype_dict::{
    spawn_loader, AsyncDict, FallbackPolicy, LayeredDict, UserDict, DEFAULT_USER_BOOST,
};
use retype_engine::{
    offline_cloud, BackendOptions, Kernel, KernelBackend, KernelConfig, LocalBackend,
};
use retype_pinyin::Lexicon;
use retype_types::{InputEvent, KernelAction, LearningStore};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::OnceLock;
use std::time::Duration;

/// 词库文件位置的环境变量覆盖（开发/诊断时最常用）。
pub const ENV_DICT: &str = "RETYPE_DICT";
/// 影子模式：把按键喂给内核但**不吃掉**它们，用于在真实宿主进程里观察内核行为。
///
/// M0 默认关闭。打开后仍然不会有任何文字被输入法接管（`OnKeyDown` 一律返回
/// `eaten = false`），所以即使配错了也不会丢用户的按键。
pub const ENV_SHADOW: &str = "RETYPE_TSF_SHADOW";

/// 本 DLL 自己所在的目录。
///
/// **TIP 的当前工作目录是宿主进程的**（notepad、chrome……），绝不是我们的安装目录，
/// 所以任何相对路径都不可信。安装器把词库放在 `{app}\retype-dict.tsv`，
/// 也就是和 `retype_ime.dll` 同目录，因此「DLL 自己在哪」是唯一可靠的锚点。
///
/// 用 `GetModuleHandleExW(FROM_ADDRESS, &本函数)` 拿到**我们自己**的模块句柄
/// ——`GetModuleHandleW(None)` 拿到的是宿主 exe，那是错的。
pub fn dll_dir() -> Option<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HMODULE, MAX_PATH};
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    };

    let mut module = HMODULE::default();
    // FROM_ADDRESS 模式下第二个参数不是字符串，而是「模块内任意地址」被当成 PCWSTR 传入。
    // 用本函数的地址，它一定落在我们自己的代码段里。
    //
    // 注意：FROM_ADDRESS 不带 UNCHANGED_REFCOUNT 时会给模块加一次引用计数且我们不释放。
    // 这是刻意的 —— TIP 本来就该在宿主进程生命周期内常驻（`DllCanUnloadNow` 恒返回 S_FALSE）。
    let addr = dll_dir as *const () as *const u16;
    // SAFETY: addr 指向本模块内的有效代码；module 是本函数栈上的有效出参。
    let ok = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            PCWSTR(addr),
            &mut module,
        )
    };
    if ok.is_err() {
        return None;
    }

    let mut buf = [0u16; MAX_PATH as usize];
    // SAFETY: buf 是 MAX_PATH 长的有效缓冲区；module 来自上一步成功的调用。
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    // len == 0 表示失败；len >= buf.len() 表示路径被截断，两者都不可信
    if len == 0 || len >= buf.len() {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len]);
    PathBuf::from(path).parent().map(|p| p.to_path_buf())
}

/// 词库查找顺序：环境变量 → **本 DLL 同目录** → `%LOCALAPPDATA%\retype\`。
///
/// 第二项是安装器产物的位置；第三项是给手动部署（`register.ps1`）留的退路。
pub fn default_dict_path() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_DICT) {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(dir) = dll_dir() {
        // x86 DLL resides in an architecture subdirectory; dictionaries are shared.
        for base in std::iter::once(dir.as_path()).chain(
            dir.parent()
                .filter(|_| dir.file_name().is_some_and(|name| name == "x86")),
        ) {
            for name in ["retype-dict.bin", "retype-dict.tsv"] {
                let candidate = base.join(name);
                if candidate.exists() {
                    return candidate;
                }
            }
        }
    }
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if !base.trim().is_empty() {
            let root = PathBuf::from(base).join("retype");
            let binary = root.join("retype-dict.bin");
            return if binary.exists() {
                binary
            } else {
                root.join("retype-dict.tsv")
            };
        }
    }
    PathBuf::from("retype-dict.tsv")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

pub struct Session {
    pub backend: Arc<LocalBackend>,
    pub dict: Arc<AsyncDict>,
    pub packs: Arc<AsyncDict>,
    pack_generation: AtomicU32,
    pub user: Arc<UserDict>,
    /// 影子模式：喂内核但不吃按键
    pub shadow: bool,
    pub dict_path: PathBuf,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("dict_path", &self.dict_path)
            .field("dict_loaded", &self.dict.is_loaded())
            .field("shadow", &self.shadow)
            .finish()
    }
}

impl Session {
    /// 装配一次会话。所有耗时工作都在后台线程，本函数必须很快返回。
    pub fn start() -> Arc<Self> {
        Self::start_with(default_dict_path())
    }

    pub fn start_with(dict_path: PathBuf) -> Arc<Self> {
        let dict = AsyncDict::empty();
        let packs = AsyncDict::empty();
        // 词库不存在/损坏 → 单字模式兜底（§7）
        let _ = spawn_loader(&dict_path, Arc::clone(&dict), FallbackPolicy::SingleChar);

        let mut base = LayeredDict::new();
        base.push(retype_dict::Layer {
            name: "system",
            dict: Arc::clone(&dict) as Arc<dyn Lexicon>,
            boost: 0.0,
        });
        base.push(retype_dict::Layer {
            name: "optional",
            dict: Arc::clone(&packs) as Arc<dyn Lexicon>,
            boost: 0.0,
        });
        let sys: Arc<dyn Lexicon> = Arc::new(base);
        let (user, learner) = shared_learning(Arc::clone(&sys));
        let layers = LayeredDict::with_system_and_user(sys, Arc::clone(&user), DEFAULT_USER_BOOST);
        let layered: Arc<dyn Lexicon> = Arc::new(layers);

        // M0/M1：不配云端。二刷要等 M3 接了真实供应商才有意义，
        // 在那之前每次按键都发一个必然失败的请求只会白白消耗宿主进程的资源。
        let cloud = offline_cloud(Duration::from_millis(200));
        let kernel = Kernel::new(
            KernelConfig {
                pinyin_scheme: crate::preferences::scheme(),
                decode: retype_pinyin::DecodeOptions {
                    page_size: 8,
                    ..Default::default()
                },
                rerank_enabled: false,
                ..Default::default()
            },
            layered,
            learner,
            Arc::clone(&cloud),
        );
        let backend = LocalBackend::with_options(kernel, cloud, BackendOptions::default());

        let session = Arc::new(Self {
            backend,
            dict,
            packs,
            pack_generation: AtomicU32::new(u32::MAX),
            user,
            shadow: env_flag(ENV_SHADOW),
            dict_path,
        });
        session.sync_packs();
        session
    }

    /// Refresh on focus only; loading and parsing always run on a worker thread.
    pub fn sync_packs(self: &Arc<Self>) {
        let generation = crate::preferences::pack_generation();
        if self.pack_generation.swap(generation, Ordering::SeqCst) == generation {
            return;
        }
        let Some(root) = crate::preferences::pack_root() else {
            self.packs.uninstall();
            return;
        };
        let enabled = crate::preferences::enabled_packs();
        let session = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("retype-pack-load".into())
            .spawn(move || {
                // Base loading starts first, and supplies the shared score denominator.
                let deadline = std::time::Instant::now() + Duration::from_secs(30);
                while !session.dict.is_loaded() && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !session.dict.is_loaded() {
                    tracing::warn!(
                        "base dictionary did not finish loading; optional packs skipped"
                    );
                    return;
                }
                let merged =
                    crate::packs::load_enabled(&root, enabled, session.dict.total_frequency());
                if session.pack_generation.load(Ordering::SeqCst) == generation {
                    session.packs.install(Arc::new(merged));
                }
            });
    }

    pub fn submit(&self, ev: InputEvent) -> Vec<KernelAction> {
        self.drain_actions(self.backend.submit(ev))
    }

    pub(crate) fn submit_in_context(
        &self,
        ev: InputEvent,
        allow_learning: bool,
    ) -> Vec<KernelAction> {
        self.drain_actions(self.backend.submit_deferred_learning(ev, allow_learning))
    }

    fn drain_actions(&self, mut acts: Vec<KernelAction>) -> Vec<KernelAction> {
        // 顺带收干异步队列：M1 会把这一步交给候选窗 UI 线程的消息循环
        while let Some(a) = self.backend.poll_action() {
            acts.push(a);
        }
        acts
    }

    /// 词库是否已就绪。用于决定是否显示「降级」状态。
    pub fn dict_ready(&self) -> bool {
        self.dict.is_loaded()
    }
}

fn shared_learning(system: Arc<dyn Lexicon>) -> (Arc<UserDict>, Arc<dyn LearningStore>) {
    // Unit tests must never start a production broker or change the developer's learned words.
    #[cfg(test)]
    {
        let user = Arc::new(UserDict::new());
        let learner = Arc::new(retype_dict::Learner::with_system(Arc::clone(&user), system));
        (user, learner)
    }
    #[cfg(not(test))]
    {
        static CLIENT: OnceLock<Arc<retype_learning::client::Client>> = OnceLock::new();
        let client = CLIENT.get_or_init(|| {
            let host = dll_dir().map(|directory| {
                let base = if directory.file_name().is_some_and(|name| name == "x86") {
                    directory
                        .parent()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or(directory)
                } else {
                    directory
                };
                base.join("retype-learning-host.exe")
            });
            retype_learning::client::Client::start(system, host)
        });
        (
            Arc::clone(&client.user),
            Arc::clone(client) as Arc<dyn LearningStore>,
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn session_starts_fast_without_a_dict_file() {
        // 词库文件不存在是常态（用户还没构建），会话必须照样起来
        let started = std::time::Instant::now();
        let s = Session::start_with(PathBuf::from("Z:/nope/retype-dict.tsv"));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "会话装配不能在宿主 UI 线程上做重活，实际 {:?}",
            started.elapsed()
        );
        assert!(!s.dict_ready(), "文件不存在时不该立刻就绪");
        assert_eq!(s.backend.with_kernel(|k| k.render_state().page_size), 8);
    }

    #[test]
    fn session_falls_back_to_single_char_when_dict_missing() {
        let s = Session::start_with(PathBuf::from("Z:/nope/retype-dict.tsv"));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline && !s.dict_ready() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(s.dict_ready(), "加载失败也必须装上单字兜底词库");
        assert!(s.dict.len() > 10000);
    }

    #[test]
    fn env_dict_path_wins() {
        // 只验证解析逻辑，不改进程环境（避免测试间互相干扰）
        let p = default_dict_path();
        assert!(
            p.file_name()
                .is_some_and(|name| name == "retype-dict.tsv" || name == "retype-dict.bin"),
            "默认路径应指向 retype-dict.tsv 或 retype-dict.bin，实际 {p:?}"
        );
    }

    #[test]
    fn dll_dir_resolves_to_a_real_directory() {
        // 在测试里这个函数位于 exe 而不是 DLL，但 FROM_ADDRESS 的语义一样：
        // 拿到「包含这段代码的模块」所在目录。
        let dir = dll_dir().expect("应能解析出本模块所在目录");
        assert!(
            dir.is_absolute(),
            "必须是绝对路径，否则宿主的工作目录会污染它: {dir:?}"
        );
        assert!(dir.exists(), "目录应真实存在: {dir:?}");
    }

    #[test]
    fn default_dict_path_is_absolute_or_env_driven() {
        // 关键不变量：绝不能返回一个相对路径。
        // TIP 的工作目录是宿主进程的，相对路径会指向完全不可预期的位置。
        if std::env::var(ENV_DICT).is_err() {
            let p = default_dict_path();
            let exists_next_to_module = dll_dir()
                .map(|d| d.join("retype-dict.tsv").exists())
                .unwrap_or(false);
            if !exists_next_to_module {
                assert!(
                    p.is_absolute(),
                    "没有环境变量、模块旁边也没词库时，应退回绝对路径的 LOCALAPPDATA，实际 {p:?}"
                );
            }
        }
    }
}
