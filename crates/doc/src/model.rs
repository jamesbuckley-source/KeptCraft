//! Editable screenshot document: base image, annotations, crop and background.
//! Geometry is in base-image pixels.

use std::path::Path;
use std::sync::Arc;

use ecolor::Color32;
use emath::{Align, Align2, Pos2, Rect, Vec2, pos2, vec2};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, fonts};

pub const PROJECT_EXT: &str = "shotcraft";
const PROJECT_FORMAT: &str = "shotcraft-project";
const PROJECT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArrowStyle {
    /// Tapered shaft with a wide head.
    Tapered,
    /// Uniform line with a triangular head.
    Classic,
    /// Heads on both ends.
    Double,
    /// Quadratic curve; `bend` controls the curvature.
    Curved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextStyle {
    Plain,
    Outlined,
    Shadow,
    Filled,
    Light,
    Dark,
    Stroke,
}

impl TextStyle {
    pub const ALL: [TextStyle; 7] =
        [TextStyle::Plain, TextStyle::Outlined, TextStyle::Shadow, TextStyle::Filled, TextStyle::Light, TextStyle::Dark, TextStyle::Stroke];

    pub fn has_box(self) -> bool {
        matches!(self, TextStyle::Filled | TextStyle::Light | TextStyle::Dark)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RedactMode {
    /// Block average with per-block noise so the original cannot be recovered by re-pixelating.
    Pixelate,
    /// Pixelate, then blur: smooth look without leaking detail.
    SecureBlur,
    /// Plain blur; looks softest but keeps more information.
    Blur,
    /// Opaque fill in the item colour.
    Solid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CounterLabel {
    #[default]
    Numbers,
    /// 1 = A, 26 = Z, 27 = AA, so a run of steps can read as a list.
    Letters,
}

/// How numbered steps look: size, label kind, ring, shadow and the space around the label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CounterStyle {
    /// Circle diameter in base pixels.
    pub size: f32,
    pub label: CounterLabel,
    /// Ring around the circle, drawn in this colour; `None` keeps the fill only.
    pub outline: Option<Color32>,
    pub outline_width: f32,
    /// 0..=1 drop shadow strength; [`CounterStyle::shadow_on`] decides whether it is drawn.
    pub shadow: f32,
    /// Draw the drop shadow at all.
    #[serde(default = "yes")]
    pub shadow_on: bool,
    /// How far the shadow's soft edge reaches, in base pixels.
    #[serde(default = "shadow_blur_default")]
    pub shadow_blur: f32,
    /// Direction the shadow falls, in degrees: 0 = right, 90 = down, 180 = left, 270 = up.
    #[serde(default = "shadow_angle_default")]
    pub shadow_angle: f32,
    /// How far the shadow sits from the circle, in base pixels.
    #[serde(default = "shadow_distance_default")]
    pub shadow_distance: f32,
    /// Fraction of the radius kept clear between the label and the circle's edge.
    pub padding: f32,
}

fn yes() -> bool {
    true
}

fn shadow_blur_default() -> f32 {
    6.0
}

fn shadow_angle_default() -> f32 {
    90.0
}

fn shadow_distance_default() -> f32 {
    4.0
}

impl Default for CounterStyle {
    fn default() -> Self {
        Self {
            size: 28.0,
            label: CounterLabel::Numbers,
            outline: None,
            outline_width: 3.0,
            shadow: 0.3,
            shadow_on: true,
            shadow_blur: shadow_blur_default(),
            shadow_angle: shadow_angle_default(),
            shadow_distance: shadow_distance_default(),
            padding: 0.28,
        }
    }
}

impl CounterStyle {
    pub fn label_for(&self, n: u32) -> String {
        match self.label {
            CounterLabel::Numbers => n.to_string(),
            CounterLabel::Letters => letters(n),
        }
    }
}

/// Spreadsheet-style letters: 1 = A … 26 = Z, 27 = AA.
pub fn letters(n: u32) -> String {
    let mut n = n.max(1);
    let mut out = String::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.insert(0, (b'A' + rem) as char);
        n = (n - 1) / 26;
    }
    out
}

/// One numbered step's comment, shown beside the image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommentEntry {
    pub n: u32,
    pub text: String,
}

/// Which edge of the image the comment band is attached to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentarySide {
    Top,
    Right,
    #[default]
    Bottom,
    Left,
}

impl CommentarySide {
    pub const ALL: [Self; 4] = [Self::Top, Self::Right, Self::Bottom, Self::Left];

    pub fn label(&self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Right => "Right",
            Self::Bottom => "Bottom",
            Self::Left => "Left",
        }
    }

    /// True when the band sits above or below the image, so the image keeps its width.
    pub fn horizontal(&self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }
}

