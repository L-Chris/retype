//! Shared Lucide geometry; generated from the vendored SVGs, without a runtime renderer.
use eframe::egui::{self, Color32};
use std::{collections::BTreeMap, sync::OnceLock};
type Paths = BTreeMap<String, Vec<Vec<[f32; 2]>>>;
pub(crate) fn draw(
    painter: &egui::Painter,
    center: egui::Pos2,
    name: &str,
    size: f32,
    color: Color32,
) {
    static PATHS: OnceLock<Paths> = OnceLock::new();
    let paths = PATHS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/icons/paths.json")).unwrap_or_default()
    });
    let Some(contours) = paths.get(name) else {
        return;
    };
    let scale = size / 24.0;
    let stroke = egui::Stroke::new(2.0 * scale, color);
    for contour in contours {
        let points: Vec<_> = contour
            .iter()
            .map(|[x, y]| center + egui::vec2((x - 12.0) * scale, (y - 12.0) * scale))
            .collect();
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            // egui paths use round joins; add SVG's round end caps explicitly.
            painter.circle_filled(*first, stroke.width / 2.0, color);
            painter.circle_filled(*last, stroke.width / 2.0, color);
        }
        painter.add(egui::Shape::line(points, stroke));
    }
}
