//! Optional dictionary catalog, pinned source validation and compilation for every adapter.
use crate::{hash_name, hash_reader, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufReader, Cursor, Read},
    path::Path,
};
pub const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pack {
    pub id: String,
    pub title: String,
    pub description: String,
    pub bytes: u64,
    pub sha256: String,
}
impl Pack {
    pub fn source_url(&self) -> String {
        format!(
            "https://raw.githubusercontent.com/amzxyz/rime-wanxiang/v18.0.14/dicts/{}.dict.yaml",
            self.id
        )
    }
    pub fn verify(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 != self.bytes
            || self.bytes > MAX_SOURCE_BYTES
            || hash_reader(&mut &bytes[..])? != self.sha256
        {
            return Err("词库校验失败，请重试".into());
        }
        Ok(())
    }
}
pub fn catalog() -> Result<Vec<Pack>> {
    let packs: Vec<Pack> = serde_json::from_str(include_str!("../dictionary-packs.json"))
        .map_err(|e| e.to_string())?;
    if packs.len() > 31
        || packs.iter().any(|p| {
            p.id.is_empty()
                || !p.id.bytes().all(|b| b.is_ascii_lowercase())
                || !(1..=MAX_SOURCE_BYTES).contains(&p.bytes)
                || !hash_name(&p.sha256)
        })
    {
        return Err("词库目录无效".into());
    }
    Ok(packs)
}
pub fn compile_bytes(pack: &Pack, bytes: &[u8], destination: &Path) -> Result<usize> {
    pack.verify(bytes)?;
    compile_rime(bytes, destination)
}
/// Local/imported sources do not have a catalog identity. Network packs must use
/// `compile_source` so their pinned hash is checked before compilation.
pub fn compile_local(source: &Path, destination: &Path) -> Result<usize> {
    let mut bytes = Vec::new();
    std::fs::File::open(source)
        .map_err(|e| e.to_string())?
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    compile_rime(&bytes, destination)
}
fn compile_rime(bytes: &[u8], destination: &Path) -> Result<usize> {
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err("词库超过大小限制".into());
    }
    let tsv = retype_dict::rime::tsv(BufReader::new(bytes))?;
    let result = (|| {
        let file = std::fs::File::create(destination).map_err(|e| e.to_string())?;
        let count =
            retype_dict::binary::compile(Cursor::new(tsv), &file).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(count)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}
pub fn compile_source(id: &str, source: &Path, destination: &Path) -> Result<usize> {
    let pack = catalog()?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or("无效词库")?;
    let mut bytes = Vec::new();
    std::fs::File::open(source)
        .map_err(|e| e.to_string())?
        .take(pack.bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    compile_bytes(&pack, &bytes, destination)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_bounded_and_source_hash_prevents_compiling_untrusted_data() {
        assert!(catalog().is_ok());
        let bytes = "name: test\n...\n你好\tni hao\t100\n".as_bytes();
        let p = Pack {
            id: "test".into(),
            title: String::new(),
            description: String::new(),
            bytes: bytes.len() as u64,
            sha256: hash_reader(&mut &bytes[..]).unwrap_or_default(),
        };
        assert!(p.verify(bytes).is_ok());
        assert!(p.verify(b"tampered").is_err());
        let mut wrong = p.clone();
        wrong.sha256 = "0".repeat(64);
        assert!(wrong.verify(bytes).is_err());
        assert!(compile_source("../escape", Path::new("unused"), Path::new("unused")).is_err());
    }
}
