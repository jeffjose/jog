//! What Edit draws over a picture: shapes, lines, words, badges, emoji,
//! numbered marks, blurred patches, spotlights and zoomed-in views. They
//! are kept in the picture's own pixels, so the same marks are drawn
//! small over the picture on screen and at full size into the file that
//! is saved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use image::RgbaImage;
use rayon::prelude::*;

use crate::lettering::{self, Look};
use crate::ui::mix;

pub type Point = (f32, f32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Rect,
    Oval,
    Arrow,
    Line,
    Pen,
    Marker,
    Text,
    Counter,
    Badge,
    Emoji,
    Blur,
    Spotlight,
    Zoom,
    Crop,
    /// Takes a color from the picture.
    Pipette,
}

impl Tool {
    /// The tools in the order of the bar, a `None` where a gap parts them.
    pub const BAR: [Option<Tool>; 18] = [
        Some(Tool::Select),
        None,
        Some(Tool::Rect),
        Some(Tool::Oval),
        Some(Tool::Arrow),
        Some(Tool::Line),
        Some(Tool::Pen),
        Some(Tool::Marker),
        Some(Tool::Text),
        Some(Tool::Counter),
        Some(Tool::Badge),
        Some(Tool::Emoji),
        None,
        Some(Tool::Blur),
        Some(Tool::Spotlight),
        Some(Tool::Zoom),
        None,
        Some(Tool::Crop),
    ];

    /// The key that picks it, as hotshot has them.
    pub fn key(self) -> &'static str {
        match self {
            Tool::Select => "v",
            Tool::Rect => "r",
            Tool::Oval => "o",
            Tool::Arrow => "a",
            Tool::Line => "l",
            Tool::Pen => "p",
            Tool::Marker => "h",
            Tool::Text => "t",
            Tool::Counter => "n",
            Tool::Badge => "j",
            Tool::Emoji => "e",
            Tool::Blur => "b",
            Tool::Spotlight => "s",
            Tool::Zoom => "z",
            Tool::Crop => "x",
            Tool::Pipette => "i",
        }
    }

    pub fn of_key(key: &str) -> Option<Tool> {
        Tool::BAR.into_iter().flatten().chain([Tool::Pipette]).find(|tool| tool.key() == key)
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rect => "Rectangle",
            Tool::Oval => "Oval",
            Tool::Arrow => "Arrow",
            Tool::Line => "Line",
            Tool::Pen => "Pen",
            Tool::Marker => "Marker",
            Tool::Text => "Text",
            Tool::Counter => "Counter",
            Tool::Badge => "Badge",
            Tool::Emoji => "Emoji",
            Tool::Blur => "Blur",
            Tool::Spotlight => "Spotlight",
            Tool::Zoom => "Zoom",
            Tool::Crop => "Crop",
            Tool::Pipette => "Pick a color",
        }
    }

    /// Whether it is drawn by dragging out from one point to another.
    pub fn spans(self) -> bool {
        matches!(self, Tool::Rect | Tool::Oval | Tool::Arrow | Tool::Line | Tool::Blur | Tool::Spotlight | Tool::Zoom)
    }

    /// Whether its size is that of its letters, kept in proportion.
    pub fn lettered(self) -> bool {
        matches!(self, Tool::Text | Tool::Emoji | Tool::Badge)
    }

    pub fn is_line(self) -> bool {
        matches!(self, Tool::Arrow | Tool::Line)
    }
}

/// How a line or an outline is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Dash {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// The ways words can be set, any of them together: heavier, slanted, on
/// a wash of yellow as if gone over with a marker, on a patch of their
/// own color, and each line after a dot.
pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const HIGHLIGHT: u8 = 4;
pub const BOXED: u8 = 8;
pub const BULLETS: u8 = 16;

/// The colors to hand, a click each.
pub const COLORS: [u32; 8] = [0xFF3B30, 0xFF9500, 0xFFCC00, 0x34C759, 0x0A84FF, 0xAF52DE, 0xFFFFFF, 0x1C1C1E];

/// The rest, in the color picker: Radix's, at the step meant for solid
/// backgrounds, as hotshot has them; the reds round to the violets, the
/// blues to the greens, the bright ones and those that are no color.
pub const PALETTE: [u32; 24] = [
    0xE54D2E, 0xE5484D, 0xE54666, 0xE93D82, 0xD6409F, 0xAB4ABA, 0x8E4EC6, 0x6E56CF,
    0x5B5BD6, 0x3E63DD, 0x0090FF, 0x00A2C7, 0x12A594, 0x29A383, 0x30A46C, 0x46A758,
    0xBDEE63, 0xFFE629, 0xFFC53D, 0xF76B15, 0xAD7F58, 0x8D8D8D, 0xFFFFFF, 0x161616,
];

/// A color as hue (0 to 360), saturation and value (0 to 1).
pub fn to_hsv(color: u32) -> (f32, f32, f32) {
    let [r, g, b] = [16, 8, 0].map(|shift| (color >> shift & 0xFF) as f32 / 255.0);
    let (most, least) = (r.max(g).max(b), r.min(g).min(b));
    let spread = most - least;
    let hue = if spread == 0.0 {
        0.0
    } else if most == r {
        60.0 * ((g - b) / spread).rem_euclid(6.0)
    } else if most == g {
        60.0 * ((b - r) / spread + 2.0)
    } else {
        60.0 * ((r - g) / spread + 4.0)
    };
    (hue, if most == 0.0 { 0.0 } else { spread / most }, most)
}

/// The color of a hue, saturation and value.
pub fn from_hsv((hue, saturation, value): (f32, f32, f32)) -> u32 {
    let part = |n: f32| {
        let k = (n + hue / 60.0).rem_euclid(6.0);
        let v = value - value * saturation * k.min(4.0 - k).clamp(0.0, 1.0);
        (v.clamp(0.0, 1.0) * 255.0).round() as u32
    };
    part(5.0) << 16 | part(3.0) << 8 | part(1.0)
}

/// How much darker what is outside a spotlight is: it keeps this much of
/// its brightness, out of 256.
const SPOTLIGHT_KEEPS: u32 = 96;
/// How see-through the marker is, out of 256.
const MARKER_OPACITY: u32 = 110;
/// What a highlight is the color of, and how opaque out of 256.
const HIGHLIGHT_COLOR: (u32, u32) = (0xFFE629, 150);
/// How many straight lines a bent one is drawn as.
const BENDS: usize = 24;

/// A mark on the picture, in the picture's pixels.
#[derive(Clone, PartialEq, Debug)]
pub struct Shape {
    pub tool: Tool,
    pub color: u32,
    /// Out of 256.
    pub opacity: u32,
    /// How wide its line is; for words and emoji, how tall the letters are.
    pub stroke: f32,
    /// Where the drag began and where it ended; a pen's every point; the
    /// top-left of words and emoji; the middle of a counter and the tip
    /// of its tail; what a zoom is of, and where it shows it.
    pub points: Vec<Point>,
    /// The words, or the emoji.
    pub text: String,
    /// Filled rather than outlined; a zoom square rather than round.
    pub filled: bool,
    /// How round a rectangle's or a spotlight's corners are.
    pub radius: f32,
    /// Whether a line has a head at its first end and at its last.
    pub heads: [bool; 2],
    pub dash: Dash,
    /// How long a dash is, or how wide a dot; and the gap between them.
    pub dash_length: f32,
    pub gap: f32,
    /// Where a bent line passes half way along.
    pub bend: Option<Point>,
    /// The face its words are in.
    pub face: usize,
    /// How its words are set: some of `BOLD`, `ITALIC` and the rest.
    pub style: u8,
    /// Which of the looks a badge has.
    pub look: usize,
    /// How far it is turned about its middle, clockwise, in radians:
    /// boxes, ovals, words and emoji. Lines and strokes are turned by their
    /// points instead.
    pub angle: f32,
}

/// Where the picture is drawn: its pixel `(px, py)` lands at
/// `(x + px * scale, y + py * scale)`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

impl Place {
    pub const WHOLE: Place = Place { x: 0.0, y: 0.0, scale: 1.0 };

    pub fn to_screen(self, (px, py): Point) -> Point {
        (self.x + px * self.scale, self.y + py * self.scale)
    }

    pub fn to_picture(self, (sx, sy): Point) -> Point {
        ((sx - self.x) / self.scale, (sy - self.y) / self.scale)
    }
}

fn unit(native: (u32, u32)) -> f32 {
    (native.0.max(native.1) as f32 / 600.0).max(1.0)
}

/// The widths of line the four sizes are, for a picture this large: the
/// same share of it whatever its resolution.
pub fn widths(native: (u32, u32)) -> [f32; 4] {
    [1.5, 3.0, 5.0, 8.0].map(|times| times * unit(native))
}

/// The heights of words the four sizes are.
pub fn text_sizes(native: (u32, u32)) -> [f32; 4] {
    [8.0, 12.0, 18.0, 28.0].map(|times| times * unit(native))
}

/// Whichever of white and near-black reads on `color`.
pub fn ink_on(color: u32) -> u32 {
    let (r, g, b) = ((color >> 16) & 255, (color >> 8) & 255, color & 255);
    if r * 299 + g * 587 + b * 114 > 160_000 { 0x1C1C1E } else { 0xFFFFFF }
}

/// What a picture is drawn into: its pixels, how wide it is, the part of
/// it that may be drawn on (left, top, right, bottom), and how opaque each
/// pixel is, when that is kept.
pub struct Surface<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub clip: [i32; 4],
    pub alpha: Option<&'a mut [u8]>,
}

impl Surface<'_> {
    /// A pixel `amount`/256 of the way to `color`.
    fn blend(&mut self, x: i32, y: i32, color: u32, amount: u32) {
        if amount == 0 || x < self.clip[0] || y < self.clip[1] || x >= self.clip[2] || y >= self.clip[3] {
            return;
        }
        let at = y as usize * self.width + x as usize;
        self.pixels[at] = mix(self.pixels[at], color, amount.min(256));
        if let Some(alpha) = self.alpha.as_deref_mut() {
            alpha[at] = alpha[at].max(amount.min(255) as u8);
        }
    }

    /// Words, as `lettering` draws them.
    #[allow(clippy::too_many_arguments)]
    fn words(&mut self, at: Point, px: f32, color: u32, opacity: u32, look: Look, words: &str) {
        lettering::draw(self.pixels, self.width, self.clip, at, px, color, opacity, look, words, self.alpha.as_deref_mut());
    }

    /// A rounded rectangle (left, top, right, bottom), each pixel in what
    /// `color` says for how far across and down it is, from 0 to 1. `soft`
    /// fades its edge over that many pixels, as a shadow or glow has it.
    fn patch(&mut self, [l, t, r, b]: [f32; 4], radius: f32, soft: f32, amount: u32, color: impl Fn(f32, f32) -> u32) {
        let (hw, hh) = ((r - l) / 2.0, (b - t) / 2.0);
        if hw <= 0.0 || hh <= 0.0 {
            return;
        }
        let (cx, cy) = (l + hw, t + hh);
        let radius = radius.min(hw).min(hh).max(0.0);
        let reach = soft.ceil() as i32 + 1;
        for y in (t.floor() as i32 - reach).max(self.clip[1])..(b.ceil() as i32 + reach).min(self.clip[3]) {
            for x in (l.floor() as i32 - reach).max(self.clip[0])..(r.ceil() as i32 + reach).min(self.clip[2]) {
                let away = round_box(x as f32 + 0.5 - cx, y as f32 + 0.5 - cy, hw, hh, radius);
                let cover = if soft > 0.0 { (0.5 - away / soft).clamp(0.0, 1.0).powi(2) } else { (0.5 - away).clamp(0.0, 1.0) };
                if cover > 0.0 {
                    let (across, down) = (((x as f32 - l) / (r - l)).clamp(0.0, 1.0), ((y as f32 - t) / (b - t)).clamp(0.0, 1.0));
                    self.blend(x, y, color(across, down), (cover * amount as f32) as u32);
                }
            }
        }
    }

    /// The edge of a rounded rectangle, `thick` wide.
    fn outline(&mut self, [l, t, r, b]: [f32; 4], radius: f32, thick: f32, color: u32, amount: u32) {
        let (hw, hh) = ((r - l) / 2.0, (b - t) / 2.0);
        let (cx, cy) = (l + hw, t + hh);
        let radius = radius.min(hw).min(hh).max(0.0);
        for y in (t.floor() as i32 - 2).max(self.clip[1])..(b.ceil() as i32 + 2).min(self.clip[3]) {
            for x in (l.floor() as i32 - 2).max(self.clip[0])..(r.ceil() as i32 + 2).min(self.clip[2]) {
                let away = round_box(x as f32 + 0.5 - cx, y as f32 + 0.5 - cy, hw - thick / 2.0, hh - thick / 2.0, (radius - thick / 2.0).max(0.0)).abs() - thick / 2.0;
                self.blend(x, y, color, ((0.5 - away).clamp(0.0, 1.0) * amount as f32) as u32);
            }
        }
    }

    fn disc(&mut self, (cx, cy): Point, radius: f32, color: u32, amount: u32) {
        self.patch([cx - radius, cy - radius, cx + radius, cy + radius], radius, 0.0, amount, |_, _| color);
    }
}