/// The band beside the image: one entry per numbered step that has something to say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Commentary {
    pub background: Color32,
    pub text_color: Color32,
    pub size: f32,
    /// Space above the first line, below the last, and on both sides.
    pub padding: f32,
    pub entries: Vec<CommentEntry>,
    /// Edge of the image the band is attached to.
    #[serde(default)]
    pub side: CommentarySide,
    /// Width of a side band in base pixels; 0 fits the comments to the image's height.
    #[serde(default)]
    pub width: f32,
    /// Ring each step in the colour of its own counter instead of [`Commentary::badge`].
    #[serde(default = "yes")]
    pub match_counter: bool,
    /// Badge colour used when the step no longer has a counter, or matching is off.
    #[serde(default = "badge_default")]
    pub badge: Color32,
}

fn badge_default() -> Color32 {
    Color32::from_rgb(226, 62, 48)
}

impl Default for Commentary {
    fn default() -> Self {
        Self {
            background: Color32::from_rgb(28, 28, 30),
            text_color: Color32::WHITE,
            size: 22.0,
            padding: 24.0,
            entries: Vec::new(),
            side: CommentarySide::Bottom,
            width: 0.0,
            match_counter: true,
            badge: badge_default(),
        }
    }
}

impl Commentary {
    /// The entries that actually say something, in step order.
    pub fn filled(&self) -> Vec<&CommentEntry> {
        let mut out: Vec<&CommentEntry> = self.entries.iter().filter(|e| !e.text.trim().is_empty()).collect();
        out.sort_by_key(|e| e.n);
        out
    }

