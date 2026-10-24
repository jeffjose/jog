//! Checks drawn on the picture as it plays, as a monitor on a film set
//! draws them: zebra stripes over what is clipped, and false color for
//! how exposed each part is. Done in the player process, over each frame
//! as it is drawn, so that they cost jog's window nothing.

use rayon::prelude::*;

/// Which checks are drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Checks {
    /// Stripes over what is as bright as full white, or within a hair of
    /// it: the highlights the picture has lost. By brightness, as a
    /// camera's zebra is, not by a channel: a saturated red at full is a
    /// color, not a lost highlight.
    pub zebra: bool,
    /// Each part colored for how bright it is, in bands.
    pub false_color: bool,
}

impl Checks {
    pub fn any(self) -> bool {
        self.zebra || self.false_color
    }
}

/// From this up, out of 255 in a channel, a pixel counts as clipped.
const CLIPPED: u32 = 250;

/// The bands of false color, by brightness out of 255 (Rec. 709 luma of
/// the picture as shown), and the color each is drawn in; outside them a
/// pixel is drawn in gray at its own brightness. Pure black is left as it
/// is: the bars around a picture are that, and are not the picture.
const BANDS: &[(u32, u32, u32)] = &[
    // Crushed: detail gone into black.
    (1, 5, 0x6A0DAD),
    // Very dark: 2-10%.
    (5, 26, 0x1E4FD8),
    // Middle gray, and the darker skin tones: 40-50%.
    (102, 128, 0x2EB82E),
    // The lighter skin tones: 70-80%.
    (178, 204, 0xE87BB0),
    // Near clipping: 95%.
    (242, CLIPPED, 0xF2E33A),
    // Clipped.
    (CLIPPED, 256, 0xE5232B),
];

/// Draws `checks` over a frame `width` pixels wide, `0RGB` a pixel.
pub fn apply(pixels: &mut [u32], width: u32, checks: Checks) {
    if !checks.any() || width == 0 {
        return;
    }
    let width = width as usize;
    pixels.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, pixel) in row.iter_mut().enumerate() {
            let (r, g, b) = (*pixel >> 16 & 0xFF, *pixel >> 8 & 0xFF, *pixel & 0xFF);
            if checks.false_color {
                *pixel = false_color(r, g, b);
            }
            // Diagonal stripes, black and white, over what is clipped.
            if checks.zebra && luma(r, g, b) >= CLIPPED {
                *pixel = if (x + y) % 8 < 4 { 0x000000 } else { 0xFFFFFF };
            }
        }
    });
}

/// The checks as an mpv user shader, for a video drawn on the GPU: run on
/// the picture as it goes to the screen, before mpv lays anything over it,
/// with the same bands and stripes as `apply`. `None` for no checks.
pub fn shader(checks: Checks) -> Option<String> {
    if !checks.any() {
        return None;
    }
    let channel = |color: u32, shift: u32| f64::from(color >> shift & 0xFF) / 255.0;
    let bands: String = BANDS
        .iter()
        .map(|(from, to, color)| format!("        if (l >= {from}.0 && l < {to}.0) return vec3({:.4}, {:.4}, {:.4});\n", channel(*color, 16), channel(*color, 8), channel(*color, 0)))
        .collect();
    Some(format!(
        "//!HOOK OUTPUT
//!BIND HOOKED
//!DESC jog checks
vec3 luma_band(vec3 c, float l) {{
    if (c == vec3(0.0)) return c;
{bands}    return vec3(l / 255.0);
}}
vec4 hook() {{
    vec4 c = HOOKED_texOff(0);
    // Out of 255, as the picture is drawn in software.
    float l = floor(dot(floor(c.rgb * 255.0 + 0.5), vec3(218.0, 732.0, 74.0)) / 1024.0);
    vec3 rgb = c.rgb;
    if ({false_color}) rgb = luma_band(c.rgb, l);
    if ({zebra} && l >= {CLIPPED}.0) {{
        vec2 at = floor(HOOKED_pos * HOOKED_size);
        rgb = mod(at.x + at.y, 8.0) < 4.0 ? vec3(0.0) : vec3(1.0);
    }}
    return vec4(rgb, c.a);
}}
",
        false_color = checks.false_color,
        zebra = checks.zebra,
    ))
}

/// A pixel in false color: its band's color, or gray at its brightness.
/// Rec. 709 luma, in integers: 0.2126, 0.7152, 0.0722 of 1024.
fn luma(r: u32, g: u32, b: u32) -> u32 {
    (r * 218 + g * 732 + b * 74) >> 10
}

fn false_color(r: u32, g: u32, b: u32) -> u32 {
    if r == 0 && g == 0 && b == 0 {
        return 0;
    }
    let luma = luma(r, g, b);
    match BANDS.iter().find(|(from, to, _)| (*from..*to).contains(&luma)) {
        Some((_, _, color)) => *color,
        None => luma << 16 | luma << 8 | luma,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_is_striped_and_the_rest_is_not() {
        let mut row = vec![0x00FF_FFFF, 0x0080_8080, 0x00FF_0000, 0x0010_1010];
        apply(&mut row, 4, Checks { zebra: true, false_color: false });
        assert!(row[0] == 0 || row[0] == 0x00FF_FFFF, "white is clipped: a stripe");
        assert_eq!(row[1], 0x0080_8080, "middle gray is left");
        assert_eq!(row[2], 0x00FF_0000, "a saturated red at full is a color, not a lost highlight");
        assert_eq!(row[3], 0x0010_1010);
    }

    #[test]
    fn false_color_bands_by_brightness() {
        assert_eq!(false_color(0, 0, 0), 0, "black, as bars are, is left");
        assert_eq!(false_color(2, 2, 2), 0x6A0DAD, "crushed");
        assert_eq!(false_color(118, 118, 118), 0x2EB82E, "middle gray");
        assert_eq!(false_color(255, 255, 255), 0xE5232B, "clipped");
        assert_eq!(false_color(60, 60, 60), 0x3C3C3C, "between bands, gray at its own brightness");
    }
}
