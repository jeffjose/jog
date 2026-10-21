//! What Edit draws over a picture: shapes, lines, words, emoji, numbered
//! marks, blurred patches and spotlights. They are kept in the picture's
//! own pixels, so the same marks are drawn small over the picture on
//! screen and at full size into the file that is saved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use image::RgbaImage;
use rayon::prelude::*;

use crate::ui::{mix, Canvas, Text};

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
    Emoji,
    Blur,
    Spotlight,
    Crop,
}

impl Tool {
    /// The tools in the order of the bar, a `None` where a gap parts them.
    pub const BAR: [Option<Tool>; 16] = [
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
        Some(Tool::Emoji),
        None,
        Some(Tool::Blur),
        Some(Tool::Spotlight),
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
            Tool::Emoji => "e",
            Tool::Blur => "b",
            Tool::Spotlight => "s",
            Tool::Crop => "x",
        }
    }

    pub fn of_key(key: &str) -> Option<Tool> {
        Tool::BAR.into_iter().flatten().find(|tool| tool.key() == key)
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
            Tool::Emoji => "Emoji",
            Tool::Blur => "Blur",
            Tool::Spotlight => "Spotlight",
            Tool::Crop => "Crop",
        }
    }

    /// Whether it is drawn by dragging out from one point to another.
    pub fn spans(self) -> bool {
        matches!(self, Tool::Rect | Tool::Oval | Tool::Arrow | Tool::Line | Tool::Blur | Tool::Spotlight)
    }

    /// Whether its size is that of its letters, kept in proportion.
    pub fn lettered(self) -> bool {
        matches!(self, Tool::Text | Tool::Emoji)
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

/// The colors to draw in: hotshot's.
pub const COLORS: [u32; 8] = [0xFF3B30, 0xFF9500, 0xFFCC00, 0x34C759, 0x0A84FF, 0xAF52DE, 0xFFFFFF, 0x1C1C1E];
/// How opaque a mark can be, out of 256.
pub const OPACITIES: [u32; 4] = [64, 128, 192, 256];

/// How much darker what is outside a spotlight is: it keeps this much of
/// its brightness, out of 256.
const SPOTLIGHT_KEEPS: u32 = 96;
/// How see-through the marker is, out of 256.
const MARKER_OPACITY: u32 = 110;
/// How many straight lines a bent one is drawn as.
const BENDS: usize = 24;

/// A mark on the picture, in the picture's pixels.
#[derive(Clone, PartialEq, Debug)]
pub struct Shape {
    pub tool: Tool,
    pub color: u32,
    /// Out of 256.
    pub opacity: u32,
    /// How wide its line is; for text and emoji, how tall its letters are.
    pub stroke: f32,
    /// Where the drag began and where it ended; a pen's every point; the
    /// top-left of text and emoji; the middle of a counter.
    pub points: Vec<Point>,
    /// The words, or the emoji.
    pub text: String,
    pub filled: bool,
    /// How round a rectangle's or a spotlight's corners are.
    pub radius: f32,
    /// Whether a line has a head at its first end and at its last.
    pub heads: [bool; 2],
    pub dash: Dash,
    /// Where a bent line passes half way along.
    pub bend: Option<Point>,
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

/// The heights of text the four sizes are.
pub fn text_sizes(native: (u32, u32)) -> [f32; 4] {
    [8.0, 12.0, 18.0, 28.0].map(|times| times * unit(native))
}

/// Whichever of white and near-black reads on `color`.
pub fn ink_on(color: u32) -> u32 {
    let (r, g, b) = ((color >> 16) & 255, (color >> 8) & 255, color & 255);
    if r * 299 + g * 587 + b * 114 > 160_000 { 0x1C1C1E } else { 0xFFFFFF }
}

impl Shape {
    pub fn new(tool: Tool, color: u32, stroke: f32, at: Point) -> Shape {
        let points = if tool.spans() { vec![at, at] } else { vec![at] };
        let heads = [false, tool == Tool::Arrow];
        Shape { tool, color, opacity: 256, stroke, points, text: String::new(), filled: false, radius: stroke * 1.2, heads, dash: Dash::Solid, bend: None }
    }

    /// The rectangle a two-point shape spans: left, top, right, bottom.
    pub fn frame(&self) -> [f32; 4] {
        let (a, b) = (self.points[0], *self.points.last().unwrap_or(&self.points[0]));
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
    }

    pub fn counter_radius(&self) -> f32 {
        self.stroke * 2.4 + 6.0
    }

    /// What it covers, in the picture's pixels: left, top, right, bottom.
    pub fn bounds(&self, text: &mut Text) -> [f32; 4] {
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
                let px = self.stroke.round().max(1.0) as u32;
                let width = text.width(&self.text, px).max(px as i32 / 2) as f32;
                [x, y, x + width, y + Text::line_height(px) as f32]
            }
            Tool::Emoji => {
                let (x, y) = self.points[0];
                [x, y, x + self.stroke, y + self.stroke]
            }
            Tool::Counter => {
                let ((x, y), r) = (self.points[0], self.counter_radius());
                [x - r, y - r, x + r, y + r]
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
    pub fn hit(&self, p: Point, reach: f32, text: &mut Text) -> bool {
        match self.tool {
            Tool::Text | Tool::Emoji | Tool::Counter | Tool::Blur | Tool::Spotlight => {
                let [l, t, r, b] = self.bounds(text);
                p.0 >= l - reach && p.0 <= r + reach && p.1 >= t - reach && p.1 <= b + reach
            }
            Tool::Pen | Tool::Marker => {
                let half = self.line_width() / 2.0 + reach;
                match self.points.as_slice() {
                    [one] => dist(p, *one) <= half,
                    points => points.windows(2).any(|pair| segment(p, pair[0], pair[1]) <= half),
                }
            }
            // Filled, or outlined: on the line, or anywhere inside.
            Tool::Rect | Tool::Oval if self.filled => self.distance(p, false) <= reach,
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
            Tool::Text | Tool::Emoji => self.text.trim().is_empty(),
            Tool::Counter | Tool::Pen | Tool::Marker => false,
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
        match self.tool {
            Tool::Rect => {
                let inside = round_box(p.0 - cx, p.1 - cy, hw, hh, self.radius.min(hw).min(hh).max(0.0));
                if self.filled { inside - half } else { inside.abs() - half }
            }
            Tool::Oval => {
                let inside = ellipse(p.0 - cx, p.1 - cy, hw, hh);
                if self.filled { inside - half } else { inside.abs() - half }
            }
            Tool::Line | Tool::Arrow => self.line_distance(p, plain),
            Tool::Counter => dist(p, self.points[0]) - self.counter_radius(),
            _ => f32::MAX,
        }
    }

    fn line_distance(&self, p: Point, plain: bool) -> f32 {
        let path = self.path();
        let lengths: Vec<f32> = path.windows(2).map(|pair| dist(pair[0], pair[1])).collect();
        let whole: f32 = lengths.iter().sum();
        let half = self.stroke / 2.0;
        if whole < 0.5 {
            return dist(p, path[0]) - half;
        }
        // The heads: as long as they would be, but no more than a share of
        // the line, which would be all head.
        let most = if self.heads == [true, true] { 0.45 } else { 0.7 };
        let (head, _) = self.head();
        let head = head.min(whole * most);
        let wide = head * 0.46;
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
        let first = if self.heads[0] { head * 0.8 } else { 0.0 };
        let last = whole - if self.heads[1] { head * 0.8 } else { 0.0 };
        let dash = if plain { Dash::Solid } else { self.dash };
        let (period, on) = match dash {
            Dash::Solid => (0.0, 0.0),
            Dash::Dashed => (self.stroke * 4.5 + 7.0, self.stroke * 2.5 + 4.0),
            Dash::Dotted => (self.stroke * 2.2, 0.0),
        };
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
                    // How far along the dashes it is, and how far that is
                    // from the nearest dash or dot.
                    let s = begins + s0 - first + t * run;
                    let into = s.rem_euclid(period);
                    let off = match dash {
                        Dash::Dotted => into.min(period - into),
                        _ => if into <= on { 0.0 } else { (into - on).min(period - into) },
                    };
                    across.hypot(off) - half
                }
            };
            nearest = nearest.min(far);
        }
        nearest
    }
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
#[allow(clippy::too_many_arguments)]
pub fn paint(shapes: &[Shape], pixels: &mut [u32], width: usize, place: Place, clip: [i32; 4], text: &mut Text, mut alpha: Option<&mut [u8]>) {
    let height = pixels.len() / width.max(1);
    let clip = [clip[0].max(0), clip[1].max(0), (clip[0] + clip[2]).min(width as i32), (clip[1] + clip[3]).min(height as i32)];
    if clip[2] <= clip[0] || clip[3] <= clip[1] {
        return;
    }
    // The spotlights darken the picture before anything is drawn on it:
    // the arrow pointing at what one lights is not to be darkened.
    let lit: Vec<([f32; 4], f32)> = shapes.iter().filter(|shape| shape.tool == Tool::Spotlight).map(|shape| (shape.frame(), shape.radius)).collect();
    if !lit.is_empty() {
        spotlight(pixels, width, place, clip, &lit);
    }
    let mut number = 0;
    for shape in shapes {
        let [l, t, r, b] = shape.bounds(text);
        let (sl, st) = place.to_screen((l, t));
        let (sr, sb) = place.to_screen((r, b));
        let area = [(sl.floor() as i32).max(clip[0]), (st.floor() as i32).max(clip[1]), (sr.ceil() as i32 + 1).min(clip[2]), (sb.ceil() as i32 + 1).min(clip[3])];
        if shape.tool == Tool::Counter {
            number += 1;
        }
        if area[2] <= area[0] || area[3] <= area[1] {
            continue;
        }
        match shape.tool {
            Tool::Select | Tool::Crop | Tool::Spotlight => {}
            Tool::Blur => pixelate(pixels, width, place, area, (shape.stroke * 4.0).max(8.0)),
            Tool::Pen | Tool::Marker => {
                let half = shape.line_width() / 2.0 * place.scale;
                let points: Vec<Point> = shape.points.iter().map(|p| place.to_screen(*p)).collect();
                let mut mask = vec![0u8; (area[2] - area[0]) as usize * (area[3] - area[1]) as usize];
                stroke_mask(&mut mask, area, &points, half);
                let opacity = if shape.tool == Tool::Marker { MARKER_OPACITY * shape.opacity / 256 } else { shape.opacity };
                blend_mask(pixels, width, area, &mask, shape.color, opacity, alpha.as_deref_mut());
            }
            Tool::Text => {
                let px = (shape.stroke * place.scale).round().max(1.0) as u32;
                let (x, y) = place.to_screen(shape.points[0]);
                draw_words(pixels, width, clip, text, x.round() as i32, y.round() as i32, px, shape.color, &shape.text, alpha.as_deref_mut());
            }
            Tool::Emoji => {
                let side = (shape.stroke * place.scale).round().max(1.0) as u32;
                let (x, y) = place.to_screen(shape.points[0]);
                if let Some(picture) = emoji(&shape.text, side) {
                    stamp(pixels, width, clip, &picture, (x.round() as i32, y.round() as i32), shape.opacity, alpha.as_deref_mut());
                }
            }
            _ => {
                let scale = place.scale;
                let distance = |sx: f32, sy: f32| shape.distance(place.to_picture((sx, sy)), false) * scale;
                let mask = coverage(area, distance);
                blend_mask(pixels, width, area, &mask, shape.color, shape.opacity, alpha.as_deref_mut());
                if shape.tool == Tool::Counter {
                    let (cx, cy) = place.to_screen(shape.points[0]);
                    let px = (shape.counter_radius() * 1.15 * scale).round().max(1.0) as u32;
                    let words = number.to_string();
                    let w = text.width(&words, px);
                    let (x, y) = (cx.round() as i32 - w / 2, cy.round() as i32 - Text::line_height(px) / 2);
                    draw_words(pixels, width, clip, text, x, y, px, ink_on(shape.color), &words, None);
                }
            }
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
    let mut text = Text::load();
    paint(shapes, &mut pixels, w, Place::WHOLE, [0, 0, w as i32, h as i32], &mut text, Some(&mut alpha));
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
        // Smaller: each pixel the average of those it is made from, by how
        // opaque each is. Larger: the nearest.
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

/// Words drawn into a bare picture through a canvas over it, and made
/// opaque where they are drawn.
#[allow(clippy::too_many_arguments)]
fn draw_words(pixels: &mut [u32], width: usize, clip: [i32; 4], text: &mut Text, x: i32, y: i32, px: u32, color: u32, words: &str, alpha: Option<&mut [u8]>) {
    let height = pixels.len() / width.max(1);
    // Only what is inside the clip is handed over: a canvas of those rows,
    // the letters moved up by as many.
    let (top, bottom) = (clip[1].max(0) as usize, (clip[3] as usize).min(height));
    let rows = &mut pixels[top * width..bottom * width];
    let mut canvas = Canvas::new(rows, width as u32, (bottom - top) as u32);
    text.draw(&mut canvas, x, y - top as i32, px, color, words);
    if let Some(alpha) = alpha {
        let line = Text::line_height(px);
        let w = text.width(words, px);
        for row in y.max(clip[1])..(y + line).min(clip[3]) {
            for column in x.max(clip[0])..(x + w).min(clip[2]) {
                alpha[row as usize * width + column as usize] = 255;
            }
        }
    }
}

/// How much of each pixel of `area` a shape covers, from how far each
/// pixel's middle is from its edge, in pixels.
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

fn blend_mask(pixels: &mut [u32], width: usize, area: [i32; 4], mask: &[u8], color: u32, opacity: u32, alpha: Option<&mut [u8]>) {
    let w = (area[2] - area[0]) as usize;
    for (row, line) in mask.chunks_exact(w.max(1)).enumerate() {
        let start = (area[1] as usize + row) * width + area[0] as usize;
        for (pixel, cover) in pixels[start..start + w].iter_mut().zip(line) {
            if *cover > 0 {
                *pixel = mix(*pixel, color, (u32::from(*cover) + 1) * opacity / 256);
            }
        }
    }
    if let Some(alpha) = alpha {
        for (row, line) in mask.chunks_exact(w.max(1)).enumerate() {
            let start = (area[1] as usize + row) * width + area[0] as usize;
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
        let mut text = Text::load();
        paint(shapes, &mut pixels, w, place, [0, 0, w as i32, h as i32], &mut text, None);
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
            line.dash = dash;
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
    fn a_bent_line_passes_through_its_bend() {
        let mut line = Shape::new(Tool::Line, 0, 4.0, (0.0, 0.0));
        line.points[1] = (100.0, 0.0);
        line.bend_to((50.0, 30.0), 4.0);
        let mut text = Text::load();
        assert!(line.hit((50.0, 30.0), 1.0, &mut text));
        assert!(!line.hit((50.0, 0.0), 1.0, &mut text));
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
        let mut text = Text::load();
        let mut line = Shape::new(Tool::Line, 0, 4.0, (0.0, 0.0));
        line.points[1] = (100.0, 0.0);
        assert!(line.hit((50.0, 3.0), 2.0, &mut text));
        assert!(!line.hit((50.0, 20.0), 2.0, &mut text));
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
    fn the_emoji_are_listed_with_their_names() {
        let list = emoji_list();
        assert!(list.len() > 1000);
        assert_eq!(list[0], ("😀", "grinning face"));
    }
}
