#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

#[test]
fn resource_api_selection_requires_the_pinned_official_attachment() {
    let component = Catalog::builtin().unwrap().components.remove(1);
    let valid = serde_json::json!({
        "tag_name":component.release_tag(), "draft":false, "prerelease":false,
        "assets":[{"id":42,"url":"https://api.github.com/repos/L-Chris/retype/releases/assets/42",
            "name":component.asset(),"state":"uploaded","size":component.compressed_bytes,
            "digest":format!("sha256:{}",component.compressed_sha256)}]
    });
    assert_eq!(
        component.download_url(&valid).unwrap(),
        valid["assets"][0]["url"]
    );
    assert!(component
        .release_url()
        .ends_with(&format!("/tags/{}", component.release_tag())));
    for (field, value) in [
        ("url", serde_json::json!("https://example.com/resource.gz")),
        ("size", serde_json::json!(1)),
        (
            "digest",
            serde_json::json!(format!("sha256:{}", "0".repeat(64))),
        ),
        ("state", serde_json::json!("new")),
        ("name", serde_json::json!("other.gz")),
        ("id", serde_json::json!(0)),
    ] {
        let mut bad = valid.clone();
        bad["assets"][0][field] = value;
        assert!(component.download_url(&bad).is_err(), "{field}");
    }
    let mut old = valid.clone();
    old["tag_name"] = serde_json::json!("v0.0.1");
    assert!(component.download_url(&old).is_err());
    let mut legacy = valid;
    legacy["assets"][0]["digest"] = serde_json::Value::Null;
    assert!(component.download_url(&legacy).is_ok());
}

pub(crate) struct Temp(pub PathBuf);
impl Temp {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "retype-resource-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        for entry in fs::read_dir(&self.0).unwrap().flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    let mut permissions = metadata.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    permissions.set_readonly(false);
                    let _ = fs::set_permissions(entry.path(), permissions);
                }
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn archive(root: &Path) -> (Component, PathBuf, Vec<u8>) {
    let mut raw = Vec::new();
    retype_english::compile(&b"hello 10\nhelp 8\nworld 6\n"[..], &mut raw).unwrap();
    let archive = root.join("english.gz");
    let mut encoder = flate2::write::GzEncoder::new(
        File::create(&archive).unwrap(),
        flate2::Compression::default(),
    );
    encoder.write_all(&raw).unwrap();
    encoder.finish().unwrap();
    (
        Component {
            id: "english".into(),
            format: "RTENGL01".into(),
            bytes: raw.len() as u64,
            sha256: hex(&Sha256::digest(&raw)),
            compressed_bytes: fs::metadata(&archive).unwrap().len(),
            compressed_sha256: checksum(&archive).unwrap(),
            release: None,
        },
        archive,
        raw,
    )
}
#[test]
fn validated_install_is_immutable_and_reused() {
    let root = Temp::new();
    let (c, archive, raw) = archive(&root.0);
    install_component(&root.0, &c, &archive, &AtomicBool::new(false)).unwrap();
    assert_eq!(fs::read(c.path(&root.0)).unwrap(), raw);
    assert!(fs::metadata(c.path(&root.0))
        .unwrap()
        .permissions()
        .readonly());
    let mapped = retype_english::Vocabulary::open(&c.path(&root.0)).unwrap();
    let modified = fs::metadata(c.path(&root.0)).unwrap().modified().unwrap();
    install_component(&root.0, &c, &archive, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        fs::metadata(c.path(&root.0)).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(mapped.count(), 3);
}
#[test]
fn corrupt_cancelled_or_malformed_archive_never_replaces_the_current_resource() {
    let root = Temp::new();
    let (c, archive, raw) = archive(&root.0);
    install_component(&root.0, &c, &archive, &AtomicBool::new(false)).unwrap();
    assert!(install_component(&root.0, &c, &archive, &AtomicBool::new(true)).is_err());
    let mut bytes = fs::read(&archive).unwrap();
    bytes[12] ^= 1;
    fs::write(&archive, bytes).unwrap();
    assert!(install_component(&root.0, &c, &archive, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read(c.path(&root.0)).unwrap(), raw);
    let mut malformed = c.clone();
    malformed.compressed_sha256 = checksum(&archive).unwrap();
    malformed.sha256 = "0".repeat(64);
    assert!(install_component(&root.0, &malformed, &archive, &AtomicBool::new(false)).is_err());
    assert!(!malformed.path(&root.0).exists());
}
#[test]
fn compatible_legacy_dictionary_is_migrated_only_once() {
    let root = Temp::new();
    let old = Temp::new();
    let file = old.0.join("retype-dict.bin");
    retype_dict::binary::compile(
        "你好\tni hao\t100\n".as_bytes(),
        File::create(&file).unwrap(),
    )
    .unwrap();
    // Use the builder's annotated column order.
    assert!(retype_dict::binary::open_shared(&file).is_ok());
    let state = migrate(&root.0, std::slice::from_ref(&file)).unwrap();
    assert!(state.base.is_some());
    assert!(!state.ready);
    assert!(file.exists());
    let installed = state.base.unwrap();
    let before = fs::metadata(&installed).unwrap().modified().unwrap();
    assert_eq!(
        migrate(&root.0, &[file]).unwrap().base,
        Some(installed.clone())
    );
    assert_eq!(fs::metadata(installed).unwrap().modified().unwrap(), before);
}
#[test]
fn previous_catalog_resources_are_reused_in_place() {
    let root = Temp::new();
    let (component, archive, _) = archive(&root.0);
    install_component(&root.0, &component, &archive, &AtomicBool::new(false)).unwrap();
    let path = component.path(&root.0);
    let state = migrate(&root.0, &[]).unwrap();
    assert_eq!(state.english, Some(path.clone()));
    assert!(!root
        .0
        .join(format!("legacy-english-{}.bin", component.sha256))
        .exists());
    assert_eq!(migrate(&root.0, &[]).unwrap().english, Some(path));
}
#[test]
fn catalog_and_import_reject_untrusted_shape_before_install() {
    let mut c = Catalog::builtin().unwrap();
    c.components[0].id = "../outside".into();
    assert!(c.validate().is_err());
    let mut c = Catalog::builtin().unwrap();
    c.components[1].release = Some("v0/../../bad".into());
    assert!(c.validate().is_err());
    let root = Temp::new();
    let file = root.0.join("untrusted.bundle");
    fs::write(&file, b"RTRES001\xff\xff\xff\xff\xff\xff\xff\xff").unwrap();
    assert!(import_bundle(&root.0, &file, &AtomicBool::new(false)).is_err());
    assert!(status(&root.0).unwrap().base.is_none());
    let lease = Lease::acquire(&root.0).unwrap();
    assert!(Lease::acquire(&root.0).is_err());
    drop(lease);
    assert!(Lease::acquire(&root.0).is_ok());
}
