//! Isolated profiling; no TIP registration, live DB writes, or typed text output.
use retype_pinyin::Lexicon;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    sync::Arc,
    time::Instant,
};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct Tracked;
fn add(bytes: usize) {
    let value = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(value, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = System.realloc(p, layout, size);
        if !next.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            add(size);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;
fn sample(stage: &str, started: Instant) {
    use windows_sys::Win32::System::{ProcessStatus::*, Threading::GetCurrentProcess};
    let live = LIVE.load(Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed);
    let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&counters) as u32;
    counters.cb = size;
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            size,
        )
    };
    assert_ne!(ok, 0);
    println!(
        "{}",
        serde_json::json!({"stage":stage,"elapsed_ms":started.elapsed().as_millis(),"private_bytes":counters.PrivateUsage,"working_set":counters.WorkingSetSize,"peak_working_set":counters.PeakWorkingSetSize,"rust_heap_live":live,"rust_heap_peak":peak})
    );
}
fn load(path: &str) -> retype_dict::binary::Dictionary {
    let (dict, _) = retype_dict::binary::load(std::fs::File::open(path).unwrap()).unwrap();
    println!(
        "{}",
        serde_json::json!({"entries":dict.len(),"nodes":dict.node_count()})
    );
    let [entries, text, capacity, nodes, used_nodes, edges, terminals] = dict.allocation_stats();
    println!(
        "{}",
        serde_json::json!({"allocation_breakdown":{"entries":entries,"arc_text_estimate":text,"node_capacity":capacity,"nodes_reserved_bytes":nodes,"nodes_used_bytes":used_nodes,"edges":edges,"terminal_ids":terminals}})
    );
    dict
}
fn candidate() {
    use retype_candidate_ui::surface::Surface;
    use retype_types::{Candidate, CandidateSource, RenderState};
    let start = Instant::now();
    sample("candidate_baseline", start);
    let mut surface = Surface::default();
    sample("candidate_fonts_loaded", start);
    let mut state = RenderState {
        status: retype_types::StatusFlags::CHINESE,
        candidates: [
            "我是", "我说", "我市", "我想", "我上", "我时", "我司", "卧室",
        ]
        .iter()
        .map(|s| Candidate::new(*s, CandidateSource::Local))
        .collect(),
        page_size: 8,
        ..Default::default()
    };
    let widths = surface.measure(&state, 472, 1.0);
    sample("candidate_first_measure", start);
    let bitmap = surface.render(&state, &widths, 2, 480, 1.0);
    sample("candidate_first_bitmap", start);
    drop(bitmap);
    for i in 0..500 {
        state.candidates = (0..8)
            .map(|j| {
                Candidate::new(
                    char::from_u32(0x4e00 + (i * 8 + j) % 2000)
                        .unwrap()
                        .to_string(),
                    CandidateSource::Local,
                )
            })
            .collect();
        let w = surface.measure(&state, 472, 1.0);
        std::hint::black_box(surface.render(&state, &w, 2, 480, 1.0));
    }
    sample("candidate_500_updates_2000_glyphs", start);
    for i in 0..10_000 {
        state.candidates = (0..8)
            .map(|j| {
                Candidate::new(
                    char::from_u32(0x4e00 + (i * 8 + j) % 2000)
                        .unwrap()
                        .to_string(),
                    CandidateSource::Local,
                )
            })
            .collect();
        let widths = surface.measure(&state, 472, 1.0);
        std::hint::black_box(surface.render(&state, &widths, 2, 480, 1.0));
    }
    sample("candidate_10000_updates_2000_glyphs", start);
    let mut second = Surface::default();
    let widths = second.measure(&state, 472, 1.0);
    std::hint::black_box(second.render(&state, &widths, 2, 480, 1.0));
    sample("candidate_second_surface", start);
    drop(second);
    drop(surface);
    sample("candidate_all_dropped", start);
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let start = Instant::now();
    match args.get(1).map(String::as_str).unwrap_or("") {
        "compile" => {
            let source = std::io::BufReader::new(std::fs::File::open(&args[2]).unwrap());
            let destination = std::io::BufWriter::new(std::fs::File::create_new(&args[3]).unwrap());
            let records = retype_dict::binary::compile(source, destination).unwrap();
            println!("{}", serde_json::json!({"compiled_records":records}));
        }
        "verify" => {
            use std::io::BufRead;
            let old = load(&args[2]);
            let new = retype_dict::binary::open_shared(std::path::Path::new(&args[3])).unwrap();
            assert_eq!(old.len(), new.len());
            assert_eq!(old.total_frequency(), new.total_frequency());
            let input = std::io::BufReader::new(std::fs::File::open(&args[4]).unwrap());
            let mut checked = 0usize;
            let mut a = Vec::new();
            let mut b = Vec::new();
            for line in input.lines() {
                let line = line.unwrap();
                if line.trim().is_empty() || line.starts_with('#') {
                    continue;
                }
                let pinyin = line.split('\t').nth(1).unwrap();
                let ids = retype_dict::annotate::parse_pinyin(pinyin).unwrap();
                a.clear();
                b.clear();
                old.lookup(&ids, &mut a);
                new.lookup(&ids, &mut b);
                assert_eq!(a.len(), b.len());
                for (a, b) in a.iter().zip(&b) {
                    assert_eq!(
                        (&a.text, a.logp.to_bits(), a.flags),
                        (&b.text, b.logp.to_bits(), b.flags)
                    );
                }
                for n in 0..=ids.len() {
                    assert_eq!(old.has_prefix(&ids[..n]), new.has_prefix(&ids[..n]));
                }
                checked += 1;
            }
            println!(
                "{}",
                serde_json::json!({"verified_source_records":checked,"lookup_order_scores_flags_prefixes_identical":true,"elapsed_ms":start.elapsed().as_millis()})
            );
        }
        "mapped" | "hold-map" | "mapped-queries" => {
            sample("mapped_baseline", start);
            let dict = retype_dict::binary::open_shared(std::path::Path::new(&args[2])).unwrap();
            sample("mapped_loaded", start);
            println!(
                "{}",
                serde_json::json!({"entries":dict.len(),"nodes":dict.node_count(),"mapped_bytes":dict.image_bytes()})
            );
            let second = retype_dict::binary::open_shared(std::path::Path::new(&args[2])).unwrap();
            assert!(Arc::ptr_eq(&dict, &second));
            sample("mapped_second_reference", start);
            if args[1] == "mapped-queries" {
                use std::io::BufRead;
                let source = std::io::BufReader::new(std::fs::File::open(&args[3]).unwrap());
                let mut hits = Vec::new();
                for (i, line) in source.lines().take(100_000).enumerate() {
                    let line = line.unwrap();
                    if line.trim().is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let ids = retype_dict::annotate::parse_pinyin(line.split('\t').nth(1).unwrap())
                        .unwrap();
                    hits.clear();
                    dict.lookup(&ids, &mut hits);
                    if i == 999 {
                        sample("mapped_after_1000_queries", start);
                    }
                }
                sample("mapped_after_100000_queries", start);
            }
            if args[1] == "hold-map" {
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
            drop(second);
            drop(dict);
            sample("mapped_dropped", start);
        }
        "dict" => {
            sample("dict_baseline", start);
            let dict = load(&args[2]);
            sample("dict_loaded", start);
            drop(dict);
            sample("dict_dropped", start);
            let dict = load(&args[2]);
            sample("dict_reloaded", start);
            std::hint::black_box(&dict);
        }
        "candidate" => candidate(),
        "broker" => {
            sample("broker_baseline", start);
            // Destination must not exist: never open/migrate the live user DB.
            let scratch = std::path::Path::new(&args[4]);
            assert!(!scratch.exists());
            let source = rusqlite::Connection::open_with_flags(
                &args[3],
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let count: i64 = source
                .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
                .unwrap();
            println!("{}", serde_json::json!({"learning_entries":count}));
            source.backup(rusqlite::MAIN_DB, scratch, None).unwrap();
            drop(source);
            let system = retype_dict::AsyncDict::empty();
            let store = retype_learning::store::Store::open(
                scratch,
                Arc::clone(&system) as Arc<dyn Lexicon>,
            )
            .unwrap();
            sample("broker_user_db_loaded", start);
            system.install_binary(
                retype_dict::binary::open_shared(std::path::Path::new(&args[2])).unwrap(),
            );
            sample("broker_with_dictionary", start);
            std::hint::black_box(&store);
            drop(store);
            drop(system);
            sample("broker_all_dropped", start);
            // Only this invocation's newly-created backup, never a supplied source.
            std::fs::remove_file(scratch).unwrap();
        }
        _ => panic!("dict <bin> | candidate | broker <bin> <read-only-source.db> <new-scratch.db>"),
    }
}
