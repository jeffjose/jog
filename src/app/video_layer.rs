//! What jog draws over a video the player draws on the GPU. The video is
//! in a window of its own over jog's, which covers whatever jog draws
//! there; so that is handed to the player instead, as pictures with
//! transparency for mpv to lay over the video.
//!
//! jog draws by blending onto what is already there, a pixel at a time,
//! with no transparency of its own. Drawn once over black and once over
//! white, each pixel says how opaque it is and what color, exactly: over
//! black a blend leaves the color times its opacity, and over white that
//! plus white times the rest.

use super::*;

/// What is drawn over the video: bands of rows with something in them,
/// each cut to its own width, their pixels one after another.
#[derive(Clone, PartialEq, Debug, Default)]
struct Picture {
    pixels: Vec<u32>,
    /// Each band's left, top, width and height in the video's window.
    parts: Vec<[i32; 4]>,
}

/// What was last handed over, and what drawing it takes.
#[derive(Default)]
pub struct Layer {
    /// The window drawn over white, kept from one drawing to the next.
    white: Vec<u32>,
    /// The part of the window it was whitened for.
    white_for: Option<[i32; 4]>,
    /// The picture last sent, to leave it be when unchanged.
    sent: Option<Picture>,
    /// Both buffers were being copied when it changed: to be sent again.
    due: bool,
}

/// At most this many bands: more, and they are put together as one.
const BANDS: usize = 16;

/// Rows with nothing in them that still do not part two bands: a band
/// apiece for each line of a few words close together would be no gain.
const GAP: usize = 8;

impl App {
    /// The window drawn while a video plays in a window of its own: what is
    /// around and over it, and what is over it handed to the player.
    pub(super) fn paint_over_video(&mut self, pixels: &mut [u32], window: (u32, u32)) {
        self.scene_live = false;
        let (left, (stage_w, stage_h)) = self.stage();
        let place = [left.max(0), 0, (stage_w as i32).min(window.0 as i32 - left.max(0)), (stage_h as i32).min(window.1 as i32)];
        // Over black: what the window shows around the video, and with it
        // the colors of what goes over it.
        ui::fill_all(pixels, 0);
        // What of the frame paused on is outside BT.709, if asked: a blend
        // over the picture as the rest is, and so laid over it the same way.
        self.draw_gamut(pixels, window);
        self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
        // Over white, in a buffer of this one's own, which keeps what was
        // drawn in it: white over all of it only when it is new, of another
        // size, or the video's part of the window has moved; otherwise it is
        // white already but where something was drawn last time, which was
        // whitened again then.
        let mut white = std::mem::take(&mut self.video_layer.white);
        if white.len() != pixels.len() || self.video_layer.white_for != Some(place) {
            ui::size_for(&mut white, pixels.len());
            ui::fill_all(&mut white, 0xFFFFFF);
            self.video_layer.white_for = Some(place);
        }
        self.draw_gamut(&mut white, window);
        self.draw_overlays(&mut Canvas::new(&mut white, window.0, window.1));
        let picture = picture_of(pixels, &white, window.0 as usize, place);
        // Where anything was drawn over the video: white again for next
        // time; and black, as under the video's window nothing of this one
        // shows, and left as it was the display is told only of what
        // changed around it.
        for [x, y, w, h] in &picture.parts {
            Canvas::new(&mut white, window.0, window.1).fill(place[0] + x, place[1] + y, *w, *h, 0xFFFFFF);
            Canvas::new(pixels, window.0, window.1).fill(place[0] + x, place[1] + y, *w, *h, 0);
        }
        self.video_layer.white = white;
        let unchanged = self.video_layer.sent.as_ref().map_or(picture.parts.is_empty(), |sent| *sent == picture);
        if unchanged && !self.video_layer.due {
            return;
        }
        let Some(player) = &self.player else { return };
        let sent = player.overlay(&picture.pixels, &picture.parts);
        // Not taken: tried again at the next drawing, soon.
        self.video_layer.due = !sent;
        if sent {
            self.video_layer.sent = Some(picture);
        } else {
            self.redraw();
        }
    }

