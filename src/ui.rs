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
    /// When set, rounded rectangles are drawn only in these rows: the
    /// curved top and bottom of a panel whose middle is laid in whole.
    pub band: Option<std::ops::Range<i32>>,
    /// Which rows have been drawn in, a flag a row, when asked to keep
    /// count: all that has to be looked at to tell what was drawn.
    touched: Option<Vec<bool>>,
    /// How opaque each pixel is, when kept, a byte a pixel: drawn over a
    /// video, what is drawn is handed over with its opacity, kept here as
    /// it is drawn rather than found by drawing it all a second time.
    alpha: Option<&'a mut [u8]>,
    /// The pixels were handed out whole, and what was done to them is not
    /// known: the opacity kept says nothing of them.
    raw: bool,
}

/// `a` taken `amount`/256 of the way to `to`, as a blend takes a pixel.
fn toward(a: u8, to: u8, amount: u32) -> u8 {
    ((u32::from(a) * (256 - amount) + u32::from(to) * amount) >> 8) as u8
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32) -> Self {
        Canvas { pixels, width: width as i32, height: height as i32, band: None, touched: None, alpha: None, raw: false }
    }

    /// One that keeps how opaque each pixel is in `alpha`, as it draws.
    pub fn keeping_alpha(mut self, alpha: &'a mut [u8]) -> Self {
        self.alpha = (alpha.len() == self.pixels.len()).then_some(alpha);
        self
    }

    /// Whether the pixels were handed out whole while the opacity was
    /// kept: then it cannot be gone by.
    pub fn raw_used(&self) -> bool {
        self.raw
    }

    /// The opacity of a rectangle's pixels on the canvas taken `amount`/256
    /// of the way to `to`: set to it, at 256.
    fn alpha_rect(&mut self, x: i32, y: i32, w: i32, h: i32, to: u8, amount: u32) {
        let Some(alpha) = self.alpha.as_deref_mut() else { return };
        let (x0, x1) = (x.clamp(0, self.width) as usize, (x + w).clamp(0, self.width) as usize);
        let (y0, y1) = (y.clamp(0, self.height) as usize, (y + h).clamp(0, self.height) as usize);
        let width = self.width.max(1) as usize;
        for row in alpha.chunks_exact_mut(width).skip(y0).take(y1.saturating_sub(y0)) {
            let row = &mut row[x0..x1.max(x0)];
            match amount >= 256 {
                true => row.fill(to),
                false => row.iter_mut().for_each(|a| *a = toward(*a, to, amount)),
            }
        }
    }

    /// One pixel's opacity, as `alpha_rect` does a rectangle's; the pixel
    /// on the canvas.
    fn alpha_at(&mut self, x: i32, y: i32, to: u8, amount: u32) {
        if let Some(alpha) = self.alpha.as_deref_mut() {
            let a = &mut alpha[(y * self.width + x) as usize];
            *a = if amount >= 256 { to } else { toward(*a, to, amount) };
        }
    }

    /// One that keeps count of the rows drawn in.
    pub fn counting(mut self) -> Self {
        self.touched = Some(vec![false; self.height.max(0) as usize]);
        self
    }

    /// Which rows were drawn in, a flag a row; none when not counted.
    pub fn touched(self) -> Option<Vec<bool>> {
        self.touched
    }

    /// Rows `y0` to `y1` drawn in, as much of them as is on the canvas.
    fn touch(&mut self, y0: i32, y1: i32) {
        if let Some(rows) = &mut self.touched {
            let (y0, y1) = (y0.max(0) as usize, y1.clamp(0, self.height) as usize);
            if y0 < y1 {
                rows[y0..y1].fill(true);
            }
        }
    }

    /// Rows of the part of a rectangle that is on the canvas.
    fn rows(&mut self, x: i32, y: i32, w: i32, h: i32) -> impl Iterator<Item = &mut [u32]> {
        let (x0, x1) = (x.clamp(0, self.width) as usize, (x + w).clamp(0, self.width) as usize);
        let (y0, y1) = (y.clamp(0, self.height) as usize, (y + h).clamp(0, self.height) as usize);
        if x0 < x1 {
            self.touch(y0 as i32, y1 as i32);
        }
        let rows = self.pixels.chunks_exact_mut(self.width.max(1) as usize);
        rows.skip(y0).take(y1.saturating_sub(y0)).map(move |row| &mut row[x0..x1.max(x0)])
    }

    /// `rows`, each with its row of opacities when they are kept.
    fn rows_and_alpha(&mut self, x: i32, y: i32, w: i32, h: i32) -> impl Iterator<Item = (&mut [u32], Option<&mut [u8]>)> {
        let (x0, x1) = (x.clamp(0, self.width) as usize, (x + w).clamp(0, self.width) as usize);
        let (y0, y1) = (y.clamp(0, self.height) as usize, (y + h).clamp(0, self.height) as usize);
        if x0 < x1 {
            self.touch(y0 as i32, y1 as i32);
        }
        let (width, count) = (self.width.max(1) as usize, y1.saturating_sub(y0));
        let pixels = self.pixels.chunks_exact_mut(width).skip(y0).take(count).map(move |row| &mut row[x0..x1.max(x0)]);
        let mut alpha = self.alpha.as_deref_mut().map(|alpha| alpha.chunks_exact_mut(width).skip(y0).take(count).map(move |row| &mut row[x0..x1.max(x0)]));
        pixels.map(move |row| (row, alpha.as_mut().and_then(Iterator::next)))
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        for (row, alpha) in self.rows_and_alpha(x, y, w, h) {
            row.fill(color);
            if let Some(alpha) = alpha {
                alpha.fill(255);
            }
        }
    }

    /// What is in a rectangle now, to be laid back over it with `dissolve`;
    /// with the opacity kept, that too, in the byte above the colors.
    pub fn copy(&mut self, x: i32, y: i32, w: i32, h: i32) -> Vec<u32> {
        let mut out = Vec::with_capacity(w.max(0) as usize * h.max(0) as usize);
        for (row, alpha) in self.rows_and_alpha(x, y, w, h) {
            match alpha {
                Some(alpha) => out.extend(row.iter().zip(alpha.iter()).map(|(pixel, a)| pixel & 0xFFFFFF | u32::from(*a) << 24)),
                None => out.extend_from_slice(row),
            }
        }
        out
    }

    /// Lays what `copy` took from this same rectangle back over it,
    /// `amount`/256 opaque: one picture giving way to another.
    pub fn dissolve(&mut self, x: i32, y: i32, w: i32, h: i32, was: &[u32], amount: u32) {
        if amount >= 256 {
            return self.paste(x, y, w, h, was);
        }
        let mut rest = was;
        for (row, alpha) in self.rows_and_alpha(x, y, w, h) {
            let n = row.len().min(rest.len());
            for (pixel, was) in row[..n].iter_mut().zip(rest) {
                *pixel = mix(*pixel, *was, amount);
            }
            if let Some(alpha) = alpha {
                for (a, was) in alpha[..n].iter_mut().zip(rest) {
                    *a = toward(*a, (was >> 24) as u8, amount);
                }
            }
            rest = &rest[n..];
        }
    }

    /// Lays what `copy` took back over the same rectangle, as it was: a
    /// row at a time, which is a plain copy of memory.
    pub fn paste(&mut self, x: i32, y: i32, w: i32, h: i32, was: &[u32]) {
        let mut rest = was;
        for (row, alpha) in self.rows_and_alpha(x, y, w, h) {
            let n = row.len().min(rest.len());
            match alpha {
                Some(alpha) => {
                    for ((pixel, a), was) in row[..n].iter_mut().zip(alpha.iter_mut()).zip(rest) {
                        (*pixel, *a) = (was & 0xFFFFFF, (was >> 24) as u8);
                    }
                }
                None => row[..n].copy_from_slice(&rest[..n]),
            }
            rest = &rest[n..];
        }
    }

    /// Lays pixels `was_w` wide, copied from a rectangle of another size,
    /// at the top left of this one: as much as fits, the rest left as it is.
    pub fn paste_from(&mut self, x: i32, y: i32, w: i32, h: i32, was: &[u32], was_w: i32) {
        let n = w.min(was_w).max(0) as usize;
        let was_w = was_w.max(1) as usize;
        for (row, from) in self.rows(x, y, n as i32, h).zip(was.chunks_exact(was_w)) {
            let n = row.len().min(n);
            row[..n].copy_from_slice(&from[..n]);
        }
        // What is laid there is a picture of its own: opaque.
        self.alpha_rect(x, y, n as i32, h.min((was.len() / was_w) as i32), 255, 256);
    }

    /// Draws `frame` scaled to a rectangle, on every core: made larger, by
    /// blending the pixels either side, which blurs where the nearest alone
    /// would show as blocks; made smaller, by the nearest. Rougher than
    /// `image` and several times as quick, which is the right trade for a
    /// picture on the move.
    pub fn image_fast(&mut self, frame: &Frame, x: i32, y: i32, w: i32, h: i32) {
        let (sw, sh) = (frame.width as usize, frame.height as usize);
        let (x0, x1) = (x.max(0), (x + w).min(self.width));
        if w <= 0 || h <= 0 || x0 >= x1 || sw == 0 || sh == 0 || frame.pixels.len() < sw * sh {
            return;
        }
        let larger = w as usize > sw;
        // For each column drawn, the two source columns it lies between and
        // how far towards the second, out of 256.
        let columns: Vec<(usize, usize, u32)> = (x0..x1)
            .map(|px| {
                let at = i64::from(px - x) * 256 * sw as i64 / i64::from(w);
                let first = ((at >> 8) as usize).min(sw - 1);
                (first, (first + 1).min(sw - 1), if larger { (at & 255) as u32 } else { 0 })
            })
            .collect();
        let width = self.width as usize;
        self.touch(y, y + h);
        self.alpha_rect(x0, y, x1 - x0, h, 255, 256);
        self.pixels.par_chunks_exact_mut(width).enumerate().for_each(|(py, row)| {
            let dy = py as i32 - y;
            if dy < 0 || dy >= h {
                return;
            }
            let at = i64::from(dy) * 256 * sh as i64 / i64::from(h);
            let first = ((at >> 8) as usize).min(sh - 1);
            let (second, down) = ((first + 1).min(sh - 1), if larger { (at & 255) as u32 } else { 0 });
            let (upper, lower) = (&frame.pixels[first * sw..(first + 1) * sw], &frame.pixels[second * sw..(second + 1) * sw]);
            for (pixel, &(left, right, across)) in row[x0 as usize..x1 as usize].iter_mut().zip(&columns) {
                *pixel = match larger {
                    true => mix(mix(upper[left], upper[right], across), mix(lower[left], lower[right], across), down),
                    false => upper[left],
                };
            }
        });
    }

    /// Everything on the canvas.
    pub fn all(&mut self) -> &mut [u32] {
        self.touch(0, self.height);
        self.raw |= self.alpha.is_some();
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
                // As it was: its opacity too, kept in the byte above.
                self.cover_toward(px, py, *was, (outside * 256.0) as u32, (*was >> 24) as u8);
            }
        }
    }

    /// Darkens a rectangle more and more towards its bottom, from all of
    /// the brightness kept at the top to `keep`/256 of it. The change is
    /// spread by a fine pattern, so that it shows no steps.
    pub fn fade_down(&mut self, x: i32, y: i32, w: i32, h: i32, keep: u32) {
        // An ordered dither: which of two neighboring levels a pixel
        // takes depends on where it is in a 4 x 4 tile.
        const TILE: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let top = y.clamp(0, self.height);
        let (x0, x1) = (x.clamp(0, self.width) as usize, (x + w).clamp(0, self.width) as usize);
        let (y0, y1) = (y.clamp(0, self.height) as usize, (y + h).clamp(0, self.height) as usize);
        let width = self.width.max(1) as usize;
        // Across the whole of a wide window, under the seek bar, this is
        // most of a million pixels on every frame: a row per core, and
        // the level worked out once for the row, in sixteenths, so that
        // the dither is a whole number added.
        self.touch(y0 as i32, y1 as i32);
        let amounts = |row: usize| -> [u32; 4] {
            let down = (f64::from(top + row as i32 - y) / f64::from(h.max(1))).clamp(0.0, 1.0);
            // Gently at first and at the last, as a shadow falls off.
            let eased = down * down * (3.0 - 2.0 * down);
            let level = ((256.0 - (256.0 - f64::from(keep)) * eased) * 16.0) as u32;
            let tile = &TILE[(top as usize + row) % 4];
            std::array::from_fn(|column| ((level + tile[column]) / 16).min(256))
        };
        let rows = self.pixels[y0 * width..y1.max(y0) * width].par_chunks_exact_mut(width);
        rows.enumerate().for_each(|(row, pixels)| {
            let amounts = amounts(row);
            for (column, pixel) in pixels[x0..x1.max(x0)].iter_mut().enumerate() {
                *pixel = mix(0, *pixel, amounts[column % 4]);
            }
        });
        // Darkened is black laid over it: that much the more opaque.
        if let Some(alpha) = self.alpha.as_deref_mut() {
            alpha[y0 * width..y1.max(y0) * width].par_chunks_exact_mut(width).enumerate().for_each(|(row, alpha)| {
                let amounts = amounts(row);
                for (column, a) in alpha[x0..x1.max(x0)].iter_mut().enumerate() {
                    *a = toward(*a, 255, 256 - amounts[column % 4]);
                }
            });
        }
    }

    /// Darkens a rectangle, keeping `keep`/256 of the brightness.
    pub fn shade(&mut self, x: i32, y: i32, w: i32, h: i32, keep: u32) {
        for pixel in self.rows(x, y, w, h).flatten() {
            *pixel = mix(0, *pixel, keep);
        }
        self.alpha_rect(x, y, w, h, 255, 256 - keep.min(256));
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
            self.touch(py, py + 1);
            for px in left..right {
                let pixel = &mut self.pixels[(py * self.width + px) as usize];
                *pixel = mix(*pixel, color, amount);
                self.alpha_at(px, py, 255, amount);
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
        self.round_ring(x, y, w, h, radius, thickness, color, 256);
    }

    /// A soft shadow under a rounded rectangle, `depth` pixels deep and
    /// falling a little below it: rings of black, fainter outwards, drawn
    /// only around it, as what is drawn over it hides the rest.
    #[allow(clippy::too_many_arguments)]
    pub fn soft_shadow(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, depth: i32) {
        let drop = depth / 3;
        let depth = depth.max(1);
        // From as far in as it falls, so that it meets the edge it falls from.
        for out in 1 - drop..=depth {
            let amount = 44 * (depth + 1 - out.max(1)) / (depth + 1);
            self.round_ring(x - out, y - out + drop, w + 2 * out, h + 2 * out, radius + out, 1, 0x000000, amount as u32);
        }
    }

    /// `round_outline`, `amount`/256 opaque.
    #[allow(clippy::too_many_arguments)]
    pub fn round_ring(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, thickness: i32, color: u32, amount: u32) {
        let band = radius.max(thickness);
        let only = self.band.clone();
        // Between the corners, the ring is only near the top and bottom
        // edges: further in than its thickness and the half pixel blended
        // at its edge, nothing. A pill is all corner rows, and its shadow
        // many rings, each gone over whole before, for nothing in most.
        let (half_w, t) = (f64::from(w) / 2.0, f64::from(thickness));
        let corner = f64::from(radius).clamp(0.0, half_w.min(f64::from(h) / 2.0).max(0.0));
        let reach = (half_w - corner).min(half_w - t - 0.5);
        let middle = f64::from(x) + half_w;
        let clear = (reach >= 0.0).then(|| ((middle - reach - 0.5).ceil() as i32, (middle + reach - 0.5).floor() as i32 + 1));
        for py in (y..y + h).filter(|py| only.as_ref().is_none_or(|rows| rows.contains(py))) {
            // Away from the top and bottom only the two sides have any.
            let full = py < y + band || py >= y + h - band;
            let in_from_edge = (f64::from(py - y) + 0.5).min(f64::from(y + h - py) - 0.5);
            let spans = match (full, clear) {
                (true, Some((from, to))) if in_from_edge >= t + 0.5 => [(x, from.max(x)), (to.max(from).min(x + w), x + w)],
                (true, _) => [(x, x + w), (0, 0)],
                (false, _) => [(x, x + thickness), (x + w - thickness, x + w)],
            };
            let ring = |px: i32| rounded_cover(px, py, x, y, w, h, radius, 0.0) - rounded_cover(px, py, x, y, w, h, radius, t);
            // Out from the middle, the edge's distance grows by at most a
            // pixel a pixel: well inside the ring's inner edge, as many
            // pixels as there is room for are passed over; past its outer
            // edge, nothing further out has any. Exact while the inner edge
            // is the outer one moved in, as with a corner no tighter than
            // the ring is thick.
            if corner >= t && spans[1].0 > spans[0].1 {
                let inner = -(t + 0.5);
                for (start, end, step) in [(spans[0].1 - 1, spans[0].0 - 1, -1), (spans[1].0, spans[1].1, 1)] {
                    let mut px = start;
                    while px != end && (px - end) * step < 0 {
                        let outside = rounded_outside(px, py, x, y, w, h, radius, 0.0);
                        if outside >= 0.5 {
                            break;
                        }
                        if outside <= inner {
                            px += step * ((inner - outside).floor() as i32 + 1);
                            continue;
                        }
                        self.cover(px, py, color, (ring(px) * f64::from(amount)) as u32);
                        px += step;
                    }
                }
                continue;
            }
            for (from, to) in spans {
                for px in from..to {
                    let ring = rounded_cover(px, py, x, y, w, h, radius, 0.0) - rounded_cover(px, py, x, y, w, h, radius, f64::from(thickness));
                    self.cover(px, py, color, (ring * f64::from(amount)) as u32);
                }
            }
        }
    }

    /// A filled rectangle with rounded corners, `amount`/256 opaque.
    #[allow(clippy::too_many_arguments)]
    pub fn round_fill(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: u32, amount: u32) {
        let radius = radius.clamp(0, w.min(h) / 2);
        let only = self.band.clone();
        for py in (y..y + h).filter(|py| only.as_ref().is_none_or(|rows| rows.contains(py))) {
            // Only the ends of the rows at the top and bottom curve; the
            // rest of every row is plain.
            let curved = py < y + radius || py >= y + h - radius;
            let inset = if curved { radius } else { 0 };
            for px in (x..x + inset).chain(x + w - inset..x + w) {
                let cover = rounded_cover(px, py, x, y, w, h, radius, 0.0);
                self.cover(px, py, color, (cover * f64::from(amount)) as u32);
            }
            for row in self.rows(x + inset, py, w - 2 * inset, 1) {
                // Opaque, it is a plain fill, much the quicker.
                match amount >= 256 {
                    true => row.fill(color),
                    false => row.iter_mut().for_each(|pixel| *pixel = mix(*pixel, color, amount)),
                }
            }
            self.alpha_rect(x + inset, py, w - 2 * inset, 1, 255, amount);
        }
    }

    fn cover(&mut self, x: i32, y: i32, color: u32, amount: u32) {
        self.cover_toward(x, y, color, amount, 255);
    }

    /// `cover`, the pixel's opacity taken toward `to` as its color is.
    fn cover_toward(&mut self, x: i32, y: i32, color: u32, amount: u32, to: u8) {
        if amount > 0 && x >= 0 && y >= 0 && x < self.width && y < self.height {
            self.touch(y, y + 1);
            let pixel = &mut self.pixels[(y * self.width + x) as usize];
            *pixel = mix(*pixel, color, amount.min(256));
            self.alpha_at(x, y, to, amount.min(256));
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
        let (y0, y1) = (y.max(0), (y + h).min(self.height));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let width = self.width as usize;
        let draw_row = |row: usize, out: &mut [u32]| {
            let dy = y0 + row as i32 - y;
            let sy = (i64::from(dy) * sh as i64 / i64::from(h)) as usize;
            let source = &source[sy * sw..(sy + 1) * sw];
            for (pixel, &column) in out[x0 as usize..x1 as usize].iter_mut().zip(&columns[(x0 - x) as usize..]) {
                *pixel = source[column];
            }
        };
        self.touch(y0, y1);
        self.alpha_rect(x0, y0, x1 - x0, y1 - y0, 255, 256);
        let rows = self.pixels[y0 as usize * width..y1 as usize * width].chunks_exact_mut(width);
        // A large picture has its rows done each on its own core; a small
        // one is quicker drawn than handed out.
        if (x1 - x0) as usize * (y1 - y0) as usize >= SHARE {
            rows.collect::<Vec<_>>().into_par_iter().enumerate().for_each(|(row, out)| draw_row(row, out));
        } else {
            rows.enumerate().for_each(|(row, out)| draw_row(row, out));
        }
    }

    /// Draws many frames, each scaled to its rectangle as `image` does,
    /// in one pass over the rows of the canvas, each row on its own core.
    /// Later rectangles are drawn over earlier ones where they overlap.
    pub fn images(&mut self, blits: &[(&Frame, [i32; 4])]) {
        struct Blit<'a> {
            source: &'a [u32],
            sw: usize,
            sh: usize,
            x: i32,
            y: i32,
            h: i32,
            x0: usize,
            x1: usize,
            y0: i32,
            y1: i32,
            columns: Vec<usize>,
        }
        let (width, height) = (self.width, self.height);
        let shrunk: Vec<Option<Vec<u32>>> = blits.iter().map(|(frame, [_, _, w, h])| shrink(frame, (*w).max(0) as u32, (*h).max(0) as u32)).collect();
        let blits: Vec<Blit> = blits
            .iter()
            .zip(&shrunk)
            .filter_map(|((frame, place), shrunk)| {
                let [x, y, w, h] = *place;
                if w <= 0 || h <= 0 {
                    return None;
                }
                let (source, sw): (&[u32], usize) = match shrunk {
                    Some(pixels) => (pixels, w as usize),
                    None => (&frame.pixels, frame.width as usize),
                };
                let (x0, x1) = (x.max(0), (x + w).min(width));
                let (y0, y1) = (y.max(0), (y + h).min(height));
                if x0 >= x1 || y0 >= y1 {
                    return None;
                }
                let columns = (0..w).map(|dx| (i64::from(dx) * sw as i64 / i64::from(w)) as usize).collect();
                Some(Blit { source, sw, sh: source.len() / sw.max(1), x, y, h, x0: x0 as usize, x1: x1 as usize, y0, y1, columns })
            })
            .collect();
        if blits.is_empty() {
            return;
        }
        let (top, bottom) = (blits.iter().map(|b| b.y0).min().unwrap_or(0), blits.iter().map(|b| b.y1).max().unwrap_or(0));
        let stride = width as usize;
        self.touch(top, bottom);
        for blit in &blits {
            self.alpha_rect(blit.x0 as i32, blit.y0, (blit.x1 - blit.x0) as i32, blit.y1 - blit.y0, 255, 256);
        }
        let rows = self.pixels[top as usize * stride..bottom as usize * stride].par_chunks_exact_mut(stride);
        rows.enumerate().for_each(|(row, out)| {
            let py = top + row as i32;
            for blit in blits.iter().filter(|blit| (blit.y0..blit.y1).contains(&py)) {
                let sy = (i64::from(py - blit.y) * blit.sh as i64 / i64::from(blit.h)) as usize;
                let source = &blit.source[sy * blit.sw..(sy + 1) * blit.sw];
                let columns = &blit.columns[(blit.x0 as i32 - blit.x) as usize..];
                for (pixel, &column) in out[blit.x0..blit.x1].iter_mut().zip(columns) {
                    *pixel = source[column];
                }
            }
        });
    }

    fn blend(&mut self, x: i32, y: i32, color: u32, coverage: u8) {
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            self.touch(y, y + 1);
            let pixel = &mut self.pixels[(y * self.width + x) as usize];
            let amount = u32::from(coverage) + u32::from(coverage >> 7);
            *pixel = mix(*pixel, color, amount);
            self.alpha_at(x, y, 255, amount);
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
    crate::render::with_resizer(|resizer| resizer.resize(&source, &mut target, &options)).ok()?;
    Some(pixels)
}

/// Blends `amount`/256 of `b` into `a`, two channels at a time.
/// How much of the pixel at `(px, py)`, from 0 to 1, lies within a
/// rectangle with corners of `radius`, shrunk by `inset` on every side.
/// Fractions along the edge are what make the curve smooth.
#[allow(clippy::too_many_arguments)]
fn rounded_cover(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, radius: i32, inset: f64) -> f64 {
    (0.5 - rounded_outside(px, py, x, y, w, h, radius, inset)).clamp(0.0, 1.0)
}

/// How far the middle of the pixel at `(px, py)` is outside that shape,
/// less than nothing inside it: as far as it is from the nearest edge.
#[allow(clippy::too_many_arguments)]
fn rounded_outside(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, radius: i32, inset: f64) -> f64 {
    let (half_w, half_h) = (f64::from(w) / 2.0 - inset, f64::from(h) / 2.0 - inset);
    let radius = (f64::from(radius) - inset).clamp(0.0, half_w.min(half_h).max(0.0));
    // Distance from the middle, less the straight part of each side.
    let across = (f64::from(px) + 0.5 - f64::from(x) - f64::from(w) / 2.0).abs() - (half_w - radius);
    let down = (f64::from(py) + 0.5 - f64::from(y) - f64::from(h) / 2.0).abs() - (half_h - radius);
    across.max(0.0).hypot(down.max(0.0)) + across.max(down).min(0.0) - radius
}

pub fn mix(a: u32, b: u32, amount: u32) -> u32 {
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

/// A buffer of `len` pixels, as pages the system hands out blank: they
/// cost nothing until first written, unlike a buffer grown by writing
/// zeros into it, which at 4K takes one core several milliseconds.
pub fn blank(len: usize) -> Vec<u32> {
    vec![0; len]
}

/// Gives a buffer the size of a picture, keeping it if it has that size.
pub fn size_for(buffer: &mut Vec<u32>, len: usize) {
    if buffer.len() != len {
        *buffer = blank(len);
    }
}

/// Copies a whole picture into a buffer kept for the purpose, on every core.
pub fn copy_all(into: &mut Vec<u32>, from: &[u32]) {
    size_for(into, from.len());
    copy_into(into, from);
}

/// Copies one whole picture over another of the same size, on every core.
pub fn copy_into(into: &mut [u32], from: &[u32]) {
    into.par_chunks_mut(SHARE).zip(from.par_chunks(SHARE)).for_each(|(into, from)| into.copy_from_slice(from));
}

/// Fills a whole picture with one color, on every core.
pub fn fill_all(pixels: &mut [u32], color: u32) {
    pixels.par_chunks_mut(SHARE).for_each(|part| part.fill(color));
}

/// How many rows are looked at together when telling what has changed.
const BAND: usize = 32;
/// More rectangles than this are sent as the one that holds them all.
const MOST_PARTS: usize = 48;

/// Where one picture differs from another of the same size, as a few
/// rectangles of left, top, width and height, bringing the other up to
/// date as it goes. Handing the display only these is what keeps a small
/// change cheap on a large window: the whole of a 4K frame is 33 MB.
/// Given the rows that can have changed, a flag a row, only their bands are
/// looked at; the rest are the same as they were.
pub fn changed(now: &[u32], was: &mut [u32], width: usize, only: Option<&[bool]>) -> Vec<[u32; 4]> {
    if width == 0 || now.len() != was.len() {
        return Vec::new();
    }
    let looked = |band: usize| only.is_none_or(|rows| rows.iter().skip(band * BAND).take(BAND).any(|row| *row));
    // For each band of rows, the columns between which something changed.
    let scan = |(band, (now, was)): (usize, (&[u32], &mut [u32]))| {
        if !looked(band) || now == was {
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
    };
    // A few rows told of are done here: handing them to other cores is
    // a wait for those to be woken, which on a busy machine is the most
    // of it. The whole window, a band to a core.
    let bands: Vec<Option<(usize, usize)>> = match only {
        Some(_) => now.chunks(width * BAND).zip(was.chunks_mut(width * BAND)).enumerate().map(scan).collect(),
        None => now.par_chunks(width * BAND).zip(was.par_chunks_mut(width * BAND)).enumerate().map(scan).collect(),
    };
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
    /// The interface font a weight up, for titles and what is chosen;
    /// and whether that is what is being drawn now.
    medium: Option<fontdue::Font>,
    strong: bool,
    glyphs: HashMap<(char, u32, bool), (fontdue::Metrics, Vec<u8>)>,
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
        let icons = load(include_bytes!("../assets/solar.ttf").to_vec());
        // Only with the font carried along: another named has no such weight.
        let medium = std::env::var_os("JOG_FONT").is_none().then(|| load(include_bytes!("../assets/Geist-Medium.ttf").to_vec())).flatten();
        Text { font, icons, medium, strong: false, others: Vec::new(), loading: None, sought: HashSet::new(), glyphs: HashMap::new() }
    }

    /// A line of text and the air around it: rows of menus and lists are
    /// this tall, and lines of writing this far apart.
    pub fn line_height(px: u32) -> i32 {
        (px * 3 / 2) as i32
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
            0 if self.strong => self.medium.as_ref().or(self.font.as_ref()),
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

    /// As wide as `text` is a weight up, as `draw_strong` draws it.
    pub fn width_strong(&mut self, text: &str, px: u32) -> i32 {
        self.strong = true;
        let width = self.width(text, px);
        self.strong = false;
        width
    }

    /// Draws a line a weight up: a title, a heading, the row chosen.
    pub fn draw_strong(&mut self, canvas: &mut Canvas, x: i32, y: i32, px: u32, color: u32, text: &str) {
        self.strong = true;
        self.draw(canvas, x, y, px, color, text);
        self.strong = false;
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
            let key = (c, px, self.strong);
            if !self.glyphs.contains_key(&key) {
                let which = self.font_with(c);
                let Some(font) = self.font(which) else { continue };
                let glyph = font.rasterize(c, Self::size(which, px));
                self.glyphs.insert(key, glyph);
            }
            let (metrics, coverage) = &self.glyphs[&key];
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
        assert!(changed(&now, &mut was, width, None).is_empty());
        // A dot near the top and a bar lower down, in different bands.
        now[3 * width + 10] = 1;
        now[70 * width + 50..70 * width + 90].fill(2);
        assert_eq!(changed(&now, &mut was, width, None), [[10, 0, 1, 32], [50, 64, 40, 32]]);
        assert_eq!(was, now);
        assert!(changed(&now, &mut was, width, None).is_empty());
        // Rows of a last band that is short of full are counted as they are.
        now[99 * width + 199] = 3;
        assert_eq!(changed(&now, &mut was, width, None), [[199, 96, 1, 4]]);
    }

    #[test]
    fn told_which_rows_changed_only_their_bands_are_looked_at() {
        let (width, height) = (200, 100);
        let mut was = vec![0u32; width * height];
        let mut now = was.clone();
        now[3 * width + 10] = 1;
        now[70 * width + 50] = 2;
        // Told only of row 70: the dot near the top is not looked for, and
        // is still to be told of.
        let mut rows = vec![false; height];
        rows[70] = true;
        assert_eq!(changed(&now, &mut was, width, Some(&rows)), [[50, 64, 1, 32]]);
        assert_eq!(changed(&now, &mut was, width, None), [[10, 0, 1, 32]]);
        // Told of none: nothing.
        now[3 * width + 11] = 1;
        assert!(changed(&now, &mut was, width, Some(&[false; 100])).is_empty());
    }
}

#[cfg(test)]
mod touched_tests {
    use super::Canvas;

    /// The rows flagged as drawn in, as ranges.
    fn spans(rows: &[bool]) -> Vec<std::ops::Range<usize>> {
        let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
        for (y, _) in rows.iter().enumerate().filter(|(_, drawn)| **drawn) {
            match spans.last_mut() {
                Some(last) if last.end == y => last.end = y + 1,
                _ => spans.push(y..y + 1),
            }
        }
        spans
    }

    #[test]
    fn a_canvas_knows_the_rows_drawn_in() {
        let mut pixels = vec![0u32; 100 * 50];
        // Not asked to keep count, it keeps none.
        let mut canvas = Canvas::new(&mut pixels, 100, 50);
        canvas.fill(5, 20, 10, 3, 1);
        assert_eq!(canvas.touched(), None);
        let mut canvas = Canvas::new(&mut pixels, 100, 50).counting();
        // Wholly off the canvas, or no width: nothing.
        canvas.fill(-20, 10, 10, 5, 1);
        canvas.fill(10, 60, 10, 5, 1);
        canvas.round_fill(10, 10, 0, 5, 2, 1, 256);
        // A fill, a pill off the bottom and a ring: each its own rows,
        // with the rows between left out.
        canvas.fill(5, 20, 10, 3, 1);
        canvas.pill(5, 40, 30, 20, 1, 256, 30);
        canvas.round_ring(5, 2, 30, 4, 2, 1, 1, 256);
        assert_eq!(spans(&canvas.touched().unwrap()), [2..6, 20..23, 40..50]);
        // Handed the whole of it, all of it may be drawn in.
        let mut canvas = Canvas::new(&mut pixels, 100, 50).counting();
        let _ = canvas.all();
        assert_eq!(spans(&canvas.touched().unwrap()), [0..50]);
    }
}

#[cfg(test)]
mod ring_tests {
    use super::{rounded_cover, Canvas};

    #[test]
    fn a_ring_is_drawn_as_when_every_pixel_was_gone_over() {
        // Every pixel of every corner row, as it was done before: what the
        // ring has to come out as, pixel for pixel.
        let reference = |pixels: &mut Vec<u32>, [x, y, w, h, radius, thickness]: [i32; 6], amount: u32| {
            let mut canvas = Canvas::new(pixels, 120, 90);
            let band = radius.max(thickness);
            for py in y..y + h {
                let full = py < y + band || py >= y + h - band;
                let spans = if full { [(x, x + w), (0, 0)] } else { [(x, x + thickness), (x + w - thickness, x + w)] };
                for (from, to) in spans {
                    for px in from..to {
                        let ring = rounded_cover(px, py, x, y, w, h, radius, 0.0) - rounded_cover(px, py, x, y, w, h, radius, f64::from(thickness));
                        canvas.cover(px, py, 0x123456, (ring * f64::from(amount)) as u32);
                    }
                }
            }
        };
        // Pills, boxes, sharp and rounder than they are tall, odd and even,
        // thin rings and thick, partly off the canvas.
        let shapes = [
            [10, 10, 80, 40, 20, 1],
            [10, 10, 81, 41, 20, 1],
            [5, 5, 100, 30, 15, 2],
            [5, 5, 60, 60, 8, 1],
            [5, 5, 60, 60, 0, 3],
            [5, 5, 60, 20, 40, 1],
            [-10, 70, 50, 30, 15, 1],
            [20, 20, 7, 7, 3, 2],
            [3, 3, 90, 12, 1, 4],
            // The rings of a pill's shadow, as `soft_shadow` draws them.
            [2, 2, 100, 50, 25, 1],
            [-3, 1, 106, 56, 28, 1],
            [-5, -5, 130, 100, 50, 1],
            [5, 5, 60, 40, 2, 3],
        ];
        for shape in shapes {
            for amount in [44, 256] {
                let ground: Vec<u32> = (0..120 * 90).map(|at| (at as u32).wrapping_mul(2_654_435_761) & 0xFFFFFF).collect();
                let (mut want, mut got) = (ground.clone(), ground);
                reference(&mut want, shape, amount);
                let [x, y, w, h, radius, thickness] = shape;
                Canvas::new(&mut got, 120, 90).round_ring(x, y, w, h, radius, thickness, 0x123456, amount);
                assert!(want == got, "ring {shape:?} at {amount} drawn differently");
            }
        }
    }
}
