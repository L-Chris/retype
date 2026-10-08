//! Reproduce typing/backspace latency without installing a TIP or editing user data.
//! cargo run --release -p retype-diag --features candidate-benchmark
//!   --example typing-latency -- [input] [rounds] [flypy|full]
use retype_candidate_ui::surface::Surface;
use retype_dict::{Layer, LayeredDict, Learner, UserDict, DEFAULT_USER_BOOST};
use retype_engine::{offline_cloud, Kernel, KernelConfig};
use retype_pinyin::{DecodeOptions, Lexicon};
use retype_types::{InputEvent, InputSource, Key, Modifiers, PinyinScheme};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

fn key(key: Key) -> InputEvent {
    InputEvent::Key {
        key,
        mods: Modifiers::NONE,
        source: InputSource::Keyboard,
    }
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let input = args.get(1).map(String::as_str).unwrap_or("wdsssssssssss");
    let rounds = args
        .get(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20)
        .max(1);
    let scheme = if args.get(3).is_some_and(|s| s == "full") {
        PinyinScheme::Full
    } else {
        PinyinScheme::Flypy
    };
    let binary =
        retype_dict::binary::open_shared(std::path::Path::new("data/dict/retype-dict.bin"))?;
    println!(
        "entries={} input={input} scheme={scheme:?} rounds={rounds}",
        binary.len()
    );
    let mut base = LayeredDict::new();
    base.push(Layer {
        name: "system",
        dict: binary,
        boost: 0.0,
    });
    // Match the two dictionary layers used by the Windows session, without
    // loading credentials, user history or optional downloaded dictionaries.
    base.push(Layer {
        name: "optional",
        dict: Arc::new(UserDict::new()),
        boost: 0.0,
    });
    let user = Arc::new(UserDict::new());
    let dict: Arc<dyn Lexicon> = Arc::new(LayeredDict::with_system_and_user(
        Arc::new(base),
        Arc::clone(&user),
        DEFAULT_USER_BOOST,
    ));
    let learner = Arc::new(Learner::new(user));
    for scale in [1.0_f32, 1.5, 2.0] {
        let mut kernel = Kernel::new(
            KernelConfig {
                pinyin_scheme: scheme,
                rerank_enabled: false,
                decode: DecodeOptions {
                    page_size: 8,
                    ..Default::default()
                },
                ..Default::default()
            },
            Arc::clone(&dict),
            Arc::clone(&learner) as Arc<dyn retype_types::LearningStore>,
            offline_cloud(Duration::from_millis(10)),
        );
        let mut surface = Surface::default();
        let viewport = (480.0 * scale) as i32;
        let available = viewport - (8.0 * scale) as i32;
        let gap = (2.0 * scale) as i32;
        let mut samples: BTreeMap<(usize, usize), Vec<[f64; 5]>> = BTreeMap::new();
        let mut info = BTreeMap::new();
        for round in 0..=rounds {
            let events: Vec<_> = input
                .chars()
                .map(Key::Char)
                .chain(std::iter::repeat_n(Key::Backspace, input.len()))
                .collect();
            for (step, event) in events.into_iter().enumerate() {
                let total = Instant::now();
                let t = Instant::now();
                let actions = kernel.handle(key(event));
                let decode = ms(t);
                let t = Instant::now();
                let initial = kernel.render_state();
                let snapshot = ms(t);
                let t = Instant::now();
                let widths = surface.measure(&initial, available, scale);
                let measure = ms(t);
                let t = Instant::now();
                kernel.layout_candidates(&widths, available, gap);
                let render = kernel.render_state();
                let layout = ms(t);
                let t = Instant::now();
                if !render.visible().is_empty() {
                    let visible_widths =
                        &widths[render.page_start..render.page_start + render.visible().len()];
                    std::hint::black_box(surface.render(
                        &render,
                        visible_widths,
                        gap,
                        viewport,
                        scale,
                    ));
                }
                let paint = ms(t);
                std::hint::black_box(actions);
                let elapsed = ms(total);
                let direction = usize::from(step >= input.len());
                let key = (direction, step);
                info.insert(
                    key,
                    (
                        render.composition.clone(),
                        render.candidates.len(),
                        render.visible().len(),
                    ),
                );
                if round == 0 {
                    println!("cold scale={scale} dir={direction} text={:?} n={} visible={} decode={decode:.3} measure={measure:.3} raster={paint:.3} total={elapsed:.3}", render.composition, render.candidates.len(), render.visible().len());
                } else {
                    samples.entry(key).or_default().push([
                        decode,
                        measure,
                        paint,
                        snapshot + layout,
                        elapsed,
                    ]);
                }
            }
        }
        println!("warm scale,dir,text,candidates,visible,decode_p50,decode_p99,measure_p50,raster_p50,copy_layout_p50,total_p50,total_p99");
        for (key, rows) in samples {
            let (text, count, visible) = &info[&key];
            let mut columns: [Vec<f64>; 5] =
                std::array::from_fn(|i| rows.iter().map(|r| r[i]).collect());
            for column in &mut columns {
                column.sort_by(f64::total_cmp);
            }
            let mid = rounds / 2;
            let high = ((rounds as f64 * 0.99).ceil() as usize).saturating_sub(1);
            println!(
                "{scale},{},{text:?},{count},{visible},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
                key.0,
                columns[0][mid],
                columns[0][high],
                columns[1][mid],
                columns[2][mid],
                columns[3][mid],
                columns[4][mid],
                columns[4][high]
            );
        }
    }
    Ok(())
}
