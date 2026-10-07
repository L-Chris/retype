//! Shared voice protocol and bounded, ordered PCM segmentation. No platform IO.
use serde::{Deserialize, Serialize};
pub const RATE: usize = 16_000;
pub const MAX_SAMPLES: usize = RATE * 120;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: String,
    pub model: String,
    pub language: String,
    pub tidy: bool,
    pub microphone: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            language: "auto".into(),
            tidy: false,
            microphone: u32::MAX,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.language.as_str(), "auto" | "zh" | "en")
            || self.provider.len() > 128
            || self.model.len() > 512
        {
            return Err("语音设置无效".into());
        }
        Ok(())
    }
    pub fn selected<'a>(
        &self,
        config: &'a crate::config::Config,
    ) -> Result<&'a crate::config::Provider, String> {
        self.validate()?;
        config
            .providers
            .iter()
            .find(|p| {
                p.id == self.provider
                    && p.models.contains(&self.model)
                    && matches!(
                        p.kind,
                        crate::config::ApiKind::Compatible | crate::config::ApiKind::Gemini
                    )
            })
            .ok_or_else(|| "请在设置 → 语音输入中选择支持音频的模型".into())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Recording,
    Recognizing,
    Done,
    Error,
    Cancelled,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub phase: Phase,
    pub text: String,
    pub level: f32,
    pub seconds: u32,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum Command {
    Start { id: String, settings: Settings },
    Poll { id: String },
    Stop { id: String },
    Cancel { id: String },
}
pub struct Audio {
    pub samples: Vec<i16>,
    start: usize,
    silence: usize,
    voiced: bool,
    completed_segments: usize,
    pub has_speech: bool,
    pub level: f32,
}
impl Default for Audio {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            start: 0,
            silence: 0,
            voiced: false,
            completed_segments: 0,
            has_speech: false,
            level: 0.0,
        }
    }
}
impl Audio {
    /// A completed segment is owned by the caller and must never be silently dropped.
    pub fn push(&mut self, frame: &[i16]) -> Result<Option<Vec<i16>>, String> {
        if self.samples.len().saturating_add(frame.len()) > MAX_SAMPLES {
            return Err("录音已达到两分钟上限".into());
        }
        let rms = (frame.iter().map(|v| (*v as f64).powi(2)).sum::<f64>()
            / frame.len().max(1) as f64)
            .sqrt();
        self.level = (rms / 3000.0).min(1.0) as f32;
        if rms > 140.0 {
            self.voiced = true;
            self.has_speech = true;
            self.silence = 0;
        } else {
            self.silence += frame.len();
        }
        self.samples.extend_from_slice(frame);
        let length = self.samples.len() - self.start;
        if self.voiced && length >= RATE && (self.silence >= RATE * 7 / 10 || length >= RATE * 8) {
            Ok(self.take_segment())
        } else {
            Ok(None)
        }
    }
    pub fn take_segment(&mut self) -> Option<Vec<i16>> {
        let result = self.voiced.then(|| self.samples[self.start..].to_vec());
        if result.is_some() {
            self.completed_segments += 1;
        }
        self.start = self.samples.len();
        self.silence = 0;
        self.voiced = false;
        result
    }
    pub fn completed_segments(&self) -> usize {
        self.completed_segments
    }
    pub fn has_unsegmented_speech(&self) -> bool {
        self.voiced
    }
}
pub fn wav(samples: &[i16]) -> Result<Vec<u8>, String> {
    if samples.is_empty() || samples.len() > MAX_SAMPLES {
        return Err("录音为空或超过上限".into());
    }
    let size = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(size as usize + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(size + 36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&((RATE * 2) as u32).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&size.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(out)
}

/// Confirmed voice output only; numbers and punctuation are not typing activity.
pub fn counts(text: &str) -> (u32, u32) {
    let chinese = text.chars().filter(|ch| matches!(*ch as u32,0x3400..=0x4dbf|0x4e00..=0x9fff|0xf900..=0xfaff|0x20000..=0x323af)).count() as u32;
    let mut words = retype_types::statistics::WordCounter::default();
    let english = words.feed(text).saturating_add(words.finish());
    (chinese, english)
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn microphones() -> Vec<(u32, String)> {
    use windows_sys::Win32::Media::Audio::*;
    let mut devices = vec![(u32::MAX, "系统默认输入设备".into())];
    // SAFETY: read-only WinMM enumeration; fixed-size caps buffer matches its byte length.
    unsafe {
        for id in 0..waveInGetNumDevs().min(128) {
            let mut caps: WAVEINCAPSW = std::mem::zeroed();
            if waveInGetDevCapsW(
                id as usize,
                &mut caps,
                std::mem::size_of::<WAVEINCAPSW>() as u32,
            ) == 0
            {
                let name = caps.szPname;
                let len = name.iter().position(|ch| *ch == 0).unwrap_or(name.len());
                devices.push((id, String::from_utf16_lossy(&name[..len])));
            }
        }
    }
    devices
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gemini_voice_selection_accepts_saved_model_but_not_missing_model() {
        let provider = crate::config::Provider {
            id: "gemini".into(),
            kind: crate::config::ApiKind::Gemini,
            models: vec!["gemini-3.5-transcribe-live".into()],
            ..Default::default()
        };
        let config = crate::config::Config {
            providers: vec![provider],
            ..Default::default()
        };
        let mut settings = Settings {
            provider: "gemini".into(),
            model: "gemini-3.5-transcribe-live".into(),
            ..Default::default()
        };
        assert!(settings.selected(&config).is_ok());
        settings.model = "missing".into();
        assert!(settings.selected(&config).is_err());
    }
    #[test]
    fn silence_segments_and_tail_are_preserved() {
        let mut audio = Audio::default();
        assert!(audio.push(&vec![1000; RATE]).ok().flatten().is_none());
        let segment = audio
            .push(&vec![0; RATE])
            .ok()
            .flatten()
            .unwrap_or_default();
        assert_eq!(segment.len(), RATE * 2);
        let _ = audio.push(&vec![1200; 320]);
        assert_eq!(audio.take_segment().unwrap_or_default().len(), 320);
        assert_eq!(audio.samples.len(), RATE * 2 + 320);
    }
    #[test]
    fn silence_is_not_a_speech_segment_and_wave_has_correct_size() {
        let mut audio = Audio::default();
        let _ = audio.push(&vec![0; RATE]);
        assert!(audio.take_segment().is_none());
        assert_eq!(wav(&[12, -12]).unwrap_or_default().len(), 48);
        assert!(wav(&[]).is_err());
        assert!(audio.push(&vec![0; MAX_SAMPLES]).is_err());
    }
}
