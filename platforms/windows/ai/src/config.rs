use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiKind {
    #[default]
    Compatible,
    Anthropic,
    Gemini,
    Ollama,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub preset: String,
    pub name: String,
    pub kind: ApiKind,
    pub base_url: String,
    pub models: Vec<String>,
}
pub const PRESETS: &[(&str, &str, ApiKind)] = &[
    ("自定义", "", ApiKind::Compatible),
    ("OpenAI", "https://api.openai.com/v1", ApiKind::Compatible),
    (
        "DeepSeek",
        "https://api.deepseek.com/v1",
        ApiKind::Compatible,
    ),
    (
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        ApiKind::Compatible,
    ),
    (
        "硅基流动",
        "https://api.siliconflow.cn/v1",
        ApiKind::Compatible,
    ),
    ("Anthropic", "https://api.anthropic.com", ApiKind::Anthropic),
    (
        "Gemini",
        "https://generativelanguage.googleapis.com/v1beta",
        ApiKind::Gemini,
    ),
    ("Ollama", "http://localhost:11434", ApiKind::Ollama),
];
impl Default for Provider {
    fn default() -> Self {
        Self {
            id: new_id(),
            preset: "自定义".into(),
            name: "自定义".into(),
            kind: ApiKind::Compatible,
            base_url: String::new(),
            models: Vec::new(),
        }
    }
}
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub providers: Vec<Provider>,
    pub provider: String,
    pub model: String,
    pub target: String,
    pub preview: bool,
    pub instructions: String,
    pub reasoning: String,
    pub timeout_seconds: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            providers: Vec::new(),
            provider: String::new(),
            model: String::new(),
            target: "English".into(),
            preview: false,
            instructions: String::new(),
            reasoning: "default".into(),
            timeout_seconds: 60,
        }
    }
}
impl Config {
    pub fn selected(&self) -> Result<&Provider, String> {
        self.providers
            .iter()
            .find(|p| p.id == self.provider && p.models.contains(&self.model))
            .ok_or_else(|| "请先在设置 → 翻译中选择可用模型".into())
    }
    pub fn load() -> io::Result<Self> {
        Self::load_at(&path()?)
    }
    pub fn load_at(path: &std::path::Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let config: Self = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if config.version != 1 {
                    return Err(io::Error::other("不支持的 AI 配置版本"));
                }
                Ok(config)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }
    pub fn save(&self) -> io::Result<()> {
        let path = path()?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("配置目录无效"))?;
        #[cfg(windows)]
        retype_learning::transport::private_directory(
            parent,
            &retype_learning::transport::user_sid()?,
        )?;
        #[cfg(not(windows))]
        std::fs::create_dir_all(parent)?;
        let temp = path.with_extension(format!("{}.tmp", new_id()));
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&temp, bytes)?;
        // MoveFileEx atomically replaces the previous file on Windows.
        #[cfg(windows)]
        crate::secrets::replace_file(&temp, &path)?;
        #[cfg(not(windows))]
        std::fs::rename(&temp, &path)?;
        Ok(())
    }
}
fn path() -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA missing"))?,
    )
    .join("retype/ai/settings.json"))
}

/// Windows virtual key plus Ctrl/Alt/Shift/Win bit flags. A modifier-only binding is a tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortcut {
    pub vk: u16,
    pub modifiers: u8,
}
impl Shortcut {
    pub const SHIFT: Self = Self {
        vk: 0x10,
        modifiers: 0,
    };
    pub const TRANSLATE: Self = Self {
        vk: 0x30,
        modifiers: 3,
    };
    pub const DISABLED: Self = Self {
        vk: 0,
        modifiers: 0,
    };
    pub fn is_tap(self) -> bool {
        self.modifiers == 0 && matches!(self.vk, 0x10 | 0x11)
    }
    pub fn valid(self, mode: bool) -> bool {
        self == Self::DISABLED
            || (mode && self.is_tap())
            || (self.modifiers & !15 == 0
                && (1..=2).contains(&self.modifiers.count_ones())
                && (self.vk == 0x20
                    || (0x30..=0x39).contains(&self.vk)
                    || (0x41..=0x5a).contains(&self.vk)
                    || (0x70..=0x87).contains(&self.vk))
                && !((self.modifiers & 8 != 0
                    && matches!(self.vk, 0x20 | 0x4c | 0x44 | 0x45 | 0x52))
                    || (self.modifiers == 2 && self.vk == 0x73)))
    }
    pub fn label(self) -> String {
        if self.vk == 0 {
            return "未设置".into();
        }
        if self.is_tap() {
            return if self.vk == 0x10 { "Shift" } else { "Ctrl" }.into();
        }
        let mut parts = Vec::new();
        for (bit, name) in [(1, "Ctrl"), (2, "Alt"), (4, "Shift"), (8, "Win")] {
            if self.modifiers & bit != 0 {
                parts.push(name.to_owned());
            }
        }
        parts.push(if self.vk == 0x20 {
            "Space".into()
        } else if (0x70..=0x87).contains(&self.vk) {
            format!("F{}", self.vk - 0x6f)
        } else {
            char::from_u32(self.vk as u32).unwrap_or('?').to_string()
        });
        parts.join(" + ")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub mode: Shortcut,
    pub translate: Shortcut,
}
impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            mode: Shortcut::SHIFT,
            translate: Shortcut::TRANSLATE,
        }
    }
}
impl Shortcuts {
    pub fn validate(self) -> Result<(), String> {
        if !self.mode.valid(true) || !self.translate.valid(false) {
            return Err("请使用最多三个按键的组合键".into());
        }
        if self.mode.vk != 0 && self.mode == self.translate {
            return Err("两个快捷键不能相同".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bindings_reject_plain_letters_and_duplicates() {
        assert!(Shortcuts::default().validate().is_ok());
        assert!(!Shortcut {
            vk: 65,
            modifiers: 0
        }
        .valid(false));
        assert!(Shortcuts {
            mode: Shortcut::TRANSLATE,
            translate: Shortcut::TRANSLATE
        }
        .validate()
        .is_err());
    }
    #[test]
    fn missing_model_does_not_silently_select_another() {
        assert!(Config::default().selected().is_err());
    }
}
