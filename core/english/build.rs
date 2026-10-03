use std::{collections::BTreeMap, env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let source = "../../data/dict/raw/english/frequency.txt";
    println!("cargo:rerun-if-changed={source}");
    let mut words = BTreeMap::<String, u64>::new();
    for line in fs::read_to_string(source)?
        .trim_start_matches('\u{feff}')
        .lines()
    {
        let Some((word, count)) = line.rsplit_once(' ') else {
            continue;
        };
        if !word.is_empty()
            && word.len() <= 64
            && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'\'')
        {
            let count = count.parse::<u64>().map_err(io::Error::other)?;
            *words.entry(word.to_owned()).or_default() += count;
        }
    }
    let words: Vec<_> = words.into_iter().collect();
    let size = words.len().next_power_of_two();
    let mut best = vec![u32::MAX; size * 2];
    for i in 0..words.len() {
        best[size + i] = i as u32;
    }
    let better = |a: u32, b: u32| {
        if a == u32::MAX {
            b
        } else if b == u32::MAX || words[a as usize].1 >= words[b as usize].1 {
            a
        } else {
            b
        }
    };
    for i in (1..size).rev() {
        best[i] = better(best[i * 2], best[i * 2 + 1]);
    }
    let mut arena = Vec::new();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(words.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(size as u32).to_le_bytes());
    for (word, count) in &words {
        bytes.extend_from_slice(&(arena.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(word.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        arena.extend_from_slice(word.as_bytes());
    }
    for id in best {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    bytes.extend_from_slice(&arena);
    let out = env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("missing OUT_DIR"))?;
    fs::write(PathBuf::from(out).join("english.bin"), bytes)
}
