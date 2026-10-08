//! On-demand egui candidate rendering into a native-window-compatible bitmap.
//! No event loop, GPU device, keyboard interception, or platform calls live here.
use egui::{epaint, Color32, FontId, Pos2, Rect, Vec2};
use retype_types::RenderState;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

fn shared_cjk_font() -> Option<&'static [u8]> {
    static FONT: OnceLock<Option<retype_file_map::ReadOnlyFile>> = OnceLock::new();
    FONT.get_or_init(|| {
        let directory = std::path::PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts");
        ["msyh.ttc", "msjh.ttc", "simhei.ttf", "simsun.ttc"]
            .iter()
            .find_map(|name| {
                retype_file_map::ReadOnlyFile::open(&directory.join(name), 64 * 1024 * 1024).ok()
            })
    })
    .as_ref()
    .map(AsRef::as_ref)
}

const BACKGROUND: Color32 = Color32::from_rgb(252, 252, 253);
const ACCENT: Color32 = Color32::from_rgb(19, 143, 150);
const INK: Color32 = Color32::from_rgb(52, 61, 68);
const CANDIDATE_FONT_SIZE: f32 = 16.0;
const MUTED: Color32 = Color32::from_rgb(142, 147, 154);

#[derive(Clone, Debug)]
pub struct Geometry {
    /// Physical pixel coordinates, also used for native mouse hit testing.
    pub cells: Vec<Rect>,
    pub width: usize,
    pub height: usize,
}

impl Geometry {
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        self.cells
            .iter()
            .position(|cell| x >= cell.min.x && x < cell.max.x && y >= cell.min.y && y < cell.max.y)
    }
}

pub struct Bitmap {
    pub geometry: Geometry,
    /// Opaque, top-down BGRX pixels for Win32 BI_RGB presentation.
    pub pixels: Vec<u32>,
}

struct Texture {
    size: [usize; 2],
    pixels: Vec<Color32>,
    alpha: Option<Vec<u8>>,
    monochrome: bool,
}

/// One cache per TSF apartment, retained across compositions. Fonts and atlas are
/// initialized lazily; idle windows do not run frames or timers.
pub struct Surface {
    context: egui::Context,
    textures: HashMap<egui::TextureId, Texture>,
    initialized: bool,
    widths: HashMap<String, (f32, u64)>,
    width_bytes: usize,
    width_tick: u64,
    width_scale: u32,
    last_frame: Option<(PaintKey, Arc<Bitmap>)>,
}

const WIDTH_CACHE_ENTRIES: usize = 4096;
const WIDTH_CACHE_BYTES: usize = 128 * 1024;

struct PaintKey {
    texts: Vec<String>,
    selected: Option<usize>,
    widths: Vec<i32>,
    gap: i32,
    viewport: i32,
    scale: u32,
}

fn painted_selection(state: &RenderState) -> Option<usize> {
    (state.status.contains(retype_types::StatusFlags::CHINESE)
        || state
            .status
            .contains(retype_types::StatusFlags::ENGLISH_SELECTED))
    .then(|| state.selected.checked_sub(state.page_start))
    .flatten()
    .filter(|&index| index < state.visible().len())
}

impl PaintKey {
    fn matches(
        &self,
        state: &RenderState,
        widths: &[i32],
        gap: i32,
        viewport: i32,
        scale: f32,
    ) -> bool {
        self.selected == painted_selection(state)
            && self.widths == widths
            && self.gap == gap
            && self.viewport == viewport
            && self.scale == scale.to_bits()
            && self
                .texts
                .iter()
                .map(String::as_str)
                .eq(state.visible().iter().map(|c| c.text.as_str()))
    }
}