    /// The video no longer in a window of its own: nothing left over it.
    pub(super) fn clear_video_layer(&mut self) {
        if self.video_layer.sent.take().is_some_and(|sent| !sent.parts.is_empty()) {
            if let Some(player) = &self.player {
                player.overlay(&[], &[]);
            }
        }
        self.video_layer.due = false;
    }
}

/// The picture with transparency that drawing over black and over white
/// make, within `place` (left, top, width, height) of a window `width`
/// wide: in bands of the rows with something in them, each cut to its
/// width, premultiplied BGRA, and where each goes in the part.
fn picture_of(black: &[u32], white: &[u32], width: usize, [left, top, w, h]: [i32; 4]) -> Picture {
    use rayon::prelude::*;
    let (left, top, w, h) = (left as usize, top as usize, w.max(0) as usize, h.max(0) as usize);
    // How opaque each pixel is: white shows through as much as it is not.
    let alpha = |b: u32, w: u32| 255 - ((w >> 8 & 255).saturating_sub(b >> 8 & 255)).min(255);
    let (none_black, none_white) = (vec![0u32; w], vec![0xFFFFFFu32; w]);
    let rows = |y: usize| {
        let row = (top + y) * width + left;
        (&black[row..row + w], &white[row..row + w])
    };
    // Each row's first and last pixel with anything on it. A row left as
    // it was, black over black and white over white, is passed over whole
    // by comparing memory: most rows are. The rest, a row to a thread.
    let spans: Vec<Option<(usize, usize)>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let (b, wh) = rows(y);
            if b == none_black.as_slice() && wh == none_white.as_slice() {
                return None;
            }
            let first = (0..w).find(|x| alpha(b[*x], wh[*x]) > 0)?;
            let last = (first..w).rev().find(|x| alpha(b[*x], wh[*x]) > 0).unwrap_or(first);
            Some((first, last + 1))
        })
        .collect();
    let mut parts = bands(&spans);
    if parts.len() > BANDS {
        let (x0, x1) = (parts.iter().map(|part| part[0]).min().unwrap_or(0), parts.iter().map(|part| part[0] + part[2]).max().unwrap_or(0));
        let (y0, y1) = (parts[0][1], parts[parts.len() - 1][1] + parts[parts.len() - 1][3]);
        parts = vec![[x0, y0, x1 - x0, y1 - y0]];
    }
    let mut pixels = vec![0u32; parts.iter().map(|[_, _, w, h]| (*w * *h) as usize).sum()];
    let mut rest = pixels.as_mut_slice();
    for [x0, y0, cw, ch] in &parts {
        let (band, after) = rest.split_at_mut((*cw * *ch) as usize);
        rest = after;
        band.par_chunks_mut(*cw as usize).enumerate().for_each(|(at, out)| {
            let y = *y0 as usize + at;
            if spans[y].is_none() {
                return;
            }
            let (b, wh) = rows(y);
            for (x, pixel) in out.iter_mut().enumerate() {
                let (b, wh) = (b[*x0 as usize + x], wh[*x0 as usize + x]);
                let a = alpha(b, wh);
                // The color over black is the color times its opacity
                // already; never more than the opacity, which rounding
                // could make it.
                let channel = |shift: u32| (b >> shift & 255).min(a);
                *pixel = a << 24 | channel(16) << 16 | channel(8) << 8 | channel(0);
            }
        });
    }
    Picture { pixels, parts }
}

