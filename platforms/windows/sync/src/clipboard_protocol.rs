//! Bounded newline-delimited protocol, independent of platform clipboard APIs.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_TEXT: usize = 64 * 1024;
pub const MAX_FRAME: usize = 512 * 1024;
pub const TTL_MS: u64 = 600_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub origin: String,
    pub seq: u64,
    pub clock: u64,
    pub created: u64,
    pub text: String,
}
impl Clip {
    pub fn valid(&self, now: u64) -> bool {
        crate::config::valid_id(&self.origin)
            && self.seq > 0
            && self.clock > 0
            && !self.text.is_empty()
            && self.text.len() <= MAX_TEXT
            && !self.text.contains('\0')
            && self.created <= now.saturating_add(60_000)
            && now <= self.created.saturating_add(TTL_MS)
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
        let clip = Clip {
            origin: origin.into(),
            seq,
            clock: self.clock.max(now).saturating_add(1),
            created: now,
            text,
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
