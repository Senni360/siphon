//! Tray state, layout, animation and painting, independent of the window system.
//!
//! Each card counts down and slides away. Hovering pauses every countdown; a click
//! pins a card (click again to release it); the close button or a right-click
//! dismisses it. Dragging a card drops its files into any app that accepts
//! Explorer drags, such as FL Studio's playlist or channel rack.

use std::path::Path;
use std::time::Instant;

use serde_json::Value;
use tiny_skia::Pixmap;

use crate::paint::{rounded, Canvas, Fonts, Glyph, Rgba, Weight, R};

pub const LIFETIME: f32 = 7.0; // seconds a card stays without interaction
pub const RESUME_MINIMUM: f32 = 1.6; // leaving the tray never dismisses a card instantly
pub const MAX_CARDS: usize = 5;
pub const CARD_W: f32 = 392.0;
pub const CARD_H: f32 = 66.0;
pub const GAP: f32 = 10.0;
pub const MARGIN: f32 = 28.0;
pub const RADIUS: f32 = 18.0;

// Siphon's dark glass tokens: neutral surfaces, colour only from the card's service.
const SURFACE: Rgba = Rgba(22, 22, 26, 244.0 / 255.0);
pub const RAISED: Rgba = Rgba(34, 34, 39, 250.0 / 255.0);
const SPECULAR: Rgba = Rgba(255, 255, 255, 20.0 / 255.0);
pub const TEXT: &str = "#f4f4f5";
const MUTED: &str = "#a1a1aa";

pub fn accent(service: Option<&str>) -> Rgba {
    Rgba::hex(match service {
        Some("youtube" | "youtubeMusic") => "#ff3355",
        Some("soundcloud") => "#ff6a1a",
        Some("bandcamp") => "#3cb8d8",
        _ => TEXT,
    })
}

/// Critically tuned spring: motion with weight and a gentle settle.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub target: f32,
    pub velocity: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl Spring {
    pub fn new(value: f32, stiffness: f32, damping: f32) -> Spring {
        Spring {
            value,
            target: value,
            velocity: 0.0,
            stiffness,
            damping,
        }
    }
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }
    pub fn step(&mut self, dt: f32) -> bool {
        if self.value == self.target && self.velocity == 0.0 {
            return false;
        }
        for _ in 0..4 {
            let h = dt / 4.0;
            self.velocity +=
                (self.stiffness * (self.target - self.value) - self.damping * self.velocity) * h;
            self.value += self.velocity * h;
        }
        if (self.target - self.value).abs() < 5e-4 && self.velocity.abs() < 5e-3 {
            self.snap(self.target);
            return false;
        }
        true
    }
}

pub struct Card {
    pub id: u64,
    pub files: Vec<String>,
    pub title: String,
    pub subtitle: String,
    pub accent: Rgba,
    pub pinned: bool,
    pub remaining: f32,
    pub dropped_at: Option<Instant>,
    pub peaks: Option<Vec<f32>>,
    pub closing: bool,
    pub presence: Spring,
    pub y: Spring,
    pub hover: Spring,
    pub press: Spring,
    pub pin: Spring,
    pub flash: Spring,
    pub reveal: Spring,
}

pub fn normalized(path: &str) -> String {
    path.replace('/', "\\")
}

impl Card {
    pub fn new(id: u64, item: &Value, files: Vec<String>, pinned: bool) -> Card {
        let first = Path::new(&files[0]);
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                first
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let fallback = if files.len() == 1 {
            first
                .extension()
                .map(|e| e.to_string_lossy().to_uppercase())
                .unwrap_or_default()
        } else {
            format!("{} files", files.len())
        };
        let subtitle = item
            .get("subtitle")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or(fallback);
        Card {
            id,
            files,
            title,
            subtitle,
            accent: accent(item.get("service").and_then(Value::as_str)),
            pinned,
            remaining: LIFETIME,
            dropped_at: None,
            peaks: None,
            closing: false,
            presence: Spring::new(0.0, 300.0, 26.0), // 0 hidden → 1 shown; slight overshoot
            y: Spring::new(0.0, 380.0, 34.0),
            hover: Spring::new(0.0, 600.0, 46.0),
            press: Spring::new(0.0, 900.0, 56.0),
            pin: Spring::new(if pinned { 1.0 } else { 0.0 }, 520.0, 30.0),
            flash: Spring::new(0.0, 260.0, 30.0), // drop confirmation
            reveal: Spring::new(0.0, 160.0, 26.0), // waveform grows in once measured
        }
    }

