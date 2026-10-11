use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;

use fast_image_resize as fir;
use rayon::prelude::*;

use crate::frame::Frame;

/// Drawing primitives over a window buffer. Everything is clipped.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    pub width: i32,
    pub height: i32,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32) -> Self {
        Canvas { pixels, width: width as i32, height: height as i32 }
    }

    /// Rows of the part of a rectangle that is on the canvas.
    fn rows(&mut self, x: i32, y: i32, w: i32, h: i32) -> impl Iterator<Item = &mut [u32]> {
        let (x0, x1) = (x.clamp(0, self.width) as usize, (x + w).clamp(0, self.width) as usize);
        let (y0, y1) = (y.clamp(0, self.height) as usize, (y + h).clamp(0, self.height) as usize);
        let rows = self.pixels.chunks_exact_mut(self.width.max(1) as usize);
        rows.skip(y0).take(y1.saturating_sub(y0)).map(move |row| &mut row[x0..x1.max(x0)])
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        self.rows(x, y, w, h).for_each(|row| row.fill(color));
    }

    /// What is in a rectangle now, to be laid back over it with `dissolve`.
    pub fn copy(&mut self, x: i32, y: i32, w: i32, h: i32) -> Vec<u32> {
        self.rows(x, y, w, h).flat_map(|row| row.iter().copied()).collect()
    }

    /// Lays what `copy` took from this same rectangle back over it,
    /// `amount`/256 opaque: one picture giving way to another.
    pub fn dissolve(&mut self, x: i32, y: i32, w: i32, h: i32, was: &[u32], amount: u32) {
        for (pixel, was) in self.rows(x, y, w, h).flatten().zip(was) {
            *pixel = mix(*pixel, *was, amount.min(256));
        }
    }

    /// Draws `frame` scaled to a rectangle by taking the nearest pixel for
    /// each, on every core. Rougher than `image` and several times as
    /// quick, which is the right trade for a picture on the move.
    pub fn image_fast(&mut self, frame: &Frame, x: i32, y: i32, w: i32, h: i32) {
        let (sw, sh) = (frame.width as usize, frame.height as usize);
        let (x0, x1) = (x.max(0), (x + w).min(self.width));
        if w <= 0 || h <= 0 || x0 >= x1 || sw == 0 || frame.pixels.len() < sw * sh {
            return;
        }
        let columns: Vec<usize> = (x0..x1).map(|px| (i64::from(px - x) * sw as i64 / i64::from(w)) as usize).collect();
        let width = self.width as usize;
        self.pixels.par_chunks_exact_mut(width).enumerate().for_each(|(py, row)| {
            let dy = py as i32 - y;
            if dy < 0 || dy >= h {
                return;
            }
            let sy = (i64::from(dy) * sh as i64 / i64::from(h)) as usize;
            let source = &frame.pixels[sy * sw..(sy + 1) * sw];
            for (pixel, &column) in row[x0 as usize..x1 as usize].iter_mut().zip(&columns) {
                *pixel = source[column];
            }
        });
    }

    /// Everything on the canvas.
    pub fn all(&mut self) -> &mut [u32] {
        self.pixels
    }

    /// Draws `frame` as `image` does, with rounded corners, whatever lies
    /// behind it showing through outside the curve.
    pub fn image_rounded(&mut self, frame: &Frame, x: i32, y: i32, w: i32, h: i32, radius: i32) {
        let radius = radius.min(w / 2).min(h / 2).max(0);
        // Only the four corner squares need keeping, to be put back.
        let corners = [(x, y), (x + w - radius, y), (x, y + h - radius), (x + w - radius, y + h - radius)];
        let behind: Vec<Vec<u32>> = corners.iter().map(|&(cx, cy)| self.copy(cx, cy, radius, radius)).collect();
        self.image(frame, x, y, w, h);
        for (&(cx, cy), was) in corners.iter().zip(&behind) {
            // A corner partly off the window is left square.
            if was.len() != (radius * radius) as usize {
                continue;
            }
            for (at, was) in was.iter().enumerate() {
                let (px, py) = (cx + at as i32 % radius, cy + at as i32 / radius);
                let outside = 1.0 - rounded_cover(px, py, x, y, w, h, radius, 0.0);
                self.cover(px, py, *was, (outside * 256.0) as u32);
            }
        }
    }

    /// Darkens a rectangle more and more towards its bottom, from all of
    /// the brightness kept at the top to `keep`/256 of it. The change is
    /// spread by a fine pattern, so that it shows no steps.
    pub fn fade_down(&mut self, x: i32, y: i32, w: i32, h: i32, keep: u32) {
        // An ordered dither: which of two neighbouring levels a pixel
        // takes depends on where it is in a 4 x 4 tile.
        const TILE: [[f64; 4]; 4] =
            [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];
        let top = y.clamp(0, self.height);
        for (row, pixels) in self.rows(x, y, w, h).enumerate() {
            let down = (f64::from(top + row as i32 - y) / f64::from(h.max(1))).clamp(0.0, 1.0);
            // Gently at first and at the last, as a shadow falls off.
            let eased = down * down * (3.0 - 2.0 * down);
            let level = 256.0 - (256.0 - f64::from(keep)) * eased;
            let tile = &TILE[(top as usize + row) % 4];
            for (column, pixel) in pixels.iter_mut().enumerate() {
                let amount = (level + tile[column % 4] / 16.0).floor().clamp(0.0, 256.0) as u32;
                *pixel = mix(0, *pixel, amount);
            }
        }
    }

    /// Darkens a rectangle, keeping `keep`/256 of the brightness.
    pub fn shade(&mut self, x: i32, y: i32, w: i32, h: i32, keep: u32) {
        for pixel in self.rows(x, y, w, h).flatten() {
            *pixel = mix(0, *pixel, keep);
        }
    }

    /// Blends in a bar with fully rounded ends, `amount`/256 opaque. Only the
    /// leftmost `upto` pixels of its width are drawn, which is how a
    /// progress bar fills the same shape as its track.
    #[allow(clippy::too_many_arguments)]
    pub fn pill(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32, amount: u32, upto: i32) {
        let radius = f64::from(h) / 2.0;
        for row in 0..h.max(0) {
            let py = y + row;
            if py < 0 || py >= self.height {
                continue;
            }
            // How far the rounded end cuts into this row.
            let from_middle = f64::from(row) + 0.5 - radius;
            let inset = (radius - (radius * radius - from_middle * from_middle).max(0.0).sqrt()).round() as i32;
            let left = (x + inset).max(0);
            let right = (x + w - inset).min(x + upto).min(self.width);
            for px in left..right {
                let pixel = &mut self.pixels[(py * self.width + px) as usize];
                *pixel = mix(*pixel, color, amount);
            }
        }
    }

    /// Fills a triangle pointing right, the "play" symbol, in a square of
    /// side `size` with its top-left at `(x, y)`.
    pub fn play_symbol(&mut self, x: i32, y: i32, size: i32, color: u32) {
        for row in 0..size {
            // Widest at the middle row, a point at the right.
            let from_middle = (2 * row + 1 - size).abs();
            self.fill(x, y + row, size - from_middle, 1, color);
        }
    }

    /// Fills two upright bars, the "pause" symbol, in the same square.
    pub fn pause_symbol(&mut self, x: i32, y: i32, size: i32, color: u32) {
        let bar = size / 3;
        self.fill(x, y, bar, size, color);
        self.fill(x + size - bar, y, bar, size, color);
    }

    /// Paints `color` over the corners of a rectangle, outside a curve of
    /// `radius`, so that what was drawn in it has rounded corners. `color`
    /// is whatever lies behind it.
    pub fn round_off(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: u32) {
        let radius = radius.min(w / 2).min(h / 2);
        for (corner_x, corner_y) in [(x, y), (x + w - radius, y), (x, y + h - radius), (x + w - radius, y + h - radius)] {
            for py in corner_y..corner_y + radius {
                for px in corner_x..corner_x + radius {
                    let inside = rounded_cover(px, py, x, y, w, h, radius, 0.0);
                    self.cover(px, py, color, ((1.0 - inside) * 256.0) as u32);
                }
            }
        }
    }

    /// An outline `thickness` wide with rounded corners, `radius` being
    /// that of its outer edge.
    #[allow(clippy::too_many_arguments)]
    pub fn round_outline(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, thickness: i32, color: u32) {
        let band = radius.max(thickness);
        for py in y..y + h {
            // Away from the top and bottom only the two sides have any.
            let full = py < y + band || py >= y + h - band;
            let spans = if full { [(x, x + w), (0, 0)] } else { [(x, x + thickness), (x + w - thickness, x + w)] };
            for (from, to) in spans {
                for px in from..to {
                    let ring = rounded_cover(px, py, x, y, w, h, radius, 0.0) - rounded_cover(px, py, x, y, w, h, radius, f64::from(thickness));
                    self.cover(px, py, color, (ring * 256.0) as u32);
                }
            }
        }
    }

    /// A filled rectangle with rounded corners, `amount`/256 opaque.
    #[allow(clippy::too_many_arguments)]
    pub fn round_fill(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: u32, amount: u32) {
        let radius = radius.clamp(0, w.min(h) / 2);
        for py in y..y + h {
            // Only the ends of the rows at the top and bottom curve; the
            // rest of every row is plain.
            let curved = py < y + radius || py >= y + h - radius;
            let inset = if curved { radius } else { 0 };
            for px in (x..x + inset).chain(x + w - inset..x + w) {
                let cover = rounded_cover(px, py, x, y, w, h, radius, 0.0);
                self.cover(px, py, color, (cover * f64::from(amount)) as u32);
            }
            for row in self.rows(x + inset, py, w - 2 * inset, 1) {
                row.iter_mut().for_each(|pixel| *pixel = mix(*pixel, color, amount.min(256)));
            }
        }
    }

    fn cover(&mut self, x: i32, y: i32, color: u32, amount: u32) {
        if amount > 0 && x >= 0 && y >= 0 && x < self.width && y < self.height {
            let pixel = &mut self.pixels[(y * self.width + x) as usize];
            *pixel = mix(*pixel, color, amount.min(256));
        }
    }

    /// Draws `frame` scaled to exactly fill a rectangle: filtered when it
    /// is shrunk, which thumbnails usually are, and by repeating pixels
    /// when it is enlarged.
    pub fn image(&mut self, frame: &Frame, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let shrunk;
        let (source, sw): (&[u32], usize) = match shrink(frame, w as u32, h as u32) {
            Some(pixels) => {
                shrunk = pixels;
                (&shrunk, w as usize)
            }
            None => (&frame.pixels, frame.width as usize),
        };
        let sh = source.len() / sw.max(1);
        let columns: Vec<usize> = (0..w).map(|dx| (i64::from(dx) * sw as i64 / i64::from(w)) as usize).collect();
        let (x0, x1) = (x.max(0), (x + w).min(self.width));
        if x0 >= x1 {
            return;
        }
        for dy in 0..h {
            let py = y + dy;
            if py < 0 || py >= self.height {
                continue;
            }
            let sy = (i64::from(dy) * sh as i64 / i64::from(h)) as usize;
            let row = &source[sy * sw..(sy + 1) * sw];
            let out = &mut self.pixels[(py * self.width + x0) as usize..(py * self.width + x1) as usize];
            for (pixel, &column) in out.iter_mut().zip(&columns[(x0 - x) as usize..]) {
                *pixel = row[column];
            }
        }
    }

    fn blend(&mut self, x: i32, y: i32, color: u32, coverage: u8) {
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            let pixel = &mut self.pixels[(y * self.width + x) as usize];
            let amount = u32::from(coverage) + u32::from(coverage >> 7);
            *pixel = mix(*pixel, color, amount);
        }
    }
}

