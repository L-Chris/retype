//! Cross-platform release selection. HTTP and package installation stay in adapters.
use crate::{parse_version, Platform, SemVer, UpdateError};
use serde::Serialize;
use serde_json::Value;
pub const REPOSITORY: &str = "L-Chris/retype";
pub const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub version: String,
    pub url: String,
    pub hash_url: String,
    pub size: u64,
    pub notes: String,
}
pub fn android_targets(abis: &[String]) -> Vec<Platform> {
    let mut targets = Vec::new();
    for abi in abis {
        let platform = match abi.as_str() {
            "arm64-v8a" => Platform::AndroidArm64,
            "x86_64" => Platform::AndroidX64,
            _ => continue,
        };
        if !targets.contains(&platform) {
            targets.push(platform);
        }
    }
    targets.push(Platform::AndroidUniversal);
    targets
}
pub fn asset_name(version: &str, platform: Platform) -> String {
    format!("retype-{version}{}", platform.asset_suffix())
}
pub fn android_version_code(version: &str) -> Result<u32, UpdateError> {
    let fields: Vec<_> = version.split('.').collect();
    if fields.len() != 3
        || fields
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err(UpdateError::InvalidVersion(version.into()));
    }
    let p = parse_version(version)?;
    if p.major > 200_000 || p.minor > 99 || p.patch > 99 {
        return Err(UpdateError::InvalidVersion(version.into()));
    }
    Ok((p.major * 10000 + p.minor * 100 + p.patch) as u32)
}
pub fn valid_asset_url(url: &str) -> bool {
    url.strip_prefix(&format!(
        "https://api.github.com/repos/{REPOSITORY}/releases/assets/"
    ))
    .is_some_and(|s| {
        s.parse::<u64>()
            .is_ok_and(|id| id > 0 && id.to_string() == s)
    })
}
fn asset_url(asset: &Value) -> Option<String> {
    let id = asset["id"].as_u64().filter(|id| *id > 0)?;
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/assets/{id}");
    (asset["url"] == url).then_some(url)
}
pub fn select(
    releases: &Value,
    targets: &[Platform],
    current: Option<&SemVer>,
) -> Result<Option<Package>, UpdateError> {
    let releases = releases
        .as_array()
        .ok_or_else(|| UpdateError::Malformed("更新列表无效".into()))?;
    let android = targets.iter().any(|p| {
        matches!(
            p,
            Platform::AndroidArm64
                | Platform::AndroidX64
                | Platform::AndroidUniversal
                | Platform::AndroidArm32
        )
    });
    let mut selected: Option<(SemVer, Package)> = None;
    let mut invalid = false;
    for r in releases {
        if r["draft"].as_bool().unwrap_or(false) || r["prerelease"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(tag) = r["tag_name"].as_str() else {
            continue;
        };
        let Ok(version) = parse_version(tag) else {
            continue;
        };
        if version.pre.is_some()
            || current.is_some_and(|c| &version <= c)
            || selected.as_ref().is_some_and(|(v, _)| v >= &version)
        {
            continue;
        }
        let version_text = version.to_string();
        if android && android_version_code(&version_text).is_err() {
            continue;
        }
        let Some(assets) = r["assets"].as_array() else {
            continue;
        };
        for platform in targets {
            let name = asset_name(&version_text, *platform);
            let Some(asset) = assets.iter().find(|a| a["name"] == name) else {
                continue;
            };
            let Some(hash) = assets
                .iter()
                .find(|a| a["name"] == format!("{name}.sha256"))
            else {
                continue;
            };
            let pair = asset_url(asset).zip(asset_url(hash));
            let size = asset["size"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= MAX_PACKAGE_BYTES);
            let (Some((url, hash_url)), Some(size)) = (pair, size) else {
                invalid = true;
                continue;
            };
            if url == hash_url {
                invalid = true;
                continue;
            }
            selected = Some((
                version.clone(),
                Package {
                    version: version_text.clone(),
                    url,
                    hash_url,
                    size,
                    notes: r["body"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(24000)
                        .collect(),
                },
            ));
            break;
        }
    }
    match selected {
        Some((_, package)) => Ok(Some(package)),
        None if invalid => Err(UpdateError::Malformed(
            "更新附件身份、地址或大小无效".into(),
        )),
        None => Ok(None),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn release(version: &str, p: Platform, id: u64) -> Value {
        let name = asset_name(version, p);
        let asset = |id, name: String| json!({"id":id,"name":name,"url":format!("https://api.github.com/repos/{REPOSITORY}/releases/assets/{id}"),"size":100});
        json!({"tag_name":format!("v{version}"),"assets":[asset(id,name.clone()),asset(id+1,format!("{name}.sha256"))]})
    }
    #[test]
    fn selection_uses_matching_abi_and_universal_fallback_with_exact_checksum_pair() {
        let targets = android_targets(&["x86_64".into(), "arm64-v8a".into(), "x86_64".into()]);
        assert_eq!(
            targets,
            vec![
                Platform::AndroidX64,
                Platform::AndroidArm64,
                Platform::AndroidUniversal
            ]
        );
        let mut r = release("1.2.3", Platform::AndroidUniversal, 1);
        let arm = release("1.2.3", Platform::AndroidArm64, 3);
        r["assets"] = json!([
            r["assets"][0].clone(),
            r["assets"][1].clone(),
            arm["assets"][0].clone(),
            arm["assets"][1].clone()
        ]);
        let p = select(&json!([r]), &targets, None).ok().flatten();
        assert_eq!(
            p.map(|p| p.url),
            Some(format!(
                "https://api.github.com/repos/{REPOSITORY}/releases/assets/3"
            ))
        );
    }
    #[test]
    fn newest_stable_matching_platform_wins_and_untrusted_assets_fail_closed() {
        let mut pre = release("9.0.0", Platform::MacosUniversal, 10);
        pre["prerelease"] = json!(true);
        let mut bad = release("8.0.0", Platform::MacosUniversal, 20);
        bad["assets"][0]["url"] = json!("https://example.com/file");
        assert!(select(&json!([bad.clone()]), &[Platform::MacosUniversal], None).is_err());
        let selected = select(
            &json!([
                pre,
                bad,
                release("1.2.4", Platform::WindowsX64, 30),
                release("1.2.3", Platform::MacosUniversal, 40)
            ]),
            &[Platform::MacosUniversal],
            None,
        )
        .ok()
        .flatten();
        assert_eq!(selected.map(|p| p.version), Some("1.2.3".into()));
        assert_eq!(android_version_code("0.6.10").ok(), Some(610));
        assert!(android_version_code("1.100.0").is_err());
        assert!(!valid_asset_url(
            "https://api.github.com/repos/L-Chris/retype/releases/assets/01"
        ));
    }
}
