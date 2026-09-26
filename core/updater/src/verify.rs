//! 产物完整性校验。
//!
//! 自动更新等于「从网上下载一个会被注入到每个进程的 DLL」，
//! 所以校验不是可选项：**没有通过 sha256 校验的产物一律不许落盘安装**。
//! 校验文件必须和产物成对发布（见 `Release::checksum_asset`）。

use crate::UpdateError;
use sha2::{Digest, Sha256};

/// 计算 sha256，返回小写十六进制。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        // 手写十六进制，避免引入 hex crate
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

/// 校验字节内容的 sha256。不匹配返回 `ChecksumMismatch`（带上两个值，便于诊断）。
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), UpdateError> {
    let actual = sha256_hex(bytes);
    let expected = expected_hex.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(UpdateError::Malformed(format!(
            "sha256 值格式非法（应为 64 位十六进制）: {expected_hex:?}"
        )));
    }
    if actual == expected {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch { expected, actual })
    }
}

/// 解析 `sha256sum` 格式（coreutils 兼容）：`<hex><空格><空格或*>文件名`，每行一条。
///
/// 返回 `(hex, 文件名)`。文件名里的路径前缀会被剥掉，只留基名 ——
/// CI 里生成时可能带 `./`，比对时不该因此失配。
pub fn parse_sha256sum(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // PowerShell 的 `Set-Content -Encoding UTF8` 会写 BOM，而 U+FEFF 在 Rust 里
    // 不算空白字符，`trim()` 去不掉 —— 不特殊处理的话第一行的 hash 会多一个字符，
    // 长度校验失败，用户会看到「校验文件里没有对应条目」这种误导性的报错。
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // 二进制模式标记 `*` 紧跟在 hash 之后
        let (hex, rest) = match line.split_once(' ') {
            Some((h, r)) => (h, r.trim_start_matches('*').trim()),
            None => continue,
        };
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let base = rest.rsplit(['/', '\\']).next().unwrap_or(rest);
        if !base.is_empty() {
            out.push((hex.to_ascii_lowercase(), base.to_owned()));
        }
    }
    out
}

/// 从 sha256sum 文本里取出指定文件名的期望哈希。
pub fn expected_for(text: &str, filename: &str) -> Option<String> {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    parse_sha256sum(text)
        .into_iter()
        .find(|(_, name)| name == base)
        .map(|(hex, _)| hex)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// NIST 标准测试向量，用来确认我们没把 sha256 算成别的什么
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn matches_known_vectors() {
        assert_eq!(sha256_hex(b"abc"), ABC);
        assert_eq!(sha256_hex(b""), EMPTY);
    }

    #[test]
    fn verify_accepts_correct_hash_and_rejects_tampering() {
        assert!(verify_sha256(b"abc", ABC).is_ok());
        assert!(
            verify_sha256(b"abc", &ABC.to_uppercase()).is_ok(),
            "大小写不敏感"
        );
        assert!(verify_sha256(b"abd", ABC).is_err(), "内容被改过必须失败");
        let e = verify_sha256(b"abd", ABC).unwrap_err();
        assert!(matches!(e, UpdateError::ChecksumMismatch { .. }));
    }

    #[test]
    fn verify_rejects_malformed_expected_value() {
        assert!(verify_sha256(b"abc", "deadbeef").is_err());
        assert!(verify_sha256(b"abc", "").is_err());
        // 64 位但含非十六进制字符
        assert!(verify_sha256(b"abc", &"z".repeat(64)).is_err());
    }

    #[test]
    fn parses_coreutils_format() {
        let text = format!(
            "{ABC}  retype-0.1.0-windows-x64-setup.exe\n{EMPTY} *other.bin\n# 注释\n\nbadline\n"
        );
        let parsed = parse_sha256sum(&text);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].1, "retype-0.1.0-windows-x64-setup.exe");
        assert_eq!(parsed[1].0, EMPTY, "二进制模式的 * 标记应被剥掉");
    }

    #[test]
    fn strips_path_prefixes_when_looking_up() {
        // coreutils 生成的是 "<hash>  <path>"，path 可能带 ./dist/ 前缀
        let text = format!("{ABC}  ./dist/retype-0.1.0-windows-x64-setup.exe\n");
        assert_eq!(
            expected_for(&text, "retype-0.1.0-windows-x64-setup.exe").as_deref(),
            Some(ABC)
        );
        assert_eq!(
            expected_for(&text, "C:\\out\\retype-0.1.0-windows-x64-setup.exe").as_deref(),
            Some(ABC),
            "Windows 反斜杠路径也要能匹配"
        );
        assert!(expected_for(&text, "nope.exe").is_none());
    }

    #[test]
    fn missing_entry_is_none_not_a_panic() {
        assert!(expected_for("", "x.exe").is_none());
        assert!(expected_for("garbage", "x.exe").is_none());
    }

    /// PowerShell 的 `Set-Content -Encoding UTF8` 会写 BOM。用户用这种「最直觉的方式」
    /// 生成校验文件时不该被拒收，更不该报「没有对应条目」这种误导性错误。
    #[test]
    fn tolerates_utf8_bom() {
        let with_bom = format!("\u{feff}{ABC}  retype-0.1.0-windows-x64-setup.exe\n");
        assert_eq!(
            expected_for(&with_bom, "retype-0.1.0-windows-x64-setup.exe").as_deref(),
            Some(ABC)
        );
        // CRLF 也要能吃（Windows 上的文本文件几乎都是 CRLF）
        let crlf = format!("{ABC}  a.exe\r\n{EMPTY}  b.exe\r\n");
        let parsed = parse_sha256sum(&crlf);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].1, "b.exe", "文件名末尾不该残留 \\r");
    }
}