impl Default for Surface {
    fn default() -> Self {
        let context = egui::Context::default();
        // The popup has a light background; dark-mode font coverage would
        // artificially thicken the edges of black-on-white candidate text.
        context.set_theme(egui::Theme::Light);
        let mut fonts = egui::FontDefinitions::default();
        if let Some(data) = shared_cjk_font() {
            fonts.font_data.insert(
                "candidate-cjk".into(),
                Arc::new(egui::FontData::from_static(data)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "candidate-cjk".into());
        }
        context.set_fonts(fonts);
        Self {
            context,
            textures: HashMap::new(),
            initialized: false,
            widths: HashMap::new(),
            width_bytes: 0,
            width_tick: 0,
            width_scale: 0,
            last_frame: None,
        }
    }
}

impl Surface {
    fn input(&self, width: f32, height: f32, scale: f32) -> egui::RawInput {
        self.context.set_pixels_per_point(scale);
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height))),
            ..Default::default()
        }
    }

    fn apply_textures(&mut self, delta: &mut epaint::textures::TexturesDelta) {
        for (id, changes) in &delta.set {
            for change in changes {
                let epaint::ImageData::Color(image) = &change.image;
                let pixels = &image.pixels;
                let size = image.size;
                let monochrome = pixels.iter().all(|pixel| {
                    let [r, g, b, a] = pixel.to_array();
                    r == a && g == a && b == a
                });
                if let Some([x, y]) = change.pos {
                    if let Some(texture) = self.textures.get_mut(id) {
                        if !monochrome {
                            if let Some(alpha) = texture.alpha.take() {
                                texture.pixels = alpha
                                    .into_iter()
                                    .map(|a| Color32::from_rgba_premultiplied(a, a, a, a))
                                    .collect();
                            }
                        }
                        texture.monochrome &= monochrome;
                        for row in 0..size[1] {
                            let start = (y + row) * texture.size[0] + x;
                            if let Some(alpha) = &mut texture.alpha {
                                if let Some(target) = alpha.get_mut(start..start + size[0]) {
                                    for (to, from) in target
                                        .iter_mut()
                                        .zip(&pixels[row * size[0]..(row + 1) * size[0]])
                                    {
                                        *to = from.a();
                                    }
                                }
                            } else if let Some(target) =
                                texture.pixels.get_mut(start..start + size[0])
                            {
                                target.copy_from_slice(&pixels[row * size[0]..(row + 1) * size[0]]);
                            }
                        }
                    }
                } else {
                    self.textures.insert(
                        *id,
                        Texture {
                            size,
                            pixels: if monochrome {
                                Vec::new()
                            } else {
                                pixels.clone()
                            },
                            alpha: monochrome.then(|| pixels.iter().map(Color32::a).collect()),
                            monochrome,
                        },
                    );
                }
            }
        }
        delta.set.clear();
    }

    fn free_textures(&mut self, delta: &mut epaint::textures::TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
        delta.clear();
    }

    fn initialize(&mut self, scale: f32) {
        // fonts() needs an initialized pass; keep its atlas delta for the first paint.
        if !self.initialized || self.context.pixels_per_point() != scale {
            let mut output = self.context.run_ui(self.input(480.0, 38.0, scale), |_| {});
            self.apply_textures(&mut output.textures_delta);
            self.free_textures(&mut output.textures_delta);
            self.initialized = true;
        }
    }

    pub fn measure(&mut self, state: &RenderState, available: i32, scale: f32) -> Vec<i32> {
        if self.width_scale != scale.to_bits() {
            self.widths.clear();
            self.width_bytes = 0;
            self.width_scale = scale.to_bits();
        }
        self.initialize(scale);
        self.context.fonts_mut(|fonts| {
            state
                .candidates
                .iter()
                .map(|candidate| {
                    self.width_tick = self.width_tick.wrapping_add(1);
                    let width = if let Some((width, used)) = self.widths.get_mut(&candidate.text) {
                        *used = self.width_tick;
                        *width
                    } else {
                        let width = fonts
                            .layout_no_wrap(
                                candidate.text.clone(),
                                FontId::proportional(CANDIDATE_FONT_SIZE),
                                INK,
                            )
                            .size()
                            .x;
                        // Cache metrics, not galleys or font-atlas UV coordinates.
                        // The latter become invalid when egui rebuilds its atlas.
                        if candidate.text.len() <= WIDTH_CACHE_BYTES / 8 {
                            if self.widths.len() >= WIDTH_CACHE_ENTRIES
                                || self.width_bytes + candidate.text.len() > WIDTH_CACHE_BYTES
                            {
                                let cutoff = self
                                    .width_tick
                                    .saturating_sub((WIDTH_CACHE_ENTRIES / 2) as u64);
                                self.widths.retain(|_, (_, used)| *used > cutoff);
                                self.width_bytes = self.widths.keys().map(String::len).sum();
                                if self.widths.len() >= WIDTH_CACHE_ENTRIES
                                    || self.width_bytes + candidate.text.len() > WIDTH_CACHE_BYTES
                                {
                                    self.widths.clear();
                                    self.width_bytes = 0;
                                }
                            }
                            self.width_bytes += candidate.text.len();
                            self.widths
                                .insert(candidate.text.clone(), (width, self.width_tick));
                        }
                        width
                    };
                    // Round upwards: pagination must never underestimate the painted width.
                    ((width + 22.0) * scale)
                        .ceil()
                        .max(36.0 * scale)
                        .min(available as f32) as i32
                })
                .collect()
        })
    }

    pub fn render(
        &mut self,
        state: &RenderState,
        widths: &[i32],
        gap: i32,
        viewport: i32,
        scale: f32,
    ) -> Arc<Bitmap> {
        if let Some((key, bitmap)) = &self.last_frame {
            if key.matches(state, widths, gap, viewport, scale) {
                return Arc::clone(bitmap);
            }
        }
        self.initialize(scale);
        let pad = (4.0 * scale).round();
        let chinese = state.status.contains(retype_types::StatusFlags::CHINESE);
        // Both modes show candidate numbers. Match measure(), retaining one
        // point of rounding slack between measured and available widths.
        let text_padding = 21.0;
        let galleys = self.context.fonts_mut(|fonts| {
            state
                .visible()
                .iter()
                .zip(widths)
                .map(|(candidate, width)| {
                    fonts.layout(
                        candidate.text.clone(),
                        FontId::proportional(CANDIDATE_FONT_SIZE),
                        INK,
                        (*width as f32 / scale - text_padding).max(1.0),
                    )
                })
                .collect::<Vec<_>>()
        });
        let row = galleys
            .iter()
            .map(|g| (g.size().y * scale).ceil() + (10.0 * scale).round())
            .fold((30.0 * scale).round(), f32::max);
        let mut x = pad;
        let cells: Vec<_> = widths
            .iter()
            .map(|width| {
                let rect = Rect::from_min_size(Pos2::new(x, pad), Vec2::new(*width as f32, row));
                x += *width as f32 + gap as f32;
                rect
            })
            .collect();
        let width = if cells.is_empty() {
            (60.0 * scale).round()
        } else {
            x - gap as f32 + pad
        }
        .min(viewport as f32)
        .max(1.0) as usize;
        let height = (row + 2.0 * pad).max(1.0) as usize;
        let geometry = Geometry {
            cells,
            width,
            height,
        };
        let input = self.input(width as f32 / scale, height as f32 / scale, scale);
        let mut output = self.context.run_ui(input, |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let bounds =
                Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32) / scale);
            painter.rect_filled(bounds, 5, BACKGROUND);
            painter.rect_stroke(
                bounds,
                5,
                egui::Stroke::new(1.0_f32, Color32::from_rgb(223, 226, 230)),
                egui::StrokeKind::Inside,
            );
            for (i, (cell, galley)) in geometry.cells.iter().zip(&galleys).enumerate() {
                let cell = Rect::from_min_max(cell.min / scale, cell.max / scale);
                let selected = state.page_start + i == state.selected
                    && (chinese
                        || state
                            .status
                            .contains(retype_types::StatusFlags::ENGLISH_SELECTED));
                if selected {
                    painter.rect_filled(cell, 4, ACCENT);
                }
                painter.text(
                    Pos2::new(cell.left() + 4.0, cell.center().y),
                    egui::Align2::LEFT_CENTER,
                    (i + 1).to_string(),
                    FontId::proportional(11.0),
                    if selected { Color32::WHITE } else { MUTED },
                );
                painter.galley_with_override_text_color(
                    Pos2::new(cell.left() + 17.0, cell.top() + 5.0),
                    Arc::clone(galley),
                    if selected { Color32::WHITE } else { INK },
                );
            }
        });
        self.apply_textures(&mut output.textures_delta);
        let primitives = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        let mut pixels = vec![0x00fcfcfd; width * height];
        for primitive in primitives {
            if let epaint::Primitive::Mesh(mesh) = primitive.primitive {
                if let Some(texture) = self.textures.get(&mesh.texture_id) {
                    rasterize(
                        &mut pixels,
                        width,
                        height,
                        &mesh,
                        primitive.clip_rect,
                        scale,
                        texture,
                    );
                }
            }
        }
        self.free_textures(&mut output.textures_delta);
        let bitmap = Arc::new(Bitmap { geometry, pixels });
        self.last_frame = Some((
            PaintKey {
                texts: state.visible().iter().map(|c| c.text.clone()).collect(),
                selected: painted_selection(state),
                widths: widths.to_vec(),
                gap,
                viewport,
                scale: scale.to_bits(),
            },
            Arc::clone(&bitmap),
        ));
        bitmap
    }
}

