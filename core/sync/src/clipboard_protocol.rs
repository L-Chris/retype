//! Bounded newline-delimited protocol, independent of platform clipboard APIs.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_TEXT: usize = 64 * 1024;
pub const MAX_IMAGE: usize = 8 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;
// Base64 image plus JSON envelope (text frames still support worst-case escaping).
pub const MAX_FRAME: usize = MAX_IMAGE.div_ceil(3) * 4 + 512 * 1024;
pub const TTL_MS: u64 = 600_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub origin: String,
    pub seq: u64,
    pub clock: u64,
    pub created: u64,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}
impl Clip {
    pub fn valid(&self, now: u64) -> bool {
        crate::config::valid_id(&self.origin)
            && self.seq > 0
            && self.clock > 0
            && match &self.image {
                Some(image) => {
                    self.text.is_empty() && crate::clipboard_image::decode(image).is_some()
                }
                None => {
                    !self.text.is_empty()
                        && self.text.len() <= MAX_TEXT
                        && !self.text.contains('\0')
                }
            }
            && self.created <= now.saturating_add(60_000)
            && now <= self.created.saturating_add(TTL_MS)
    }
    pub fn supported_by(&self, formats: &[String]) -> bool {
        self.image.as_ref().is_none_or(|encoded| {
            crate::clipboard_image::decode(encoded)
                .and_then(|bytes| crate::clipboard_image::mime_type(&bytes))
                .is_some_and(|mime| formats.iter().any(|supported| supported == mime))
        })
    }
    fn order(&self) -> (u64, &str, u64) {
        (self.clock, &self.origin, self.seq)
    }
}

#[derive(Default)]
pub struct Journal {
    pub latest: Option<Clip>,
    clock: u64,
    seen: BTreeMap<String, u64>,
}
impl Journal {
    pub fn accept(&mut self, clip: Clip, now: u64) -> bool {
        if !clip.valid(now) || clip.seq <= *self.seen.get(&clip.origin).unwrap_or(&0) {
            return false;
        }
        if !self.seen.contains_key(&clip.origin) && self.seen.len() >= 32 {
            return false;
        }
        self.clock = self.clock.max(clip.clock);
        self.seen.insert(clip.origin.clone(), clip.seq);
        if self
            .latest
            .as_ref()
            .is_some_and(|previous| clip.order() <= previous.order())
        {
            return false;
        }
        self.latest = Some(clip);
        true
    }
    pub fn local(&mut self, origin: &str, seq: u64, text: String, now: u64) -> Option<Clip> {
        self.local_content(origin, seq, text, None, now)
    }
    pub fn local_image(&mut self, origin: &str, seq: u64, bytes: &[u8], now: u64) -> Option<Clip> {
        self.local_content(
            origin,
            seq,
            String::new(),
            crate::clipboard_image::encode(bytes),
            now,
        )
    }
    pub fn local_content(
        &mut self,
        origin: &str,
        seq: u64,
        text: String,
        image: Option<String>,
        now: u64,
    ) -> Option<Clip> {
        let clip = Clip {
            origin: origin.into(),
            seq,
            clock: self.clock.max(now).saturating_add(1),
            created: now,
            text,
            image,
        };
        self.accept(clip.clone(), now).then_some(clip)
    }
}

#[derive(Default)]
pub struct Decoder(Vec<u8>);
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<serde_json::Value>, String> {
        let mut messages = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                let frame = std::mem::take(&mut self.0);
                messages.push(serde_json::from_slice(&frame).map_err(|_| "无效的设备消息")?);
            } else {
                if self.0.len() >= MAX_FRAME {
                    return Err("设备消息过大".into());
                }
                self.0.push(*byte);
            }
        }
        Ok(messages)
    }
}

