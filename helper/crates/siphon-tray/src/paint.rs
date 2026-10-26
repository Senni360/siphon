//! Drawing primitives on a premultiplied RGBA pixmap, in logical (96-dpi) units.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, VariableFont};
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, Path,
    PathBuilder, Pixmap, Point, Rect, Shader, SpreadMode, Stroke, Transform,
};

const GEIST: &[u8] = include_bytes!("../assets/Geist-Variable.ttf");

pub struct Fonts {
    pub title: FontRef<'static>,
    pub body: FontRef<'static>,
}

impl Fonts {
    pub fn load() -> Fonts {
        let mut title = FontRef::try_from_slice(GEIST).expect("bundled font");
        title.set_variation(b"wght", 600.0);
        let mut body = FontRef::try_from_slice(GEIST).expect("bundled font");
        body.set_variation(b"wght", 400.0);
        Fonts { title, body }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub f32);

impl Rgba {
    pub fn hex(value: &str) -> Rgba {
        let v = u32::from_str_radix(value.trim_start_matches('#'), 16).unwrap_or(0xf4f4f5);
        Rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, 1.0)
    }
    pub fn alpha(self, a: f32) -> Rgba {
        Rgba(self.0, self.1, self.2, a.clamp(0.0, 1.0))
    }
    pub fn lighter(self, percent: f32) -> Rgba {
        let f = percent / 100.0;
        let c = |v: u8| ((v as f32 * f).min(255.0)) as u8;
        Rgba(c(self.0), c(self.1), c(self.2), self.3)
    }
    fn color(self, opacity: f32) -> Color {
        Color::from_rgba8(
            self.0,
            self.1,
            self.2,
            (self.3 * opacity * 255.0).round().clamp(0.0, 255.0) as u8,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, l: f32, t: f32, r: f32, b: f32) -> R {
        R::new(self.x + l, self.y + t, self.w - l - r, self.h - t - b)
    }
    pub fn scaled_about_center(&self, scale: f32) -> R {
        let (cx, cy) = self.center();
        R::new(
            cx - self.w * scale / 2.0,
            cy - self.h * scale / 2.0,
            self.w * scale,
            self.h * scale,
        )
    }
}

pub fn rounded(rect: R, radius: f32) -> Option<Path> {
    let r = radius.min(rect.w / 2.0).min(rect.h / 2.0).max(0.0);
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    // Cubic approximation of quarter circles.
    let k = r * 0.552_284_8;
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    p.line_to(x, y + r);
    p.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    p.close();
    p.finish()
}

pub struct Canvas<'a> {
    pub pixmap: &'a mut Pixmap,
    pub scale: f32,
    pub opacity: f32,
    clip: Option<Mask>,
    fonts: &'a Fonts,
}

impl<'a> Canvas<'a> {
    pub fn new(pixmap: &'a mut Pixmap, scale: f32, fonts: &'a Fonts) -> Canvas<'a> {
        Canvas {
            pixmap,
            scale,
            opacity: 1.0,
            clip: None,
            fonts,
        }
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    pub fn clip_to(&mut self, path: Option<&Path>) {
        self.clip = path.and_then(|path| {
            let mut mask = Mask::new(self.pixmap.width(), self.pixmap.height())?;
            mask.fill_path(path, FillRule::Winding, true, self.transform());
            Some(mask)
        });
    }

    pub fn fill(&mut self, path: Option<&Path>, color: Rgba) {
        let Some(path) = path else { return };
        let mut paint = Paint::default();
        paint.set_color(color.color(self.opacity));
        paint.anti_alias = true;
        self.pixmap.fill_path(
            path,
            &paint,
            FillRule::Winding,
            self.transform(),
            self.clip.as_ref(),
        );
    }