impl Shape {
    pub fn new(tool: Tool, color: u32, stroke: f32, at: Point) -> Shape {
        let points = if tool.spans() { vec![at, at] } else { vec![at] };
        let heads = [false, tool == Tool::Arrow];
        Shape { tool, color, opacity: 256, stroke, points, text: String::new(), filled: false, radius: stroke * 1.2, heads, dash: Dash::Solid, dash_length: 0.0, gap: 0.0, bend: None, face: 0, style: 0, look: 0, angle: 0.0 }
    }

    /// Drawn solid, in dashes or in dots, those as long and as far apart
    /// as suits its width until they are set otherwise.
    pub fn set_dash(&mut self, dash: Dash) {
        if dash == self.dash {
            return;
        }
        self.dash = dash;
        (self.dash_length, self.gap) = match dash {
            Dash::Solid => (self.dash_length, self.gap),
            Dash::Dashed => (self.stroke * 3.0 + 4.0, self.stroke * 2.0 + 3.0),
            Dash::Dotted => (self.stroke, self.stroke * 1.2 + 1.0),
        };
    }

    /// The rectangle its first two points span: left, top, right, bottom.
    pub fn frame(&self) -> [f32; 4] {
        let (a, b) = (self.points[0], *self.points.get(1).unwrap_or(&self.points[0]));
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
    }

    pub fn counter_radius(&self) -> f32 {
        self.stroke * 2.4 + 6.0
    }

    pub fn lettering(&self) -> Look {
        Look { face: self.face, bold: self.style & BOLD != 0, italic: self.style & ITALIC != 0 }
    }

