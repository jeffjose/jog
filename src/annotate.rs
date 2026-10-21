//! What Edit draws over a picture: shapes, lines, words, numbered marks,
//! blurred patches and spotlights. They are kept in the picture's own
//! pixels, so the same marks are drawn small over the picture on screen
//! and at full size into the file that is saved.

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
    Blur,
    Spotlight,
    Crop,
}

impl Tool {
    /// The tools in the order of the bar, a `None` where a gap parts them.
    pub const BAR: [Option<Tool>; 15] = [
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
            Tool::Blur => "b",
            Tool::Spotlight => "s",
            Tool::Crop => "x",
        }
    }

    pub fn of_key(key: &str) -> Option<Tool> {
        Tool::BAR.into_iter().flatten().find(|tool| tool.key().eq_ignore_ascii_case(key))
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
            Tool::Blur => "Blur",
            Tool::Spotlight => "Spotlight",
            Tool::Crop => "Crop",
        }
    }

    /// Whether it is drawn by dragging out from one point to another.
    pub fn spans(self) -> bool {
        matches!(self, Tool::Rect | Tool::Oval | Tool::Arrow | Tool::Line | Tool::Blur | Tool::Spotlight)
    }
}

/// The colors to draw in: hotshot's.
pub const COLORS: [u32; 8] = [0xFF3B30, 0xFF9500, 0xFFCC00, 0x34C759, 0x0A84FF, 0xAF52DE, 0xFFFFFF, 0x1C1C1E];

/// How much darker what is outside a spotlight is: it keeps this much of
/// its brightness, out of 256.
const SPOTLIGHT_KEEPS: u32 = 96;
/// How see-through the marker is, out of 256.
const MARKER_OPACITY: u32 = 110;

