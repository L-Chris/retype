//! Settings schema and trust-boundary validation shared by every sync client.
use crate::{config, model::Snapshot, statistics, Result};
use retype_ai_core::config::Provider;
pub fn validate_snapshot(s: &Snapshot, id: &str) -> Result<()> {
    if s.version != 1
        || s.device != id
        || !config::valid_id(id)
        || s.name.len() > 256
        || s.fields.len() > 2000
        || !s.learning.valid()
    {
        return Err("云端数据无效或版本不兼容".into());
    }
    let mut buckets = Vec::new();
    statistics::merge(&mut buckets, &s.statistics)?;
    for (key, f) in &s.fields {
        if !known_key(key)
            || key.len() > 256
            || f.ancestors.len() > 20000
            || f.stamp.revision == u64::MAX
            || !config::valid_id(&f.stamp.device)
            || f.ancestors.iter().any(|s| !config::valid_id(&s.device))
        {
            return Err("云端设置版本无效".into());
        }
        if key.starts_with("providers/") && !f.value.is_null() {
            let p: Provider =
                serde_json::from_value(f.value.clone()).map_err(|_| "云端提供商无效")?;
            if format!("providers/{}", p.id) != *key
                || p.base_url.contains(['@', '?', '#'])
                || p.models.len() > 10000
            {
                return Err("云端提供商无效".into());
            }
        }
    }
    Ok(())
}
fn known_key(key: &str) -> bool {
    matches!(
        key,
        "input.scheme"
            | "input.english"
            | "input.english_spelling"
            | "input.symbol_completion"
            | "dictionary.enabled"
            | "updates.auto_check"
            | "shortcuts"
            | "shortcuts.macos"
            | "translation.model"
            | "translation.target"
            | "translation.preview"
            | "translation.instructions"
            | "translation.reasoning"
            | "translation.timeout"
            | "voice.settings"
    ) || key
        .strip_prefix("providers/")
        .is_some_and(|s| !s.is_empty() && s.len() <= 128 && !s.contains(['/', '\\']))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Field, Stamp};
    use std::collections::{BTreeMap, BTreeSet};
    fn snapshot(key: &str) -> Snapshot {
        let id = "a".repeat(32);
        Snapshot {
            version: 1,
            device: id.clone(),
            name: "Mac".into(),
            learning: Default::default(),
            statistics: vec![],
            fields: BTreeMap::from([(
                key.into(),
                Field {
                    stamp: Stamp {
                        revision: 1,
                        device: id,
                    },
                    ancestors: BTreeSet::new(),
                    value: serde_json::json!({}),
                },
            )]),
        }
    }
    #[test]
    fn platform_shortcuts_survive_validation_on_every_client() {
        let s = snapshot("shortcuts.macos");
        assert!(validate_snapshot(&s, &s.device).is_ok());
        let mut state = crate::model::State::default();
        state.merge(&s);
        assert_eq!(
            state.values().get("shortcuts.macos"),
            Some(&serde_json::json!({}))
        );
        assert!(validate_snapshot(&s, &"b".repeat(32)).is_err());
    }
    #[test]
    fn invalid_fields_and_provider_values_are_rejected_consistently() {
        for key in ["unknown", "providers/", "providers/a/b", "providers/a\\b"] {
            let s = snapshot(key);
            assert!(validate_snapshot(&s, &s.device).is_err(), "{key}");
        }
        let s = snapshot("providers/bad");
        assert!(validate_snapshot(&s, &s.device).is_err());
    }
}