    /// What each line of its words begins with: a dot, with bullets.
    pub fn lead(&self) -> &'static str {
        if self.style & BULLETS != 0 { "\u{2022} " } else { "" }
    }

    /// How far a badge's words are from its left edge.
    fn badge_lead(&self) -> f32 {
        let px = self.stroke;
        match self.look {
            0 | 4 => px * 1.25,
            5 => px,
            9 => px * 1.5,
            10 => px * 0.5,
            _ => px * 0.75,
        }
    }

    /// Where its words begin: the top-left of the first line's box, unturned.
    pub fn text_origin(&self) -> Point {
        let (x, y) = self.points[0];
        match self.tool {
            Tool::Badge => (x + self.badge_lead(), y + lettering::line_height(self.stroke) / 6.0),
            _ => (x, y),
        }
    }

    /// What each line begins with, as drawn: a badge has no bullets.
    fn line_lead(&self) -> &'static str {
        if self.tool == Tool::Text { self.lead() } else { "" }
    }

    /// Where the caret stands before letter `at` of its words: the top of
    /// it, unturned, and how tall it is.
    pub fn caret_spot(&self, at: usize) -> (Point, f32) {
        let before: String = self.text.chars().take(at).collect();
        let line = before.matches('\n').count();
        let column = before.rsplit('\n').next().unwrap_or_default();
        let (x, y) = self.text_origin();
        let height = lettering::line_height(self.stroke);
        let wide = lettering::width(self.lettering(), &format!("{}{column}", self.line_lead()), self.stroke);
        let wide = if column.is_empty() && self.line_lead().is_empty() { 0.0 } else { wide };
        ((x + wide, y + line as f32 * height), height)
    }

    /// The letter whose place is nearest a point of the picture, where a
    /// click there puts the caret.
    pub fn caret_at(&self, p: Point) -> usize {
        let p = self.unturn(p);
        let (x, y) = self.text_origin();
        let height = lettering::line_height(self.stroke);
        let lines: Vec<&str> = self.text.split('\n').collect();
        let line = (((p.1 - y) / height).floor().max(0.0) as usize).min(lines.len() - 1);
        let before: usize = lines[..line].iter().map(|words| words.chars().count() + 1).sum();
        let words = lines[line];
        let lead = self.line_lead();
        let column = (0..=words.chars().count())
            .min_by(|a, b| {
                let at = |n: usize| x + lettering::width(self.lettering(), &format!("{lead}{}", words.chars().take(n).collect::<String>()), self.stroke);
                (at(*a) - p.0).abs().total_cmp(&(at(*b) - p.0).abs())
            })
            .unwrap_or(0);
        before + column
    }

    /// Where a zoom's larger view is: left, top, right, bottom.
    pub fn zoomed(&self) -> [f32; 4] {
        // Still being dragged out, it shows nothing larger yet.
        let (Some(c), Some(d)) = (self.points.get(2).copied(), self.points.get(3).copied()) else { return self.frame() };
        [c.0.min(d.0), c.1.min(d.1), c.0.max(d.0), c.1.max(d.1)]
    }

    /// A zoom just dragged out: shown twice as large, to the right of what
    /// it is of and a little above.
    pub fn place_zoom(&mut self) {
        let [l, t, r, b] = self.frame();
        let (w, h) = (r - l, b - t);
        let at = (r + w / 2.0 + self.stroke * 4.0, t - h / 2.0);
        self.points.truncate(2);
        self.points.extend([at, (at.0 + w * 2.0, at.1 + h * 2.0)]);
    }

    /// How many times larger a zoom shows what it is of.
    pub fn zoom_times(&self) -> f32 {
        let [l, _, r, _] = self.frame();
        let [zl, _, zr, _] = self.zoomed();
        (zr - zl) / (r - l).max(1.0)
    }

    /// A zoom's two handles: in the middle of its larger view, which moves
    /// that, and on its rim, which says how many times larger it is.
    pub fn zoom_handles(&self) -> [Point; 2] {
        let [l, t, r, b] = self.zoomed();
        let middle = ((l + r) / 2.0, (t + b) / 2.0);
        let out = if self.filled { 0.5 } else { std::f32::consts::FRAC_1_SQRT_2 / 2.0 };
        [middle, (middle.0 + (r - l) * out, middle.1 + (b - t) * out)]
    }

    /// One of a zoom's handles taken to a point: its middle there, or its
    /// rim under the point, its middle staying.
    pub fn zoom_to(&mut self, rim: bool, to: Point) {
        if self.points.len() < 4 {
            return;
        }
        let [l, t, r, b] = self.zoomed();
        let [ol, ot, or, ob] = self.frame();
        let (mut middle, mut size) = (((l + r) / 2.0, (t + b) / 2.0), (r - l, b - t));
        if rim {
            let out = if self.filled { 0.5 } else { std::f32::consts::FRAC_1_SQRT_2 / 2.0 };
            let across = (to.0 - middle.0).abs() / out / (or - ol).max(1.0);
            let down = (to.1 - middle.1).abs() / out / (ob - ot).max(1.0);
            let times = across.max(down).clamp(1.25, 12.0);
            size = ((or - ol) * times, (ob - ot) * times);
        } else {
            middle = to;
        }
        self.points[2] = (middle.0 - size.0 / 2.0, middle.1 - size.1 / 2.0);
        self.points[3] = (middle.0 + size.0 / 2.0, middle.1 + size.1 / 2.0);
    }

    /// Whether it turns by an angle of its own, rather than by its points.
    pub fn turnable(&self) -> bool {
        matches!(self.tool, Tool::Rect | Tool::Oval | Tool::Text | Tool::Badge | Tool::Emoji)
    }

    /// Whether it can be turned at all: not a blur, a spotlight or a zoom,
    /// which are of what is square to the picture.
    pub fn turns(&self) -> bool {
        self.turnable() || matches!(self.tool, Tool::Pen | Tool::Marker | Tool::Arrow | Tool::Line)
    }

    /// What it turns about: the middle of its box as it is unturned.
    pub fn centre(&self) -> Point {
        let [l, t, r, b] = self.local_bounds();
        ((l + r) / 2.0, (t + b) / 2.0)
    }

    /// A point of the picture as it would be if the mark were not turned.
    pub fn unturn(&self, p: Point) -> Point {
        if self.angle == 0.0 {
            return p;
        }
        turn_point(p, self.centre(), -self.angle)
    }

    /// A point of the mark as unturned, where it is with the mark turned.
    pub fn returned(&self, p: Point) -> Point {
        if self.angle == 0.0 {
            return p;
        }
        turn_point(p, self.centre(), self.angle)
    }

    /// Turned by `by` about `about`: by its angle, moved round with it,
    /// or by its points.
    pub fn turn(&mut self, by: f32, about: Point) {
        if self.turnable() {
            let (from, to) = (self.centre(), turn_point(self.centre(), about, by));
            self.translate(to.0 - from.0, to.1 - from.1);
            self.angle = (self.angle + by).rem_euclid(std::f32::consts::TAU);
        } else {
            for point in &mut self.points {
                *point = turn_point(*point, about, by);
            }
            if let Some(bend) = &mut self.bend {
                *bend = turn_point(*bend, about, by);
            }
        }
    }

    /// The corners of its box as unturned, turned: where they are.
    pub fn corners(&self) -> [Point; 4] {
        let [l, t, r, b] = self.local_bounds();
        [(l, t), (r, t), (r, b), (l, b)].map(|p| self.returned(p))
    }

    /// What it covers, in the picture's pixels: left, top, right, bottom;
    /// turned, the box that holds it as it is.
    pub fn bounds(&self) -> [f32; 4] {
        if self.angle == 0.0 {
            return self.local_bounds();
        }
        let corners = self.corners();
        let (xs, ys) = (corners.map(|p| p.0), corners.map(|p| p.1));
        [xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max)]
    }

    /// What it covers as if it were not turned.
    pub fn local_bounds(&self) -> [f32; 4] {
        let reach = self.stroke * 0.5 + 1.0;
        let around = |points: &[Point], reach: f32| {
            let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (x, y) in points {
                (l, t, r, b) = (l.min(*x), t.min(*y), r.max(*x), b.max(*y));
            }
            [l - reach, t - reach, r + reach, b + reach]
        };
        match self.tool {
            Tool::Text => {
                let (x, y) = self.points[0];
                let line = lettering::line_height(self.stroke);
                let lines: Vec<&str> = self.text.split('\n').collect();
                let wide = lines.iter().map(|words| lettering::width(self.lettering(), &format!("{}{words}", self.lead()), self.stroke)).fold(self.stroke / 4.0, f32::max);
                let high = line * lines.len() as f32;
                // On a patch, the patch with it.
                let room = if self.style & BOXED != 0 { line / 4.0 } else { 0.0 };
                [x - room * 1.5, y - room / 2.0, x + wide + room * 1.5, y + high + room / 2.0]
            }
            Tool::Badge => {
                let (x, y) = self.points[0];
                let line = lettering::line_height(self.stroke);
                let wide = lettering::width(self.lettering(), &self.text, self.stroke) + self.badge_lead() + self.stroke * 0.75;
                [x, y, x + wide, y + line * 4.0 / 3.0]
            }
            Tool::Emoji => {
                let (x, y) = self.points[0];
                [x, y, x + self.stroke, y + self.stroke]
            }
            Tool::Counter => {
                let ((x, y), r) = (self.points[0], self.counter_radius());
                let [l, t, rr, b] = [x - r, y - r, x + r, y + r];
                match self.points.get(1) {
                    Some(tip) => [l.min(tip.0), t.min(tip.1), rr.max(tip.0), b.max(tip.1)],
                    None => [l, t, rr, b],
                }
            }
            Tool::Zoom => {
                let ([l, t, r, b], [zl, zt, zr, zb]) = (self.frame(), self.zoomed());
                [l.min(zl) - reach, t.min(zt) - reach, r.max(zr) + reach, b.max(zb) + reach]
            }
            Tool::Arrow | Tool::Line => around(&self.path(), reach + self.head().1),
            Tool::Pen | Tool::Marker => around(&self.points, self.line_width() * 0.5 + 1.0),
            Tool::Blur | Tool::Spotlight => self.frame(),
            _ => {
                let [l, t, r, b] = self.frame();
                [l - reach, t - reach, r + reach, b + reach]
            }
        }
    }

    fn line_width(&self) -> f32 {
        if self.tool == Tool::Marker { self.stroke * 3.5 } else { self.stroke }
    }

    /// An arrow's head: how long, and how far out to either side.
    fn head(&self) -> (f32, f32) {
        let length = (self.stroke * 3.2 + 9.0).max(1.0);
        (length, length * 0.46)
    }

    /// A line as the straight lines it is drawn as: its two ends, or the
    /// curve through its bend.
    pub fn path(&self) -> Vec<Point> {
        let (a, b) = (self.points[0], self.points[1]);
        let Some(through) = self.bend else { return vec![a, b] };
        // What the curve is pulled towards, twice as far from the straight
        // line as the curve gets.
        let pull = (2.0 * through.0 - (a.0 + b.0) / 2.0, 2.0 * through.1 - (a.1 + b.1) / 2.0);
        (0..=BENDS)
            .map(|step| {
                let (t, u) = (step as f32 / BENDS as f32, 1.0 - step as f32 / BENDS as f32);
                (u * u * a.0 + 2.0 * u * t * pull.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * pull.1 + t * t * b.1)
            })
            .collect()
    }

    /// The outline of a rectangle or an oval as straight lines all round,
    /// for its dashes to be counted along.
    fn ring(&self) -> Vec<Point> {
        let [l, t, r, b] = self.frame();
        let (cx, cy, hw, hh) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
        let arc = |cx: f32, cy: f32, rx: f32, ry: f32, from: f32, steps: usize| -> Vec<Point> {
            (0..=steps).map(|step| from + std::f32::consts::FRAC_PI_2 * step as f32 / steps as f32).map(|angle| (cx + rx * angle.cos(), cy + ry * angle.sin())).collect()
        };
        let mut points = match self.tool {
            Tool::Oval => (0..4).flat_map(|quarter| arc(cx, cy, hw, hh, quarter as f32 * std::f32::consts::FRAC_PI_2, 18)).collect(),
            _ => {
                let radius = self.radius.min(hw).min(hh).max(0.0);
                let corners = [(r - radius, b - radius), (l + radius, b - radius), (l + radius, t + radius), (r - radius, t + radius)];
                corners.iter().enumerate().flat_map(|(at, (x, y))| arc(*x, *y, radius, radius, at as f32 * std::f32::consts::FRAC_PI_2, 8)).collect::<Vec<_>>()
            }
        };
        points.push(points[0]);
        points
    }

    /// Where the dot that bends a line is: on it, half way.
    pub fn middle(&self) -> Point {
        self.bend.unwrap_or_else(|| {
            let (a, b) = (self.points[0], self.points[1]);
            ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
        })
    }

    /// Bends a line through `to`, or straightens it if that is near enough
    /// the straight line.
    pub fn bend_to(&mut self, to: Point, slack: f32) {
        let straight = segment(to, self.points[0], self.points[1]) < slack;
        self.bend = (!straight).then_some(to);
    }

    /// Whether a point of the picture is on it, `reach` pixels allowed.
    pub fn hit(&self, p: Point, reach: f32) -> bool {
        // Turned, the point is taken round the other way, onto it unturned.
        let p = self.unturn(p);
        let within = |[l, t, r, b]: [f32; 4]| p.0 >= l - reach && p.0 <= r + reach && p.1 >= t - reach && p.1 <= b + reach;
        match self.tool {
            Tool::Text | Tool::Badge | Tool::Emoji | Tool::Blur | Tool::Spotlight => within(self.local_bounds()),
            Tool::Zoom => within(self.frame()) || within(self.zoomed()),
            Tool::Pen | Tool::Marker => {
                let half = self.line_width() / 2.0 + reach;
                match self.points.as_slice() {
                    [one] => dist(p, *one) <= half,
                    points => points.windows(2).any(|pair| segment(p, pair[0], pair[1]) <= half),
                }
            }
            // Filled, or outlined: on the line, or anywhere inside.
            Tool::Rect | Tool::Oval if self.filled => self.distance(p, true) <= reach,
            _ => self.distance(p, true) <= reach,
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for point in &mut self.points {
            point.0 += dx;
            point.1 += dy;
        }
        if let Some(bend) = &mut self.bend {
            bend.0 += dx;
            bend.1 += dy;
        }
    }

    /// Made larger or smaller by `times` across and down, `still` keeping
    /// its place. Letters keep their proportions, by the larger of the two.
    pub fn scale(&mut self, still: Point, times: (f32, f32)) {
        let times = if self.tool.lettered() {
            let both = times.0.abs().max(times.1.abs()).max(0.02);
            (both, both)
        } else {
            times
        };
        let move_point = |p: &mut Point| *p = (still.0 + (p.0 - still.0) * times.0, still.1 + (p.1 - still.1) * times.1);
        self.points.iter_mut().for_each(move_point);
        if let Some(bend) = &mut self.bend {
            move_point(bend);
        }
        if self.tool.lettered() {
            self.stroke = (self.stroke * times.0).max(6.0);
        }
    }

    /// Whether it came to nothing: a click with a tool that wants a drag.
    pub fn is_slight(&self) -> bool {
        match self.tool {
            Tool::Text | Tool::Emoji | Tool::Badge => self.text.trim().is_empty(),
            Tool::Counter | Tool::Pen | Tool::Marker => false,
            Tool::Zoom => {
                let [l, t, r, b] = self.frame();
                (r - l).min(b - t) < 4.0
            }
            _ => {
                let [l, t, r, b] = self.frame();
                (r - l).max(b - t) < 3.0
            }
        }
    }

    /// How far a point is from its outline, negative inside it; `plain`
    /// leaves out dashes, as for telling whether a click is on it.
    fn distance(&self, p: Point, plain: bool) -> f32 {
        let half = self.stroke / 2.0;
        let [l, t, r, b] = self.frame();
        let (cx, cy, hw, hh) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
        let outline = |inside: f32| {
            if self.filled {
                return inside - half;
            }
            let solid = inside.abs() - half;
            // Dashed: counted along the outline, but only near it, where
            // it can make a difference.
            if plain || self.dash == Dash::Solid || solid > self.stroke {
                solid
            } else {
                run_distance(p, &self.ring(), (self.dash, self.dash_length, self.gap), self.stroke, (0.0, 0.0))
            }
        };
        match self.tool {
            Tool::Rect => outline(round_box(p.0 - cx, p.1 - cy, hw, hh, self.radius.min(hw).min(hh).max(0.0))),
            Tool::Oval => outline(ellipse(p.0 - cx, p.1 - cy, hw, hh)),
            Tool::Line | Tool::Arrow => self.line_distance(p, plain),
            Tool::Counter => {
                let (middle, radius) = (self.points[0], self.counter_radius());
                let disc = dist(p, middle) - radius;
                let Some(tip) = self.points.get(1).copied() else { return disc };
                let length = dist(middle, tip);
                if length < radius * 1.4 {
                    return disc;
                }
                // A tail from the disc to what it points at.
                let (dx, dy) = ((tip.0 - middle.0) / length, (tip.1 - middle.1) / length);
                let wide = radius * 0.55;
                disc.min(triangle(p, tip, (middle.0 - dy * wide, middle.1 + dx * wide), (middle.0 + dy * wide, middle.1 - dx * wide)) - 0.6)
            }
            _ => f32::MAX,
        }
    }

    fn line_distance(&self, p: Point, plain: bool) -> f32 {
        let path = self.path();
        let whole: f32 = path.windows(2).map(|pair| dist(pair[0], pair[1])).sum();
        let half = self.stroke / 2.0;
        if whole < 0.5 {
            return dist(p, path[0]) - half;
        }
        // The heads: as long as they would be, but no more than a share of
        // the line, which would be all head.
        let most = if self.heads == [true, true] { 0.45 } else { 0.7 };
        let head = self.head().0.min(whole * most);
        let wide = head * 0.46;
        let lengths: Vec<f32> = path.windows(2).map(|pair| dist(pair[0], pair[1])).collect();
        let mut nearest = f32::MAX;
        for (end, on) in self.heads.iter().enumerate() {
            if !on {
                continue;
            }
            let (tip, back) = if end == 1 { (path[path.len() - 1], along(&path, &lengths, whole - head)) } else { (path[0], along(&path, &lengths, head)) };
            let length = dist(tip, back).max(1e-3);
            let (dx, dy) = ((tip.0 - back.0) / length, (tip.1 - back.1) / length);
            let base = (tip.0 - dx * head, tip.1 - dy * head);
            nearest = nearest.min(triangle(p, tip, (base.0 - dy * wide, base.1 + dx * wide), (base.0 + dy * wide, base.1 - dx * wide)) - 0.6);
        }
        // The shaft stops inside a head, whose point its round end would
        // otherwise blunt.
        let trim = (if self.heads[0] { head * 0.8 } else { 0.0 }, if self.heads[1] { head * 0.8 } else { 0.0 });
        let dash = if plain { Dash::Solid } else { self.dash };
        nearest.min(run_distance(p, &path, (dash, self.dash_length, self.gap), self.stroke, trim))
    }
}