#[inline(always)]
fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}
#[inline(always)]
fn top_left(a: Pos2, b: Pos2) -> bool {
    b.y < a.y || (b.y == a.y && b.x > a.x)
}

fn rasterize(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    mesh: &epaint::Mesh,
    clip: Rect,
    scale: f32,
    texture: &Texture,
) {
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let Some(a) = mesh.vertices.get(triangle[0] as usize) else {
            continue;
        };
        let Some(b) = mesh.vertices.get(triangle[1] as usize) else {
            continue;
        };
        let Some(c) = mesh.vertices.get(triangle[2] as usize) else {
            continue;
        };
        let mut vertices = [a, b, c];
        let mut points = [a.pos * scale, b.pos * scale, c.pos * scale];
        let mut area = edge(points[0], points[1], points[2]);
        if area < 0.0 {
            vertices.swap(1, 2);
            points.swap(1, 2);
            area = -area;
        }
        if area <= f32::EPSILON {
            continue;
        }
        let colors = vertices.map(|v| v.color.to_array().map(f32::from));
        let uniform_color = (a.color == b.color && b.color == c.color).then_some(colors[0]);
        let uniform_texel = (a.uv == b.uv && b.uv == c.uv).then(|| sample(texture, a.uv.to_vec2()));
        let solid = uniform_color.zip(uniform_texel).and_then(|(color, texel)| {
            (color[3] == 255.0 && texel[3] == 255.0).then(|| {
                let channel = |i: usize| (color[i] * texel[i] / 255.0).round() as u32;
                channel(0) << 16 | channel(1) << 8 | channel(2)
            })
        });
        let inclusive = [
            top_left(points[1], points[2]),
            top_left(points[2], points[0]),
            top_left(points[0], points[1]),
        ];
        let x0 = points
            .iter()
            .map(|p| p.x)
            .fold(f32::INFINITY, f32::min)
            .max(clip.left() * scale)
            .floor()
            .max(0.0) as usize;
        let x1 = points
            .iter()
            .map(|p| p.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .min(clip.right() * scale)
            .ceil()
            .min(width as f32)
            .max(0.0) as usize;
        let y0 = points
            .iter()
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min)
            .max(clip.top() * scale)
            .floor()
            .max(0.0) as usize;
        let y1 = points
            .iter()
            .map(|p| p.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .min(clip.bottom() * scale)
            .ceil()
            .min(height as f32)
            .max(0.0) as usize;
        if let Some(color) = solid {
            // Convex triangles cover one continuous span per scanline. Find
            // its ends with the exact same edge/top-left tests as the pixel
            // path, then fill it without per-pixel geometry or blending.
            for y in y0..y1 {
                let py = y as f32 + 0.5;
                if py < clip.top() * scale || py >= clip.bottom() * scale {
                    continue;
                }
                let mut lo = x0.max((clip.left() * scale - 0.5).ceil().max(0.0) as usize);
                let mut hi = x1.min((clip.right() * scale - 0.5).ceil().max(0.0) as usize);
                for (i, (a, b)) in [
                    (points[1], points[2]),
                    (points[2], points[0]),
                    (points[0], points[1]),
                ]
                .into_iter()
                .enumerate()
                {
                    let inside = |x: usize| {
                        let value = edge(a, b, Pos2::new(x as f32 + 0.5, py));
                        value > 0.0 || value == 0.0 && inclusive[i]
                    };
                    if a.y == b.y {
                        if !inside(lo) {
                            hi = lo;
                        }
                    } else {
                        let increasing = a.y > b.y;
                        let (mut left, mut right) = (lo, hi);
                        while left < right {
                            let mid = left + (right - left) / 2;
                            if inside(mid) == increasing {
                                right = mid;
                            } else {
                                left = mid + 1;
                            }
                        }
                        if increasing {
                            lo = left;
                        } else {
                            hi = left;
                        }
                    }
                    if lo >= hi {
                        break;
                    }
                }
                if lo < hi {
                    pixels[y * width + lo..y * width + hi].fill(color);
                }
            }
            continue;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                if p.x < clip.left() * scale
                    || p.x >= clip.right() * scale
                    || p.y < clip.top() * scale
                    || p.y >= clip.bottom() * scale
                {
                    continue;
                }
                let e = [
                    edge(points[1], points[2], p),
                    edge(points[2], points[0], p),
                    edge(points[0], points[1], p),
                ];
                // Shared triangle edges belong to exactly one triangle (no double alpha).
                if e.iter()
                    .enumerate()
                    .any(|(i, v)| *v < 0.0 || (*v == 0.0 && !inclusive[i]))
                {
                    continue;
                }
                let bary = [e[0] / area, e[1] / area, e[2] / area];
                let uv = vertices
                    .iter()
                    .zip(bary)
                    .fold(Vec2::ZERO, |sum, (v, w)| sum + v.uv.to_vec2() * w);
                let texel = uniform_texel.unwrap_or_else(|| sample(texture, uv));
                let mut source = [0.0; 4];
                for (channel, output) in source.iter_mut().enumerate() {
                    let color = uniform_color
                        .map(|color| color[channel])
                        .unwrap_or_else(|| {
                            colors[0][channel] * bary[0]
                                + colors[1][channel] * bary[1]
                                + colors[2][channel] * bary[2]
                        });
                    *output = color * texel[channel] / 255.0;
                }
                let destination = pixels[y * width + x];
                let inverse = 1.0 - source[3] / 255.0;
                let channels = [
                    ((destination >> 16) & 255) as f32,
                    ((destination >> 8) & 255) as f32,
                    (destination & 255) as f32,
                ];
                let blend = |i: usize| {
                    (source[i] + channels[i] * inverse)
                        .round()
                        .clamp(0.0, 255.0) as u32
                };
                pixels[y * width + x] = blend(0) << 16 | blend(1) << 8 | blend(2);
            }
        }
    }
}