/// A filtered copy of `frame` at `w` x `h`, or `None` when it would not be
/// smaller, or the resizer refuses the sizes.
fn shrink(frame: &Frame, w: u32, h: u32) -> Option<Vec<u32>> {
    if w >= frame.width && h >= frame.height {
        return None;
    }
    let source = fir::images::ImageRef::new(frame.width, frame.height, bytemuck::cast_slice(&frame.pixels), fir::PixelType::U8x4).ok()?;
    let mut pixels = vec![0u32; w as usize * h as usize];
    let mut target = fir::images::Image::from_slice_u8(w, h, bytemuck::cast_slice_mut(&mut pixels), fir::PixelType::U8x4).ok()?;
    let options = fir::ResizeOptions::new()
        .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom))
        // The fourth byte is padding, not alpha.
        .use_alpha(false);
    fir::Resizer::new().resize(&source, &mut target, &options).ok()?;
    Some(pixels)
}

/// Blends `amount`/256 of `b` into `a`, two channels at a time.
/// How much of the pixel at `(px, py)`, from 0 to 1, lies within a
/// rectangle with corners of `radius`, shrunk by `inset` on every side.
/// Fractions along the edge are what make the curve smooth.
#[allow(clippy::too_many_arguments)]
fn rounded_cover(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, radius: i32, inset: f64) -> f64 {
    let (half_w, half_h) = (f64::from(w) / 2.0 - inset, f64::from(h) / 2.0 - inset);
    let radius = (f64::from(radius) - inset).clamp(0.0, half_w.min(half_h).max(0.0));
    // Distance from the middle, less the straight part of each side.
    let across = (f64::from(px) + 0.5 - f64::from(x) - f64::from(w) / 2.0).abs() - (half_w - radius);
    let down = (f64::from(py) + 0.5 - f64::from(y) - f64::from(h) / 2.0).abs() - (half_h - radius);
    let outside = across.max(0.0).hypot(down.max(0.0)) + across.max(down).min(0.0) - radius;
    (0.5 - outside).clamp(0.0, 1.0)
}