/// How far a point is from a run of straight lines `stroke` wide, less
/// `trim` at either end: in dashes `length` long, or dots `length` across,
/// with `gap` between them, if so.
fn run_distance(p: Point, path: &[Point], (dash, piece, gap): (Dash, f32, f32), stroke: f32, trim: (f32, f32)) -> f32 {
    let lengths: Vec<f32> = path.windows(2).map(|pair| dist(pair[0], pair[1])).collect();
    let whole: f32 = lengths.iter().sum();
    let half = stroke / 2.0;
    let (first, last) = (trim.0, whole - trim.1);
    let (piece, gap) = (piece.max(0.5), gap.max(0.5));
    // A dash's round ends make it longer by the width of the line: what
    // is between them is that much shorter, so that it is as long as asked.
    let (period, on) = match dash {
        Dash::Solid => (0.0, 0.0),
        Dash::Dashed => (piece + gap, (piece - stroke).max(0.0)),
        Dash::Dotted => (piece + gap, 0.0),
    };
    let mut nearest = f32::MAX;
    let mut gone = 0.0f32;
    for (pair, length) in path.windows(2).zip(&lengths) {
        let (begins, ends) = (gone, gone + length);
        gone = ends;
        if ends <= first || begins >= last || *length <= 0.0 {
            continue;
        }
        let at = |s: f32| (pair[0].0 + (pair[1].0 - pair[0].0) * s / length, pair[0].1 + (pair[1].1 - pair[0].1) * s / length);
        let (s0, s1) = ((first - begins).max(0.0), (last - begins).min(*length));
        let (from, to) = (at(s0), at(s1));
        let run = (s1 - s0).max(1e-3);
        let t = (((p.0 - from.0) * (to.0 - from.0) + (p.1 - from.1) * (to.1 - from.1)) / (run * run)).clamp(0.0, 1.0);
        let foot = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        let across = dist(p, foot);
        let far = match dash {
            Dash::Solid => across - half,
            _ => {
                // How far along the dashes it is, and how far that is from
                // the nearest dash or dot.
                let s = begins + s0 - first + t * run;
                match dash {
                    // Dots as large as asked, whatever the line's width.
                    Dash::Dotted => {
                        let into = (s - piece / 2.0).rem_euclid(period);
                        across.hypot(into.min(period - into)) - piece / 2.0
                    }
                    _ => {
                        let into = (s - half).rem_euclid(period);
                        let off = if into <= on { 0.0 } else { (into - on).min(period - into) };
                        across.hypot(off) - half
                    }
                }
            }
        };
        nearest = nearest.min(far);
    }
    nearest
}

/// The point `s` along a run of straight lines.
fn along(path: &[Point], lengths: &[f32], s: f32) -> Point {
    let mut gone = 0.0;
    for (pair, length) in path.windows(2).zip(lengths) {
        if gone + length >= s && *length > 0.0 {
            let t = (s - gone) / length;
            return (pair[0].0 + (pair[1].0 - pair[0].0) * t, pair[0].1 + (pair[1].1 - pair[0].1) * t);
        }
        gone += length;
    }
    path[path.len() - 1]
}

/// Draws `shapes` over `pixels`, a picture `width` wide, as they look at
/// `place`, inside `clip` (left, top, width, height). Where `alpha` is
/// given, what is drawn becomes as opaque there as it is.
pub fn paint(shapes: &[Shape], pixels: &mut [u32], width: usize, place: Place, clip: [i32; 4], alpha: Option<&mut [u8]>) {
    let height = pixels.len() / width.max(1);
    let clip = [clip[0].max(0), clip[1].max(0), (clip[0] + clip[2]).min(width as i32), (clip[1] + clip[3]).min(height as i32)];
    if clip[2] <= clip[0] || clip[3] <= clip[1] {
        return;
    }
    let mut surface = Surface { pixels, width, clip, alpha };
    // The spotlights darken the picture before anything is drawn on it:
    // the arrow pointing at what one lights is not to be darkened.
    let lit: Vec<([f32; 4], f32)> = shapes.iter().filter(|shape| shape.tool == Tool::Spotlight).map(|shape| (shape.frame(), shape.radius)).collect();
    if !lit.is_empty() {
        spotlight(surface.pixels, width, place, clip, &lit);
    }
    let mut number = 0;
    for shape in shapes {
        if shape.tool == Tool::Counter {
            number += 1;
        }
        draw_shape(&mut surface, shape, place, number);
    }
}

fn draw_shape(surface: &mut Surface, shape: &Shape, place: Place, number: usize) {
    let clip = surface.clip;
    let [l, t, r, b] = shape.bounds();
    let (sl, st) = place.to_screen((l, t));
    let (sr, sb) = place.to_screen((r, b));
    let area = [(sl.floor() as i32).max(clip[0]), (st.floor() as i32).max(clip[1]), (sr.ceil() as i32 + 1).min(clip[2]), (sb.ceil() as i32 + 1).min(clip[3])];
    if area[2] <= area[0] || area[3] <= area[1] {
        return;
    }
    let scale = place.scale;
    let width = surface.width;
    match shape.tool {
        // Words and emoji turned: drawn straight on a sheet of their own,
        // and the sheet laid down turned.
        Tool::Text | Tool::Badge | Tool::Emoji if shape.angle != 0.0 => draw_turned(surface, shape, place, area),
        Tool::Select | Tool::Crop | Tool::Spotlight | Tool::Pipette => {}
        Tool::Blur => pixelate(surface.pixels, width, place, area, (shape.stroke * 4.0).max(8.0)),
        Tool::Pen | Tool::Marker => {
            let half = shape.line_width() / 2.0 * scale;
            let points: Vec<Point> = shape.points.iter().map(|p| place.to_screen(*p)).collect();
            let mut mask = vec![0u8; (area[2] - area[0]) as usize * (area[3] - area[1]) as usize];
            stroke_mask(&mut mask, area, &points, half);
            let opacity = if shape.tool == Tool::Marker { MARKER_OPACITY * shape.opacity / 256 } else { shape.opacity };
            blend_mask(surface, area, &mask, shape.color, opacity);
        }
        Tool::Text => draw_words(surface, shape, place),
        Tool::Badge => draw_badge(surface, shape, place),
        Tool::Emoji => {
            let side = (shape.stroke * scale).round().max(1.0) as u32;
            let (x, y) = place.to_screen(shape.points[0]);
            if let Some(picture) = emoji(&shape.text, side) {
                stamp(surface.pixels, width, clip, &picture, (x.round() as i32, y.round() as i32), shape.opacity, surface.alpha.as_deref_mut());
            }
        }
        Tool::Zoom => draw_zoom(surface, shape, place),
        _ => {
            let distance = |sx: f32, sy: f32| shape.distance(shape.unturn(place.to_picture((sx, sy))), false) * scale;
            let mask = coverage(area, distance);
            blend_mask(surface, area, &mask, shape.color, shape.opacity);
            if shape.tool == Tool::Counter {
                let (cx, cy) = place.to_screen(shape.points[0]);
                let px = shape.counter_radius() * 1.15 * scale;
                let words = number.to_string();
                let w = lettering::width(Look::default(), &words, px);
                surface.words((cx - w / 2.0, cy - lettering::line_height(px) / 2.0), px, ink_on(shape.color), shape.opacity, Look::default(), &words);
            }
        }
    }
}

/// A mark that cannot be drawn turned where it is, words and emoji: drawn
/// unturned on a clear sheet, which keeps its colors already weighed by
/// how much of each pixel is covered, and the sheet laid over `area`
/// turned about the mark's middle, each pixel a blend of the four under it.
fn draw_turned(surface: &mut Surface, shape: &Shape, place: Place, area: [i32; 4]) {
    let mut flat = shape.clone();
    flat.angle = 0.0;
    let [l, t, r, b] = flat.local_bounds();
    let ((sl, st), (sr, sb)) = (place.to_screen((l, t)), place.to_screen((r, b)));
    let (ox, oy) = ((sl - 2.0).floor(), (st - 2.0).floor());
    let (w, h) = ((sr - ox + 2.0).ceil().max(1.0) as usize, (sb - oy + 2.0).ceil().max(1.0) as usize);
    // A sheet past any sane size is a mark far larger than the picture.
    if w * h > 64_000_000 {
        return;
    }
    let (mut pixels, mut alpha) = (vec![0u32; w * h], vec![0u8; w * h]);
    {
        let mut sheet = Surface { pixels: &mut pixels, width: w, clip: [0, 0, w as i32, h as i32], alpha: Some(&mut alpha) };
        draw_shape(&mut sheet, &flat, Place { x: place.x - ox, y: place.y - oy, scale: place.scale }, 0);
    }
    let (cx, cy) = place.to_screen(shape.centre());
    let (sin, cos) = (-shape.angle).sin_cos();
    let sample = |x: i32, y: i32| -> (u32, u32) {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return (0, 0);
        }
        let at = y as usize * w + x as usize;
        (pixels[at], u32::from(alpha[at]))
    };
    for y in area[1]..area[3] {
        for x in area[0]..area[2] {
            // Where on the sheet this pixel's middle comes from.
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let (fx, fy) = (cx + dx * cos - dy * sin - ox - 0.5, cy + dx * sin + dy * cos - oy - 0.5);
            let (ax, ay) = (fx.floor() as i32, fy.floor() as i32);
            let (tx, ty) = (((fx - ax as f32) * 256.0) as u32, ((fy - ay as f32) * 256.0) as u32);
            let ((c00, a00), (c10, a10), (c01, a01), (c11, a11)) = (sample(ax, ay), sample(ax + 1, ay), sample(ax, ay + 1), sample(ax + 1, ay + 1));
            let a = ((a00 * (256 - tx) + a10 * tx) * (256 - ty) + (a01 * (256 - tx) + a11 * tx) * ty) >> 16;
            if a == 0 {
                continue;
            }
            let color = mix(mix(c00, c10, tx), mix(c01, c11, tx), ty);
            let at = y as usize * surface.width + x as usize;
            // Over what is there: it shows through by as much as is not
            // covered, and the sheet's colors are added on.
            let under = surface.pixels[at];
            let keep = 255 - a.min(255);
            let channel = |shift: u32| ((under >> shift & 255) * keep / 255 + (color >> shift & 255)).min(255) << shift;
            surface.pixels[at] = channel(16) | channel(8) | channel(0);
            if let Some(alpha) = surface.alpha.as_deref_mut() {
                alpha[at] = alpha[at].max(a.min(255) as u8);
            }
        }
    }
}

/// Words, a line at a time: gone over with a marker, or on a patch of
/// their color, and after a dot each, if they are to be.
fn draw_words(surface: &mut Surface, shape: &Shape, place: Place) {
    let px = shape.stroke * place.scale;
    let (x, y) = place.to_screen(shape.points[0]);
    let line = lettering::line_height(px);
    let look = shape.lettering();
    let mut ink = shape.color;
    if shape.style & BOXED != 0 && !shape.text.is_empty() {
        let [l, t, r, b] = shape.local_bounds();
        let ((sl, st), (sr, sb)) = (place.to_screen((l, t)), place.to_screen((r, b)));
        surface.patch([sl, st, sr, sb], line / 4.0, 0.0, shape.opacity, |_, _| shape.color);
        ink = ink_on(shape.color);
    }
    for (index, words) in shape.text.split('\n').enumerate() {
        let shown = format!("{}{words}", shape.lead());
        let top = y + index as f32 * line;
        if shape.style & HIGHLIGHT != 0 && !words.is_empty() {
            let wide = lettering::width(look, &shown, px);
            let (color, amount) = HIGHLIGHT_COLOR;
            surface.patch([x - line / 8.0, top + line / 10.0, x + wide + line / 8.0, top + line * 0.9], line / 8.0, 0.0, amount * shape.opacity / 256, |_, _| color);
        }
        surface.words((x, top), px, ink, shape.opacity, look, &shown);
    }
}

/// How many looks a badge has, and what each is called: hotshot's.
pub const BADGE_NAMES: [&str; 12] = ["Soft", "Solid", "Outline", "Pill", "Dark", "Label", "Gloss", "Blend", "Glow", "Tag", "Marker", "Sticker"];

/// A color turned so many degrees round the wheel, as light and strong.
fn turned(color: u32, degrees: f32) -> u32 {
    let [r, g, b] = [16, 8, 0].map(|shift| (color >> shift & 0xFF) as f32 / 255.0);
    let (most, least) = (r.max(g).max(b), r.min(g).min(b));
    let (light, spread) = ((most + least) / 2.0, most - least);
    if spread < 1e-4 {
        return color;
    }
    let strength = spread / (1.0 - (2.0 * light - 1.0).abs()).max(1e-4);
    let hue = if most == r { ((g - b) / spread).rem_euclid(6.0) } else if most == g { (b - r) / spread + 2.0 } else { (r - g) / spread + 4.0 };
    let hue = (hue * 60.0 + degrees).rem_euclid(360.0);
    let part = |turn: f32| {
        let at = (turn + hue / 30.0).rem_euclid(12.0);
        let value = light - strength * light.min(1.0 - light) * (at - 3.0).min(9.0 - at).clamp(-1.0, 1.0);
        (value.clamp(0.0, 1.0) * 255.0).round() as u32
    };
    part(0.0) << 16 | part(8.0) << 8 | part(4.0)
}

