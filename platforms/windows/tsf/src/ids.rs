//! 固定标识符。**一旦发布就绝不能改** —— 改了等于让用户装了两个输入法，
//! 旧的那个还留在系统里删不掉。

use windows_core::GUID;

/// TIP 的 CLSID。注册表 `HK*\SOFTWARE\Microsoft\CTF\TIP\{CLSID}` 用它做键名。
pub const CLSID_RETYPE_TIP: GUID = GUID::from_values(
    0x7E4C_9A21,
    0x5B38,
    0x4D2E,
    [0x9F, 0x6A, 0x1C, 0x0D, 0x8E, 0x7B, 0x4A, 0x52],
);
pub const CLSID_RETYPE_TIP_STR: &str = "{7E4C9A21-5B38-4D2E-9F6A-1C0D8E7B4A52}";

/// 语言配置档 GUID（`LanguageProfile` 子键）。
pub const GUID_PROFILE_RETYPE: GUID = GUID::from_values(
    0xA3F1_C6D9,
    0x2E47,
    0x4B8A,
    [0x9C, 0x51, 0x6D, 0x0E, 0x8F, 0x2A, 0x3B, 0x74],
);
pub const GUID_PROFILE_RETYPE_STR: &str = "{A3F1C6D9-2E47-4B8A-9C51-6D0E8F2A3B74}";

/// 简体中文 LCID。
pub const LANGID_ZH_CN: u16 = 0x0804;

/// 显示名称（M1 应改为资源 DLL 里的字符串 ID，才能本地化）。
pub const DISPLAY_NAME: &str = "retype 输入法";
pub const DISPLAY_DESC: &str = "retype 拼音输入法（本地首刷 + 云端二刷）";

/// 注册表键名用的 `{...}` 形式。
///
/// `windows_core::GUID` 的 `Debug` 输出**不带花括号**，而 CTF 的注册表键名带，
/// 所以统一从这里生成，避免手写字符串和二进制常量对不上。
pub fn braced(g: &GUID) -> String {
    format!("{{{g:?}}}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn guid_strings_match_the_values() {
        // 字符串形式是给注册表用的，必须和二进制形式一致，否则注册了也激活不了
        assert_eq!(braced(&CLSID_RETYPE_TIP), CLSID_RETYPE_TIP_STR);
        assert_eq!(braced(&GUID_PROFILE_RETYPE), GUID_PROFILE_RETYPE_STR);
    }

    #[test]
    fn guid_strings_parse_back() {
        let bare = &CLSID_RETYPE_TIP_STR[1..CLSID_RETYPE_TIP_STR.len() - 1];
        assert_eq!(GUID::try_from(bare).unwrap(), CLSID_RETYPE_TIP);
        let bare = &GUID_PROFILE_RETYPE_STR[1..GUID_PROFILE_RETYPE_STR.len() - 1];
        assert_eq!(GUID::try_from(bare).unwrap(), GUID_PROFILE_RETYPE);
    }
}