/// The bands of rows with something in them, as left, top, width and
/// height: rows apart by no more than `GAP` go together.
fn bands(spans: &[Option<(usize, usize)>]) -> Vec<[i32; 4]> {
    let mut parts: Vec<[i32; 4]> = Vec::new();
    // The band being gathered: its first and last row, and leftmost and
    // rightmost pixel.
    let mut open: Option<(usize, usize, usize, usize)> = None;
    for (y, span) in spans.iter().enumerate() {
        match (span, &mut open) {
            (Some((first, end)), Some((_, last, x0, x1))) if y - *last <= GAP + 1 => {
                (*last, *x0, *x1) = (y, (*x0).min(*first), (*x1).max(*end));
            }
            (Some((first, end)), _) => {
                if let Some((y0, last, x0, x1)) = open.take() {
                    parts.push([x0 as i32, y0 as i32, (x1 - x0) as i32, (last + 1 - y0) as i32]);
                }
                open = Some((y, y, *first, *end));
            }
            (None, _) => {}
        }
    }
    if let Some((y0, last, x0, x1)) = open {
        parts.push([x0 as i32, y0 as i32, (x1 - x0) as i32, (last + 1 - y0) as i32]);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_over_black_and_white_gives_back_the_color_and_its_opacity() {
        // A 4x2 window: nothing, a solid red, half-opaque white, a quarter
        // of blue; blended as Canvas blends.
        let width = 4;
        let draw = |ground: u32| {
            let mut pixels = vec![ground; 8];
            pixels[5] = ui::mix(ground, 0xFF0000, 256);
            pixels[6] = ui::mix(ground, 0xFFFFFF, 128);
            pixels[7] = ui::mix(ground, 0x0000FF, 64);
            pixels
        };
        let (black, white) = (draw(0), draw(0xFFFFFF));
        let picture = picture_of(&black, &white, width, [0, 0, 4, 2]);
        // Cut down to the second row, from its second pixel.
        assert_eq!(picture.parts, [[1, 1, 3, 1]]);
        let a = |p: u32| p >> 24;
        assert_eq!(picture.pixels[0], 0xFFFF0000);
        assert!((127..=129).contains(&a(picture.pixels[1])) && (picture.pixels[1] & 0xFF) == a(picture.pixels[1]));
        assert!((63..=65).contains(&a(picture.pixels[2])) && (picture.pixels[2] >> 16 & 0xFF) == 0);
        // Nothing drawn is nothing to show.
        assert!(picture_of(&draw(0)[..4], &draw(0xFFFFFF)[..4], width, [0, 0, 4, 1]).parts.is_empty());
    }

    #[test]
    fn rows_far_apart_are_bands_of_their_own() {
        // Words at the top, a bar at the bottom: two bands, not one the
        // height of the window; rows close together are one.
        let mut spans = vec![None; 100];
        spans[2] = Some((10, 40));
        spans[5] = Some((12, 50));
        spans[90] = Some((0, 100));
        spans[95] = Some((0, 100));
        assert_eq!(bands(&spans), [[10, 2, 40, 4], [0, 90, 100, 6]]);
    }
}

impl App {
    /// The window player `index` draws a video into on the GPU, made the
    /// first time it is wanted; `None` when videos are drawn in software,
    /// or no such window can be made.
    pub(super) fn video_window_id(&mut self, index: usize) -> Option<u64> {
        if !crate::mpv::gpu_video() {
            return None;
        }
        let slot = self.video_windows.get_mut(index)?;
        if slot.is_none() {
            *slot = self.window.as_ref().and_then(|window| x11::VideoWindow::new(window));
        }
        slot.as_ref().map(x11::VideoWindow::id)
    }

    /// The playing player's window shown over `place`, or none; the other
    /// player's hidden, holding what it holds out of sight.
    pub(super) fn show_video_windows(&mut self, place: Option<[i32; 4]>) {
        let active = usize::from(self.active);
        for (index, window) in self.video_windows.iter_mut().enumerate() {
            if let Some(window) = window {
                window.show(place.filter(|_| index == active));
            }
        }
    }

    /// Whether the playing player's window is up.
    pub(super) fn video_window_shown(&self) -> bool {
        self.video_windows.get(usize::from(self.active)).and_then(Option::as_ref).is_some_and(x11::VideoWindow::shown)
    }
}