/// A badge: words on a patch in one of twelve looks, each made of the one
/// color it is given. hotshot's.
fn draw_badge(surface: &mut Surface, shape: &Shape, place: Place) {
    let [l, t, r, b] = shape.local_bounds();
    let ((l, t), (r, b)) = (place.to_screen((l, t)), place.to_screen((r, b)));
    let (high, size) = (b - t, shape.stroke * place.scale);
    let solid = shape.color;
    let soft = mix(solid, 0xFFFFFF, 216);
    let deep = mix(solid, 0x000000, 70);
    let amount = shape.opacity;
    let rect = [l, t, r, b];
    let shaded = |solid: u32| move |_: f32, down: f32| mix(mix(solid, 0xFFFFFF, 26), mix(solid, 0x000000, 20), (down * 256.0) as u32);
    let dot = |surface: &mut Surface| surface.disc((l + size * 0.625, t + high / 2.0), (size / 5.0).max(2.0), solid, amount);
    let ink = match shape.look {
        // Soft: the pale under the dark, with a dot and a faint edge.
        0 => {
            surface.patch(rect, high / 3.0, 0.0, amount, |_, _| mix(soft, solid, 70));
            surface.patch([l + 1.0, t + 1.0, r - 1.0, b - 1.0], high / 3.0 - 1.0, 0.0, amount, |_, _| soft);
            dot(surface);
            deep
        }
        1 => {
            surface.patch(rect, high / 3.0, 0.0, amount, shaded(solid));
            ink_on(solid)
        }
        // Outlined: nothing under the words but what was there.
        2 => {
            surface.outline(rect, high / 3.0, (size / 8.0).max(2.0), solid, amount);
            solid
        }
        // A pill: ends as round as it is tall.
        3 => {
            surface.patch(rect, high / 2.0, 0.0, amount, shaded(solid));
            ink_on(solid)
        }
        // Dark: near black, with a dot of the color.
        4 => {
            surface.patch(rect, high / 3.0, 0.0, amount, |_, _| 0x161616);
            dot(surface);
            0xEEEEEE
        }
        // Gloss: lighter at the top, with a pale line along its top edge.
        6 => {
            let (top, bottom) = (mix(solid, 0xFFFFFF, 60), mix(solid, 0x000000, 50));
            surface.patch(rect, high / 3.0, 0.0, amount, |_, _| mix(solid, 0x000000, 90));
            surface.patch([l + 1.0, t + 1.0, r - 1.0, b - 1.0], high / 3.0 - 1.0, 0.0, amount, |_, down| mix(top, bottom, (down * 256.0) as u32));
            surface.patch([l + high / 3.0, t + 1.0, r - high / 3.0, t + 2.0], 0.0, 0.0, 90 * amount / 256, |_, _| 0xFFFFFF);
            ink_on(solid)
        }
        // Blend: from the color to one a little round the wheel from it.
        7 => {
            let far = turned(solid, 50.0);
            surface.patch(rect, high / 2.0, 0.0, amount, |across, down| mix(solid, far, ((across * 0.85 + down * 0.15) * 256.0) as u32));
            ink_on(solid)
        }
        // Glow: the color, with light of the same color round it.
        8 => {
            let grow = size / 6.0;
            surface.patch([l - grow, t + size / 6.0 - grow, r + grow, b + size / 6.0 + grow], high / 2.0 + grow, size * 0.6, 150 * amount / 256, |_, _| solid);
            surface.patch(rect, high / 2.0, 0.0, amount, |_, down| mix(mix(solid, 0xFFFFFF, 36), solid, (down * 256.0) as u32));
            ink_on(solid)
        }
        // A tag: pointed at its left end, with a hole to hang it by.
        9 => {
            let (point, middle) = (high / 2.0, t + high / 2.0);
            surface.patch([l + point, t, r, b], high / 5.0, 0.0, amount, |_, _| solid);
            let reach = l + point + high / 5.0 + 1.0;
            for y in t.floor() as i32..b.ceil() as i32 {
                for x in l.floor() as i32..reach.ceil() as i32 {
                    let away = triangle((x as f32 + 0.5, y as f32 + 0.5), (l, middle), (reach, t), (reach, b));
                    surface.blend(x, y, solid, ((0.5 - away).clamp(0.0, 1.0) * amount as f32) as u32);
                }
            }
            surface.disc((l + point + size / 6.0, middle), (size / 6.0).max(2.0), ink_on(solid), amount);
            ink_on(solid)
        }
        // Marker: gone over with a wide pen, the words as dark as what a
        // marker is used on.
        10 => {
            surface.patch([l, t + high / 5.0, r, b - high / 5.0], high / 6.0, 0.0, amount, |across, _| mix(mix(solid, 0xFFFFFF, 110), mix(solid, 0xFFFFFF, 60), (across * 256.0) as u32));
            mix(solid, 0x000000, 190)
        }
        // A sticker: white all round it, and a shadow under that.
        11 => {
            let edge = (size / 6.0).max(2.0);
            surface.patch([l - edge, t, r + edge, b + edge * 2.0], high / 3.0 + edge, size * 0.5, 110 * amount / 256, |_, _| 0x000000);
            surface.patch([l - edge, t - edge, r + edge, b + edge], high / 3.0 + edge, 0.0, amount, |_, _| 0xFFFFFF);
            surface.patch(rect, high / 3.0, 0.0, amount, |_, down| mix(mix(solid, 0xFFFFFF, 30), mix(solid, 0x000000, 24), (down * 256.0) as u32));
            ink_on(solid)
        }
        // A label: white, with a bar of the color down its left side.
        _ => {
            surface.patch(rect, high / 5.0, 0.0, amount, |_, _| solid);
            surface.patch([l + size * 0.4, t, r, b], high / 5.0, 0.0, amount, |_, _| 0xFFFFFF);
            surface.patch([l + size * 0.4, t, l + size * 0.4 + high / 5.0 + 1.0, b], 0.0, 0.0, amount, |_, _| 0xFFFFFF);
            0x161616
        }
    };
    let at = (l + shape.badge_lead() * place.scale, t + (high - lettering::line_height(size)) / 2.0);
    surface.words(at, size, ink, shape.opacity, shape.lettering(), &shape.text);
}

/// A zoom: what it is of, ringed; lines from that to the larger view of
/// it; and the larger view, ringed, over them. Round, as a lens is,
/// unless it is wanted square.
fn draw_zoom(surface: &mut Surface, shape: &Shape, place: Place) {
    let screen = |[l, t, r, b]: [f32; 4]| {
        let ((l, t), (r, b)) = (place.to_screen((l, t)), place.to_screen((r, b)));
        [l, t, r, b]
    };
    let (of, shown) = (screen(shape.frame()), screen(shape.zoomed()));
    let thick = (shape.stroke * place.scale).max(1.0);
    let round = !shape.filled;
    let color = shape.color;
    // Lines from what it is of to the view: corner to corner, or the two
    // that touch both rings on either side.
    let middle = |[l, t, r, b]: [f32; 4]| ((l + r) / 2.0, (t + b) / 2.0);
    // Being dragged out, it is only what it is of, ringed.
    let ready = shape.points.len() >= 4;
    let lines: Vec<(Point, Point)> = if !ready {
        Vec::new()
    } else if round {
        let (from, to) = (middle(of), middle(shown));
        let far = dist(from, to).max(1.0);
        let (nx, ny) = (-(to.1 - from.1) / far, (to.0 - from.0) / far);
        let edge = |rect: [f32; 4], side: f32| {
            let (cx, cy) = middle(rect);
            (cx + nx * side * (rect[2] - rect[0]) / 2.0, cy + ny * side * (rect[3] - rect[1]) / 2.0)
        };
        vec![(edge(of, 1.0), edge(shown, 1.0)), (edge(of, -1.0), edge(shown, -1.0))]
    } else {
        let corners = |[l, t, r, b]: [f32; 4]| [(l, t), (r, t), (l, b), (r, b)];
        corners(of).into_iter().zip(corners(shown)).collect()
    };
    for (from, to) in lines {
        let area = [from.0.min(to.0).floor() as i32 - 2, from.1.min(to.1).floor() as i32 - 2, from.0.max(to.0).ceil() as i32 + 2, from.1.max(to.1).ceil() as i32 + 2];
        let area = [area[0].max(surface.clip[0]), area[1].max(surface.clip[1]), area[2].min(surface.clip[2]), area[3].min(surface.clip[3])];
        if area[2] > area[0] && area[3] > area[1] {
            let mask = coverage(area, |x, y| segment((x, y), from, to) - thick / 4.0 - 0.5);
            blend_mask(surface, area, &mask, color, shape.opacity);
        }
    }
    let rings: &[[f32; 4]] = if ready {
        magnify(surface, of, shown, round);
        &[of, shown]
    } else {
        &[of]
    };
    for rect in rings.iter().copied() {
        let grown = [rect[0] - thick / 2.0, rect[1] - thick / 2.0, rect[2] + thick / 2.0, rect[3] + thick / 2.0];
        if round {
            let (cx, cy) = middle(rect);
            let (hw, hh) = ((rect[2] - rect[0]) / 2.0, (rect[3] - rect[1]) / 2.0);
            let area = [(grown[0].floor() as i32 - 1).max(surface.clip[0]), (grown[1].floor() as i32 - 1).max(surface.clip[1]), (grown[2].ceil() as i32 + 1).min(surface.clip[2]), (grown[3].ceil() as i32 + 1).min(surface.clip[3])];
            if area[2] > area[0] && area[3] > area[1] {
                let mask = coverage(area, |x, y| ellipse(x - cx, y - cy, hw, hh).abs() - thick / 2.0);
                blend_mask(surface, area, &mask, color, shape.opacity);
            }
        } else {
            surface.outline(grown, thick + 3.0, thick, color, shape.opacity);
        }
    }
}

/// What is in one rectangle drawn again in another, each pixel a blend of
/// the four it falls among; with `round`, only inside an oval set in it.
fn magnify(surface: &mut Surface, from: [f32; 4], into: [f32; 4], round: bool) {
    let clip = surface.clip;
    let width = surface.width;
    // Taken first: where the two overlap, it is the picture as it was
    // that is wanted, not as it is being made.
    let source = [(from[0].floor() as i32).max(clip[0]), (from[1].floor() as i32).max(clip[1]), (from[2].ceil() as i32).min(clip[2]), (from[3].ceil() as i32).min(clip[3])];
    if source[2] <= source[0] || source[3] <= source[1] || into[2] <= into[0] || into[3] <= into[1] {
        return;
    }
    let (sw, sh) = ((source[2] - source[0]) as usize, (source[3] - source[1]) as usize);
    let taken: Vec<u32> = (0..sh).flat_map(|row| {
        let start = (source[1] as usize + row) * width + source[0] as usize;
        surface.pixels[start..start + sw].to_vec()
    }).collect();
    let at = |x: i32, y: i32| taken[(y.clamp(0, sh as i32 - 1) as usize) * sw + x.clamp(0, sw as i32 - 1) as usize];
    let (iw, ih) = (into[2] - into[0], into[3] - into[1]);
    let (fw, fh) = (from[2] - from[0], from[3] - from[1]);
    for y in (into[1].floor() as i32).max(clip[1])..(into[3].ceil() as i32).min(clip[3]) {
        for x in (into[0].floor() as i32).max(clip[0])..(into[2].ceil() as i32).min(clip[2]) {
            let (dx, dy) = (x as f32 + 0.5 - into[0], y as f32 + 0.5 - into[1]);
            let cover = if round { (0.5 - ellipse(dx - iw / 2.0, dy - ih / 2.0, iw / 2.0, ih / 2.0)).clamp(0.0, 1.0) } else { 1.0 };
            if cover <= 0.0 {
                continue;
            }
            let (sx, sy) = (from[0] + dx * fw / iw - source[0] as f32 - 0.5, from[1] + dy * fh / ih - source[1] as f32 - 0.5);
            let (ax, ay) = (sx.floor() as i32, sy.floor() as i32);
            let (fx, fy) = (((sx - ax as f32) * 256.0) as u32, ((sy - ay as f32) * 256.0) as u32);
            let upper = mix(at(ax, ay), at(ax + 1, ay), fx);
            let lower = mix(at(ax, ay + 1), at(ax + 1, ay + 1), fx);
            surface.blend(x, y, mix(upper, lower, fy), (cover * 256.0) as u32);
        }
    }
}