pub fn pairing_code(cert: &[u8], client: &str, server: &str) -> String {
    let digest = crate::hash(&[cert, client.as_bytes(), server.as_bytes()].concat());
    let value = u32::from_str_radix(&digest[..8], 16).unwrap_or_default();
    format!("{:06}", value % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png() -> Vec<u8> {
        include_bytes!("../../../assets/logo.png").to_vec()
    }
    #[test]
    fn images_share_text_ordering_expiry_and_duplicate_rules() -> Result<(), String> {
        let mut source = Journal::default();
        let image = source
            .local_image(&"a".repeat(32), 1, &png(), 1_000)
            .ok_or("image")?;
        assert!(image.text.is_empty());
        let mut receiver = Journal::default();
        assert!(receiver.accept(image.clone(), 1_001));
        assert!(!receiver.accept(image.clone(), 1_002));
        assert!(!image.valid(1_000 + TTL_MS + 1));
        let mut mixed = image.clone();
        mixed.text = "ambiguous".into();
        assert!(!mixed.valid(1_001));
        let next = source
            .local(&"a".repeat(32), 2, "newer text".into(), 1_003)
            .ok_or("text")?;
        assert!(receiver.accept(next, 1_004));
        assert!(!receiver.accept(image, 1_004));
        assert_eq!(
            receiver.latest.map(|v| v.text).as_deref(),
            Some("newer text")
        );
        Ok(())
    }
    #[test]
    fn text_wire_format_remains_compatible_and_image_frames_fragment() -> Result<(), String> {
        let mut source = Journal::default();
        let text = source
            .local(&"a".repeat(32), 1, "legacy".into(), 1_000)
            .ok_or("text")?;
        let wire = serde_json::to_value(&text).map_err(|e| e.to_string())?;
        assert!(wire.get("image").is_none());
        assert_eq!(
            serde_json::from_value::<Clip>(wire).map_err(|e| e.to_string())?,
            text
        );
        let image = source
            .local_image(&"a".repeat(32), 2, &png(), 1_001)
            .ok_or("image")?;
        let mut bytes = serde_json::to_vec(&serde_json::json!({"type":"clip", "clip":image}))
            .map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let mut decoder = Decoder::default();
        let mut frames = Vec::new();
        for fragment in bytes.chunks(37) {
            frames.extend(decoder.feed(fragment)?);
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(
            serde_json::from_value::<Clip>(frames[0]["clip"].clone()).map_err(|e| e.to_string())?,
            image
        );
        assert!(source
            .local_image(&"a".repeat(32), 3, &vec![0; MAX_IMAGE + 1], 1_002)
            .is_none());
        Ok(())
    }
    #[test]
    fn duplicate_expired_and_reordered_events_do_not_replace_new_text() {
        let a = "a".repeat(32);
        let mut journal = Journal::default();
        let clip = journal.local(&a, 1, "你好\nworld 👋".into(), 1_000);
        assert!(clip.is_some());
        if let Some(clip) = clip {
            assert!(!journal.accept(clip.clone(), 1_001));
            assert!(!journal.accept(Clip { seq: 2, ..clip }, 700_000));
        }
        let old = Clip {
            origin: "b".repeat(32),
            seq: 1,
            clock: 1,
            created: 1_000,
            text: "old".into(),
            image: None,
        };
        assert!(journal.local(&a, 2, "new".into(), 1_002).is_some());
        assert!(!journal.accept(old, 1_003));
        assert_eq!(journal.latest.map(|v| v.text).as_deref(), Some("new"));
    }
    #[test]
    fn framing_handles_fragmentation_and_limits_escaped_text() -> Result<(), String> {
        let mut decoder = Decoder::default();
        assert!(decoder.feed(b"{\"x\":").is_ok_and(|m| m.is_empty()));
        assert_eq!(decoder.feed(b"1}\n{\"x\":2}\n")?.len(), 2);
        assert!(decoder.feed(&vec![b' '; MAX_FRAME + 1]).is_err());
        Ok(())
    }
    #[test]
    fn simultaneous_events_converge_regardless_of_arrival_order() {
        let one = Clip {
            origin: "a".repeat(32),
            seq: 1,
            clock: 1,
            created: 1,
            text: "a".into(),
            image: None,
        };
        let two = Clip {
            origin: "b".repeat(32),
            text: "b".into(),
            ..one.clone()
        };
        let mut left = Journal::default();
        let mut right = Journal::default();
        left.accept(one.clone(), 1);
        left.accept(two.clone(), 1);
        right.accept(two, 1);
        right.accept(one, 1);
        assert_eq!(left.latest, right.latest);
    }
}
