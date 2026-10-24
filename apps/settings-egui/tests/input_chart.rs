#![cfg(not(windows))]
#![allow(dead_code, clippy::unwrap_used)]
#[path = "../src/input_chart.rs"]
mod input_chart;
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use retype_types::statistics::InputStats;
#[test]
fn clicking_a_bucket_pins_the_shared_tooltip_and_escape_clears_it() {
    let context = egui::Context::default();
    let mut input = InputStats::default();
    for n in [5, 6, 7, 8, 10] {
        input.record(true, n, true, 1);
    }
    let mut pinned = None;
    {
        let mut frame = |events: Vec<Event>| {
            let mut output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(512.0, 280.0))),
                    events,
                    ..Default::default()
                },
                |ui| input_chart::draw(ui, &input, true, &mut pinned),
            );
            output.textures_delta.clear();
        };
        frame(vec![]);
        let point = Pos2::new(45.0 + (512.0 - 88.0) / 6.0 * 4.5, 100.0);
        frame(vec![
            Event::PointerMoved(point),
            Event::PointerButton {
                pos: point,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        frame(vec![Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
    }
    assert_eq!(pinned, Some(4));
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(512.0, 280.0))),
            events: vec![Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| input_chart::draw(ui, &input, true, &mut pinned),
    );
    output.textures_delta.clear();
    assert_eq!(pinned, None);
}