/// The picture at full size with the marks drawn on it, cropped to `crop`
/// (left, top, right, bottom) and its corners rounded by `radius`: what
/// Edit saves.
pub fn render(image: &RgbaImage, shapes: &[Shape], crop: Option<[f32; 4]>, radius: f32) -> RgbaImage {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mut pixels: Vec<u32> = image.pixels().map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])).collect();
    let mut alpha: Vec<u8> = image.pixels().map(|p| p[3]).collect();
    paint(shapes, &mut pixels, w, Place::WHOLE, [0, 0, w as i32, h as i32], Some(&mut alpha));
    let [l, t, r, b] = crop.map_or([0, 0, w, h], |[l, t, r, b]| {
        let edge = |v: f32, most: usize| (v.round().max(0.0) as usize).min(most);
        [edge(l, w), edge(t, h), edge(r, w), edge(b, h)]
    });
    let (cw, ch) = ((r - l).max(1), (b - t).max(1));
    let mut out = RgbaImage::new(cw as u32, ch as u32);
    let radius = radius.min(cw as f32 / 2.0).min(ch as f32 / 2.0).max(0.0);
    out.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
        let sy = (t + y).min(h - 1);
        for x in 0..cw {
            let at = sy * w + (l + x).min(w - 1);
            let rgb = pixels[at];
            let mut a = f32::from(alpha[at]);
            if radius > 0.0 {
                let inside = round_box(x as f32 + 0.5 - cw as f32 / 2.0, y as f32 + 0.5 - ch as f32 / 2.0, cw as f32 / 2.0, ch as f32 / 2.0, radius);
                a *= (0.5 - inside).clamp(0.0, 1.0);
            }
            row[x * 4..x * 4 + 4].copy_from_slice(&[(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, a.round() as u8]);
        }
    });
    out
}

/// The emoji there are, with their names, in Unicode's order.
pub fn emoji_list() -> &'static [(&'static str, &'static str)] {
    static LIST: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    LIST.get_or_init(|| {
        include_str!("../assets/emoji.txt")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_once('\t'))
            .collect()
    })
}

/// Where the system's color emoji usually are.
const EMOJI_PATHS: &[&str] = &["/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf", "/usr/share/fonts/noto/NotoColorEmoji.ttf", "/usr/share/fonts/google-noto-emoji/NotoColorEmoji.ttf", "/usr/share/fonts/noto-emoji/NotoColorEmoji.ttf"];

/// An emoji as a picture `side` pixels high, its alpha in the top byte,
/// from the system's emoji font, which keeps each as a small PNG. Kept
/// once made; `None` where there are no emoji, or not that one.
pub fn emoji(which: &str, side: u32) -> Option<Arc<(i32, i32, Vec<u32>)>> {
    static FONT: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    static MADE: OnceLock<Mutex<HashMap<(String, u32), Option<Arc<(i32, i32, Vec<u32>)>>>>> = OnceLock::new();
    let made = MADE.get_or_init(Default::default);
    if let Some(found) = made.lock().ok()?.get(&(which.to_string(), side)) {
        return found.clone();
    }
    let picture = (|| {
        // Twemoji, drawn at the size asked for; the system's own only for
        // what it lacks.
        if let Some(drawn) = twemoji(which, side) {
            return Some(Arc::new(drawn));
        }
        let bytes = FONT.get_or_init(|| EMOJI_PATHS.iter().find_map(|path| std::fs::read(path).ok())).as_deref()?;
        let face = ttf_parser::Face::parse(bytes, 0).ok()?;
        let c = which.chars().next()?;
        let image = face.glyph_raster_image(face.glyph_index(c)?, u16::MAX)?;
        if image.format != ttf_parser::RasterImageFormat::PNG {
            return None;
        }
        let png = image::load_from_memory_with_format(image.data, image::ImageFormat::Png).ok()?.to_rgba8();
        let (wide, high) = (png.width() as i32, png.height() as i32);
        let h = side as i32;
        let w = (wide * h / high.max(1)).max(1);
        // Larger than the font keeps it: each pixel a blend of the four it
        // falls among, by how opaque each is, rather than the nearest
        // repeated, which shows as squares.
        if h > high {
            let at = |x: i32, y: i32| png.get_pixel(x.clamp(0, wide - 1) as u32, y.clamp(0, high - 1) as u32).0.map(f32::from);
            let mut pixels = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                let sy = (y as f32 + 0.5) * high as f32 / h as f32 - 0.5;
                for x in 0..w {
                    let sx = (x as f32 + 0.5) * wide as f32 / w as f32 - 0.5;
                    let (ax, ay) = (sx.floor() as i32, sy.floor() as i32);
                    let (fx, fy) = (sx - ax as f32, sy - ay as f32);
                    let corners = [(at(ax, ay), (1.0 - fx) * (1.0 - fy)), (at(ax + 1, ay), fx * (1.0 - fy)), (at(ax, ay + 1), (1.0 - fx) * fy), (at(ax + 1, ay + 1), fx * fy)];
                    let (mut sum, mut alpha) = ([0.0f32; 3], 0.0f32);
                    for (p, weight) in corners {
                        let a = p[3] * weight;
                        (0..3).for_each(|channel| sum[channel] += p[channel] * a);
                        alpha += a;
                    }
                    let channel = |at: usize| if alpha > 0.0 { (sum[at] / alpha).round().min(255.0) as u32 } else { 0 };
                    pixels.push((alpha.round().min(255.0) as u32) << 24 | channel(0) << 16 | channel(1) << 8 | channel(2));
                }
            }
            return Some(Arc::new((w, h, pixels)));
        }
        // Smaller: each pixel the average of those it is made from, by how
        // opaque each is.
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let (x0, y0) = (x * wide / w, y * high / h);
                let (x1, y1) = (((x + 1) * wide / w).max(x0 + 1), ((y + 1) * high / h).max(y0 + 1));
                let (mut sum, mut count) = ([0u32; 4], 0u32);
                for sy in y0..y1.min(high) {
                    for sx in x0..x1.min(wide) {
                        let p = png.get_pixel(sx as u32, sy as u32);
                        let a = u32::from(p[3]);
                        (0..3).for_each(|channel| sum[channel] += u32::from(p[channel]) * a);
                        sum[3] += a;
                        count += 1;
                    }
                }
                let a = sum[3] / count.max(1);
                let channel = |at: usize| sum[at] / sum[3].max(1);
                pixels.push(a << 24 | channel(0) << 16 | channel(1) << 8 | channel(2));
            }
        }
        Some(Arc::new((w, h, pixels)))
    })();
    if let Ok(mut made) = made.lock() {
        // A few sizes of a few emoji: what is shown, and one saved.
        if made.len() > 512 {
            made.clear();
        }
        made.insert((which.to_string(), side), picture.clone());
    }
    picture
}

/// Twemoji, as Discord and X have them, in Mozilla's font of them, whose
/// emoji are outlines filled with flat colors, one over another: drawn
/// at any size, sharp.
const TWEMOJI: &[u8] = include_bytes!("../assets/Twemoji.Mozilla.ttf");

/// The layers of an emoji as the font has them: which outline, in which
/// color, the first lowest.
#[derive(Default)]
struct Layers {
    outline: Option<ttf_parser::GlyphId>,
    layers: Vec<(ttf_parser::GlyphId, ttf_parser::RgbaColor)>,
}

impl<'a> ttf_parser::colr::Painter<'a> for Layers {
    fn outline_glyph(&mut self, glyph: ttf_parser::GlyphId) {
        self.outline = Some(glyph);
    }

    fn paint(&mut self, paint: ttf_parser::colr::Paint<'a>) {
        // The font is of flat colors only.
        if let (Some(glyph), ttf_parser::colr::Paint::Solid(color)) = (self.outline, paint) {
            self.layers.push((glyph, color));
        }
    }

    fn push_clip(&mut self) {}
    fn push_clip_box(&mut self, _: ttf_parser::colr::ClipBox) {}
    fn pop_clip(&mut self) {}
    fn push_layer(&mut self, _: ttf_parser::colr::CompositeMode) {}
    fn pop_layer(&mut self) {}
    fn push_transform(&mut self, _: ttf_parser::Transform) {}
    fn pop_transform(&mut self) {}
}

/// An outline of the font laid onto a rasterizer: its points taken from
/// the font's units, up being up, to the square's pixels, down being down.
struct Trace<'a> {
    fill: &'a mut ab_glyph_rasterizer::Rasterizer,
    at: (f32, f32),
    start: (f32, f32),
    fit: f32,
    left: f32,
    base: f32,
}

impl Trace<'_> {
    fn point(&self, x: f32, y: f32) -> ab_glyph_rasterizer::Point {
        ab_glyph_rasterizer::point(self.left + x * self.fit, self.base - y * self.fit)
    }
}