fn mix(a: u32, b: u32, amount: u32) -> u32 {
    let lerp = |mask: u32| (((a & mask) * (256 - amount) + (b & mask) * amount) >> 8) & mask;
    lerp(0xFF00FF) | lerp(0x00FF00)
}

/// How many pixels each core is handed at a time when a whole picture is
/// worked over.
const SHARE: usize = 1 << 16;

/// Lays one whole picture back over another of the same size, `amount`/256
/// opaque, on every core: at 4K that is eight million pixels a frame.
pub fn dissolve_all(pixels: &mut [u32], was: &[u32], amount: u32) {
    let amount = amount.min(256);
    pixels.par_chunks_mut(SHARE).zip(was.par_chunks(SHARE)).for_each(|(pixels, was)| {
        for (pixel, was) in pixels.iter_mut().zip(was) {
            *pixel = mix(*pixel, *was, amount);
        }
    });
}

/// Copies a whole picture into a buffer kept for the purpose, on every core.
pub fn copy_all(into: &mut Vec<u32>, from: &[u32]) {
    into.resize(from.len(), 0);
    into.par_chunks_mut(SHARE).zip(from.par_chunks(SHARE)).for_each(|(into, from)| into.copy_from_slice(from));
}

/// How many rows are looked at together when telling what has changed.
const BAND: usize = 32;
/// More rectangles than this are sent as the one that holds them all.
const MOST_PARTS: usize = 48;