    pub fn fill_vertical_gradient(
        &mut self,
        path: Option<&Path>,
        rect: R,
        top: Rgba,
        bottom: Rgba,
    ) {
        let Some(path) = path else { return };
        let shader = LinearGradient::new(
            Point::from_xy(rect.x, rect.y),
            Point::from_xy(rect.x, rect.bottom()),
            vec![
                GradientStop::new(0.0, top.color(self.opacity)),
                GradientStop::new(1.0, bottom.color(self.opacity)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        );
        let paint = Paint {
            shader: shader.unwrap_or(Shader::SolidColor(bottom.color(self.opacity))),
            anti_alias: true,
            ..Paint::default()
        };
        self.pixmap.fill_path(
            path,
            &paint,
            FillRule::Winding,
            self.transform(),
            self.clip.as_ref(),
        );
    }

    pub fn stroke(&mut self, path: Option<&Path>, color: Rgba, width: f32) {
        let Some(path) = path else { return };
        let mut paint = Paint::default();
        paint.set_color(color.color(self.opacity));
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        self.pixmap
            .stroke_path(path, &paint, &stroke, self.transform(), self.clip.as_ref());
    }

    pub fn stroke_clipped_top(&mut self, path: Option<&Path>, color: Rgba, band: R) {
        let previous = self.clip.take();
        let band_path = PathBuilder::from_rect(
            Rect::from_xywh(band.x, band.y, band.w, band.h)
                .unwrap_or(Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap()),
        );
        self.clip_to(Some(&band_path));
        self.stroke(path, color, 1.0);
        self.clip = previous;
    }

    /// Soft drop shadow from layered rounded rectangles.
    pub fn shadow(&mut self, rect: R, radius: f32, layers: u32) {
        for i in (1..=layers).rev() {
            let grow = i as f32 * 2.2;
            let alpha = 30.0 * (1.0 - i as f32 / (layers as f32 + 1.0)).powi(2) / 255.0;
            let r = R::new(
                rect.x - grow,
                rect.y - grow + 4.0,
                rect.w + 2.0 * grow,
                rect.h + 2.0 * grow + 2.0,
            );
            self.fill(rounded(r, radius + grow).as_ref(), Rgba(0, 0, 0, alpha));
        }
    }

    /// One 1.5px stroked icon centred in `rect`: close, pin, or the Siphon mark.
    pub fn glyph(&mut self, rect: R, kind: Glyph, color: Rgba) {
        let size = rect.w.min(rect.h) * 0.46;
        let (cx, cy) = rect.center();
        let mut p = PathBuilder::new();
        match kind {
            Glyph::Close => {
                let h = size / 2.0;
                p.move_to(cx - h, cy - h);
                p.line_to(cx + h, cy + h);
                p.move_to(cx + h, cy - h);
                p.line_to(cx - h, cy + h);
            }
            Glyph::Pin => {
                let s = size / 10.0;
                for (a, b, c, d) in [
                    (-3.0, -5.0, 3.0, -5.0),
                    (-2.0, -5.0, -2.4, 0.5),
                    (2.0, -5.0, 2.4, 0.5),
                    (-4.5, 0.5, 4.5, 0.5),
                    (0.0, 0.5, 0.0, 6.0),
                ] {
                    p.move_to(cx + a * s, cy + b * s);
                    p.line_to(cx + c * s, cy + d * s);
                }
            }
            Glyph::Audio => {
                // The Siphon mark: two nested U shapes.
                let s = size / 8.0;
                for outer in [4.0f32, 1.5] {
                    let r = outer * s;
                    let k = r * 0.552_284_8;
                    p.move_to(cx - r, cy - 4.0 * s);
                    p.line_to(cx - r, cy);
                    p.cubic_to(cx - r, cy + k, cx - k, cy + r, cx, cy + r);
                    p.cubic_to(cx + k, cy + r, cx + r, cy + k, cx + r, cy);
                    p.line_to(cx + r, cy - 4.0 * s);
                }
            }
        }
        self.stroke(p.finish().as_ref(), color, 1.5);
    }

    fn font(&self, weight: Weight) -> &FontRef<'static> {
        match weight {
            Weight::Title => &self.fonts.title,
            Weight::Body => &self.fonts.body,
        }
    }

    /// Text elided with "…" to fit `max_width`.
    pub fn elide(&self, text: &str, weight: Weight, points: f32, max_width: f32) -> String {
        elide(self.font(weight), text, points * 96.0 / 72.0, max_width)
    }

    /// Draw text left-aligned, vertically centred in `rect`.
    pub fn text(&mut self, text: &str, rect: R, weight: Weight, points: f32, color: Rgba) {
        let px = points * 96.0 / 72.0 * self.scale;
        let font = match weight {
            Weight::Title => &self.fonts.title,
            Weight::Body => &self.fonts.body,
        };
        let scaled = font.as_scaled(PxScale::from(px));
        let baseline = rect.center().1 * self.scale + (scaled.ascent() + scaled.descent()) / 2.0;
        let mut x = rect.x * self.scale;
        let color = color.alpha(color.3 * self.opacity);
        let (width, height) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let pixels = self.pixmap.pixels_mut();
        let mut previous = None;
        for ch in text.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(prev) = previous {
                x += scaled.kern(prev, id);
            }
            let glyph = id.with_scale_and_position(px, ab_glyph::point(x, baseline));
            x += scaled.h_advance(id);
            previous = Some(id);
            let Some(outline) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let (px_x, px_y) = (
                    bounds.min.x as i32 + gx as i32,
                    bounds.min.y as i32 + gy as i32,
                );
                if px_x < 0 || px_y < 0 || px_x >= width || px_y >= height {
                    return;
                }
                let alpha = (coverage.clamp(0.0, 1.0) * color.3).min(1.0);
                if alpha <= 0.0 {
                    return;
                }
                let index = (px_y * width + px_x) as usize;
                let dst = pixels[index];
                let inv = 1.0 - alpha;
                let blend = |src: u8, dst: u8| {
                    ((src as f32 * alpha) + dst as f32 * inv)
                        .round()
                        .clamp(0.0, 255.0) as u8
                };
                let a = (255.0 * alpha + dst.alpha() as f32 * inv)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let (r, g, b) = (
                    blend(color.0, dst.red()),
                    blend(color.1, dst.green()),
                    blend(color.2, dst.blue()),
                );
                if let Some(value) =
                    tiny_skia::PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
                {
                    pixels[index] = value;
                }
            });
        }
    }
}