impl ttf_parser::OutlineBuilder for Trace<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        (self.at, self.start) = ((x, y), (x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (from, to) = (self.point(self.at.0, self.at.1), self.point(x, y));
        self.fill.draw_line(from, to);
        self.at = (x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (from, via, to) = (self.point(self.at.0, self.at.1), self.point(x1, y1), self.point(x, y));
        self.fill.draw_quad(from, via, to);
        self.at = (x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (from, a, b, to) = (self.point(self.at.0, self.at.1), self.point(x1, y1), self.point(x2, y2), self.point(x, y));
        self.fill.draw_cubic(from, a, b, to);
        self.at = (x, y);
    }

    fn close(&mut self) {
        let (x, y) = self.start;
        self.line_to(x, y);
    }
}

/// An emoji `side` pixels square from Twemoji, each layer's outline filled
/// at that size and laid over the ones before; its alpha in the top byte,
/// its colors as they are. `None` for one the font has no colors for.
fn twemoji(which: &str, side: u32) -> Option<(i32, i32, Vec<u32>)> {
    let face = ttf_parser::Face::parse(TWEMOJI, 0).ok()?;
    let glyph = face.glyph_index(which.chars().next()?)?;
    let mut painted = Layers::default();
    face.paint_color_glyph(glyph, 0, ttf_parser::RgbaColor::new(0, 0, 0, 255), &mut painted)?;
    if painted.layers.is_empty() {
        return None;
    }
    // All the layers together, in the font's units: the square is fitted
    // to that, so that every emoji fills it the same.
    let mut around = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for (layer, _) in &painted.layers {
        if let Some(rect) = face.glyph_bounding_box(*layer) {
            around = [around[0].min(rect.x_min.into()), around[1].min(rect.y_min.into()), around[2].max(rect.x_max.into()), around[3].max(rect.y_max.into())];
        }
    }
    let (wide, high) = ((around[2] - around[0]).max(1) as f32, (around[3] - around[1]).max(1) as f32);
    let fit = side as f32 / wide.max(high);
    // Where the font's origin falls in the square, the emoji in the middle.
    let left = (side as f32 - wide * fit) / 2.0 - around[0] as f32 * fit;
    // The top of all its layers at the top of the emoji, down being up in
    // the font.
    let base = (side as f32 - high * fit) / 2.0 + around[3] as f32 * fit;
    let side = side as usize;
    // Colors kept weighed by how much of each pixel they cover, as layers
    // are laid one over another; weighed back out at the end.
    let mut sum = vec![[0f32; 4]; side * side];
    for (layer, color) in &painted.layers {
        let mut fill = ab_glyph_rasterizer::Rasterizer::new(side, side);
        face.outline_glyph(*layer, &mut Trace { fill: &mut fill, at: (0.0, 0.0), start: (0.0, 0.0), fit, left, base })?;
        let opacity = f32::from(color.alpha) / 255.0;
        fill.for_each_pixel(|at, cover| {
            let a = cover.min(1.0) * opacity;
            if a <= 0.0 {
                return;
            }
            let pixel = &mut sum[at];
            for (channel, value) in [color.red, color.green, color.blue].into_iter().enumerate() {
                pixel[channel] = pixel[channel] * (1.0 - a) + f32::from(value) * a;
            }
            pixel[3] = pixel[3] * (1.0 - a) + a;
        });
    }
    let side = side as i32;
    let pixels = sum
        .iter()
        .map(|[r, g, b, a]| {
            if *a <= 0.0 {
                return 0;
            }
            let channel = |v: f32| (v / a).round().clamp(0.0, 255.0) as u32;
            ((a * 255.0).round() as u32) << 24 | channel(*r) << 16 | channel(*g) << 8 | channel(*b)
        })
        .collect();
    Some((side, side, pixels))
}

/// A picture with its own alpha laid over `pixels` at `at`.
pub fn stamp(pixels: &mut [u32], width: usize, clip: [i32; 4], picture: &(i32, i32, Vec<u32>), at: (i32, i32), opacity: u32, mut alpha: Option<&mut [u8]>) {
    let (w, h, source) = picture;
    for row in 0..*h {
        let y = at.1 + row;
        if y < clip[1] || y >= clip[3] {
            continue;
        }
        for column in 0..*w {
            let x = at.0 + column;
            if x < clip[0] || x >= clip[2] {
                continue;
            }
            let p = source[(row * w + column) as usize];
            let a = (p >> 24) * opacity / 256;
            if a == 0 {
                continue;
            }
            let i = y as usize * width + x as usize;
            pixels[i] = mix(pixels[i], p & 0xFFFFFF, a + (a >> 7));
            if let Some(alpha) = alpha.as_deref_mut() {
                alpha[i] = alpha[i].max(a.min(255) as u8);
            }
        }
    }
}

/// How much of each pixel of `area` (left, top, right, bottom) a shape
/// covers, from how far each pixel's middle is from its edge, in pixels.
fn coverage(area: [i32; 4], distance: impl Fn(f32, f32) -> f32 + Sync) -> Vec<u8> {
    let w = (area[2] - area[0]) as usize;
    let mut mask = vec![0u8; w * (area[3] - area[1]) as usize];
    mask.par_chunks_mut(w.max(1)).enumerate().for_each(|(row, line)| {
        let sy = (area[1] + row as i32) as f32 + 0.5;
        for (column, cover) in line.iter_mut().enumerate() {
            let sx = (area[0] + column as i32) as f32 + 0.5;
            *cover = ((0.5 - distance(sx, sy)).clamp(0.0, 1.0) * 255.0) as u8;
        }
    });
    mask
}

/// A pen's line, `half` wide either side, as coverage of `area`: each
/// short line laid over only the pixels near it.
fn stroke_mask(mask: &mut [u8], area: [i32; 4], points: &[Point], half: f32) {
    let w = (area[2] - area[0]) as usize;
    let pairs: Vec<(Point, Point)> = match points {
        [one] => vec![(*one, *one)],
        points => points.windows(2).map(|pair| (pair[0], pair[1])).collect(),
    };
    for (a, b) in pairs {
        let reach = half + 1.0;
        let x0 = ((a.0.min(b.0) - reach).floor() as i32).max(area[0]);
        let x1 = ((a.0.max(b.0) + reach).ceil() as i32).min(area[2]);
        let y0 = ((a.1.min(b.1) - reach).floor() as i32).max(area[1]);
        let y1 = ((a.1.max(b.1) + reach).ceil() as i32).min(area[3]);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = segment((x as f32 + 0.5, y as f32 + 0.5), a, b) - half;
                let cover = ((0.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
                let at = (y - area[1]) as usize * w + (x - area[0]) as usize;
                mask[at] = mask[at].max(cover);
            }
        }
    }
}

fn blend_mask(surface: &mut Surface, area: [i32; 4], mask: &[u8], color: u32, opacity: u32) {
    let w = (area[2] - area[0]) as usize;
    let width = surface.width;
    for (row, line) in mask.chunks_exact(w.max(1)).enumerate() {
        let start = (area[1] as usize + row) * width + area[0] as usize;
        for (pixel, cover) in surface.pixels[start..start + w].iter_mut().zip(line) {
            if *cover > 0 {
                *pixel = mix(*pixel, color, (u32::from(*cover) + 1) * opacity / 256);
            }
        }
        if let Some(alpha) = surface.alpha.as_deref_mut() {
            for (a, cover) in alpha[start..start + w].iter_mut().zip(line) {
                *a = (*a).max(((u32::from(*cover) * opacity) >> 8).min(255) as u8);
            }
        }
    }
}

/// Squares of one color each, `cell` of the picture's pixels across,
/// laid on the picture's own grid so that they fall the same on screen
/// and in the file.
fn pixelate(pixels: &mut [u32], width: usize, place: Place, area: [i32; 4], cell: f32) {
    let step = (cell * place.scale).max(2.0);
    let (ox, oy) = (place.x.rem_euclid(step), place.y.rem_euclid(step));
    let first = |from: i32, origin: f32| origin + ((from as f32 - origin) / step).floor() * step;
    let mut y = first(area[1], oy);
    while y < area[3] as f32 {
        let (y0, y1) = ((y.round() as i32).max(area[1]), ((y + step).round() as i32).min(area[3]));
        let mut x = first(area[0], ox);
        while x < area[2] as f32 {
            let (x0, x1) = ((x.round() as i32).max(area[0]), ((x + step).round() as i32).min(area[2]));
            if x1 > x0 && y1 > y0 {
                let (mut sum, mut n) = ([0u64; 3], 0u64);
                for row in y0..y1 {
                    for &p in &pixels[row as usize * width + x0 as usize..row as usize * width + x1 as usize] {
                        sum[0] += u64::from(p >> 16 & 255);
                        sum[1] += u64::from(p >> 8 & 255);
                        sum[2] += u64::from(p & 255);
                        n += 1;
                    }
                }
                let color = ((sum[0] / n) << 16 | (sum[1] / n) << 8 | sum[2] / n) as u32;
                for row in y0..y1 {
                    pixels[row as usize * width + x0 as usize..row as usize * width + x1 as usize].fill(color);
                }
            }
            x += step;
        }
        y += step;
    }
}

/// Darkens everything in `clip` outside the rectangles of `lit`, each with
/// corners as round as it says.
fn spotlight(pixels: &mut [u32], width: usize, place: Place, clip: [i32; 4], lit: &[([f32; 4], f32)]) {
    let screen: Vec<([f32; 4], f32)> = lit
        .iter()
        .map(|([l, t, r, b], radius)| {
            let (sl, st) = place.to_screen((*l, *t));
            let (sr, sb) = place.to_screen((*r, *b));
            ([sl, st, sr, sb], radius * place.scale)
        })
        .collect();
    let rows = pixels.par_chunks_mut(width).enumerate().skip(clip[1] as usize).take((clip[3] - clip[1]) as usize);
    rows.for_each(|(y, row)| {
        let sy = y as f32 + 0.5;
        for x in clip[0]..clip[2] {
            let sx = x as f32 + 0.5;
            let inside = screen
                .iter()
                .map(|([l, t, r, b], radius)| {
                    let (hw, hh) = ((r - l) / 2.0, (b - t) / 2.0);
                    round_box(sx - (l + r) / 2.0, sy - (t + b) / 2.0, hw, hh, radius.min(hw).min(hh).max(0.0))
                })
                .fold(f32::MAX, f32::min);
            let dark = (0.5 + inside).clamp(0.0, 1.0);
            if dark > 0.0 {
                let pixel = &mut row[x as usize];
                *pixel = mix(*pixel, 0, ((256 - SPOTLIGHT_KEEPS) as f32 * dark) as u32);
            }
        }
    });
}

fn dist(a: Point, b: Point) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// `p` turned by `by` radians, clockwise on screen, about `about`.
pub fn turn_point(p: Point, about: Point, by: f32) -> Point {
    let (sin, cos) = by.sin_cos();
    let (dx, dy) = (p.0 - about.0, p.1 - about.1);
    (about.0 + dx * cos - dy * sin, about.1 + dx * sin + dy * cos)
}

/// How far `p` is from the line from `a` to `b`.
pub fn segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, (a.0 + dx * t, a.1 + dy * t))
}