/// Where one picture differs from another of the same size, as a few
/// rectangles of left, top, width and height, bringing the other up to
/// date as it goes. Handing the display only these is what keeps a small
/// change cheap on a large window: the whole of a 4K frame is 33 MB.
pub fn changed(now: &[u32], was: &mut [u32], width: usize) -> Vec<[u32; 4]> {
    if width == 0 || now.len() != was.len() {
        return Vec::new();
    }
    // For each band of rows, the columns between which something changed.
    let bands: Vec<Option<(usize, usize)>> = now
        .par_chunks(width * BAND)
        .zip(was.par_chunks_mut(width * BAND))
        .map(|(now, was)| {
            if now == was {
                return None;
            }
            let mut span: Option<(usize, usize)> = None;
            for (now, was) in now.chunks(width).zip(was.chunks(width)) {
                let Some(first) = now.iter().zip(was).position(|(a, b)| a != b) else { continue };
                let last = now.iter().zip(was).rposition(|(a, b)| a != b).unwrap_or(first);
                span = Some(span.map_or((first, last), |(left, right)| (left.min(first), right.max(last))));
            }
            was.copy_from_slice(now);
            span
        })
        .collect();
    let height = now.len() / width;
    let mut parts: Vec<[u32; 4]> = Vec::new();
    let mut open: Option<(usize, usize, usize)> = None;
    let mut close = |open: &mut Option<(usize, usize, usize)>, end: usize| {
        if let Some((top, left, right)) = open.take() {
            let bottom = (end * BAND).min(height);
            parts.push([left as u32, (top * BAND) as u32, (right - left + 1) as u32, (bottom - top * BAND) as u32]);
        }
    };
    for (band, span) in bands.iter().enumerate() {
        match (span, &mut open) {
            // Bands one under another are one rectangle, as wide as the widest.
            (Some((first, last)), Some((_, left, right))) => (*left, *right) = ((*left).min(*first), (*right).max(*last)),
            (Some((first, last)), None) => open = Some((band, *first, *last)),
            (None, _) => close(&mut open, band),
        }
    }
    close(&mut open, bands.len());
    if parts.len() > MOST_PARTS {
        let left = parts.iter().map(|part| part[0]).min().unwrap_or(0);
        let right = parts.iter().map(|part| part[0] + part[2]).max().unwrap_or(0);
        let (top, bottom) = (parts[0][1], parts[parts.len() - 1][1] + parts[parts.len() - 1][3]);
        parts = vec![[left, top, right - left, bottom - top]];
    }
    parts
}

const FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
];

/// Which font the icons are, where the others are counted from 0.
const ICONS: usize = usize::MAX;

/// Text rendering with the font jog carries. Loading it takes a while, so
/// this is only created once something needs to draw text.
pub struct Text {
    font: Option<fontdue::Font>,
    /// The icons, which are letters of a font of their own.
    icons: Option<fontdue::Font>,
    /// Fonts for the letters the main one lacks, Korean say, each found
    /// through fontconfig the first time such a letter comes up. A font
    /// for a large script takes a while to load, so that happens on
    /// another thread; until it is in, the letter is drawn as a box.
    others: Vec<fontdue::Font>,
    loading: Option<mpsc::Receiver<Option<fontdue::Font>>>,
    /// Letters a font has been looked for, so each is only asked about once.
    sought: HashSet<char>,
    glyphs: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl Text {
    pub fn load() -> Self {
        // The interface font is carried along, so that jog looks the same
        // everywhere; `JOG_FONT` names another, and the system's are what
        // is left if neither can be used.
        let load = |bytes: Vec<u8>| fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok();
        let chosen = std::env::var_os("JOG_FONT").and_then(|path| load(std::fs::read(path).ok()?));
        let font = chosen
            .or_else(|| load(include_bytes!("../assets/Geist-Regular.ttf").to_vec()))
            .or_else(|| font_files().find_map(|path| load(std::fs::read(path).ok()?)));
        if font.is_none() {
            eprintln!("jog: no usable font found, text will not be drawn (set JOG_FONT to a .ttf file)");
        }
        let icons = load(include_bytes!("../assets/lucide.ttf").to_vec());
        Text { font, icons, others: Vec::new(), loading: None, sought: HashSet::new(), glyphs: HashMap::new() }
    }

    pub fn line_height(px: u32) -> i32 {
        (px * 4 / 3) as i32
    }

    /// Whether a font is still on its way, after which text drawn in the
    /// meantime will want drawing again.
    pub fn waiting(&self) -> bool {
        self.loading.is_some()
    }

    /// Which font has letter `c`: 0 for the main one, otherwise one of the
    /// others, counted from 1. Looks for a font that has it if none does.
    fn font_with(&mut self, c: char) -> usize {
        // The icons have a font, and a part of Unicode, to themselves.
        if ('\u{e000}'..='\u{f8ff}').contains(&c) {
            return ICONS;
        }
        let has = |font: &fontdue::Font| font.lookup_glyph_index(c) != 0;
        if self.font.as_ref().is_none_or(has) || c.is_control() {
            return 0;
        }
        if let Some(Ok(found)) = self.loading.as_ref().map(mpsc::Receiver::try_recv) {
            self.loading = None;
            self.others.extend(found);
            // What was drawn as boxes can now be drawn properly.
            self.glyphs.clear();
        }
        if let Some(at) = self.others.iter().position(has) {
            return at + 1;
        }
        if self.loading.is_none() && self.sought.insert(c) {
            let (sender, receiver) = mpsc::channel();
            self.loading = Some(receiver);
            std::thread::spawn(move || drop(sender.send(font_for(c))));
        }
        0
    }

    fn font(&self, which: usize) -> Option<&fontdue::Font> {
        match which {
            0 => self.font.as_ref(),
            ICONS => self.icons.as_ref(),
            other => self.others.get(other - 1),
        }
    }

    /// The size a letter is drawn at: icons a little larger than the text
    /// they sit in, which they would otherwise look small beside.
    fn size(which: usize, px: u32) -> f32 {
        if which == ICONS { px as f32 * 1.15 } else { px as f32 }
    }

    pub fn width(&mut self, text: &str, px: u32) -> i32 {
        let mut width = 0.0;
        for c in text.chars() {
            let which = self.font_with(c);
            width += self.font(which).map_or(0.0, |font| font.metrics(c, Self::size(which, px)).advance_width);
        }
        width.ceil() as i32
    }

    /// Draws one line of text whose line box has its top-left at `(x, y)`.
    pub fn draw(&mut self, canvas: &mut Canvas, x: i32, y: i32, px: u32, color: u32, text: &str) {
        let Some(font) = &self.font else { return };
        let (ascent, descent) = font
            .horizontal_line_metrics(px as f32)
            .map_or((px as f32 * 0.8, px as f32 * -0.2), |m| (m.ascent, m.descent));
        let baseline = y + ((Self::line_height(px) as f32 + ascent + descent) / 2.0).round() as i32;

        let mut pen = x as f32;
        for c in text.chars() {
            if !self.glyphs.contains_key(&(c, px)) {
                let which = self.font_with(c);
                let Some(font) = self.font(which) else { continue };
                let glyph = font.rasterize(c, Self::size(which, px));
                self.glyphs.insert((c, px), glyph);
            }
            let (metrics, coverage) = &self.glyphs[&(c, px)];
            let left = pen.round() as i32 + metrics.xmin;
            // An icon sits in the middle of the line, whatever its font
            // takes for a baseline; letters stand on the text's.
            let icon = ('\u{e000}'..='\u{f8ff}').contains(&c);
            let top = match icon {
                true => y + (Self::line_height(px) - metrics.height as i32) / 2,
                false => baseline - metrics.ymin - metrics.height as i32,
            };
            for (row, line) in coverage.chunks_exact(metrics.width.max(1)).enumerate() {
                for (column, &amount) in line.iter().enumerate() {
                    if amount > 0 {
                        canvas.blend(left + column as i32, top + row as i32, color, amount);
                    }
                }
            }
            pen += metrics.advance_width;
        }
    }
}

/// A font that has letter `c`, as fontconfig picks one. The file may hold
/// several fonts, in which case fontconfig says which.
fn font_for(c: char) -> Option<fontdue::Font> {
    let pattern = format!("sans:charset={:x}", c as u32);
    let output = Command::new("fc-match").args(["-f", "%{file}\n%{index}", &pattern]).output().ok()?;
    let answer = String::from_utf8(output.stdout).ok()?;
    let (file, index) = answer.split_once('\n')?;
    let settings = fontdue::FontSettings { collection_index: index.trim().parse().unwrap_or(0), ..Default::default() };
    let font = fontdue::Font::from_bytes(std::fs::read(file).ok()?, settings).ok()?;
    (font.lookup_glyph_index(c) != 0).then_some(font)
}

fn font_files() -> impl Iterator<Item = PathBuf> {
    let chosen = std::env::var_os("JOG_FONT").map(PathBuf::from);
    let known = FONT_PATHS.iter().map(PathBuf::from);
    // Asking fontconfig means starting a process, so it is the last resort.
    let fontconfig = std::iter::once_with(|| {
        let output = Command::new("fc-match").args(["-f", "%{file}", "sans"]).output().ok()?;
        Some(PathBuf::from(String::from_utf8(output.stdout).ok()?))
    });
    chosen.into_iter().chain(known).chain(fontconfig.flatten())
}

#[cfg(test)]
mod change_tests {
    use super::changed;

    #[test]
    fn only_what_differs_is_reported_and_then_nothing() {
        let (width, height) = (200, 100);
        let mut was = vec![0u32; width * height];
        let mut now = was.clone();
        assert!(changed(&now, &mut was, width).is_empty());
        // A dot near the top and a bar lower down, in different bands.
        now[3 * width + 10] = 1;
        now[70 * width + 50..70 * width + 90].fill(2);
        assert_eq!(changed(&now, &mut was, width), [[10, 0, 1, 32], [50, 64, 40, 32]]);
        assert_eq!(was, now);
        assert!(changed(&now, &mut was, width).is_empty());
        // Rows of a last band that is short of full are counted as they are.
        now[99 * width + 199] = 3;
        assert_eq!(changed(&now, &mut was, width), [[199, 96, 1, 4]]);
    }
}
