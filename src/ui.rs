use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use fast_image_resize as fir;

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

    pub fn outline(&mut self, x: i32, y: i32, w: i32, h: i32, thickness: i32, color: u32) {
        self.fill(x, y, w, thickness, color);
        self.fill(x, y + h - thickness, w, thickness, color);
        self.fill(x, y, thickness, h, color);
        self.fill(x + w - thickness, y, thickness, h, color);
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
        for py in y..y + h {
            for px in x..x + w {
                let cover = rounded_cover(px, py, x, y, w, h, radius, 0.0);
                self.cover(px, py, color, (cover * f64::from(amount)) as u32);
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

/// Text rendering with a system font. Loading the font takes a while, so
/// this is only created once something needs to draw text.
pub struct Text {
    font: Option<fontdue::Font>,
    glyphs: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl Text {
    pub fn load() -> Self {
        let font = font_files().find_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
        });
        if font.is_none() {
            eprintln!("jog: no usable font found, text will not be drawn (set JOG_FONT to a .ttf file)");
        }
        Text { font, glyphs: HashMap::new() }
    }

    pub fn line_height(px: u32) -> i32 {
        (px * 4 / 3) as i32
    }

    pub fn width(&mut self, text: &str, px: u32) -> i32 {
        let Some(font) = &self.font else { return 0 };
        text.chars().map(|c| font.metrics(c, px as f32).advance_width).sum::<f32>().ceil() as i32
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
            let (metrics, coverage) =
                self.glyphs.entry((c, px)).or_insert_with(|| font.rasterize(c, px as f32));
            let left = pen.round() as i32 + metrics.xmin;
            let top = baseline - metrics.ymin - metrics.height as i32;
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