    fn springs(&mut self) -> [&mut Spring; 7] {
        [
            &mut self.presence,
            &mut self.y,
            &mut self.hover,
            &mut self.press,
            &mut self.pin,
            &mut self.flash,
            &mut self.reveal,
        ]
    }
}

/// What the window layer should do after a state change.
#[derive(Default, Debug, PartialEq)]
pub struct Effects {
    /// Cards whose waveform needs measuring: (id, first file).
    pub measure: Vec<(u64, String)>,
    pub show: bool,
}

pub struct Tray {
    pub cards: Vec<Card>,
    pub hovered: Option<u64>,
    pub pressed: Option<(u64, f32, f32)>,
    pub dragging: bool,
    pub animate: bool,
    pub resident: bool,
    pub ffmpeg: Option<String>,
    pub cursor: (f32, f32),
    next_id: u64,
    pub started: Instant,
}

impl Tray {
    pub fn new(animate: bool) -> Tray {
        Tray {
            cards: Vec::new(),
            hovered: None,
            pressed: None,
            dragging: false,
            animate,
            resident: false,
            ffmpeg: None,
            cursor: (-1.0, -1.0),
            next_id: 1,
            started: Instant::now(),
        }
    }

    pub fn live(&self) -> usize {
        self.cards.iter().filter(|c| !c.closing).count()
    }

    /// Logical window size for the current cards.
    pub fn size(&self) -> (f32, f32) {
        let live = self.live().max(self.cards.len()).max(1) as f32;
        (
            CARD_W + 2.0 * MARGIN,
            live * (CARD_H + GAP) - GAP + 2.0 * MARGIN,
        )
    }

    fn targets(&mut self) {
        // Newest sits at the bottom, nearest the taskbar.
        let mut index = 0.0;
        for card in self.cards.iter_mut().rev().filter(|c| !c.closing) {
            card.y.target = index;
            index += 1.0;
        }
    }

    pub fn card_rect(&self, card: &Card, height: f32) -> R {
        let scale = 0.94 + 0.06 * card.presence.value - 0.015 * card.press.value;
        let lift = (1.0 - card.presence.value) * 22.0 - card.hover.value * 1.5;
        let bottom = height - MARGIN - card.y.value * (CARD_H + GAP);
        R::new(MARGIN, bottom - CARD_H + lift, CARD_W, CARD_H).scaled_about_center(scale)
    }

    pub fn close_rect(&self, card: &Card, height: f32) -> R {
        let rect = self.card_rect(card, height);
        R::new(rect.right() - 34.0, rect.y + 9.0, 24.0, 24.0)
    }

    pub fn card_at(&self, x: f32, y: f32, height: f32) -> Option<u64> {
        self.cards
            .iter()
            .rev()
            .find(|c| !c.closing && self.card_rect(c, height).contains(x, y))
            .map(|c| c.id)
    }

    pub fn card(&mut self, id: u64) -> Option<&mut Card> {
        self.cards.iter_mut().find(|c| c.id == id)
    }

