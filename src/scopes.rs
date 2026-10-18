//! Graphs of the colors of the picture on view, for the debug overlay:
//! how much of it is at each level of red, green and blue, where along it
//! the light and the dark are, and which way its colors lean. They are
//! counted from what is on the screen, a sample of it, each time it is
//! drawn; and only while they are asked for.

/// The graphs there are, in the order they are offered and drawn: the
/// name on a button, and what it shows.
pub const KINDS: [(&str, &str); 5] = [
    ("Histogram", "How many pixels at each level, the three colors over each other"),
    ("Rows", "The same, a row to each color"),
    ("Waveform", "How light, by where across the picture"),
    ("Parade", "The same, for each color beside the others"),
    ("Vector", "Which way the colors lean, and how far"),
];
pub const OVERLAID: usize = 0;
pub const ROWS: usize = 1;
pub const WAVEFORM: usize = 2;
pub const PARADE: usize = 3;
pub const VECTOR: usize = 4;

/// About how many pixels are looked at, however large the picture: the
/// graphs of a tenth of a million are those of them all, to the eye.
const SAMPLES: usize = 100_000;
/// How many columns the waveform has, each panel of the parade, and each
/// side of the square the colors are plotted on.
const WAVE: usize = 256;
const PANEL: usize = 96;
const SIDE: usize = 128;

/// The color each channel is drawn in: enough of the other two in it to
/// be read on black, and the three together near white.
const CHANNEL: [[u32; 3]; 3] = [[224, 56, 56], [48, 188, 84], [64, 104, 236]];
const GROUND: u32 = 0x0B0B0B;
const RULE: u32 = 0x2A2A2A;

/// What was counted from a picture.
pub struct Measured {
    /// How many of the pixels looked at had each level, by channel.
    pub levels: [[u32; 256]; 3],
    /// How many had each lightness in each column: a level's columns
    /// side by side, so that a row of the graph is read in one run.
    wave: Vec<u32>,
    /// The same for each channel, its narrower columns side by side.
    parade: Vec<u32>,
    /// How many had each color, on a square of blue-yellow across and
    /// red-cyan up.
    vector: Vec<u32>,
    /// How many were looked at.
    pub samples: u32,
    /// The column of the waveform and of the parade each pixel across
    /// the picture falls in, worked out once for its width.
    columns: Vec<(u16, u16)>,
}

impl Default for Measured {
    fn default() -> Self {
        Measured { levels: [[0; 256]; 3], wave: Vec::new(), parade: Vec::new(), vector: Vec::new(), samples: 0, columns: Vec::new() }
    }
}

impl Measured {
    /// Counts the part of a picture in a rectangle of `pixels`, which are
    /// `stride` to a row, for the graphs wanted.
    pub fn count(&mut self, pixels: &[u32], stride: usize, [x, y, w, h]: [usize; 4], wanted: [bool; 5]) {
        self.levels = [[0; 256]; 3];
        self.samples = 0;
        for (counts, cells, on) in [
            (&mut self.wave, WAVE * 256, wanted[WAVEFORM]),
            (&mut self.parade, 3 * PANEL * 256, wanted[PARADE]),
            (&mut self.vector, SIDE * SIDE, wanted[VECTOR]),
        ] {
            counts.clear();
            counts.resize(if on { cells } else { 0 }, 0);
        }
        if w == 0 || h == 0 || stride == 0 || (y + h - 1) * stride + x + w > pixels.len() {
            return;
        }
        // Every so many pixels each way, to come to about the number wanted.
        let step = ((w * h) as f64 / SAMPLES as f64).sqrt().ceil().max(1.0) as usize;
        if self.columns.len() != w {
            self.columns = (0..w).map(|column| ((column * WAVE / w) as u16, (column * PANEL / w) as u16)).collect();
        }
        let (wave_on, parade_on, vector_on) = (wanted[WAVEFORM], wanted[PARADE], wanted[VECTOR]);
        let [red_levels, green_levels, blue_levels] = &mut self.levels;
        for row in (0..h).step_by(step) {
            let line = &pixels[(y + row) * stride + x..][..w];
            for column in (0..w).step_by(step) {
                let pixel = line[column];
                let (r, g, b) = ((pixel >> 16 & 0xFF) as usize, (pixel >> 8 & 0xFF) as usize, (pixel & 0xFF) as usize);
                red_levels[r] += 1;
                green_levels[g] += 1;
                blue_levels[b] += 1;
                let (wave_at, panel_at) = self.columns[column];
                if wave_on {
                    let light = (54 * r + 183 * g + 19 * b) >> 8;
                    self.wave[light * WAVE + wave_at as usize] += 1;
                }
                if parade_on {
                    let at = panel_at as usize;
                    self.parade[r * 3 * PANEL + at] += 1;
                    self.parade[g * 3 * PANEL + PANEL + at] += 1;
                    self.parade[b * 3 * PANEL + 2 * PANEL + at] += 1;
                }
                if vector_on {
                    let (r, g, b) = (r as i32, g as i32, b as i32);
                    let blue = (-29 * r - 99 * g + 128 * b) >> 8;
                    let red = (128 * r - 116 * g - 12 * b) >> 8;
                    let across = ((blue + 128).clamp(0, 255) as usize * SIDE / 256).min(SIDE - 1);
                    let down = ((128 - red).clamp(0, 255) as usize * SIDE / 256).min(SIDE - 1);
                    self.vector[down * SIDE + across] += 1;
                }
            }
            self.samples += w.div_ceil(step) as u32;
        }
    }

