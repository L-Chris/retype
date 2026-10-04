//! Single-use short-code authentication, bound to the TLS certificate and both device IDs.
use crate::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use sha2_pairing::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub struct Exchange(Spake2<Ed25519Group>);
pub struct Proof(Vec<u8>);
pub fn valid_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}
pub fn new_code() -> String {
    format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000)
}
pub fn begin(
    code: &str,
    client: &str,
    server: &str,
    certificate_hash: &str,
) -> Result<(Exchange, String)> {
    if !valid_code(code)
        || !crate::config::valid_id(client)
        || !crate::config::valid_id(server)
        || certificate_hash.len() != 64
    {
        return Err("无效的匹配码或设备身份".into());
    }
    let identity = format!("retype-clip-v2:{client}:{server}:{certificate_hash}");
    let (state, message) = Spake2::<Ed25519Group>::start_symmetric(
        &Password::new(code.as_bytes()),
        &Identity::new(identity.as_bytes()),
    );
    Ok((Exchange(state), STANDARD.encode(message)))
}
impl Exchange {
    pub fn finish(self, message: &str) -> Result<Proof> {
        let message = STANDARD.decode(message).map_err(|_| "关联验证失败")?;
        if message.len() != 33 {
            return Err("关联验证失败".into());
        }
        self.0
            .finish(&message)
            .map(Proof)
            .map_err(|_| "关联验证失败".into())
    }
}
impl Proof {
    pub fn tag(&self, role: &str) -> Result<String> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).map_err(|_| "关联验证失败")?;
        mac.update(format!("retype-clip-v2-confirm:{role}").as_bytes());
        Ok(STANDARD.encode(mac.finalize().into_bytes()))
    }
    pub fn verify(&self, role: &str, tag: &str) -> Result<()> {
        let bytes = STANDARD.decode(tag).map_err(|_| "匹配码不正确")?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).map_err(|_| "关联验证失败")?;
        mac.update(format!("retype-clip-v2-confirm:{role}").as_bytes());
        mac.verify_slice(&bytes).map_err(|_| "匹配码不正确".into())
    }
}
type Sessions = HashMap<String, (Instant, Exchange)>;
static SESSIONS: OnceLock<Mutex<Sessions>> = OnceLock::new();
pub fn mobile(v: &serde_json::Value) -> Result<serde_json::Value> {
    let text = |key: &str| v[key].as_str().ok_or_else(|| format!("Missing {key}"));
    let mut sessions = SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "关联状态不可用")?;
    sessions.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(120));
    match text("type")? {
        "pairBegin" => {
            if sessions.len() >= 16 {
                return Err("关联请求过多".into());
            }
            let (exchange, message) = begin(
                text("code")?,
                text("client")?,
                text("server")?,
                text("certificate")?,
            )?;
            let handle = uuid::Uuid::new_v4().simple().to_string();
            sessions.insert(handle.clone(), (Instant::now(), exchange));
            Ok(serde_json::json!({"handle":handle,"message":message}))
        }
        "pairFinish" => {
            let (_, exchange) = sessions.remove(text("handle")?).ok_or("匹配码已过期")?;
            let proof = exchange.finish(text("message")?)?;
            Ok(serde_json::json!({"client":proof.tag("client")?,"server":proof.tag("server")?}))
        }
        "pairCancel" => {
            sessions.remove(text("handle")?);
            Ok(serde_json::json!({}))
        }
        _ => Err("无效的关联操作".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_code_authenticates_and_binds_certificate() -> Result<()> {
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let cert = "c".repeat(64);
        let (x, xm) = begin("123456", &a, &b, &cert)?;
        let (y, ym) = begin("123456", &a, &b, &cert)?;
        let x = x.finish(&ym)?;
        let y = y.finish(&xm)?;
        y.verify("client", &x.tag("client")?)?;
        assert!(y.verify("server", &x.tag("client")?).is_err());
        for (code, certificate) in [("654321", cert.clone()), ("123456", "d".repeat(64))] {
            let (x, xm) = begin("123456", &a, &b, &cert)?;
            let (y, ym) = begin(code, &a, &b, &certificate)?;
            assert!(y
                .finish(&xm)?
                .verify("client", &x.finish(&ym)?.tag("client")?)
                .is_err());
        }
        assert!(!valid_code("12a456"));
        Ok(())
    }
}
