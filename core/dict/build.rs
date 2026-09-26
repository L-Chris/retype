//! Keep the full pinyin database out of every host process that loads the TIP.
use pinyin::{ToPinyin, ToPinyinMulti};
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let mut primary = Vec::new();
    let mut extra = Vec::new();
    let id = |s: &str| retype_pinyin::syllables::id_of(&s.replace('ü', "v").to_ascii_lowercase());
    for cp in (0x3400..=0x9fffu32).chain(0xf900..=0xfaff) {
        let c = char::from_u32(cp).ok_or("invalid codepoint")?;
        let first = c
            .to_pinyin()
            .and_then(|p| id(p.plain()))
            .unwrap_or(u16::MAX);
        primary.extend_from_slice(&first.to_le_bytes());
        let mut seen = vec![first];
        if let Some(readings) = c.to_pinyin_multi() {
            for n in 0..readings.count() {
                if let Some(reading) = id(readings.get(n).plain()) {
                    if !seen.contains(&reading) {
                        seen.push(reading);
                        extra.extend_from_slice(&(cp as u16).to_le_bytes());
                        extra.extend_from_slice(&reading.to_le_bytes());
                    }
                }
            }
        }
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR missing")?);
    fs::write(out.join("char-primary.bin"), primary)?;
    fs::write(out.join("char-extra.bin"), extra)?;
    Ok(())
}