    /// How much of the picture is at the very bottom and the very top of
    /// any channel, as parts of a hundred: what has lost its detail.
    pub fn clipped(&self) -> (f64, f64) {
        let most = |level: usize| self.levels.iter().map(|channel| channel[level]).max().unwrap_or(0);
        let part = |count: u32| 100.0 * f64::from(count) / f64::from(self.samples.max(1));
        (part(most(0)), part(most(255)))
    }
}

/// Where a graph is drawn: the pixels of the window, how many to a row,
/// and the rectangle in them.
pub struct Place<'a> {
    pub pixels: &'a mut [u32],
    pub stride: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Place<'_> {
    /// The pixels of a row of the graph, as far as they are in the window.
    fn row(&mut self, y: i32) -> &mut [u32] {
        let (top, left) = (self.y + y, self.x.max(0));
        let right = (self.x + self.w).min(self.stride);
        if y < 0 || y >= self.h || top < 0 || right <= left {
            return &mut [];
        }
        let start = (top * self.stride) as usize;
        match self.pixels.get_mut(start + left as usize..start + right as usize) {
            Some(row) => row,
            None => &mut [],
        }
    }

    fn set(&mut self, x: i32, y: i32, color: u32) {
        let skip = (-self.x).max(0);
        if x >= skip {
            if let Some(pixel) = self.row(y).get_mut((x - skip) as usize) {
                *pixel = color;
            }
        }
    }

    /// The dark ground, with a faint line across at each quarter of the
    /// way up, or down each quarter of the way across.
    fn ground(&mut self, across: bool) {
        let (w, h) = (self.w, self.h);
        let skip = (-self.x).max(0) as usize;
        for y in 0..h {
            let rule = across && (1..4).any(|quarter| y == h * quarter / 4);
            let row = self.row(y);
            row.fill(if rule { RULE } else { GROUND });
            if !across {
                for quarter in 1..4 {
                    if let Some(pixel) = ((w * quarter / 4) as usize).checked_sub(skip).and_then(|at| row.get_mut(at)) {
                        *pixel = RULE;
                    }
                }
            }
        }
    }
}

fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r.min(255) << 16 | g.min(255) << 8 | b.min(255)
}

/// How far up a count comes, of `height`, against the largest there is:
/// in proportion, or by its logarithm, which lets the small ones be seen.
fn rise(count: u32, most: u32, height: i32, log: bool) -> i32 {
    let part = match log {
        true => f64::from(count).ln_1p() / f64::from(most.max(1)).ln_1p(),
        false => f64::from(count) / f64::from(most.max(1)),
    };
    (part.min(1.0) * f64::from(height)).round() as i32
}

/// The three channels' levels over each other, or each on a row of its
/// own. The two ends are left out of what sets the height: a picture
/// with much pure black or white would flatten everything between.
pub fn histogram(place: &mut Place, measured: &Measured, log: bool, rows: bool) {
    place.ground(false);
    let most = measured.levels.iter().flat_map(|channel| &channel[1..255]).copied().max().unwrap_or(1);
    let (w, h) = (place.w, place.h);
    let band = if rows { h / 3 } else { h };
    let skip = (-place.x).max(0);
    // How far up each channel comes in each column.
    let up: Vec<[i32; 3]> = (0..w)
        .map(|x| {
            let level = (x * 256 / w.max(1)).clamp(0, 255) as usize;
            std::array::from_fn(|channel| rise(measured.levels[channel][level], most, band - if rows { 2 } else { 0 }, log))
        })
        .collect();
    // The color of each mix of the three, by which of them reach a row.
    let mixes: [u32; 8] = std::array::from_fn(|mask| {
        let sum = (0..3).filter(|channel| mask >> channel & 1 == 1).fold([0; 3], |sum, channel| std::array::from_fn(|part| sum[part] + CHANNEL[channel][part]));
        rgb(sum[0], sum[1], sum[2])
    });
    for y in 0..h {
        let (channel_row, height_up) = if rows { ((y / band.max(1)).min(2) as usize, band * ((y / band.max(1)).min(2) + 1) - 1 - y) } else { (0, h - 1 - y) };
        let row = place.row(y);
        for (pixel, up) in row.iter_mut().zip(&up[skip as usize..]) {
            let mask = match rows {
                true => usize::from(height_up < up[channel_row]) << channel_row,
                false => (0..3).filter(|channel| height_up < up[*channel]).fold(0, |mask, channel| mask | 1 << channel),
            };
            if mask != 0 {
                *pixel = mixes[mask];
            }
        }
    }
}