    pub fn entry_mut(&mut self, n: u32) -> &mut String {
        if let Some(i) = self.entries.iter().position(|e| e.n == n) {
            return &mut self.entries[i].text;
        }
        self.entries.push(CommentEntry { n, text: String::new() });
        let i = self.entries.len() - 1;
        &mut self.entries[i].text
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Rect {
        rect: Rect,
        filled: bool,
    },
    Ellipse {
        rect: Rect,
        filled: bool,
    },
    Line {
        a: Pos2,
        b: Pos2,
    },
    Arrow {
        a: Pos2,
        b: Pos2,
        style: ArrowStyle,
        bend: f32,
    },
    Pen {
        pts: Vec<Pos2>,
    },
    Highlighter {
        pts: Vec<Pos2>,
    },
    Text {
        pos: Pos2,
        text: String,
        size: f32,
        style: TextStyle,
        /// Wrapping width for a paragraph; `None` keeps the line unbroken.
        #[serde(default)]
        wrap: Option<f32>,
    },
    Counter {
        center: Pos2,
        n: u32,
        #[serde(default)]
        style: CounterStyle,
    },
    Redact {
        rect: Rect,
        mode: RedactMode,
        seed: u64,
    },
    Spotlight {
        rect: Rect,
        ellipse: bool,
    },
    Image {
        rect: Rect,
        #[serde(with = "png_b64")]
        image: Arc<RgbaImage>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub shape: Shape,
    pub color: Color32,
    /// Stroke width in pixels; also scales arrow heads, counters and pixelation.
    pub width: f32,
}

pub const COUNTER_RADIUS_PER_WIDTH: f32 = 3.2;

impl Item {
    pub fn counter_radius(&self) -> f32 {
        match &self.shape {
            Shape::Counter { style, .. } => style.size.max(8.0) / 2.0,
            _ => (self.width * COUNTER_RADIUS_PER_WIDTH).max(12.0),
        }
    }

    pub fn bounds(&self) -> Rect {
        let pad = self.width * 0.5;
        match &self.shape {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Redact { rect, .. } | Shape::Spotlight { rect, .. } | Shape::Image { rect, .. } => {
                rect.expand(pad)
            }
            Shape::Line { a, b } => Rect::from_two_pos(*a, *b).expand(pad),
            Shape::Arrow { a, b, style, bend } => {
                let mut r = Rect::from_two_pos(*a, *b);
                if *style == ArrowStyle::Curved {
                    r.extend_with(curve_control(*a, *b, *bend));
                }
                r.expand(arrow_head_len(self.width))
            }
            Shape::Pen { pts } | Shape::Highlighter { pts } => {
                let w = if matches!(self.shape, Shape::Highlighter { .. }) { highlighter_width(self.width) } else { self.width };
                Rect::from_points(pts).expand(w * 0.5)
            }
            Shape::Text { pos, text, size, style, wrap } => text_box(*pos, text, *size, *style, *wrap),
            Shape::Counter { center, .. } => Rect::from_center_size(*center, Vec2::splat(self.counter_radius() * 2.0)),
        }
    }

    /// Whether `p` touches the item, with `tol` pixels of slack.
    pub fn hit(&self, p: Pos2, tol: f32) -> bool {
        let half = self.width * 0.5 + tol;
        match &self.shape {
            Shape::Rect { rect, filled } => {
                if *filled {
                    rect.expand(tol).contains(p)
                } else {
                    rect.expand(half).contains(p) && !rect.shrink(half).contains(p)
                }
            }
            Shape::Ellipse { rect, filled } => {
                let c = rect.center();
                let r = rect.size() * 0.5;
                if r.x < 1.0 || r.y < 1.0 {
                    return rect.expand(half).contains(p);
                }
                let d = vec2((p.x - c.x) / r.x, (p.y - c.y) / r.y).length();
                let band = half / r.x.min(r.y);
                if *filled { d <= 1.0 + band } else { (d - 1.0).abs() <= band }
            }
            Shape::Line { a, b } => dist_to_segment(p, *a, *b) <= half,
            Shape::Arrow { a, b, style, bend } => {
                let head = arrow_head_len(self.width) * 0.6;
                if *style == ArrowStyle::Curved {
                    let pts = curve_points(*a, *b, *bend, 24);
                    pts.windows(2).any(|s| dist_to_segment(p, s[0], s[1]) <= half.max(head * 0.5))
                } else {
                    dist_to_segment(p, *a, *b) <= half.max(head * 0.5)
                }
            }
            Shape::Pen { pts } | Shape::Highlighter { pts } => {
                let w = if matches!(self.shape, Shape::Highlighter { .. }) { highlighter_width(self.width) * 0.5 + tol } else { half };
                match pts.len() {
                    0 => false,
                    1 => pts[0].distance(p) <= w,
                    _ => pts.windows(2).any(|s| dist_to_segment(p, s[0], s[1]) <= w),
                }
            }
            Shape::Counter { center, .. } => center.distance(p) <= self.counter_radius() + tol,
            Shape::Text { .. } | Shape::Redact { .. } | Shape::Spotlight { .. } | Shape::Image { .. } => self.bounds().expand(tol).contains(p),
        }
    }

    pub fn translate(&mut self, d: Vec2) {
        match &mut self.shape {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Redact { rect, .. } | Shape::Spotlight { rect, .. } | Shape::Image { rect, .. } => {
                *rect = rect.translate(d)
            }
            Shape::Line { a, b } | Shape::Arrow { a, b, .. } => {
                *a += d;
                *b += d;
            }
            Shape::Pen { pts } | Shape::Highlighter { pts } => pts.iter_mut().for_each(|p| *p += d),
            Shape::Text { pos, .. } => *pos += d,
            Shape::Counter { center, .. } => *center += d,
        }
    }

    /// Drag handles in base coordinates.
    pub fn handles(&self) -> Vec<Pos2> {
        match &self.shape {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Redact { rect, .. } | Shape::Spotlight { rect, .. } | Shape::Image { rect, .. } => {
                vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()]
            }
            Shape::Line { a, b } => vec![*a, *b],
            Shape::Arrow { a, b, style, bend } => {
                let mut v = vec![*a, *b];
                if *style == ArrowStyle::Curved {
                    v.push(curve_point(*a, curve_control(*a, *b, *bend), *b, 0.5));
                }
                v
            }
            Shape::Pen { .. } | Shape::Highlighter { .. } | Shape::Text { .. } | Shape::Counter { .. } => Vec::new(),
        }
    }

    /// Move handle `i` (an index into [`Item::handles`]) to `p`.
    pub fn set_handle(&mut self, i: usize, p: Pos2, keep_aspect: bool) {
        match &mut self.shape {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Redact { rect, .. } | Shape::Spotlight { rect, .. } | Shape::Image { rect, .. } => {
                let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
                let anchor = corners[(i + 2) % 4];
                let mut p = p;
                if keep_aspect && rect.height() > 0.0 {
                    p = constrain_aspect(anchor, p, rect.width() / rect.height());
                }
                *rect = Rect::from_two_pos(anchor, p);
            }
            Shape::Line { a, b } => {
                if i == 0 {
                    *a = p
                } else {
                    *b = p
                }
            }
            Shape::Arrow { a, b, bend, .. } => match i {
                0 => *a = p,
                1 => *b = p,
                _ => *bend = bend_for_midpoint(*a, *b, p),
            },
            _ => {}
        }
    }
}

/// A point `p` moved so the rectangle from `anchor` has the given width/height ratio.
pub fn constrain_aspect(anchor: Pos2, p: Pos2, ratio: f32) -> Pos2 {
    let d = p - anchor;
    let w = d.x.abs().max(d.y.abs() * ratio);
    let h = w / ratio;
    pos2(anchor.x + w * d.x.signum(), anchor.y + h * d.y.signum())
}

pub fn highlighter_width(width: f32) -> f32 {
    (width * 5.0).max(12.0)
}

pub fn arrow_head_len(width: f32) -> f32 {
    width * 3.2 + 10.0
}

pub fn curve_control(a: Pos2, b: Pos2, bend: f32) -> Pos2 {
    let d = b - a;
    let n = vec2(-d.y, d.x);
    a + d * 0.5 + n * bend
}

pub fn curve_point(a: Pos2, c: Pos2, b: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    pos2(u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, u * u * a.y + 2.0 * u * t * c.y + t * t * b.y)
}

pub fn curve_points(a: Pos2, b: Pos2, bend: f32, n: usize) -> Vec<Pos2> {
    let c = curve_control(a, b, bend);
    (0..=n).map(|i| curve_point(a, c, b, i as f32 / n as f32)).collect()
}

/// Inverse of the curve midpoint: the bend whose curve passes through `mid` at t = 0.5.
fn bend_for_midpoint(a: Pos2, b: Pos2, mid: Pos2) -> f32 {
    let d = b - a;
    let len2 = d.length_sq();
    if len2 < 1.0 {
        return 0.0;
    }
    // Curve midpoint = (a + 2c + b) / 4 = midpoint(a,b) + n * bend / 2.
    let n = vec2(-d.y, d.x);
    let off = mid - (a + d * 0.5);
    2.0 * off.dot(n) / len2
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

pub fn text_padding(size: f32, style: TextStyle) -> f32 {
    if style.has_box() { size * 0.35 } else { size * 0.08 }
}

/// Box covering a text item, including its background box when it has one.
pub fn text_box(pos: Pos2, text: &str, size: f32, style: TextStyle, wrap: Option<f32>) -> Rect {
    let m = text_size(text, size, wrap);
    Rect::from_min_size(pos, m).expand(text_padding(size, style))
}

/// Size of a text block: one line when `wrap` is `None`, otherwise the widest wrapped line by the
/// number of lines it broke into.
pub fn text_size(text: &str, size: f32, wrap: Option<f32>) -> Vec2 {
    match wrap {
        Some(width) => {
            let lines = fonts::wrap_lines(text, size, width.max(size));
            let widest = lines.iter().map(|l| fonts::measure(l, size).x).fold(0.0, f32::max);
            vec2(widest, lines.len().max(1) as f32 * fonts::line_height(size))
        }
        None => fonts::measure(text, size),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Aspect {
    Auto,
    Ratio(u32, u32),
}

impl Aspect {
    pub const PRESETS: [(Aspect, &'static str); 8] = [
        (Aspect::Auto, "Auto"),
        (Aspect::Ratio(1, 1), "1:1 Square"),
        (Aspect::Ratio(4, 3), "4:3"),
        (Aspect::Ratio(3, 2), "3:2"),
        (Aspect::Ratio(16, 9), "16:9"),
        (Aspect::Ratio(9, 16), "9:16 Story"),
        (Aspect::Ratio(2, 1), "2:1 X/Twitter"),
        (Aspect::Ratio(191, 100), "1.91:1 LinkedIn"),
    ];

    pub fn label(self) -> &'static str {
        Self::PRESETS.iter().find(|(a, _)| *a == self).map_or("Custom", |(_, l)| l)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// Position along the gradient, 0..=1.
    pub at: f32,
    pub color: [u8; 4],
}

/// A gradient the user built: real stops, so it can live in a document and in a preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnedGradient {
    /// Degrees, 0 = left to right, 90 = top to bottom.
    pub angle: f32,
    pub stops: Vec<GradientStop>,
}

impl OwnedGradient {
    /// The colour at `t` (0..=1) along the gradient, interpolated between the stops.
    pub fn color_at(&self, t: f32) -> [u8; 4] {
        let mut stops = self.stops.clone();
        if stops.is_empty() {
            return [0, 0, 0, 255];
        }
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        let t = t.clamp(0.0, 1.0);
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else { return [0, 0, 0, 255] };
        if t <= first.at {
            return first.color;
        }
        if t >= last.at {
            return last.color;
        }
        for pair in stops.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.at {
                let span = (b.at - a.at).max(f32::EPSILON);
                let k = (t - a.at) / span;
                let mix = |i: usize| (a.color[i] as f32 + (b.color[i] as f32 - a.color[i] as f32) * k).round() as u8;
                return [mix(0), mix(1), mix(2), mix(3)];
            }
        }
        last.color
    }

    /// Usable stops, sorted: a gradient needs at least two, so a one-stop draft repeats its colour.
    pub fn sorted_stops(&self) -> Vec<GradientStop> {
        let mut stops = self.stops.clone();
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        if stops.is_empty() {
            stops.push(GradientStop { at: 0.0, color: [255, 255, 255, 255] });
        }
        if stops.len() == 1 {
            let only = stops[0];
            stops.push(only);
        }
        stops
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BgFill {
    Transparent,
    Gradient(usize),
    /// A gradient the user built; the stops travel with the document.
    Custom(Box<OwnedGradient>),
    Solid(Color32),
    Image(#[serde(with = "png_b64")] Arc<RgbaImage>),
    /// The screenshot itself, scaled to cover and heavily blurred.
    Blurred,
}

/// Where a watermark sits on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleCenter,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::TopCenter,
        Anchor::TopRight,
        Anchor::MiddleLeft,
        Anchor::MiddleCenter,
        Anchor::MiddleRight,
        Anchor::BottomLeft,
        Anchor::BottomCenter,
        Anchor::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Anchor::TopLeft => "Top left",
            Anchor::TopCenter => "Top centre",
            Anchor::TopRight => "Top right",
            Anchor::MiddleLeft => "Left",
            Anchor::MiddleCenter => "Centre",
            Anchor::MiddleRight => "Right",
            Anchor::BottomLeft => "Bottom left",
            Anchor::BottomCenter => "Bottom centre",
            Anchor::BottomRight => "Bottom right",
        }
    }
}

/// A logo or mark drawn over the finished capture, placed against an edge and then dragged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Watermark {
    #[serde(with = "png_b64")]
    pub image: Arc<RgbaImage>,
    pub anchor: Anchor,
    /// Width as a fraction of the canvas width, so the mark keeps its proportion.
    pub width: f32,
    /// Where the user dragged it from the anchored spot, in canvas pixels.
    pub offset: Vec2,
    /// 0 = invisible, 1 = opaque.
    pub opacity: f32,
}

impl Watermark {
    pub fn new(image: RgbaImage) -> Self {
        Self { image: Arc::new(image), anchor: Anchor::BottomRight, width: 0.18, offset: Vec2::ZERO, opacity: 0.9 }
    }

    /// Where the mark lands on a canvas of `canvas` pixels: the anchor's spot inset by `margin`,
    /// moved by the user's `offset`, sized by `width`.
    pub fn rect(&self, canvas: Vec2, margin: f32) -> Rect {
        let img = &self.image;
        let w = (self.width.clamp(0.02, 1.0) * canvas.x).max(1.0);
        let h = (w * img.height() as f32 / img.width().max(1) as f32).max(1.0);
        let place = |avail: f32, len: f32, align: Align| match align {
            Align::Min => margin,
            Align::Center => (avail - len) / 2.0,
            Align::Max => avail - len - margin,
        };
        let (ax, ay) = (self.anchor.x(), self.anchor.y());
        let min = pos2(place(canvas.x, w, ax), place(canvas.y, h, ay)) + self.offset;
        Rect::from_min_size(min, vec2(w, h))
    }
}

impl Anchor {
    fn x(self) -> Align {
        match self {
            Anchor::TopLeft | Anchor::MiddleLeft | Anchor::BottomLeft => Align::Min,
            Anchor::TopCenter | Anchor::MiddleCenter | Anchor::BottomCenter => Align::Center,
            Anchor::TopRight | Anchor::MiddleRight | Anchor::BottomRight => Align::Max,
        }
    }

    fn y(self) -> Align {
        match self {
            Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => Align::Min,
            Anchor::MiddleLeft | Anchor::MiddleCenter | Anchor::MiddleRight => Align::Center,
            Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => Align::Max,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Background {
    pub fill: BgFill,
    /// Space around the screenshot, in base pixels.
    pub padding: f32,
    /// Corner radius of the screenshot, in base pixels.
    pub radius: f32,
    /// Shadow strength 0..=1.
    pub shadow: f32,
    pub aspect: Aspect,
    pub align: Align2,
    /// Trim uneven uniform margins so content sits evenly inside the padding.
    pub auto_balance: bool,
    /// A logo drawn over the finished image, dragged into place.
    #[serde(default)]
    pub watermark: Option<Watermark>,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            fill: BgFill::Gradient(3),
            padding: 64.0,
            radius: 12.0,
            shadow: 0.5,
            aspect: Aspect::Auto,
            align: Align2::CENTER_CENTER,
            auto_balance: true,
            watermark: None,
        }
    }
}

pub struct Gradient {
    pub name: &'static str,
    /// Degrees, 0 = left to right, 90 = top to bottom.
    pub angle: f32,
    pub stops: &'static [[u8; 3]],
}

pub const GRADIENTS: [Gradient; 20] = [
    Gradient { name: "Sunset", angle: 135.0, stops: &[[255, 95, 109], [255, 195, 113]] },
    Gradient { name: "Ocean", angle: 135.0, stops: &[[33, 147, 176], [109, 213, 237]] },
    Gradient { name: "Orchid", angle: 135.0, stops: &[[127, 0, 255], [225, 0, 255]] },
    Gradient { name: "Indigo", angle: 135.0, stops: &[[71, 118, 230], [142, 84, 233]] },
    Gradient { name: "Mint", angle: 135.0, stops: &[[67, 233, 123], [56, 249, 215]] },
    Gradient { name: "Peach", angle: 135.0, stops: &[[246, 211, 101], [253, 160, 133]] },
    Gradient { name: "Night", angle: 160.0, stops: &[[15, 32, 39], [32, 58, 67], [44, 83, 100]] },
    Gradient { name: "Cotton", angle: 135.0, stops: &[[161, 140, 209], [251, 194, 235]] },
    Gradient { name: "Ember", angle: 135.0, stops: &[[241, 39, 17], [245, 175, 25]] },
    Gradient { name: "Lagoon", angle: 135.0, stops: &[[19, 84, 122], [128, 208, 199]] },
    Gradient { name: "Blush", angle: 135.0, stops: &[[238, 156, 167], [255, 221, 225]] },
    Gradient { name: "Royal", angle: 160.0, stops: &[[20, 30, 48], [36, 59, 85]] },
    Gradient { name: "Lime", angle: 135.0, stops: &[[168, 224, 99], [86, 171, 47]] },
    Gradient { name: "Berry", angle: 135.0, stops: &[[142, 45, 226], [74, 0, 224]] },
    Gradient { name: "Coral", angle: 135.0, stops: &[[255, 154, 158], [250, 208, 196]] },
    Gradient { name: "Sky", angle: 135.0, stops: &[[137, 247, 254], [102, 166, 255]] },
    Gradient { name: "Graphite", angle: 160.0, stops: &[[35, 37, 38], [65, 67, 69]] },
    Gradient { name: "Neon", angle: 135.0, stops: &[[252, 70, 107], [63, 94, 251]] },
    Gradient { name: "Emerald", angle: 135.0, stops: &[[17, 153, 142], [56, 239, 125]] },
    Gradient { name: "Lavender", angle: 135.0, stops: &[[224, 195, 252], [142, 197, 252]] },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orient {
    RotateCw,
    RotateCcw,
    FlipH,
    FlipV,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    #[serde(with = "png_b64")]
    pub base: Arc<RgbaImage>,
    pub items: Vec<Item>,
    /// Non-destructive crop in base pixels.
    pub crop: Option<Rect>,
    pub background: Option<Background>,
    /// One comment line per numbered step, drawn in a band under the image.
    #[serde(default)]
    pub commentary: Option<Commentary>,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: String,
    version: u32,
    #[serde(flatten)]
    doc: Doc,
}

impl Doc {
    pub fn new(base: RgbaImage) -> Self {
        Self { base: Arc::new(base), items: Vec::new(), crop: None, background: None, commentary: None }
    }

    pub fn full_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(self.base.width() as f32, self.base.height() as f32))
    }

    /// Visible part of the base image.
    pub fn content_rect(&self) -> Rect {
        self.crop.map_or(self.full_rect(), |c| c.intersect(self.full_rect()))
    }

    pub fn next_counter(&self) -> u32 {
        self.items.iter().filter_map(|i| if let Shape::Counter { n, .. } = i.shape { Some(n) } else { None }).max().unwrap_or(0) + 1
    }

    /// Rotate or flip the base image and every annotation with it.
    pub fn reorient(&mut self, op: Orient) {
        let (w, h) = (self.base.width() as f32, self.base.height() as f32);
        let map = move |p: Pos2| match op {
            Orient::FlipH => pos2(w - p.x, p.y),
            Orient::FlipV => pos2(p.x, h - p.y),
            Orient::RotateCw => pos2(h - p.y, p.x),
            Orient::RotateCcw => pos2(p.y, w - p.x),
        };
        let map_rect = |r: Rect| Rect::from_two_pos(map(r.min), map(r.max));
        let map_img = |img: &RgbaImage| match op {
            Orient::FlipH => image::imageops::flip_horizontal(img),
            Orient::FlipV => image::imageops::flip_vertical(img),
            Orient::RotateCw => image::imageops::rotate90(img),
            Orient::RotateCcw => image::imageops::rotate270(img),
        };
        let mirrored = matches!(op, Orient::FlipH | Orient::FlipV);
        self.base = Arc::new(map_img(&self.base));
        self.crop = self.crop.map(map_rect);
        for item in &mut self.items {
            match &mut item.shape {
                Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Redact { rect, .. } | Shape::Spotlight { rect, .. } => *rect = map_rect(*rect),
                Shape::Image { rect, image } => {
                    *rect = map_rect(*rect);
                    *image = Arc::new(map_img(image));
                }
                Shape::Line { a, b } => {
                    *a = map(*a);
                    *b = map(*b);
                }
                Shape::Arrow { a, b, bend, .. } => {
                    *a = map(*a);
                    *b = map(*b);
                    if mirrored {
                        *bend = -*bend;
                    }
                }
                Shape::Pen { pts } | Shape::Highlighter { pts } => pts.iter_mut().for_each(|p| *p = map(*p)),
                Shape::Text { pos, text, size, style, wrap } => {
                    // Keep text upright: move its box, not its glyphs.
                    let r = map_rect(text_box(*pos, text, *size, *style, *wrap));
                    *pos = r.min + Vec2::splat(text_padding(*size, *style));
                }
                Shape::Counter { center, .. } => *center = map(*center),
            }
        }
    }

    pub fn save_project(&self, path: &Path) -> Result<()> {
        let file = ProjectFile { format: PROJECT_FORMAT.into(), version: PROJECT_VERSION, doc: self.clone() };
        crate::write_atomic(path, &serde_json::to_vec(&file)?)?;
        Ok(())
    }

    pub fn load_project(path: &Path) -> Result<Self> {
        let file: ProjectFile = serde_json::from_slice(&std::fs::read(path)?)?;
        if file.format != PROJECT_FORMAT {
            return Err(Error::NotAProject);
        }
        if file.version > PROJECT_VERSION {
            return Err(Error::TooNew(file.version));
        }
        Ok(file.doc)
    }

    /// Open a project file or any supported image.
    pub fn open(path: &Path) -> Result<Self> {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXT)) {
            return Self::load_project(path);
        }
        Ok(Self::new(image::open(path)?.into_rgba8()))
    }
}

