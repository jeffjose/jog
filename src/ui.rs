use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

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

    pub fn outline(&mut self, x: i32, y: i32, w: i32, h: i32, thickness: i32, color: u32) {
        self.fill(x, y, w, thickness, color);
        self.fill(x, y + h - thickness, w, thickness, color);
        self.fill(x, y, thickness, h, color);
        self.fill(x + w - thickness, y, thickness, h, color);
    }

    /// Draws `frame` scaled (crudely) to exactly fill a rectangle.
    pub fn image(&mut self, frame: &Frame, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let (fw, fh) = (frame.width as i64, frame.height as i64);
        for dy in 0..h {
            let (py, sy) = (y + dy, i64::from(dy) * fh / i64::from(h));
            if py < 0 || py >= self.height {
                continue;
            }
            for dx in 0..w {
                let (px, sx) = (x + dx, i64::from(dx) * fw / i64::from(w));
                if px >= 0 && px < self.width {
                    self.pixels[(py * self.width + px) as usize] = frame.pixels[(sy * fw + sx) as usize];
                }
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

/// Blends `amount`/256 of `b` into `a`, two channels at a time.
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