/// How far a point is from a rectangle `hw` by `hh` either side of the
/// middle with corners of `radius`, negative inside.
fn round_box(x: f32, y: f32, hw: f32, hh: f32, radius: f32) -> f32 {
    let (qx, qy) = (x.abs() - hw + radius, y.abs() - hh + radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

/// Near enough how far a point is from an ellipse, negative inside.
fn ellipse(x: f32, y: f32, a: f32, b: f32) -> f32 {
    let (a, b) = (a.max(0.5), b.max(0.5));
    let k0 = (x / a).hypot(y / b);
    let k1 = (x / (a * a)).hypot(y / (b * b));
    if k1 == 0.0 { -a.min(b) } else { k0 * (k0 - 1.0) / k1 }
}

/// How far a point is from a triangle, negative inside.
fn triangle(p: Point, a: Point, b: Point, c: Point) -> f32 {
    let edge = |from: Point, to: Point| segment(p, from, to);
    let nearest = edge(a, b).min(edge(b, c)).min(edge(c, a));
    let side = |from: Point, to: Point| (to.0 - from.0) * (p.1 - from.1) - (to.1 - from.1) * (p.0 - from.0);
    let (s1, s2, s3) = (side(a, b), side(b, c), side(c, a));
    let inside = (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0);
    if inside { -nearest } else { nearest }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(at: Point, to: Point) -> Shape {
        let mut shape = Shape::new(Tool::Rect, 0xFF0000, 4.0, at);
        shape.points[1] = to;
        shape
    }

    fn painted(shapes: &[Shape], w: usize, h: usize, place: Place) -> Vec<u32> {
        let mut pixels = vec![0u32; w * h];
        paint(shapes, &mut pixels, w, place, [0, 0, w as i32, h as i32], None);
        pixels
    }

    #[test]
    fn a_rectangle_is_drawn_on_its_edge_and_not_inside() {
        let w = 60;
        let pixels = painted(&[rect((10.0, 10.0), (50.0, 30.0))], w, 40, Place::WHOLE);
        assert_eq!(pixels[20 * w + 10], 0xFF0000);
        assert_eq!(pixels[20 * w + 30], 0);
        assert_eq!(pixels[2 * w + 2], 0);
    }

    #[test]
    fn filled_it_is_drawn_inside_too() {
        let w = 60;
        let mut shape = rect((10.0, 10.0), (50.0, 30.0));
        shape.filled = true;
        assert_eq!(painted(&[shape], w, 40, Place::WHOLE)[20 * w + 30], 0xFF0000);
    }

    #[test]
    fn a_dashed_outline_has_gaps_along_its_edge() {
        let w = 220;
        let mut shape = rect((10.0, 10.0), (210.0, 60.0));
        let solid = painted(&[shape.clone()], w, 70, Place::WHOLE);
        assert!((20..200).all(|x| solid[10 * w + x] == 0xFF0000));
        shape.set_dash(Dash::Dashed);
        let dashed = painted(&[shape], w, 70, Place::WHOLE);
        assert!((20..200).filter(|x| dashed[10 * w + x] == 0).count() > 30);
    }

    #[test]
    fn drawn_smaller_the_marks_land_where_the_picture_does() {
        let w = 40;
        // The picture at half size, 10 pixels in.
        let place = Place { x: 10.0, y: 10.0, scale: 0.5 };
        let pixels = painted(&[rect((0.0, 0.0), (40.0, 40.0))], w, 40, place);
        assert_eq!(pixels[20 * w + 10], 0xFF0000);
        assert_eq!(pixels[20 * w + 30], 0xFF0000);
        assert_eq!(pixels[20 * w + 20], 0);
    }

    #[test]
    fn a_dashed_line_has_gaps_and_a_solid_one_none() {
        let w = 200;
        let mut line = Shape::new(Tool::Line, 0xFF0000, 4.0, (0.0, 10.0));
        line.points[1] = (200.0, 10.0);
        let solid = painted(&[line.clone()], w, 20, Place::WHOLE);
        assert!((0..200).filter(|x| solid[10 * w + x] == 0).count() < 4);
        for dash in [Dash::Dashed, Dash::Dotted] {
            line.set_dash(dash);
            let broken = painted(&[line.clone()], w, 20, Place::WHOLE);
            assert!((0..200).filter(|x| broken[10 * w + x] == 0).count() > 40, "{dash:?}");
        }
    }

    #[test]
    fn an_arrow_has_a_head_at_the_end_it_points_to() {
        let w = 120;
        let mut arrow = Shape::new(Tool::Arrow, 0xFF0000, 4.0, (10.0, 30.0));
        arrow.points[1] = (110.0, 30.0);
        let pixels = painted(&[arrow.clone()], w, 60, Place::WHOLE);
        // Wide at the head, thin at the tail.
        assert_eq!(pixels[(30 - 6) * w + 92], 0xFF0000);
        assert_eq!(pixels[(30 - 6) * w + 28], 0);
        arrow.heads = [true, true];
        assert_eq!(painted(&[arrow], w, 60, Place::WHOLE)[(30 - 6) * w + 28], 0xFF0000);
    }

    #[test]
    fn a_counter_dragged_out_has_a_tail_to_where_it_points() {
        let w = 120;
        let mut counter = Shape::new(Tool::Counter, 0xFF0000, 4.0, (20.0, 30.0));
        assert_eq!(painted(&[counter.clone()], w, 60, Place::WHOLE)[30 * w + 60], 0);
        counter.points.push((100.0, 30.0));
        assert_eq!(painted(&[counter], w, 60, Place::WHOLE)[30 * w + 60], 0xFF0000);
    }

    #[test]
    fn a_zoom_shows_what_it_is_of_larger() {
        let w = 200;
        let mut pixels = vec![0u32; w * 100];
        // A white square at 10..20, zoomed into 100..140.
        for y in 10..20 {
            pixels[y * w + 10..y * w + 20].fill(0xFFFFFF);
        }
        let mut zoom = Shape::new(Tool::Zoom, 0xFF0000, 1.0, (5.0, 5.0));
        zoom.points[1] = (25.0, 25.0);
        zoom.filled = true;
        zoom.points.extend([(100.0, 20.0), (180.0, 100.0)]);
        paint(&[zoom], &mut pixels, w, Place::WHOLE, [0, 0, w as i32, 100], None);
        assert_eq!(pixels[60 * w + 140], 0xFFFFFF);
        assert_eq!(pixels[30 * w + 110], 0);
    }

    #[test]
    fn a_zoom_being_dragged_out_is_drawn_and_sized_without_its_view() {
        // As it is between the press and the release: two points.
        let mut zoom = Shape::new(Tool::Zoom, 0xFF0000, 2.0, (10.0, 10.0));
        zoom.points[1] = (50.0, 40.0);
        assert_eq!(zoom.zoomed(), zoom.frame());
        let _ = zoom.bounds();
        let _ = zoom.zoom_handles();
        zoom.zoom_to(true, (80.0, 80.0));
        assert!(zoom.hit((30.0, 10.0), 2.0));
        let pixels = painted(&[zoom.clone()], 100, 60, Place::WHOLE);
        assert!(pixels.iter().any(|p| *p != 0));
        zoom.place_zoom();
        assert_eq!(zoom.points.len(), 4);
    }

    #[test]
    fn a_turned_box_is_drawn_and_found_where_it_is_turned_to() {
        // A wide, low bar about (60, 50), turned a quarter: tall and narrow.
        let mut bar = rect((20.0, 45.0), (100.0, 55.0));
        bar.filled = true;
        bar.angle = std::f32::consts::FRAC_PI_2;
        assert!(bar.hit((60.0, 20.0), 1.0));
        assert!(!bar.hit((25.0, 50.0), 1.0));
        let w = 120;
        let pixels = painted(&[bar.clone()], w, 100, Place::WHOLE);
        assert_eq!(pixels[20 * w + 60], 0xFF0000);
        assert_eq!(pixels[50 * w + 25], 0);
        let [l, t, r, b] = bar.bounds();
        assert!((r - l) < 20.0 && (b - t) > 70.0);
    }

    #[test]
    fn turned_words_are_drawn_turned() {
        let mut words = Shape::new(Tool::Text, 0xFFFFFF, 20.0, (40.0, 90.0));
        words.text = "Hello world".to_string();
        let extent = |pixels: &[u32], w: usize| {
            let lit: Vec<(usize, usize)> = pixels.iter().enumerate().filter(|(_, p)| **p != 0).map(|(at, _)| (at % w, at / w)).collect();
            let span = |pick: fn(&(usize, usize)) -> usize| lit.iter().map(pick).max().unwrap_or(0) - lit.iter().map(pick).min().unwrap_or(0);
            (span(|p| p.0), span(|p| p.1))
        };
        let (wide, high) = extent(&painted(&[words.clone()], 240, 240, Place::WHOLE), 240);
        assert!(wide > high * 2);
        words.angle = std::f32::consts::FRAC_PI_2;
        let (wide, high) = extent(&painted(&[words], 240, 240, Place::WHOLE), 240);
        assert!(high > wide * 2, "{wide} × {high}");
    }

    #[test]
    fn a_line_turns_by_its_points() {
        let mut line = Shape::new(Tool::Line, 0, 2.0, (0.0, 0.0));
        line.points[1] = (100.0, 0.0);
        line.turn(std::f32::consts::FRAC_PI_2, (50.0, 0.0));
        assert_eq!(line.angle, 0.0);
        assert!((line.points[0].0 - 50.0).abs() < 0.01 && (line.points[0].1 + 50.0).abs() < 0.01);
    }

    #[test]
    fn a_badge_draws_its_patch_and_words() {
        let w = 200;
        let mut badge = Shape::new(Tool::Badge, 0x0A84FF, 20.0, (10.0, 10.0));
        badge.text = "Label".to_string();
        for look in 0..BADGE_NAMES.len() {
            badge.look = look;
            let pixels = painted(&[badge.clone()], w, 60, Place::WHOLE);
            assert!(pixels.iter().filter(|p| **p != 0).count() > 100, "{}", BADGE_NAMES[look]);
        }
    }

    #[test]
    fn a_bent_line_passes_through_its_bend() {
        let mut line = Shape::new(Tool::Line, 0, 4.0, (0.0, 0.0));
        line.points[1] = (100.0, 0.0);
        line.bend_to((50.0, 30.0), 4.0);
        assert!(line.hit((50.0, 30.0), 1.0));
        assert!(!line.hit((50.0, 0.0), 1.0));
        // Near the straight line, it straightens.
        line.bend_to((50.0, 2.0), 4.0);
        assert_eq!(line.bend, None);
    }

    #[test]
    fn scaled_from_a_corner_that_corner_stays() {
        let mut shape = rect((10.0, 10.0), (50.0, 30.0));
        shape.scale((10.0, 10.0), (2.0, 0.5));
        assert_eq!(shape.points, vec![(10.0, 10.0), (90.0, 20.0)]);
        let mut words = Shape::new(Tool::Text, 0, 20.0, (0.0, 0.0));
        words.scale((0.0, 0.0), (2.0, 1.0));
        assert_eq!(words.stroke, 40.0);
    }

    #[test]
    fn words_on_several_lines_are_as_tall_as_their_lines() {
        let mut words = Shape::new(Tool::Text, 0, 20.0, (0.0, 0.0));
        words.text = "one\ntwo".to_string();
        let [_, t, _, b] = words.bounds();
        assert_eq!(b - t, lettering::line_height(20.0) * 2.0);
    }

    #[test]
    fn rendered_it_is_cropped_and_its_corners_turn_clear() {
        let image = RgbaImage::from_pixel(100, 80, image::Rgba([10, 20, 30, 255]));
        let out = render(&image, &[], Some([10.0, 10.0, 90.0, 70.0]), 20.0);
        assert_eq!(out.dimensions(), (80, 60));
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        assert_eq!(out.get_pixel(40, 30).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(40, 0)[3], 255);
    }

    #[test]
    fn a_click_finds_the_line_and_not_the_space_beside_it() {
        let mut line = Shape::new(Tool::Line, 0, 4.0, (0.0, 0.0));
        line.points[1] = (100.0, 0.0);
        assert!(line.hit((50.0, 3.0), 2.0));
        assert!(!line.hit((50.0, 20.0), 2.0));
    }

    #[test]
    fn blur_makes_squares_of_one_color() {
        let w = 32usize;
        let mut pixels: Vec<u32> = (0..w * w).map(|i| (i % 256) as u32).collect();
        pixelate(&mut pixels, w, Place::WHOLE, [0, 0, 32, 32], 8.0);
        assert_eq!(pixels[0], pixels[7 * w + 7]);
        assert_ne!(pixels[0], pixels[8]);
    }

    #[test]
    fn spotlight_darkens_only_outside() {
        let w = 40usize;
        let mut pixels = vec![0xFFFFFFu32; w * w];
        spotlight(&mut pixels, w, Place::WHOLE, [0, 0, 40, 40], &[([10.0, 10.0, 30.0, 30.0], 0.0)]);
        assert_eq!(pixels[20 * w + 20], 0xFFFFFF);
        assert!(pixels[2 * w + 2] < 0xFFFFFF);
    }

    #[test]
    fn twemoji_are_drawn_in_their_colors_and_sharp_when_large() {
        // A red heart: red in the middle, clear in the corners.
        let (w, h, pixels) = twemoji("\u{2764}\u{fe0f}", 64).expect("drawn");
        assert_eq!((w, h), (64, 64));
        let middle = pixels[32 * 64 + 32];
        assert_eq!(middle >> 24, 255);
        assert!((middle >> 16 & 255) > 180 && (middle >> 8 & 255) < 90);
        assert_eq!(pixels[0] >> 24, 0);
        // Large, its edge is a pixel or two of part cover, not a blur.
        let (_, _, big) = twemoji("\u{1f600}", 512).expect("drawn");
        let partly = big.iter().filter(|p| (1..255).contains(&(**p >> 24))).count();
        assert!(partly < 512 * 8, "{partly}");
        // Every emoji listed has its picture.
        assert!(emoji_list().iter().all(|(which, _)| twemoji(which, 16).is_some()));
    }

    #[test]
    fn the_emoji_are_listed_with_their_names() {
        let list = emoji_list();
        assert!(list.len() > 1000);
        assert_eq!(list[0], ("😀", "grinning face"));
    }
}
