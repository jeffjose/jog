//! Words as Edit sets them on a picture: in one of five faces, heavy or
//! slanted, at any size, with emoji among them. Apart from the interface's
//! own text, which is one face at a few sizes; these are drawn into the
//! picture too, at full size, from another thread.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use fontdue::{Font, FontSettings, Metrics};

use crate::ui::mix;

/// The faces words can be in, by what each is called; hotshot's.
pub const FACES: [&str; 5] = ["Geist", "Instrument Serif", "Architects Daughter", "Fira Code", "Anton"];

const FACE_FILES: [&[u8]; 5] = [
    include_bytes!("../assets/Geist-Regular.ttf"),
    include_bytes!("../assets/InstrumentSerif-Regular.ttf"),
    include_bytes!("../assets/ArchitectsDaughter-Regular.ttf"),
    include_bytes!("../assets/FiraCode-Regular.ttf"),
    include_bytes!("../assets/Anton-Regular.ttf"),
];

/// The heavy and slanted cuts there are: which face, heavy, slanted, and
/// the file. A face without the cut asked for has it made of its own
/// letters: laid down twice for heavy, each row moved over for slanted.
const CUTS: [(usize, bool, bool, &[u8]); 5] = [
    (0, true, false, include_bytes!("../assets/Geist-Bold.ttf")),
    (0, false, true, include_bytes!("../assets/Geist-Italic.ttf")),
    (0, true, true, include_bytes!("../assets/Geist-BoldItalic.ttf")),
    (1, false, true, include_bytes!("../assets/InstrumentSerif-Italic.ttf")),
    (3, true, false, include_bytes!("../assets/FiraCode-Bold.ttf")),
];

/// How words are drawn, apart from their size and color.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Look {
    pub face: usize,
    pub bold: bool,
    pub italic: bool,
}

/// The height of a line of words `px` high.
pub fn line_height(px: f32) -> f32 {
    px * 4.0 / 3.0
}

struct Fonts {
    faces: Vec<Option<Font>>,
    cuts: Vec<Option<Font>>,
}

fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let load = |bytes: &[u8]| Font::from_bytes(bytes, FontSettings::default()).ok();
        Fonts { faces: FACE_FILES.iter().map(|bytes| load(bytes)).collect(), cuts: CUTS.iter().map(|cut| load(cut.3)).collect() }
    })
}

/// The font a look is drawn in, and what of it is to be made by hand:
/// heavy, and slanted.
fn font_for(look: Look) -> Option<(usize, &'static Font, bool, bool)> {
    let fonts = fonts();
    let face = look.face.min(FACES.len() - 1);
    // A cut that is both, or failing that one that is either.
    let wanted = [(look.bold, look.italic), (look.bold, false), (false, look.italic)];
    for (bold, italic) in wanted.into_iter().filter(|(bold, italic)| *bold || *italic) {
        let found = CUTS.iter().position(|cut| cut.0 == face && cut.1 == bold && cut.2 == italic);
        if let Some(font) = found.and_then(|at| fonts.cuts[at].as_ref()) {
            let key = 100 + found.unwrap_or(0);
            return Some((key, font, look.bold && !bold, look.italic && !italic));
        }
    }
    let font = fonts.faces[face].as_ref().or(fonts.faces[0].as_ref())?;
    Some((face, font, look.bold, look.italic))
}

type Glyph = (Metrics, Vec<u8>);

/// A letter of a font at a size, kept once made.
fn glyph(key: usize, font: &Font, c: char, px: u32) -> Option<std::sync::Arc<Glyph>> {
    static MADE: OnceLock<Mutex<HashMap<(usize, char, u32), std::sync::Arc<Glyph>>>> = OnceLock::new();
    let made = MADE.get_or_init(Default::default);
    let mut made = made.lock().ok()?;
    if made.len() > 4096 {
        made.clear();
    }
    Some(made.entry((key, c, px)).or_insert_with(|| std::sync::Arc::new(font.rasterize(c, px as f32))).clone())
}

/// What is drawn of each letter: the font's, or for one it has not, an
/// emoji, which is a picture.
fn is_emoji(font: &Font, c: char) -> bool {
    font.lookup_glyph_index(c) == 0 && !c.is_whitespace() && !c.is_control()
}