    pub fn accept(&mut self, payload: &Value) -> Effects {
        let mut effects = Effects::default();
        if let Some(ffmpeg) = payload.get("ffmpeg").and_then(Value::as_str) {
            self.ffmpeg = Some(ffmpeg.to_string());
        }
        let pinned = payload
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        for item in payload
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let files: Vec<String> = item
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|f| Path::new(f).is_file())
                .map(normalized)
                .collect();
            if files.is_empty() {
                continue;
            }
            if let Some(replaces) = item.get("replaces").and_then(Value::as_array) {
                // Late BPM/key for a card already on screen: refresh it quietly,
                // and never bring back a card the user has dismissed.
                let replaced: Vec<String> = replaces
                    .iter()
                    .filter_map(Value::as_str)
                    .map(normalized)
                    .collect();
                if let Some(card) = self
                    .cards
                    .iter_mut()
                    .find(|c| c.files == replaced && !c.closing)
                {
                    card.files = files;
                    if let Some(title) = item
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                    {
                        card.title = title.to_string();
                    }
                    if let Some(subtitle) = item
                        .get("subtitle")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                    {
                        card.subtitle = subtitle.to_string();
                    }
                }
                continue;
            }
            if let Some(index) = self
                .cards
                .iter()
                .position(|c| c.files == files && !c.closing)
            {
                // Re-summoning a visible card refreshes it with a small pulse.
                let mut existing = self.cards.remove(index);
                existing.remaining = LIFETIME;
                existing.pinned |= pinned;
                existing.pin.target = if existing.pinned { 1.0 } else { 0.0 };
                existing.press.velocity = 9.0;
                self.cards.push(existing);
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            effects.measure.push((id, files[0].clone()));
            self.cards.push(Card::new(id, item, files, pinned));
        }
        let live: Vec<u64> = self
            .cards
            .iter()
            .filter(|c| !c.closing)
            .map(|c| c.id)
            .collect();
        for id in live.iter().take(live.len().saturating_sub(MAX_CARDS)) {
            self.dismiss(*id);
        }
        self.targets();
        for card in self
            .cards
            .iter_mut()
            .filter(|c| c.presence.target == 0.0 && !c.closing)
        {
            let target = card.y.target;
            card.y.snap(target);
            card.presence.target = 1.0;
        }
        effects.show = !self.cards.is_empty();
        effects
    }

    pub fn measured(&mut self, id: u64, peaks: Option<Vec<f32>>) {
        if let (Some(card), Some(peaks)) = (self.card(id), peaks) {
            card.peaks = Some(peaks);
            card.reveal.target = 1.0;
        }
    }

    pub fn dismiss(&mut self, id: u64) {
        if let Some(card) = self.card(id) {
            if card.closing {
                return;
            }
            card.closing = true;
            card.presence.stiffness = 520.0;
            card.presence.damping = 40.0;
            card.presence.target = 0.0;
        }
        self.targets();
    }

    /// Advance animations by `dt`; returns whether another frame is needed.
    pub fn tick(&mut self, dt: f32, now: Instant) -> bool {
        let mut busy = false;
        let paused = self.hovered.is_some() || self.dragging;
        let mut expired = Vec::new();
        for card in self.cards.iter_mut() {
            if !card.closing && !card.pinned && !paused {
                card.remaining -= dt;
                if card.remaining <= 0.0 {
                    expired.push(card.id);
                }
            }
            if card
                .dropped_at
                .is_some_and(|t| now.duration_since(t).as_secs_f32() > 0.9)
                && !card.pinned
            {
                expired.push(card.id);
            }
            if card.closing || (!card.pinned && !paused) || card.dropped_at.is_some() {
                busy = true;
            }
            if card.peaks.is_none() {
                busy = true; // idle shimmer while measuring
            }
        }
        for id in expired {
            self.dismiss(id);
        }
        let (hovered, pressed, dragging, animate) = (
            self.hovered,
            self.pressed.map(|p| p.0),
            self.dragging,
            self.animate,
        );
        for card in self.cards.iter_mut() {
            card.hover.target = if hovered == Some(card.id) { 1.0 } else { 0.0 };
            card.press.target = if pressed == Some(card.id) && !dragging {
                1.0
            } else {
                0.0
            };
            for spring in card.springs() {
                if animate {
                    busy |= spring.step(dt);
                } else {
                    let target = spring.target;
                    spring.snap(target);
                }
            }
        }
        let before = self.cards.len();
        self.cards
            .retain(|c| !(c.closing && c.presence.value <= 0.02));
        if self.cards.len() != before
            && self
                .hovered
                .is_some_and(|h| !self.cards.iter().any(|c| c.id == h))
        {
            self.hovered = None;
        }
        busy && !self.cards.is_empty()
    }

    pub fn leave(&mut self) {
        if self.hovered.is_some() {
            for card in self.cards.iter_mut() {
                card.remaining = card.remaining.max(RESUME_MINIMUM);
            }
        }
        self.hovered = None;
        self.cursor = (-1.0, -1.0);
    }

    pub fn hover(&mut self, x: f32, y: f32, height: f32) {
        self.cursor = (x, y);
        let hovered = self.card_at(x, y, height);
        if hovered.is_none() && self.hovered.is_some() {
            for card in self.cards.iter_mut() {
                card.remaining = card.remaining.max(RESUME_MINIMUM);
            }
        }
        self.hovered = hovered;
    }

    /// A completed click (press and release on the same card) toggles pinning,
    /// or dismisses through the close button.
    pub fn click(&mut self, id: u64, x: f32, y: f32, height: f32) {
        let Some(card) = self.cards.iter().find(|c| c.id == id) else {
            return;
        };
        if self.close_rect(card, height).contains(x, y) {
            self.dismiss(id);
        } else if self.card_at(x, y, height) == Some(id) {
            let card = self.card(id).unwrap();
            card.pinned = !card.pinned;
            card.pin.target = if card.pinned { 1.0 } else { 0.0 };
            card.remaining = card.remaining.max(LIFETIME * 0.6);
            card.pin.velocity = 6.0;
        }
    }

    pub fn dropped(&mut self, id: u64) {
        if let Some(card) = self.card(id) {
            card.dropped_at = Some(Instant::now());
            card.flash.target = 1.0;
        }
    }

    pub fn paint(&self, pixmap: &mut Pixmap, scale: f32, fonts: &Fonts) {
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        let height = pixmap.height() as f32 / scale;
        for card in &self.cards {
            self.paint_card(pixmap, scale, fonts, card, height);
        }
    }

    fn paint_card(&self, pixmap: &mut Pixmap, scale: f32, fonts: &Fonts, card: &Card, height: f32) {
        let presence = card.presence.value.clamp(0.0, 1.0);
        if presence <= 0.0 {
            return;
        }
        let rect = self.card_rect(card, height);
        let hover = card.hover.value;
        let mut c = Canvas::new(pixmap, scale, fonts);
        c.opacity = (presence * 1.25).min(1.0);
        c.shadow(rect, RADIUS, 10);
        let path = rounded(rect, RADIUS);
        c.fill_vertical_gradient(
            path.as_ref(),
            rect,
            SURFACE.lighter(100.0 + 22.0 * hover),
            SURFACE,
        );
        let flash = card.flash.value;
        if flash > 0.01 {
            c.fill(path.as_ref(), card.accent.alpha(0.14 * flash.min(1.0)));
        }
        // Specular top edge: the glass catches light along its upper rim.
        let shifted = rounded(R::new(rect.x, rect.y + 0.5, rect.w, rect.h), RADIUS);
        c.stroke_clipped_top(
            shifted.as_ref(),
            SPECULAR,
            R::new(rect.x, rect.y, rect.w, RADIUS),
        );
        // Pinned: a brighter neutral rim; only the pin itself carries the service colour.
        let border = if flash > 0.5 {
            card.accent.alpha(120.0 * flash / 255.0)
        } else {
            Rgba(
                255,
                255,
                255,
                (22.0 + 40.0 * hover + 46.0 * card.pin.value) / 255.0,
            )
        };
        c.stroke(path.as_ref(), border, 1.0);
        c.clip_to(path.as_ref());

        // Waveform tile: the cut start is the very first bar.
        let tile = R::new(rect.x + 12.0, rect.y + 11.0, 62.0, rect.h - 22.0);
        c.fill(
            rounded(tile, 10.0).as_ref(),
            Rgba(255, 255, 255, (12.0 + 8.0 * hover) / 255.0),
        );
        self.waveform(&mut c, tile.inset(7.0, 6.0, 7.0, 6.0), card);

        // Title and subtitle; on hover the subtitle becomes the gesture hint.
        let left = tile.right() + 12.0;
        let right = rect.right() - 44.0;
        let title = c.elide(&card.title, Weight::Title, 10.0, right - left);
        c.text(
            &title,
            R::new(left, rect.y + 13.0, right - left, 20.0),
            Weight::Title,
            10.0,
            Rgba::hex(TEXT),
        );
        let hint = if card.dropped_at.is_some() {
            if card.files.len() == 1 {
                "Dropped".to_string()
            } else {
                format!("Dropped {} files", card.files.len())
            }
        } else if card.pinned {
            "Pinned · drag into your DAW · click to release".to_string()
        } else {
            "Drag into your DAW · click to pin".to_string()
        };
        let sub = R::new(left, rect.y + 34.0, right - left, 18.0);
        if card.dropped_at.is_none() {
            let text = c.elide(&card.subtitle, Weight::Body, 8.0, right - left);
            c.text(
                &text,
                R::new(sub.x, sub.y - 4.0 * hover, sub.w, sub.h),
                Weight::Body,
                8.0,
                Rgba::hex(MUTED).alpha(1.0 - hover),
            );
        }
        let hint_alpha = if card.dropped_at.is_some() {
            1.0
        } else {
            hover
        };
        let hint_color = if card.dropped_at.is_some() || card.pinned {
            card.accent
        } else {
            Rgba::hex(MUTED)
        };
        let text = c.elide(&hint, Weight::Body, 8.0, right - left);
        c.text(
            &text,
            R::new(sub.x, sub.y + 4.0 * (1.0 - hover), sub.w, sub.h),
            Weight::Body,
            8.0,
            hint_color.alpha(hint_alpha),
        );

        // Close appears on hover; the pin badge stays while pinned.
        let close = self.close_rect(card, height);
        if hover > 0.01 {
            let hot = close.contains(self.cursor.0, self.cursor.1);
            let (cx, cy) = close.center();
            let mut circle = tiny_skia::PathBuilder::new();
            circle.push_circle(cx, cy, close.w / 2.0);
            c.fill(
                circle.finish().as_ref(),
                Rgba(
                    255,
                    255,
                    255,
                    (if hot { 30.0 } else { 12.0 }) * hover / 255.0,
                ),
            );
            c.glyph(
                close,
                Glyph::Close,
                Rgba::hex(if hot { TEXT } else { MUTED }).alpha(hover),
            );
        }
        let pin = card.pin.value;
        if pin > 0.01 {
            let badge = R::new(close.x + 3.0, close.bottom() + 6.0, 18.0, 18.0)
                .scaled_about_center(pin.max(0.0));
            c.glyph(badge, Glyph::Pin, card.accent.alpha(pin.min(1.0)));
        }

        // Countdown: a hairline that drains right-to-left and dims while paused.
        if !card.pinned && card.dropped_at.is_none() {
            let fraction = (card.remaining / LIFETIME).clamp(0.0, 1.0);
            let track = R::new(
                rect.x + RADIUS,
                rect.bottom() - 3.5,
                rect.w - 2.0 * RADIUS,
                2.0,
            );
            c.fill(
                rounded(track, 1.0).as_ref(),
                Rgba(255, 255, 255, 14.0 / 255.0),
            );
            let paused = self.hovered.is_some() || self.dragging;
            c.fill(
                rounded(R::new(track.x, track.y, track.w * fraction, track.h), 1.0).as_ref(),
                card.accent.alpha(if paused { 0.45 } else { 0.9 }),
            );
        }
    }

    fn waveform(&self, c: &mut Canvas, area: R, card: &Card) {
        let values = card.peaks.as_deref();
        let count = values.map_or(18, <[f32]>::len).max(1);
        let step = area.w / count as f32;
        let width = (step * 0.55).max(1.2);
        let grow = if values.is_some() {
            card.reveal.value
        } else {
            0.0
        };
        let middle = area.center().1;
        let breathe = self.started.elapsed().as_secs_f32() * 3.2;
        for i in 0..count {
            let level = match values {
                Some(v) => {
                    0.08 + 0.92
                        * v[i]
                        * (grow * 1.4 - i as f32 / count as f32 * 0.4).clamp(0.0, 1.2)
                }
                // Measuring: a quiet idle shimmer instead of a spinner.
                None => 0.16 + 0.1 * (1.0 + (breathe - i as f32 * 0.55).sin()) / 2.0,
            };
            let bar_height = (level * area.h).max(2.0);
            // Neutral waveform; its first bar (the trimmed start) carries the service colour.
            let color = if i == 0 && values.is_some() {
                card.accent
            } else {
                Rgba::hex(TEXT).alpha(0.62)
            };
            let x = area.x + i as f32 * step + (step - width) / 2.0;
            c.fill(
                rounded(
                    R::new(x, middle - bar_height / 2.0, width, bar_height),
                    width / 2.0,
                )
                .as_ref(),
                color,
            );
        }
    }

    /// The small card that follows the cursor during a drag.
    pub fn ghost(card: &Card, scale: f32, fonts: &Fonts) -> Pixmap {
        let (width, height) = (260.0, 44.0);
        let mut pixmap = Pixmap::new(
            ((width + 16.0) * scale).ceil() as u32,
            ((height + 16.0) * scale).ceil() as u32,
        )
        .expect("ghost size");
        let mut c = Canvas::new(&mut pixmap, scale, fonts);
        let rect = R::new(8.0, 8.0, width, height);
        c.shadow(rect, 14.0, 6);
        let path = rounded(rect, 14.0);
        c.fill(path.as_ref(), RAISED);
        c.stroke(path.as_ref(), Rgba(255, 255, 255, 40.0 / 255.0), 1.0);
        c.glyph(
            R::new(rect.x + 12.0, rect.y + 10.0, 24.0, 24.0),
            Glyph::Audio,
            card.accent,
        );
        let label = if card.files.len() == 1 {
            card.title.clone()
        } else {
            format!("{} files", card.files.len())
        };
        let text = c.elide(&label, Weight::Title, 10.0, width - 60.0);
        c.text(
            &text,
            R::new(rect.x + 44.0, rect.y, width - 56.0, height),
            Weight::Title,
            10.0,
            Rgba::hex(TEXT),
        );
        pixmap
    }
}

