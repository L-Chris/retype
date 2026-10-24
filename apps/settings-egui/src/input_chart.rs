//! One input-share histogram with a revision-rate line; no persistent chart buckets.
#[cfg(windows)]
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use retype_types::statistics::{InputStats, LengthCounts, LENGTH_LABELS};

const BAR: Color32 = Color32::from_rgb(19, 143, 150);
const LINE: Color32 = Color32::from_rgb(218, 123, 47);
const MUTED: Color32 = Color32::from_rgb(101, 119, 129);

fn rate(row: LengthCounts) -> String {
    if row.inputs == 0 {
        "—".into()
    } else {
        format!("{:.1}%", row.corrected as f64 * 100.0 / row.inputs as f64)
    }
}
fn details(label: &str, unit: &str, row: LengthCounts, total: u64) -> String {
    let mean = if row.inputs == 0 {
        "—".into()
    } else {
        format!("{:.1} {unit}", row.units as f64 / row.inputs as f64)
    };
    format!("{label} {unit}/次\n输入 {} 次 · 占比 {:.1}%\n回改 {} 次 · 回改率 {}\n平均上屏长度 {mean}{}", row.inputs, if total == 0 {0.0} else {row.inputs as f64 * 100.0 / total as f64}, row.corrected,rate(row),if row.inputs>0 && row.inputs<5 {"\n样本较少"} else {""})
}
pub fn summary(input: &InputStats, chinese: bool, scope: &str) -> String {
    let row = input.total(chinese);
    let unit = if chinese { "字" } else { "词" };
    format!("{scope}\n成功输入 {} 次 · 回改 {} 次\n总体回改率 {}\n平均上屏长度 {}\n上屏前修改拼写，不代表输错率；上屏后的删除不计入。",row.inputs,row.corrected,rate(row),if row.inputs==0 {"—".into()} else {format!("{:.1} {unit}/次",row.units as f64/row.inputs as f64)})
}
pub fn draw(ui: &mut egui::Ui, input: &InputStats, chinese: bool, pinned: &mut Option<usize>) {
    let rows = input.grouped(chinese);
    let total = input.total(chinese).inputs;
    let unit = if chinese { "字" } else { "词" };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 220.0), Sense::hover());
    let plot = Rect::from_min_max(
        rect.min + Vec2::new(45.0, 30.0),
        rect.max - Vec2::new(43.0, 30.0),
    );
    let painter = ui.painter();
    let font = FontId::proportional(11.0);
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        "输入占比",
        font.clone(),
        BAR,
    );
    painter.text(
        rect.right_top(),
        Align2::RIGHT_TOP,
        "回改率",
        font.clone(),
        LINE,
    );
    for tick in 0..=4 {
        let f = tick as f32 / 4.0;
        let y = plot.bottom() - plot.height() * f;
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0, Color32::from_gray(232)),
        );
        painter.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            format!("{:.0}%", f * 100.0),
            font.clone(),
            MUTED,
        );
        painter.text(
            Pos2::new(plot.right() + 6.0, y),
            Align2::LEFT_CENTER,
            format!("{:.0}%", f * 100.0),
            font.clone(),
            LINE,
        );
    }
    let step = plot.width() / 6.0;
    let hovered = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| plot.contains(*p) && ui.clip_rect().contains(*p))
        .map(|p| ((p.x - plot.left()) / step).floor().min(5.0) as usize);
    let highlighted = hovered.or(*pinned);
    let mut previous = None;
    for (i, row) in rows.iter().enumerate() {
        let x = plot.left() + step * (i as f32 + 0.5);
        let share = if total == 0 {
            0.0
        } else {
            row.inputs as f64 / total as f64
        };
        let h = share as f32 * plot.height();
        let hit = Rect::from_min_max(
            Pos2::new(plot.left() + step * i as f32, plot.top()),
            Pos2::new(plot.left() + step * (i + 1) as f32, plot.bottom()),
        );
        if highlighted == Some(i) {
            painter.rect_filled(hit, 0.0, BAR.gamma_multiply(0.06));
        }
        if row.inputs > 0 {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x - step * 0.24, plot.bottom() - h),
                    Pos2::new(x + step * 0.24, plot.bottom()),
                ),
                3.0,
                BAR.gamma_multiply(if highlighted == Some(i) { 1.0 } else { 0.7 }),
            );
            let point = Pos2::new(
                x,
                plot.bottom() - plot.height() * (row.corrected as f64 / row.inputs as f64) as f32,
            );
            if let Some(from) = previous {
                painter.line_segment([from, point], Stroke::new(2.0, LINE));
            }
            painter.circle_filled(point, if highlighted == Some(i) { 4.5 } else { 3.0 }, LINE);
            previous = Some(point);
        } else {
            previous = None;
        }
        painter.text(
            Pos2::new(x, plot.bottom() + 10.0),
            Align2::CENTER_TOP,
            LENGTH_LABELS[i],
            font.clone(),
            MUTED,
        );
        let bucket = ui.interact(hit, response.id.with(i), Sense::click());
        if bucket.clicked() {
            *pinned = if *pinned == Some(i) { None } else { Some(i) };
        }
        let text = details(LENGTH_LABELS[i], unit, *row, total);
        if hovered == Some(i) {
            bucket.on_hover_text_at_pointer(text);
        } else if hovered.is_none() && *pinned == Some(i) && ui.clip_rect().intersects(hit) {
            bucket.show_tooltip_text(text);
        }
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        *pinned = None;
    }
    if total == 0 {
        ui.painter().text(
            plot.center(),
            Align2::CENTER_CENTER,
            "暂无新口径样本",
            FontId::proportional(13.0),
            MUTED,
        );
    }
}
