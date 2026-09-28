//! C ABI 导出层。
//!
//! **M0 只固定「跨语言边界长什么样」**，真正的会话 API 随 Android 端在 M5 设计
//! （届时用 JNI 或 uniFFI 生成 Kotlin 绑定，见 roadmap）。
//!
//! 现在就存在的理由：它逼着内核保持「不依赖任何平台类型」的纪律 ——
//! 只有纯数据 + 纯函数的内核才导得出稳定的 C ABI。
//! 如果哪天发现某个导出需要暴露 Windows 类型，那就说明分层被破坏了。
#![allow(unsafe_code)] // C ABI 边界本身就是 unsafe 的，函数内部逻辑保持 safe

use std::os::raw::{c_char, c_int};

/// 版本号，NUL 结尾的静态字符串。
#[no_mangle]
pub extern "C" fn retype_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// 音节表大小。宿主语言可以用它校验「Rust 侧和词典是不是同一版」。
#[no_mangle]
pub extern "C" fn retype_syllable_count() -> u32 {
    retype_pinyin::syllables::count() as u32
}

/// 内核自检：装配一个空词库内核，喂一串按键，确认能组字且不 panic。
///
/// 返回 0 表示通过；非 0 是失败码。
/// 空词库时不在候选窗显示原样字母，但回车仍应提交这些字母。
#[no_mangle]
pub extern "C" fn retype_kernel_selftest() -> c_int {
    let code = std::panic::catch_unwind(|| {
        use retype_engine::{Kernel, KernelConfig};
        use retype_types::{InputEvent, InputSource, KernelAction, Key, LearningStore, Modifiers};
        use std::sync::Arc;

        let user = Arc::new(retype_dict::UserDict::new());
        let learner: Arc<dyn LearningStore> =
            Arc::new(retype_dict::Learner::new(Arc::clone(&user)));
        let dict: Arc<dyn retype_pinyin::Lexicon> = Arc::new(retype_dict::MemoryDict::default());
        let cloud = retype_engine::offline_cloud(std::time::Duration::from_millis(10));
        let mut k = Kernel::new(
            KernelConfig {
                rerank_enabled: false,
                ..Default::default()
            },
            dict,
            learner,
            cloud,
        );

        let mut rendered = 0usize;
        for c in "nihaomashijie".chars() {
            for a in k.handle(InputEvent::Key {
                key: Key::Char(c),
                mods: Modifiers::NONE,
                source: InputSource::Keyboard,
            }) {
                if matches!(a, KernelAction::Render(_)) {
                    rendered += 1;
                }
            }
        }
        if rendered == 0 {
            return 1; // 一次都没渲染
        }
        if !k.candidate_texts().is_empty() {
            return 2; // 原样字母不应作为候选展示
        }
        // 上屏路径
        let acts = k.handle(InputEvent::Key {
            key: Key::Enter,
            mods: Modifiers::NONE,
            source: InputSource::Keyboard,
        });
        if !acts.iter().any(|a| matches!(a, KernelAction::Commit(_))) {
            return 3; // 回车应上屏原始字母
        }
        if k.has_composition() {
            return 4; // 上屏后组字区必须清空
        }
        0
    });
    code.unwrap_or(99) // panic 也算失败，但不许穿过 FFI 边界
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn version_string_is_nul_terminated() {
        let p = retype_version();
        assert!(!p.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(p) };
        assert_eq!(s.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn syllable_count_matches_the_table() {
        assert_eq!(
            retype_syllable_count() as usize,
            retype_pinyin::syllables::count()
        );
        assert!(retype_syllable_count() > 400);
    }

    #[test]
    fn kernel_selftest_passes() {
        assert_eq!(retype_kernel_selftest(), 0);
    }
}
