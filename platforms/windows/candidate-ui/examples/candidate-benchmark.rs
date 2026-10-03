//! CPU rendering benchmark, without registering/installing the input method.
use retype_candidate_ui::surface::Surface;
use retype_types::{Candidate, CandidateSource, RenderState};
use std::time::Instant;

fn main() {
    let start = Instant::now();
    let mut surface = Surface::default();
    let mut state = RenderState {
        status: retype_types::StatusFlags::CHINESE,
        composition: "woui".into(),
        page_size: 8,
        candidates: [
            "我是", "我说", "我市", "我想", "我上", "我时", "我司", "卧室",
        ]
        .iter()
        .map(|text| Candidate::new(*text, CandidateSource::Local))
        .collect(),
        ..Default::default()
    };
    let widths = surface.measure(&state, 472, 1.0);
    std::hint::black_box(surface.render(&state, &widths, 2, 480, 1.0));
    println!(
        "first_measure_and_render_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    for scale in [1.0, 1.5, 2.0] {
        let widths = surface.measure(&state, (472.0 * scale) as i32, scale);
        // Initialize a new font atlas for this scale before timing warm updates.
        std::hint::black_box(surface.render(
            &state,
            &widths,
            (2.0 * scale) as i32,
            (480.0 * scale) as i32,
            scale,
        ));
        let mut times = Vec::with_capacity(200);
        for i in 0..200 {
            state.selected = i % 8;
            let start = Instant::now();
            let widths = surface.measure(&state, (472.0 * scale) as i32, scale);
            std::hint::black_box(surface.render(
                &state,
                &widths,
                (2.0 * scale) as i32,
                (480.0 * scale) as i32,
                scale,
            ));
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "scale={scale:.1} median_ms={:.3} p95_ms={:.3} p99_ms={:.3}",
            times[100], times[190], times[198]
        );
    }
}