/// One panel of a waveform: `columns` of counts by level, the first at
/// `first` in rows of `per_level`, drawn `w` wide from `left` in a color.
fn panel(place: &mut Place, counts: &[u32], per_level: usize, first: usize, columns: usize, left: i32, w: i32, color: [u32; 3], per_column: f64) {
    let h = place.h;
    let skip = (-place.x).max(0);
    // Which column of counts each pixel across draws.
    let across: Vec<usize> = (0..w).map(|x| first + (x as usize * columns / w.max(1) as usize).min(columns - 1)).collect();
    // How bright a cell is by how many fell in it, by the root so that the
    // few show: full at a tenth of a column's share.
    let full = (per_column / 10.0).max(1.0);
    let shades: Vec<u32> = (0..=full.ceil() as u32)
        .map(|count| {
            let lit = (255.0 * (f64::from(count) / full).sqrt().min(1.0)) as u32;
            rgb(11 + color[0] * lit / 255, 11 + color[1] * lit / 255, 11 + color[2] * lit / 255)
        })
        .collect();
    let mut sums = vec![0u32; w as usize];
    for y in 0..h {
        // The levels that share this row of the graph, the top one first.
        let from = ((h - 1 - y) * 256 / h).clamp(0, 255) as usize;
        let to = (((h - y) * 256 / h) as usize).clamp(from + 1, 256);
        sums.iter_mut().for_each(|sum| *sum = 0);
        for level in from..to {
            let cells = &counts[level * per_level..][..per_level];
            for (sum, column) in sums.iter_mut().zip(&across) {
                *sum += cells[*column];
            }
        }
        let row = place.row(y);
        let start = (left - skip).max(0) as usize;
        let lead = (skip - left).max(0) as usize;
        for (pixel, sum) in row.iter_mut().skip(start).zip(sums.iter().skip(lead)) {
            if *sum > 0 {
                *pixel = shades[(*sum as usize).min(shades.len() - 1)];
            }
        }
    }
}

/// How light the picture is, by where across it: its left at the left.
pub fn waveform(place: &mut Place, measured: &Measured) {
    place.ground(true);
    if measured.wave.is_empty() {
        return;
    }
    let per_column = f64::from(measured.samples) / WAVE as f64;
    let w = place.w;
    panel(place, &measured.wave, WAVE, 0, WAVE, 0, w, [236, 236, 236], per_column);
}

/// The same for red, green and blue, side by side.
pub fn parade(place: &mut Place, measured: &Measured) {
    place.ground(true);
    if measured.parade.is_empty() {
        return;
    }
    let per_column = f64::from(measured.samples) / PANEL as f64;
    let gap = (place.w / 60).max(2);
    let each = (place.w - 2 * gap) / 3;
    for channel in 0..3 {
        panel(place, &measured.parade, 3 * PANEL, channel * PANEL, PANEL, channel as i32 * (each + gap), each, CHANNEL[channel], per_column);
    }
}

/// The color that belongs at a spot on the square of colors, at a
/// middling lightness: what a pixel plotted there was near.
fn hue(blue: i32, red: i32, lit: u32) -> u32 {
    let light = 150;
    let part = |value: i32| (value.clamp(0, 255) as u32) * lit / 255;
    rgb(part(light + red * 403 / 256), part(light - blue * 48 / 256 - red * 120 / 256), part(light + blue * 475 / 256))
}