/// Images are stored as base64 PNG strings so a project is one self-contained JSON file.
mod png_b64 {
    use std::io::Cursor;
    use std::sync::Arc;

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use image::RgbaImage;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(img: &Arc<RgbaImage>, s: S) -> Result<S::Ok, S::Error> {
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).map_err(serde::ser::Error::custom)?;
        s.serialize_str(&STANDARD.encode(png))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<RgbaImage>, D::Error> {
        let b64 = String::deserialize(d)?;
        let png = STANDARD.decode(b64).map_err(D::Error::custom)?;
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).map_err(D::Error::custom)?;
        Ok(Arc::new(img.into_rgba8()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> Doc {
        let mut base = RgbaImage::new(40, 20);
        base.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut doc = Doc::new(base);
        doc.items.push(Item { shape: Shape::Line { a: pos2(1.0, 2.0), b: pos2(30.0, 10.0) }, color: Color32::RED, width: 4.0 });
        doc.items.push(Item {
            shape: Shape::Arrow { a: pos2(5.0, 5.0), b: pos2(35.0, 15.0), style: ArrowStyle::Curved, bend: 0.25 },
            color: Color32::BLUE,
            width: 3.0,
        });
        doc.crop = Some(Rect::from_min_max(pos2(2.0, 1.0), pos2(30.0, 18.0)));
        doc.background = Some(Background { fill: BgFill::Image(Arc::new(RgbaImage::new(3, 3))), ..Default::default() });
        doc
    }

    #[test]
    fn custom_gradients_interpolate_their_stops() {
        let g =
            OwnedGradient { angle: 90.0, stops: vec![GradientStop { at: 0.0, color: [0, 0, 0, 255] }, GradientStop { at: 1.0, color: [255, 255, 255, 255] }] };
        assert_eq!(g.color_at(0.0), [0, 0, 0, 255]);
        assert_eq!(g.color_at(1.0), [255, 255, 255, 255]);
        assert_eq!(g.color_at(0.5), [128, 128, 128, 255]);
        // Stops given out of order still read left to right, and a single stop repeats itself.
        let unsorted =
            OwnedGradient { angle: 0.0, stops: vec![GradientStop { at: 1.0, color: [0, 0, 255, 255] }, GradientStop { at: 0.0, color: [255, 0, 0, 255] }] };
        assert_eq!(unsorted.color_at(0.2), [204, 0, 51, 255]);
        let single = OwnedGradient { angle: 0.0, stops: vec![GradientStop { at: 0.0, color: [1, 2, 3, 255] }] };
        assert_eq!(single.sorted_stops().len(), 2);
        assert_eq!(single.color_at(0.7), [1, 2, 3, 255]);
    }

    #[test]
    fn an_old_counter_style_loads_with_the_new_shadow_defaults() {
        let old = r#"{"size":28.0,"label":"Numbers","outline":null,"outline_width":3.0,"shadow":0.3,"padding":0.28}"#;
        let style: CounterStyle = serde_json::from_str(old).unwrap();
        assert!(style.shadow_on, "projects saved before the switch keep their shadow");
        assert_eq!((style.shadow_blur, style.shadow_angle, style.shadow_distance), (6.0, 90.0, 4.0));
    }

    #[test]
    fn an_old_commentary_still_loads() {
        let old = r#"{"background":[28,28,30,255],"text_color":[255,255,255,255],"size":22.0,"padding":24.0,"entries":[]}"#;
        let commentary: Commentary = serde_json::from_str(old).unwrap();
        assert_eq!(commentary.side, CommentarySide::Bottom, "a band saved before sides existed stays under the image");
        assert_eq!(commentary.width, 0.0, "and it fits its own width");
        assert!(commentary.match_counter, "badges follow their counters by default");
    }

    #[test]
    fn counter_labels_read_as_numbers_or_letters() {
        assert_eq!(CounterStyle::default().label_for(3), "3");
        let letters_style = CounterStyle { label: CounterLabel::Letters, ..Default::default() };
        assert_eq!((letters(1).as_str(), letters(26).as_str(), letters(27).as_str()), ("A", "Z", "AA"));
        assert_eq!(letters_style.label_for(28), "AB");
    }

    #[test]
    fn a_paragraph_is_taller_and_narrower_than_one_line() {
        let text = "the quick brown fox jumps over the lazy dog";
        let one = text_size(text, 20.0, None);
        let paragraph = text_size(text, 20.0, Some(140.0));
        assert!(paragraph.y > one.y * 1.5, "{one:?} vs {paragraph:?}");
        assert!(paragraph.x <= 140.0, "{paragraph:?}");
    }

    #[test]
    fn watermark_rects_follow_the_anchor_and_the_drag() {
        let mark = |anchor, offset| Watermark {
            image: Arc::new(RgbaImage::from_pixel(10, 5, image::Rgba([255, 255, 255, 255]))),
            anchor,
            width: 0.2,
            offset,
            opacity: 1.0,
        };
        let canvas = vec2(100.0, 100.0);
        // 20 px wide, 10 px tall (the image is 2:1), inset by the margin.
        let top_left = mark(Anchor::TopLeft, Vec2::ZERO).rect(canvas, 10.0);
        assert_eq!((top_left.min, top_left.max), (pos2(10.0, 10.0), pos2(30.0, 20.0)));
        let centre = mark(Anchor::MiddleCenter, Vec2::ZERO).rect(canvas, 10.0);
        assert_eq!((centre.min, centre.max), (pos2(40.0, 45.0), pos2(60.0, 55.0)));
        let dragged = mark(Anchor::BottomRight, vec2(-5.0, -8.0)).rect(canvas, 10.0);
        assert_eq!((dragged.min, dragged.max), (pos2(65.0, 72.0), pos2(85.0, 82.0)));
    }

    #[test]
    fn project_round_trips_through_disk() {
        let doc = sample_doc();
        let path = std::env::temp_dir().join(format!("shotcraft-test-{}.shotcraft", std::process::id()));
        doc.save_project(&path).unwrap();
        let loaded = Doc::load_project(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(loaded, doc);
    }

    #[test]
    fn rejects_foreign_json() {
        let path = std::env::temp_dir().join(format!("shotcraft-foreign-{}.shotcraft", std::process::id()));
        std::fs::write(&path, br#"{"format":"other","version":1}"#).unwrap();
        assert!(Doc::load_project(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn four_rotations_restore_geometry() {
        let original = sample_doc();
        let mut doc = original.clone();
        for _ in 0..4 {
            doc.reorient(Orient::RotateCw);
        }
        assert_eq!(doc.base, original.base);
        assert_eq!(doc.items[0], original.items[0]);
        assert_eq!(doc.crop, original.crop);
    }

    #[test]
    fn rotation_moves_pixels_and_points_together() {
        let mut doc = sample_doc();
        doc.reorient(Orient::RotateCw);
        // Base pixel (0,0) of a 40x20 image lands at column h-1 = 19 after a clockwise turn.
        assert_eq!(doc.base.dimensions(), (20, 40));
        assert_eq!(doc.base.get_pixel(19, 0).0, [255, 0, 0, 255]);
        let Shape::Line { a, .. } = doc.items[0].shape else { panic!() };
        assert_eq!(a, pos2(18.0, 1.0));
    }

    #[test]
    fn flip_mirrors_curve_bend() {
        let mut doc = sample_doc();
        doc.reorient(Orient::FlipH);
        let Shape::Arrow { a, b, bend, .. } = doc.items[1].shape else { panic!() };
        let mid = curve_point(a, curve_control(a, b, bend), b, 0.5);
        // The flipped curve's midpoint is the mirror of the original midpoint.
        let orig = curve_point(pos2(5.0, 5.0), curve_control(pos2(5.0, 5.0), pos2(35.0, 15.0), 0.25), pos2(35.0, 15.0), 0.5);
        assert!((mid.x - (40.0 - orig.x)).abs() < 1e-3 && (mid.y - orig.y).abs() < 1e-3);
    }

    #[test]
    fn curved_handle_round_trips_bend() {
        let mut item =
            Item { shape: Shape::Arrow { a: pos2(0.0, 0.0), b: pos2(100.0, 0.0), style: ArrowStyle::Curved, bend: 0.3 }, color: Color32::RED, width: 4.0 };
        let mid = item.handles()[2];
        item.set_handle(2, mid, false);
        let Shape::Arrow { bend, .. } = item.shape else { panic!() };
        assert!((bend - 0.3).abs() < 1e-4);
    }

    #[test]
    fn hollow_rect_hit_only_on_border() {
        let item = Item { shape: Shape::Rect { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)), filled: false }, color: Color32::RED, width: 4.0 };
        assert!(item.hit(pos2(1.0, 50.0), 2.0));
        assert!(!item.hit(pos2(50.0, 50.0), 2.0));
    }

    #[test]
    fn counters_continue_numbering() {
        let mut doc = sample_doc();
        assert_eq!(doc.next_counter(), 1);
        doc.items.push(Item { shape: Shape::Counter { center: Pos2::ZERO, n: 4, style: CounterStyle::default() }, color: Color32::RED, width: 4.0 });
        assert_eq!(doc.next_counter(), 5);
    }
}
