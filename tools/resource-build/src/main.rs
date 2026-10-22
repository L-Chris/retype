use retype_resources::{checksum, Catalog, Component};
use std::{
    fs::{self, File},
    io::{BufReader, Write},
    path::{Path, PathBuf},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let value = |name: &str, default: &str| {
        args.windows(2)
            .find(|v| v[0] == name)
            .map_or_else(|| PathBuf::from(default), |v| PathBuf::from(&v[1]))
    };
    let out = value("--out", "dist/dictionaries");
    fs::create_dir_all(&out)?;
    let manifest = value("--manifest", "data/dict/resources.json");
    let english = out.join("english.bin");
    retype_english::compile(
        BufReader::new(File::open("data/dict/raw/english/frequency.txt")?),
        File::create(&english)?,
    )?;
    let mut components = Vec::new();
    let mut archives = Vec::new();
    for (id, format, source) in [
        ("base", "RTDICT02", Path::new("data/dict/retype-dict.bin")),
        ("english", "RTENGL01", english.as_path()),
    ] {
        let mut c = Component {
            id: id.into(),
            format: format.into(),
            bytes: fs::metadata(source)?.len(),
            sha256: checksum(source)?,
            compressed_bytes: 0,
            compressed_sha256: String::new(),
            release: None,
        };
        let archive = out.join(c.asset());
        let mut compressor = flate2::GzBuilder::new()
            .mtime(0)
            .operating_system(255)
            .write(File::create(&archive)?, flate2::Compression::new(9));
        std::io::copy(&mut File::open(source)?, &mut compressor)?;
        compressor.finish()?.sync_all()?;
        c.compressed_bytes = fs::metadata(&archive)?.len();
        c.compressed_sha256 = checksum(&archive)?;
        archives.push(archive);
        components.push(c);
    }
    let mut catalog = Catalog {
        schema: 1,
        components,
    };
    catalog.validate()?;
    if args.iter().any(|v| v == "--check") {
        let expected: Catalog = serde_json::from_reader(File::open(&manifest)?)?;
        // Publication origin can intentionally reuse a previous application's assets.
        let mut expected = expected;
        for (c, actual) in expected.components.iter_mut().zip(&mut catalog.components) {
            actual.release = c.release.clone();
            c.release = None;
        }
        let mut actual = catalog.clone();
        for component in &mut actual.components {
            component.release = None;
        }
        if serde_json::to_value(&expected)? != serde_json::to_value(&actual)? {
            return Err("Resource catalog is stale; regenerate it before publishing".into());
        }
    }
    let json = serde_json::to_string_pretty(&catalog)? + "\n";
    if !args.iter().any(|v| v == "--check") {
        fs::write(&manifest, &json)?;
    }
    fs::write(out.join("resources.json"), json)?;
    let mut bundle = File::create(out.join("retype-base.retype-resources"))?;
    bundle.write_all(retype_resources::BUNDLE_MAGIC)?;
    for (c, archive) in catalog.components.iter().zip(archives) {
        bundle.write_all(&c.compressed_bytes.to_le_bytes())?;
        std::io::copy(&mut File::open(archive)?, &mut bundle)?;
    }
    bundle.sync_all()?;
    println!(
        "Built {} downloadable dictionaries in {}",
        catalog.components.len(),
        out.display()
    );
    Ok(())
}
