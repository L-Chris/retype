#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use serde_json::json;

fn defaults() -> Value {
    perform(json!({"type":"defaults"})).unwrap()
}
#[test]
fn sync_roundtrip_preserves_models_platform_keys_and_local_identity() {
    let mut p = defaults();
    let identity = p["cloud"].clone();
    p["ai"]["providers"] = json!([{"id":"test-ai","preset":"自定义","name":"Test","kind":"Compatible","base_url":"https://example.com/v1","models":["one","two"],"voice_protocols":{"two":"GeminiLive"}}]);
    p["ai"]["provider"] = json!("test-ai");
    p["ai"]["model"] = json!("one");
    p["input"]["scheme"] = json!(1);
    p["packs"] = json!(17);
    let values = sync_values(&p).unwrap();
    assert_eq!(values["shortcuts"]["translate"]["vk"], 48);
    assert!(values["voice.settings"].get("microphone").is_none());
    let local = defaults();
    let local_id = local["cloud"]["device_id"].clone();
    let out = apply_values(&local, &values).unwrap();
    assert_eq!(out["ai"]["providers"], p["ai"]["providers"]);
    assert_eq!(out["ai"]["model"], "one");
    assert_eq!(out["cloud"]["device_id"], local_id);
    assert_ne!(out["cloud"], identity);
    assert_eq!(out["packs"], 17);
    assert_eq!(out["input"]["scheme"], 1);
    assert_eq!(sync_values(&out).unwrap(), values);
}
#[test]
fn bad_remote_settings_are_rejected_before_changing_local_values() {
    let p = defaults();
    let values = sync_values(&p).unwrap();
    for (key, bad) in [
        ("input.scheme", json!(2)),
        ("dictionary.enabled", json!(128)),
        ("input.english", json!("true")),
        ("translation.timeout", json!(999)),
        ("translation.reasoning", json!("extreme")),
    ] {
        let mut remote = values.clone();
        remote[key] = bad;
        assert!(apply_values(&p, &remote).is_err());
        assert_eq!(p["input"]["scheme"], 0);
    }
    let mut remote = values;
    remote["providers/wrong"] = json!({"id":"other","name":"Bad","preset":"自定义","kind":"Compatible","base_url":"https://example.com","models":[]});
    assert!(apply_values(&p, &remote).is_err());
}
#[test]
fn shortcuts_reject_typing_keys_reserved_combinations_and_duplicates() {
    let keys = defaults()["shortcuts"].clone();
    validate_shortcuts(&keys).unwrap();
    for binding in [
        json!({"key":0,"mods":0,"tap":false}),
        json!({"key":49,"mods":8,"tap":false}),
        json!({"key":29,"mods":32,"tap":false}),
        json!({"key":0,"mods":0,"tap":true}),
    ] {
        let mut bad = keys.clone();
        bad["translate"] = binding;
        assert!(validate_shortcuts(&bad).is_err());
    }
    let mut bad = keys;
    bad["translate"] = bad["mode"].clone();
    assert!(validate_shortcuts(&bad).is_err());
}
fn asset(id: u64, name: &str) -> Value {
    json!({"id":id,"name":name,"size":100,"url":format!("https://api.github.com/repos/L-Chris/retype/releases/assets/{id}")})
}
#[test]
fn mac_updates_require_stable_universal_assets_and_official_api_ids() {
    let release = |tag: &str, mac: bool| {
        let name = format!("retype-{}-macos-universal.zip", tag.trim_start_matches('v'));
        json!({"tag_name":tag,"draft":false,"prerelease":false,"body":"notes","assets":if mac{vec![asset(10,&name),asset(11,&format!("{name}.sha256"))]}else{vec![asset(10,"windows-setup.exe")]}})
    };
    let selected = parse_updates(&json!([
        release("v999.2.0", false),
        release("v999.1.0", true)
    ]))
    .unwrap();
    assert_eq!(selected["version"], "999.1.0");
    assert!(selected["url"]
        .as_str()
        .unwrap()
        .contains("/releases/assets/10"));
    let mut pre = release("v999.3.0", true);
    pre["prerelease"] = json!(true);
    assert_eq!(parse_updates(&json!([pre])).unwrap()["current"], true);
    let mut bad = release("v999.3.0", true);
    bad["assets"][0]["url"] = json!("https://github.com/other/repo/releases/download/v1/app.zip");
    assert!(parse_updates(&json!([bad])).is_err());
    assert!(download_update(
        Path::new("unused"),
        &json!({"url":"https://example.com/file","hashUrl":"https://example.com/hash"})
    )
    .is_err());
}