/// How wide a line of words is.
pub fn width(look: Look, words: &str, px: f32) -> f32 {
    let Some((_, font, heavy, slanted)) = font_for(look) else { return 0.0 };
    let size = px.round().max(1.0);
    let mut width = 0.0;
    for c in words.chars().filter(|c| *c != '\u{fe0f}' && *c != '\u{200d}') {
        width += if is_emoji(font, c) { emoji_side(size) as f32 * 1.08 } else { font.metrics(c, size).advance_width };
    }
    width + if heavy { (size / 14.0).max(1.0) } else { 0.0 } + if slanted { size * 0.2 } else { 0.0 }
}

fn emoji_side(px: f32) -> u32 {
    (px * 1.2).round().max(1.0) as u32
}

/// Draws a line of words whose line box has its top-left at `(x, y)`, in
/// `clip` (left, top, right, bottom). Where `alpha` is given, they make
/// the picture as opaque as they are where they are drawn.
#[allow(clippy::too_many_arguments)]
pub fn draw(pixels: &mut [u32], width: usize, clip: [i32; 4], (x, y): (f32, f32), px: f32, color: u32, opacity: u32, look: Look, words: &str, mut alpha: Option<&mut [u8]>) {
    let Some((key, font, heavy, slanted)) = font_for(look) else { return };
    let size = px.round().max(1.0);
    let (ascent, descent) = font.horizontal_line_metrics(size).map_or((size * 0.8, size * -0.2), |m| (m.ascent, m.descent));
    let line = line_height(size);
    let baseline = y + (line + ascent + descent) / 2.0;
    let extra = if heavy { (size / 14.0).max(1.0) as i32 } else { 0 };
    let slant = if slanted { 0.2 } else { 0.0 };
    let mut pen = x;
    let mut put = |px: i32, py: i32, color: u32, cover: u32| {
        if px < clip[0] || py < clip[1] || px >= clip[2] || py >= clip[3] || cover == 0 {
            return;
        }
        let at = py as usize * width + px as usize;
        let amount = cover * opacity / 256;
        pixels[at] = mix(pixels[at], color, amount);
        if let Some(alpha) = alpha.as_deref_mut() {
            alpha[at] = alpha[at].max(amount.min(255) as u8);
        }
    };
    for c in words.chars().filter(|c| *c != '\u{fe0f}' && *c != '\u{200d}') {
        if is_emoji(font, c) {
            let side = emoji_side(size);
            if let Some(picture) = crate::annotate::emoji(&c.to_string(), side) {
                let (w, h, source) = &*picture;
                let top = (y + (line - *h as f32) / 2.0).round() as i32;
                for row in 0..*h {
                    for column in 0..*w {
                        let p = source[(row * w + column) as usize];
                        let a = p >> 24;
                        put(pen.round() as i32 + column, top + row, p & 0xFFFFFF, a + (a >> 7));
                    }
                }
            }
            pen += side as f32 * 1.08;
            continue;
        }
        let Some(glyph) = glyph(key, font, c, size as u32) else { continue };
        let (metrics, coverage) = &*glyph;
        let left = pen.round() as i32 + metrics.xmin;
        let top = baseline.round() as i32 - metrics.ymin - metrics.height as i32;
        for (row, line) in coverage.chunks_exact(metrics.width.max(1)).enumerate() {
            let py = top + row as i32;
            // Slanted: each row moved over by how far above the line it is.
            let lean = ((baseline - py as f32) * slant).round() as i32;
            for (column, &amount) in line.iter().enumerate() {
                if amount > 0 {
                    for again in 0..=extra {
                        put(left + column as i32 + lean + again, py, color, u32::from(amount) + u32::from(amount >> 7));
                    }
                }
            }
        }
        pen += metrics.advance_width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heavier_and_slanted_words_are_wider_and_the_faces_differ() {
        let plain = Look::default();
        let w = width(plain, "Hello", 40.0);
        assert!(w > 50.0);
        assert!(width(Look { bold: true, ..plain }, "Hello", 40.0) > w);
        assert_ne!(width(Look { face: 4, ..plain }, "Hello", 40.0), w);
    }

    #[test]
    fn words_are_drawn_where_they_are_put() {
        let (w, h) = (200usize, 60usize);
        let mut pixels = vec![0u32; w * h];
        draw(&mut pixels, w, [0, 0, w as i32, h as i32], (10.0, 5.0), 30.0, 0xFFFFFF, 256, Look::default(), "Hi", None);
        assert!(pixels.iter().any(|p| *p != 0));
        assert!(pixels[..w * 5].iter().all(|p| *p == 0));
    }
}