#[derive(Clone, Copy)]
pub enum Glyph {
    Close,
    Pin,
    Audio,
}

#[derive(Clone, Copy)]
pub enum Weight {
    Title,
    Body,
}

fn measure(font: &FontRef<'static>, text: &str, px: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut width = 0.0;
    let mut previous = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(prev) = previous {
            width += scaled.kern(prev, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

fn elide(font: &FontRef<'static>, text: &str, px: f32, max_width: f32) -> String {
    if measure(font, text, px) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if measure(font, &candidate, px) <= max_width {
            return candidate;
        }
    }
    "…".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elides_long_text() {
        let fonts = Fonts::load();
        let short = elide(&fonts.title, "Hi", 13.3, 200.0);
        assert_eq!(short, "Hi");
        let long = elide(&fonts.title, &"Very long title ".repeat(20), 13.3, 120.0);
        assert!(long.ends_with('…') && measure(&fonts.title, &long, 13.3) <= 120.0);
    }

    #[test]
    fn draws_text_and_shapes() {
        let fonts = Fonts::load();
        let mut pixmap = Pixmap::new(200, 60).unwrap();
        let mut canvas = Canvas::new(&mut pixmap, 2.0, &fonts);
        canvas.fill(
            rounded(R::new(2.0, 2.0, 90.0, 26.0), 8.0).as_ref(),
            Rgba(22, 22, 26, 0.96),
        );
        canvas.text(
            "Siphon",
            R::new(8.0, 4.0, 80.0, 20.0),
            Weight::Title,
            10.0,
            Rgba::hex("#f4f4f5"),
        );
        canvas.glyph(
            R::new(60.0, 4.0, 24.0, 24.0),
            Glyph::Audio,
            Rgba::hex("#ff3355"),
        );
        assert!(pixmap
            .pixels()
            .iter()
            .any(|p| p.red() > 200 && p.alpha() == 255));
    }
}