/// A mark on the picture, in the picture's pixels.
#[derive(Clone, PartialEq, Debug)]
pub struct Shape {
    pub tool: Tool,
    pub color: u32,
    /// How wide its line is; for text, how tall its letters are.
    pub stroke: f32,
    /// Where the drag began and where it ended; a pen's every point; the
    /// top-left of text; the middle of a counter.
    pub points: Vec<Point>,
    pub text: String,
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

/// The widths of line the four sizes are, for a picture this large: the
/// same share of it whatever its resolution.
pub fn widths(native: (u32, u32)) -> [f32; 4] {
    let unit = (native.0.max(native.1) as f32 / 600.0).max(1.0);
    [1.5, 3.0, 5.0, 8.0].map(|times| times * unit)
}

/// The heights of text the four sizes are.
pub fn text_sizes(native: (u32, u32)) -> [f32; 4] {
    let unit = (native.0.max(native.1) as f32 / 600.0).max(1.0);
    [8.0, 12.0, 18.0, 28.0].map(|times| times * unit)
}

/// Whichever of white and near-black reads on `color`.
pub fn ink_on(color: u32) -> u32 {
    let (r, g, b) = ((color >> 16) & 255, (color >> 8) & 255, color & 255);
    if r * 299 + g * 587 + b * 114 > 160_000 { 0x1C1C1E } else { 0xFFFFFF }
}

impl Shape {
    pub fn new(tool: Tool, color: u32, stroke: f32, at: Point) -> Shape {
        let points = if tool.spans() { vec![at, at] } else { vec![at] };
        Shape { tool, color, stroke, points, text: String::new() }
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
        match self.tool {
            Tool::Text => {
                let (x, y) = self.points[0];
                let px = self.stroke.round().max(1.0) as u32;
                let width = text.width(&self.text, px).max(px as i32 / 2) as f32;
                [x, y, x + width, y + Text::line_height(px) as f32]
            }
            Tool::Counter => {
                let ((x, y), r) = (self.points[0], self.counter_radius());
                [x - r, y - r, x + r, y + r]
            }
            Tool::Arrow => {
                let [l, t, r, b] = self.frame();
                let head = self.head().1 + reach;
                [l - head, t - head, r + head, b + head]
            }
            Tool::Pen | Tool::Marker => {
                let reach = self.line_width() * 0.5 + 1.0;
                let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for (x, y) in &self.points {
                    (l, t, r, b) = (l.min(*x), t.min(*y), r.max(*x), b.max(*y));
                }
                [l - reach, t - reach, r + reach, b + reach]
            }
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
        (self.stroke * 3.2 + 8.0, self.stroke * 1.9 + 5.0)
    }

    /// Whether a point of the picture is on it, `reach` pixels allowed.
    pub fn hit(&self, p: Point, reach: f32, text: &mut Text) -> bool {
        match self.tool {
            Tool::Text | Tool::Counter | Tool::Blur | Tool::Spotlight => {
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
            _ => self.distance(p) <= reach,
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for point in &mut self.points {
            point.0 += dx;
            point.1 += dy;
        }
    }

    /// Whether it came to nothing: a click with a tool that wants a drag.
    pub fn is_slight(&self) -> bool {
        match self.tool {
            Tool::Text => self.text.trim().is_empty(),
            Tool::Counter => false,
            Tool::Pen | Tool::Marker => false,
            _ => {
                let [l, t, r, b] = self.frame();
                (r - l).max(b - t) < 3.0
            }
        }
    }

    /// How far a point is from its outline, negative inside it.
    fn distance(&self, p: Point) -> f32 {
        let half = self.stroke / 2.0;
        let [l, t, r, b] = self.frame();
        let (cx, cy, hw, hh) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
        match self.tool {
            Tool::Rect => {
                let radius = (self.stroke * 1.2).min(hw).min(hh);
                round_box(p.0 - cx, p.1 - cy, hw, hh, radius).abs() - half
            }
            Tool::Oval => ellipse(p.0 - cx, p.1 - cy, hw, hh).abs() - half,
            Tool::Line => segment(p, self.points[0], self.points[1]) - half,
            Tool::Arrow => {
                let (a, b) = (self.points[0], self.points[1]);
                let length = dist(a, b);
                if length < 0.5 {
                    return segment(p, a, b) - half;
                }
                let (head, wide) = self.head();
                let head = head.min(length);
                let (dx, dy) = ((b.0 - a.0) / length, (b.1 - a.1) / length);
                let base = (b.0 - dx * head, b.1 - dy * head);
                let tip = triangle(p, b, (base.0 - dy * wide, base.1 + dx * wide), (base.0 + dy * wide, base.1 - dx * wide)) - 0.5;
                // The shaft stops inside the head, where its round end
                // would otherwise blunt the point.
                let to = (b.0 - dx * head * 0.7, b.1 - dy * head * 0.7);
                tip.min(segment(p, a, to) - half)
            }
            Tool::Counter => dist(p, self.points[0]) - self.counter_radius(),
            _ => f32::MAX,
        }
    }
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
    let lit: Vec<[f32; 4]> = shapes.iter().filter(|shape| shape.tool == Tool::Spotlight).map(Shape::frame).collect();
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
                let opacity = if shape.tool == Tool::Marker { MARKER_OPACITY } else { 256 };
                blend_mask(pixels, width, area, &mask, shape.color, opacity, alpha.as_deref_mut());
            }
            Tool::Text => {
                let px = (shape.stroke * place.scale).round().max(1.0) as u32;
                let (x, y) = place.to_screen(shape.points[0]);
                draw_words(pixels, width, clip, text, x.round() as i32, y.round() as i32, px, shape.color, &shape.text, alpha.as_deref_mut());
            }
            _ => {
                let scale = place.scale;
                let distance = |sx: f32, sy: f32| shape.distance(place.to_picture((sx, sy))) * scale;
                let mask = coverage(area, distance);
                blend_mask(pixels, width, area, &mask, shape.color, 256, alpha.as_deref_mut());
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

/// Darkens everything in `clip` outside the rectangles of `lit`.
fn spotlight(pixels: &mut [u32], width: usize, place: Place, clip: [i32; 4], lit: &[[f32; 4]]) {
    let screen: Vec<[f32; 4]> = lit
        .iter()
        .map(|[l, t, r, b]| {
            let (sl, st) = place.to_screen((*l, *t));
            let (sr, sb) = place.to_screen((*r, *b));
            [sl, st, sr, sb]
        })
        .collect();
    let rows = pixels.par_chunks_mut(width).enumerate().skip(clip[1] as usize).take((clip[3] - clip[1]) as usize);
    rows.for_each(|(y, row)| {
        let sy = y as f32 + 0.5;
        for x in clip[0]..clip[2] {
            let sx = x as f32 + 0.5;
            let inside = screen
                .iter()
                .map(|[l, t, r, b]| {
                    let (hw, hh) = ((r - l) / 2.0, (b - t) / 2.0);
                    round_box(sx - (l + r) / 2.0, sy - (t + b) / 2.0, hw, hh, (hw.min(hh) * 0.15).min(24.0 * place.scale.max(0.2)))
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
fn segment(p: Point, a: Point, b: Point) -> f32 {
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

    #[test]
    fn a_rectangle_is_drawn_on_its_edge_and_not_inside() {
        let (w, h) = (60usize, 40usize);
        let mut pixels = vec![0u32; w * h];
        let mut text = Text::load();
        paint(&[rect((10.0, 10.0), (50.0, 30.0))], &mut pixels, w, Place::WHOLE, [0, 0, w as i32, h as i32], &mut text, None);
        assert_eq!(pixels[20 * w + 10], 0xFF0000);
        assert_eq!(pixels[20 * w + 30], 0);
        assert_eq!(pixels[2 * w + 2], 0);
    }

    #[test]
    fn drawn_smaller_the_marks_land_where_the_picture_does() {
        let (w, h) = (40usize, 40usize);
        let mut pixels = vec![0u32; w * h];
        let mut text = Text::load();
        // The picture at half size, 10 pixels in.
        let place = Place { x: 10.0, y: 10.0, scale: 0.5 };
        paint(&[rect((0.0, 0.0), (40.0, 40.0))], &mut pixels, w, place, [0, 0, w as i32, h as i32], &mut text, None);
        assert_eq!(pixels[20 * w + 10], 0xFF0000);
        assert_eq!(pixels[20 * w + 30], 0xFF0000);
        assert_eq!(pixels[20 * w + 20], 0);
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
        spotlight(&mut pixels, w, Place::WHOLE, [0, 0, 40, 40], &[[10.0, 10.0, 30.0, 30.0]]);
        assert_eq!(pixels[20 * w + 20], 0xFFFFFF);
        assert!(pixels[2 * w + 2] < 0xFFFFFF);
    }
}
