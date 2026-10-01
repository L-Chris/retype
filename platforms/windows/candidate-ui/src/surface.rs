//! On-demand egui candidate rendering into a native-window-compatible bitmap.
//! No event loop, GPU device, keyboard interception, or platform calls live here.
use egui::{epaint, Color32, FontId, Pos2, Rect, Vec2};
use retype_types::RenderState;
use std::{collections::HashMap, sync::Arc};

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
    monochrome: bool,
}

/// One cache per TSF apartment, retained across compositions. Fonts and atlas are
/// initialized lazily; idle windows do not run frames or timers.
pub struct Surface {
    context: egui::Context,
    textures: HashMap<egui::TextureId, Texture>,
    initialized: bool,
}

impl Default for Surface {
    fn default() -> Self {
        let context = egui::Context::default();
        // The popup has a light background; dark-mode font coverage would
        // artificially thicken the edges of black-on-white candidate text.
        context.set_theme(egui::Theme::Light);
        let mut fonts = egui::FontDefinitions::default();
        if let Some(directory) = std::env::var_os("WINDIR") {
            let directory = std::path::PathBuf::from(directory).join("Fonts");
            for name in ["msyh.ttc", "msjh.ttc", "simhei.ttf", "simsun.ttc"] {
                if let Ok(data) = std::fs::read(directory.join(name)) {
                    fonts.font_data.insert(
                        "candidate-cjk".into(),
                        Arc::new(egui::FontData::from_owned(data)),
                    );
                    fonts
                        .families
                        .entry(egui::FontFamily::Proportional)
                        .or_default()
                        .insert(0, "candidate-cjk".into());
                    break;
                }
            }
        }
        context.set_fonts(fonts);
        Self {
            context,
            textures: HashMap::new(),
            initialized: false,
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
                        texture.monochrome &= monochrome;
                        for row in 0..size[1] {
                            let start = (y + row) * texture.size[0] + x;
                            if let Some(target) = texture.pixels.get_mut(start..start + size[0]) {
                                target.copy_from_slice(&pixels[row * size[0]..(row + 1) * size[0]]);
                            }
                        }
                    }
                } else {
                    self.textures.insert(
                        *id,
                        Texture {
                            size,
                            pixels: pixels.clone(),
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
        self.initialize(scale);
        self.context.fonts_mut(|fonts| {
            state
                .candidates
                .iter()
                .map(|candidate| {
                    let galley = fonts.layout_no_wrap(
                        candidate.text.clone(),
                        FontId::proportional(CANDIDATE_FONT_SIZE),
                        INK,
                    );
                    // Round upwards: pagination must never underestimate the painted width.
                    ((galley.size().x + 22.0) * scale)
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
    ) -> Bitmap {
        self.initialize(scale);
        let pad = (4.0 * scale).round();
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
                        (*width as f32 / scale - 21.0).max(1.0),
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
                let selected = state.page_start + i == state.selected;
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
        Bitmap { geometry, pixels }
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
    for triangle in mesh.indices.chunks_exact(3) {
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
                if let Some(color) = solid {
                    pixels[y * width + x] = color;
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
        let at = |x: usize, y: usize| texture.pixels[y * texture.size[0] + x].a() as f32;
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
    fn state(texts: &[&str]) -> RenderState {
        RenderState {
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
    fn shared_edges_and_clip_do_not_double_blend_or_overdraw() {
        let texture = Texture {
            size: [1, 1],
            pixels: vec![Color32::WHITE],
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
        for row in pixels.chunks_exact(4) {
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