/// Normalised loudness envelope (56 bars) from mono 2 kHz samples, or None.
pub fn envelope(samples: &[f32], bars: usize) -> Option<Vec<f32>> {
    if samples.len() < bars {
        return None;
    }
    let levels: Vec<f32> = (0..bars)
        .map(|i| {
            let (a, b) = (samples.len() * i / bars, samples.len() * (i + 1) / bars);
            let slice = &samples[a..b.max(a + 1)];
            (slice.iter().map(|v| v * v).sum::<f32>() / slice.len() as f32).sqrt()
        })
        .collect();
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    // numpy.percentile(levels, 98), linear interpolation.
    let position = 0.98 * (sorted.len() - 1) as f32;
    let low = position.floor() as usize;
    let high = (low + 1).min(sorted.len() - 1);
    let top = sorted[low] + (position - low as f32) * (sorted[high] - sorted[low]);
    let top = if top > 0.0 { top } else { 1.0 };
    Some(
        levels
            .iter()
            .map(|v| (v / top).min(1.0).powf(0.8))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_file(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("siphon-tray-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn accept_resummon_replace_and_limit() {
        let mut tray = Tray::new(true);
        let a = temp_file("a.mp3");
        let effects =
            tray.accept(&json!({"items": [{"files": [a], "title": "Song", "subtitle": "MP3"}]}));
        assert_eq!(effects.measure.len(), 1);
        assert!(effects.show);
        tray.accept(&json!({"items": [{"files": [a]}]}));
        assert_eq!(tray.cards.len(), 1, "re-summoning reuses the card");
        let renamed = temp_file("a [120 BPM Am].mp3");
        tray.accept(&json!({"items": [{"files": [renamed], "subtitle": "120 BPM · Am · MP3", "replaces": [a]}]}));
        assert_eq!(tray.cards[0].files, vec![normalized(&renamed)]);
        assert_eq!(tray.cards[0].subtitle, "120 BPM · Am · MP3");
        let id = tray.cards[0].id;
        tray.dismiss(id);
        tray.accept(
            &json!({"items": [{"files": [renamed], "subtitle": "later", "replaces": [renamed]}]}),
        );
        assert_eq!(tray.live(), 0, "a dismissed card never comes back");
        for i in 0..7 {
            tray.accept(&json!({"items": [{"files": [temp_file(&format!("{i}.wav"))]}]}));
        }
        assert_eq!(tray.live(), MAX_CARDS);
    }

    #[test]
    fn countdown_hover_pin_and_cleanup() {
        let mut tray = Tray::new(false);
        let file = temp_file("b.mp3");
        tray.accept(&json!({"items": [{"files": [file]}]}));
        let id = tray.cards[0].id;
        let (_, height) = tray.size();
        tray.tick(0.016, Instant::now());
        let rect = tray.card_rect(&tray.cards[0], height);
        let (x, y) = (rect.x + 100.0, rect.y + 30.0);
        tray.hover(x, y, height);
        assert_eq!(tray.hovered, Some(id));
        let before = tray.cards[0].remaining;
        tray.tick(5.0, Instant::now());
        assert_eq!(
            tray.cards[0].remaining, before,
            "hover pauses the countdown"
        );
        tray.click(id, x, y, height);
        assert!(tray.cards[0].pinned);
        tray.leave();
        tray.tick(30.0, Instant::now());
        assert_eq!(tray.live(), 1, "pinned cards stay");
        tray.click(id, x, y, height);
        tray.tick(30.0, Instant::now());
        tray.tick(0.1, Instant::now());
        assert!(tray.cards.is_empty());
    }

    #[test]
    fn envelope_shape() {
        let samples: Vec<f32> = (0..4000)
            .map(|i| if i < 2000 { 0.1 } else { 0.5 })
            .collect();
        let peaks = envelope(&samples, 56).unwrap();
        assert_eq!(peaks.len(), 56);
        assert!(peaks[0] < peaks[55] && peaks[55] <= 1.0);
        assert!(envelope(&[0.0; 10], 56).is_none());
    }

    #[test]
    fn paints_cards() {
        let fonts = Fonts::load();
        let mut tray = Tray::new(false);
        tray.accept(&json!({"items": [{"files": [temp_file("c.mp3")], "title": "Song", "service": "youtube"}]}));
        tray.tick(0.016, Instant::now());
        let (w, h) = tray.size();
        let mut pixmap = Pixmap::new(w as u32, h as u32).unwrap();
        tray.paint(&mut pixmap, 1.0, &fonts);
        assert!(pixmap.pixels().iter().filter(|p| p.alpha() > 200).count() > 10_000);
        let ghost = Tray::ghost(&tray.cards[0], 1.5, &fonts);
        assert!(ghost.width() > 300);
    }
}