#[inline(always)]
fn sample(texture: &Texture, uv: Vec2) -> [f32; 4] {
    let x = (uv.x * texture.size[0] as f32 - 0.5).clamp(0.0, (texture.size[0] - 1) as f32);
    let y = (uv.y * texture.size[1] as f32 - 0.5).clamp(0.0, (texture.size[1] - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(texture.size[0] - 1);
    let y1 = (y0 + 1).min(texture.size[1] - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    if texture.monochrome {
        let at = |x: usize, y: usize| {
            let index = y * texture.size[0] + x;
            texture
                .alpha
                .as_ref()
                .map_or_else(|| texture.pixels[index].a(), |a| a[index]) as f32
        };
        let alpha = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
            + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
        return [alpha; 4];
    }
    let mut result = [0.0; 4];
    for (channel, value) in result.iter_mut().enumerate() {
        let at =
            |x: usize, y: usize| texture.pixels[y * texture.size[0] + x].to_array()[channel] as f32;
        *value = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
            + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use retype_types::{Candidate, CandidateSource};
    #[test]
    fn coverage_texture_preserves_every_sample_and_promotes_colored_updates() {
        let mut surface = Surface::default();
        let id = egui::TextureId::Managed(123);
        let colors: Vec<_> = [0, 64, 128, 255]
            .into_iter()
            .map(|a| Color32::from_rgba_premultiplied(a, a, a, a))
            .collect();
        let rgba = Texture {
            size: [2, 2],
            pixels: colors.clone(),
            alpha: None,
            monochrome: true,
        };
        let mut delta = epaint::textures::TexturesDelta::default();
        delta.set.insert(
            id,
            vec![epaint::ImageDelta::full(
                egui::ColorImage::new([2, 2], colors),
                egui::TextureOptions::LINEAR,
            )]
            .into(),
        );
        surface.apply_textures(&mut delta);
        let stored = &surface.textures[&id];
        assert!(stored.pixels.is_empty());
        for x in 0..=10 {
            for y in 0..=10 {
                let uv = Vec2::new(x as f32 / 10.0, y as f32 / 10.0);
                assert_eq!(sample(&rgba, uv), sample(stored, uv));
            }
        }
        delta.set.insert(
            id,
            vec![epaint::ImageDelta::partial(
                [1, 1],
                egui::ColorImage::new([1, 1], vec![Color32::RED]),
                egui::TextureOptions::LINEAR,
            )]
            .into(),
        );
        surface.apply_textures(&mut delta);
        let stored = &surface.textures[&id];
        assert!(stored.alpha.is_none());
        assert!(!stored.monochrome);
        assert_eq!(stored.pixels[3], Color32::RED);
        assert_eq!(
            stored.pixels[1],
            Color32::from_rgba_premultiplied(64, 64, 64, 64)
        );
    }
    fn state(texts: &[&str]) -> RenderState {
        RenderState {
            status: retype_types::StatusFlags::CHINESE,
            composition: "nihao".into(),
            page_size: texts.len(),
            candidates: texts
                .iter()
                .map(|text| Candidate::new(*text, CandidateSource::Local))
                .collect(),
            ..Default::default()
        }
    }
    #[test]
    fn same_metrics_drive_pixels_and_clicks_at_multiple_scales() {
        let mut surface = Surface::default();
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let state = state(&["你好", "我是", "快捷键", "乔布斯", "额度"]);
            let available = (472.0 * scale) as i32;
            let widths = surface.measure(&state, available, scale);
            let bitmap = surface.render(
                &state,
                &widths,
                (2.0 * scale) as i32,
                (480.0 * scale) as i32,
                scale,
            );
            assert!(bitmap.geometry.width <= (480.0 * scale) as usize);
            assert_eq!(
                bitmap.pixels.len(),
                bitmap.geometry.width * bitmap.geometry.height
            );
            for (i, cell) in bitmap.geometry.cells.iter().enumerate() {
                assert!(
                    surface
                        .context
                        .fonts_mut(|fonts| fonts
                            .layout_no_wrap(
                                state.candidates[i].text.clone(),
                                FontId::proportional(CANDIDATE_FONT_SIZE),
                                INK
                            )
                            .size()
                            .x)
                        .ceil()
                        <= (cell.width() / scale - 21.0).ceil()
                );
                assert_eq!(
                    bitmap.geometry.hit_test(cell.center().x, cell.center().y),
                    Some(i)
                );
            }
            assert_eq!(bitmap.geometry.hit_test(0.0, 0.0), None);
        }
    }
    #[test]
    fn english_completion_words_stay_on_one_row_at_multiple_scales() {
        let mut surface = Surface::default();
        let mut state = state(&[
            "article",
            "articles",
            "artist",
            "artists",
            "artificial",
            "artistic",
            "artillery",
        ]);
        state.status = retype_types::StatusFlags::default();
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let widths = surface.measure(&state, (472.0 * scale) as i32, scale);
            let bitmap = surface.render(
                &state,
                &widths,
                (2.0 * scale) as i32,
                (480.0 * scale) as i32,
                scale,
            );
            let single_line_height = surface.context.fonts_mut(|fonts| {
                state
                    .candidates
                    .iter()
                    .map(|candidate| {
                        let galley = fonts.layout_no_wrap(
                            candidate.text.clone(),
                            FontId::proportional(CANDIDATE_FONT_SIZE),
                            INK,
                        );
                        (galley.size().y * scale).ceil() + (10.0 * scale).round()
                    })
                    .fold((30.0 * scale).round(), f32::max)
            });
            let expected_height = single_line_height + 2.0 * (4.0 * scale).round();
            assert_eq!(bitmap.geometry.height, expected_height as usize);
            for (i, cell) in bitmap.geometry.cells.iter().enumerate() {
                assert_eq!(
                    bitmap.geometry.hit_test(cell.center().x, cell.center().y),
                    Some(i)
                );
            }
        }
    }

    #[test]
    fn long_phrase_wraps_without_ellipsis_and_atlas_survives_updates() {
        let mut surface = Surface::default();
        let state = state(&["这是一个很长的候选词组用于验证完整显示而不是省略号"
            .repeat(4)
            .as_str()]);
        let widths = surface.measure(&state, 472, 1.0);
        let bitmap = surface.render(&state, &widths, 2, 480, 1.0);
        assert_eq!(bitmap.geometry.width, 480);
        assert!(bitmap.geometry.height > 38);
        let again = surface.render(&state, &widths, 2, 480, 1.0);
        assert_eq!(bitmap.pixels, again.pixels);
    }

    #[test]
    fn selected_cell_tracks_absolute_index_on_later_pages() {
        let mut surface = Surface::default();
        let mut state = state(&["之前", "上一页", "你好", "额度"]);
        state.page_start = 2;
        state.page_size = 2;
        state.selected = 3;
        let widths = surface.measure(&state, 472, 1.0);
        let image = surface.render(&state, &widths[2..], 2, 480, 1.0);
        let cell = image.geometry.cells[1];
        let pixel = image.pixels
            [(cell.bottom() as usize - 3) * image.geometry.width + cell.center().x as usize];
        assert_eq!(pixel, 0x00138f96);
        assert_eq!(
            image
                .geometry
                .hit_test(cell.center().x, cell.center().y)
                .map(|i| state.page_start + i),
            Some(3)
        );
    }

    #[test]
    fn cached_widths_survive_other_frames_and_reclamp_for_viewport_and_dpi() {
        let mut surface = Surface::default();
        let original = state(&["我", "额度", "快捷键", "a longer English completion"]);
        let widths = surface.measure(&original, 472, 1.0);
        for _ in 0..5 {
            let other = state(&["另一组候选"]);
            let measured = surface.measure(&other, 472, 1.0);
            surface.render(&other, &measured, 2, 480, 1.0);
        }
        assert_eq!(surface.measure(&original, 472, 1.0), widths);
        assert_eq!(
            surface.measure(&original, 80, 1.0),
            widths.iter().map(|&w| w.min(80)).collect::<Vec<_>>()
        );
        for scale in [1.5, 2.0, 1.0] {
            let mut fresh = Surface::default();
            assert_eq!(
                surface.measure(&original, (472.0 * scale) as i32, scale),
                fresh.measure(&original, (472.0 * scale) as i32, scale)
            );
        }
        // Eviction changes storage, never the measured result.
        let many: Vec<_> = (0..WIDTH_CACHE_ENTRIES + 32)
            .map(|i| format!("word{i}"))
            .collect();
        let refs: Vec<_> = many.iter().map(String::as_str).collect();
        surface.measure(&state(&refs), 472, 1.0);
        assert!(surface.widths.len() <= WIDTH_CACHE_ENTRIES);
        assert!(surface.width_bytes <= WIDTH_CACHE_BYTES);
        assert_eq!(surface.measure(&original, 472, 1.0), widths);
    }

    #[test]
    fn bitmap_reuse_ignores_metadata_but_tracks_every_visual_input() {
        let mut surface = Surface::default();
        let mut current = state(&["你好", "额度"]);
        let widths = surface.measure(&current, 472, 1.0);
        let original = surface.render(&current, &widths, 2, 480, 1.0);
        current.gen += 1;
        current.composition = "nihc".into();
        current.candidates[0].score += 1.0;
        let same = surface.render(&current, &widths, 2, 480, 1.0);
        assert!(Arc::ptr_eq(&original, &same));
        current.selected = 1;
        let selected = surface.render(&current, &widths, 2, 480, 1.0);
        assert!(!Arc::ptr_eq(&same, &selected));
        assert_ne!(same.pixels, selected.pixels);
        current.status = retype_types::StatusFlags::EMPTY;
        let english = surface.render(&current, &widths, 2, 480, 1.0);
        assert_ne!(selected.pixels, english.pixels);
        current.candidates[0].text = "再见".into();
        let changed = surface.render(&current, &widths, 2, 480, 1.0);
        assert_ne!(english.pixels, changed.pixels);
        let resized = surface.render(&current, &widths, 4, 300, 1.0);
        assert!(!Arc::ptr_eq(&changed, &resized));
        let scaled_widths = surface.measure(&current, 944, 2.0);
        let scaled = surface.render(&current, &scaled_widths, 4, 960, 2.0);
        assert!(!Arc::ptr_eq(&resized, &scaled));
    }

    #[test]
    fn solid_scanlines_match_pixel_path_for_rotated_clipped_and_shared_edges() {
        let texture = Texture {
            size: [1, 1],
            pixels: vec![Color32::WHITE],
            alpha: None,
            monochrome: true,
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for shift in [0.0, 0.25, 0.5, 0.9] {
                let mut mesh = epaint::Mesh {
                    vertices: [
                        Pos2::new(0.0, 1.0),
                        Pos2::new(9.0, 0.0),
                        Pos2::new(7.0, 10.0),
                        Pos2::new(1.0, 8.0),
                    ]
                    .map(|p| epaint::Vertex {
                        pos: p + Vec2::splat(shift),
                        uv: Pos2::ZERO,
                        color: Color32::from_rgb(19, 143, 150),
                    })
                    .to_vec(),
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..Default::default()
                };
                let clip = Rect::from_min_max(Pos2::new(0.6, 0.7), Pos2::new(8.1, 9.2));
                let mut fast = vec![0; 24 * 24];
                rasterize(&mut fast, 24, 24, &mesh, clip, scale, &texture);
                // Unequal UVs force the per-pixel path, but a 1x1 white
                // texture still produces exactly the same color and coverage.
                for (i, vertex) in mesh.vertices.iter_mut().enumerate() {
                    vertex.uv = Pos2::new(i as f32 * 0.1, 0.0);
                }
                let mut reference = vec![0; 24 * 24];
                rasterize(&mut reference, 24, 24, &mesh, clip, scale, &texture);
                assert_eq!(fast, reference, "scale={scale} shift={shift}");
            }
        }
    }

    #[test]
    fn shared_edges_and_clip_do_not_double_blend_or_overdraw() {
        let texture = Texture {
            size: [1, 1],
            pixels: vec![Color32::WHITE],
            alpha: None,
            monochrome: true,
        };
        let mut mesh = epaint::Mesh::default();
        mesh.add_colored_rect(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(4.0)),
            Color32::from_rgba_premultiplied(128, 0, 0, 128),
        );
        let mut pixels = vec![0; 16];
        rasterize(
            &mut pixels,
            4,
            4,
            &mesh,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(3.0, 4.0)),
            1.0,
            &texture,
        );
        for row in pixels.as_chunks::<4>().0 {
            assert_eq!(row, &[0x00800000, 0x00800000, 0x00800000, 0]);
        }
    }
    #[test]
    fn export_visual_fixtures_when_requested() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(directory) = std::env::var("RETYPE_CANDIDATE_FIXTURES") else {
            return Ok(());
        };
        std::fs::create_dir_all(&directory)?;
        let mut surface = Surface::default();
        for (name,texts,scale) in [
            ("short",vec!["我","窝","握","卧","沃","喔","倭","涡"],1.0),
            ("phrases",vec!["我是","我说","我市","我想","我上","我时","我司"],1.0),
            ("mixed",vec!["快捷键","乔布斯","Rust","Visual Studio"],1.5),
            ("long",vec!["这是一个很长的候选词组用于验证完整显示而不是省略号这是一个很长的候选词组用于验证完整显示而不是省略号"],1.0),
        ] {
            let state = state(&texts);
            let widths = surface.measure(&state,(472.0*scale) as i32,scale);
            let image = surface.render(&state,&widths,(2.0*scale) as i32,(480.0*scale) as i32,scale);
            let file = std::fs::File::create(std::path::Path::new(&directory).join(format!("{name}.png")))?;
            let mut encoder = png::Encoder::new(file,image.geometry.width as u32,image.geometry.height as u32);
            encoder.set_color(png::ColorType::Rgb);
            let mut writer = encoder.write_header()?;
            let bytes: Vec<_> = image.pixels.iter().flat_map(|p| [(p>>16) as u8,(p>>8) as u8,*p as u8]).collect();
            writer.write_image_data(&bytes)?;
        }
        Ok(())
    }
}