/// Which way the colors of the picture lean, and how far: gray at the
/// middle, the stronger the color the further out, the hue by the way.
pub fn vector(place: &mut Place, measured: &Measured) {
    place.ground(false);
    // No quarter lines here: the ground is drawn plain over them.
    for y in 0..place.h {
        place.row(y).fill(GROUND);
    }
    let side = place.w.min(place.h);
    let (left, top) = ((place.w - side) / 2, (place.h - side) / 2);
    let (middle, reach) = (side / 2, side * 15 / 32);
    if !measured.vector.is_empty() {
        // Each cell's color once, then laid over the pixels it covers.
        let most = f64::from(measured.samples.max(1)) / 400.0;
        let cells: Vec<u32> = (0..SIDE * SIDE)
            .map(|cell| {
                let count = measured.vector[cell];
                if count == 0 {
                    return GROUND;
                }
                let lit = 70 + (185.0 * (f64::from(count) / most).sqrt().min(1.0)) as u32;
                let (across, down) = ((cell % SIDE) as i32, (cell / SIDE) as i32);
                hue((across - SIDE as i32 / 2) * 256 / SIDE as i32, (SIDE as i32 / 2 - down) * 256 / SIDE as i32, lit)
            })
            .collect();
        let skip = (-place.x).max(0);
        for y in 0..side {
            let down = (y as usize * SIDE / side as usize).min(SIDE - 1);
            let line = &cells[down * SIDE..][..SIDE];
            let row = place.row(top + y);
            let start = (left - skip).max(0) as usize;
            for (x, pixel) in row.iter_mut().skip(start).take(side as usize).enumerate() {
                *pixel = line[(x * SIDE / side as usize).min(SIDE - 1)];
            }
        }
    }
    // A ring at the edge and one half way, and a cross at the middle.
    for radius in [reach, reach / 2] {
        let steps = (radius * 8).max(64);
        for step in 0..steps {
            let angle = f64::from(step) * std::f64::consts::TAU / f64::from(steps);
            let (x, y) = (middle + (f64::from(radius) * angle.cos()).round() as i32, middle + (f64::from(radius) * angle.sin()).round() as i32);
            place.set(left + x, top + y, 0x3A3A3A);
        }
    }
    for along in -side / 40 - 2..=side / 40 + 2 {
        place.set(left + middle + along, top + middle, 0x4A4A4A);
        place.set(left + middle, top + middle + along, 0x4A4A4A);
    }
    // A small box where each pure color would fall at three quarters
    // strength: red, yellow, green, cyan, blue, magenta.
    for [r, g, b] in [[255, 0, 0], [255, 255, 0], [0, 255, 0], [0, 255, 255], [0, 0, 255], [255, 0, 255]] {
        let blue = (-29 * r - 99 * g + 128 * b) * 3 / 4 >> 8;
        let red = (128 * r - 116 * g - 12 * b) * 3 / 4 >> 8;
        let (x, y) = (middle + blue * side / 256, middle - red * side / 256);
        let half = (side / 60).max(2);
        let color = rgb(r as u32 * 200 / 255, g as u32 * 200 / 255, b as u32 * 200 / 255);
        for along in -half..=half {
            for (dx, dy) in [(along, -half), (along, half), (-half, along), (half, along)] {
                place.set(left + x + dx, top + y + dy, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_counted_by_channel_and_by_place() {
        // Left half pure red, right half white, 200 by 100.
        let (w, h) = (200, 100);
        let pixels: Vec<u32> = (0..w * h).map(|at| if at % w < w / 2 { 0xFF0000 } else { 0xFFFFFF }).collect();
        let mut measured = Measured::default();
        measured.count(&pixels, w, [0, 0, w, h], [true; 5]);
        assert_eq!(measured.samples as usize, w * h);
        assert_eq!(measured.levels[0][255] as usize, w * h);
        assert_eq!(measured.levels[1][0] as usize, w * h / 2);
        assert_eq!(measured.levels[1][255] as usize, w * h / 2);
        // The waveform has the red half dark at the left and white at the right.
        let wave_at = |level: usize, column: usize| measured.wave[level * WAVE + column];
        assert!(wave_at(53, 0) > 0 && wave_at(255, 0) == 0);
        assert!(wave_at(255, WAVE - 2) > 0);
        // White is at the middle of the square of colors; red is up from it.
        assert!(measured.vector[SIDE / 2 * SIDE + SIDE / 2] > 0);
        assert!(measured.vector[..SIDE * SIDE / 4].iter().sum::<u32>() > 0);
        let (black, white) = measured.clipped();
        assert!((black - 50.0).abs() < 0.01 && (white - 100.0).abs() < 0.01, "{black} {white}");
    }

    #[test]
    fn a_rectangle_off_the_picture_counts_nothing() {
        let mut measured = Measured::default();
        measured.count(&[0; 100], 10, [5, 5, 10, 10], [true; 5]);
        assert_eq!(measured.samples, 0);
    }

    #[test]
    fn a_graph_partly_off_the_window_is_drawn_only_where_it_is_on() {
        let mut pixels = vec![0u32; 50 * 20];
        let mut measured = Measured::default();
        measured.count(&[0x808080; 400], 20, [0, 0, 20, 20], [true; 5]);
        for draw in [0, 1, 2, 3, 4] {
            let mut place = Place { pixels: &mut pixels, stride: 50, x: -10, y: -5, w: 40, h: 30 };
            match draw {
                0 => histogram(&mut place, &measured, true, false),
                1 => histogram(&mut place, &measured, false, true),
                2 => waveform(&mut place, &measured),
                3 => parade(&mut place, &measured),
                _ => vector(&mut place, &measured),
            }
        }
        // Drawn into, and nothing past the right of the graph.
        assert!(pixels[0] != 0 && pixels[49] == 0);
    }
}
