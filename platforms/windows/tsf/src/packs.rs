//! Optional category dictionaries. File IO stays off the TSF/input thread.
use retype_dict::{binary, Layer, LayeredDict};
use std::path::Path;

pub const IDS: [&str; 7] = [
    "mingren", "renming", "yiren", "diming", "yixue", "yaopin", "huaxue",
];

pub fn load_enabled(root: &Path, enabled: u32, base_total: f64) -> LayeredDict {
    let mut merged = LayeredDict::new();
    for (index, id) in IDS.iter().enumerate() {
        if enabled & (1 << index) == 0 {
            continue;
        }
        let path = root.join(format!("{id}.bin"));
        let loaded = binary::open_shared(&path);
        match loaded {
            Ok(dict) => {
                // Each .bin is normalized by its own weight sum. Translate its
                // log probabilities to the base dictionary's denominator.
                let boost = (dict.total_frequency() / base_total.max(1.0)).ln() as f32;
                merged.push(Layer {
                    name: id,
                    dict,
                    boost,
                });
            }
            Err(error) => tracing::warn!(
                "optional dictionary {} unavailable: {}",
                path.display(),
                error
            ),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::load_enabled;
    use retype_dict::binary;
    use retype_dict::{annotate, from_pairs};
    use retype_pinyin::Lexicon;

    #[test]
    fn pack_scores_share_base_denominator() {
        let (base, _) = from_pairs([("你好", "ni hao", 1000.0), ("你们", "ni men", 1000.0)]);
        let (pack, _) = from_pairs([("鲁迅", "lu xun", 10.0)]);
        let adjustment = (pack.total_frequency() / base.total_frequency()).ln() as f32;
        let mut hits = Vec::new();
        pack.lookup(&annotate::parse_pinyin("lu xun").unwrap(), &mut hits);
        assert!((hits[0].logp + adjustment - (10.0_f32 / 2000.0).ln()).abs() < 0.001);
    }

    #[test]
    fn enabled_mask_controls_pack_visibility() {
        let root = std::env::temp_dir().join(format!("retype-pack-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("mingren.bin");
        let mut binary_data = Vec::new();
        binary::compile("鲁迅\tlu xun\t100\n".as_bytes(), &mut binary_data).unwrap();
        std::fs::write(&path, binary_data).unwrap();
        let ids = annotate::parse_pinyin("lu xun").unwrap();
        let mut out = Vec::new();
        load_enabled(&root, 0, 1000.0).lookup(&ids, &mut out);
        assert!(out.is_empty());
        load_enabled(&root, 1, 1000.0).lookup(&ids, &mut out);
        assert_eq!(&*out[0].text, "鲁迅");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
