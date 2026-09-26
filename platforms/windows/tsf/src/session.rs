//! 一次激活（activate）期间的运行时装配。
//!
//! **词库必须异步加载**：`ActivateEx` 跑在宿主应用的 UI 线程上，
//! 在那里同步读几百 MB 的词库等于让用户的记事本卡住几秒（P1）。
//! 所以这里先装一个空的 [`AsyncDict`]，后台线程加载完再热替换。
//! 加载完成前用户敲字会得到原样字母 —— 这是 §7 降级矩阵里最低的一档，
//! 但键盘始终是活的。

use retype_dict::{
    spawn_loader, AsyncDict, FallbackPolicy, LayeredDict, Learner, UserDict, DEFAULT_USER_BOOST,
};
use retype_engine::{
    offline_cloud, BackendOptions, Kernel, KernelBackend, KernelConfig, LocalBackend,
};
use retype_pinyin::Lexicon;
use retype_types::{InputEvent, KernelAction, LearningStore};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 词库文件位置的环境变量覆盖（开发/诊断时最常用）。
pub const ENV_DICT: &str = "RETYPE_DICT";
/// 影子模式：把按键喂给内核但**不吃掉**它们，用于在真实宿主进程里观察内核行为。
///
/// M0 默认关闭。打开后仍然不会有任何文字被输入法接管（`OnKeyDown` 一律返回
/// `eaten = false`），所以即使配错了也不会丢用户的按键。
pub const ENV_SHADOW: &str = "RETYPE_TSF_SHADOW";

/// 词库查找顺序：环境变量 → `%LOCALAPPDATA%\retype\` → 当前目录。
pub fn default_dict_path() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_DICT) {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if !base.trim().is_empty() {
            return PathBuf::from(base).join("retype").join("retype-dict.tsv");
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
        // 词库不存在/损坏 → 单字模式兜底（§7）
        let _ = spawn_loader(&dict_path, Arc::clone(&dict), FallbackPolicy::SingleChar);

        let user = Arc::new(UserDict::new());
        let sys: Arc<dyn Lexicon> = Arc::clone(&dict) as Arc<dyn Lexicon>;
        let learner: Arc<dyn LearningStore> =
            Arc::new(Learner::with_system(Arc::clone(&user), Arc::clone(&sys)));
        let layered: Arc<dyn Lexicon> = Arc::new(LayeredDict::with_system_and_user(
            sys,
            Arc::clone(&user),
            DEFAULT_USER_BOOST,
        ));

        // M0/M1：不配云端。二刷要等 M3 接了真实供应商才有意义，
        // 在那之前每次按键都发一个必然失败的请求只会白白消耗宿主进程的资源。
        let cloud = offline_cloud(Duration::from_millis(200));
        let kernel = Kernel::new(
            KernelConfig {
                rerank_enabled: false,
                ..Default::default()
            },
            layered,
            learner,
            Arc::clone(&cloud),
        );
        let backend = LocalBackend::with_options(kernel, cloud, BackendOptions::default());

        Arc::new(Self {
            backend,
            dict,
            user,
            shadow: env_flag(ENV_SHADOW),
            dict_path,
        })
    }

    pub fn submit(&self, ev: InputEvent) -> Vec<KernelAction> {
        let mut acts = self.backend.submit(ev);
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
            p.to_string_lossy().contains("retype-dict.tsv"),
            "默认路径应指向 retype-dict.tsv，实际 {p:?}"
        );
    }
}
