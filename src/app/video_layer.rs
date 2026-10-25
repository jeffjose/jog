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
    /// Told by the window's drawing, for the next: whether the buffer it
    /// draws in is the one drawn and shown last time, as it was then.
    pub(super) reused: Option<bool>,
    /// The rows drawn in the window's buffer last time, and its length:
    /// all of it black but those.
    left: Option<(usize, Vec<bool>)>,
    /// The rows that can differ from what was shown last, a flag a row:
    /// all the display has to be told of.
    pub(super) changed: Option<Vec<bool>>,
}

impl Layer {
    /// The window's buffer drawn in otherwise: what is in it is not known.
    pub(super) fn forget_drawn(&mut self) {
        (self.left, self.changed) = (None, None);
    }
}

/// At most this many bands: more, and they are put together as one.
const BANDS: usize = 16;

/// Rows with nothing in them that still do not part two bands: a band
/// apiece for each line of a few words close together would be no gain.
const GAP: usize = 8;

/// Pixels to go over from which the work is shared out among threads:
/// under this, the drawing thread does it sooner than others can be woken
/// to, on a busy machine by far.
const SHARED_FROM: usize = 1 << 20;

impl App {
    /// The window drawn while a video plays in a window of its own: what is
    /// around and over it, and what is over it handed to the player.
    pub(super) fn paint_over_video(&mut self, pixels: &mut [u32], window: (u32, u32)) {
        self.scene_live = false;
        let (left, (stage_w, stage_h)) = self.stage();
        let place = [left.max(0), 0, (stage_w as i32).min(window.0 as i32 - left.max(0)), (stage_h as i32).min(window.1 as i32)];
        // Over black: what the window shows around the video, and with it
        // the colors of what goes over it. The buffer drawn in last time,
        // handed back as it was, is black but for the rows drawn in then,
        // and only they are made black again.
        let reused = self.video_layer.reused.take();
        self.video_layer.changed = None;
        let left = self.video_layer.left.take().filter(|(len, _)| reused == Some(true) && *len == pixels.len());
        match &left {
            Some((_, rows)) => pixels.chunks_exact_mut(window.0 as usize).zip(rows).filter(|(_, drawn)| **drawn).for_each(|(row, _)| row.fill(0)),
            None => ui::fill_all(pixels, 0),
        }
        // What of the frame paused on is outside BT.709, if asked: a blend
        // over the picture as the rest is, and so laid over it the same way.
        let gamut = self.draw_gamut(pixels, window);
        let mut canvas = Canvas::new(pixels, window.0, window.1).counting();
        self.draw_overlays(&mut canvas);
        // Only the rows drawn in, over black or over white, can hold
        // anything: the rest are passed over without being looked at,
        // most of the window, which under a busy CPU took the longest.
        let mut drawn = canvas.touched().unwrap_or_default();
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
        // The same rows as over black.
        let _ = self.draw_gamut(&mut white, window);
        let mut canvas = Canvas::new(&mut white, window.0, window.1).counting();
        self.draw_overlays(&mut canvas);
        for (row, over_white) in drawn.iter_mut().zip(canvas.touched().unwrap_or_default()) {
            *row |= over_white;
        }
        for y in gamut.into_iter().flatten() {
            drawn[y as usize] = true;
        }
        let picture = picture_of(pixels, &white, window.0 as usize, place, &drawn);
        // Drawn for the window: what can differ from what it showed last is
        // what was drawn in then or now.
        if reused.is_some() {
            self.video_layer.changed = left.map(|(_, before)| before.iter().zip(&drawn).map(|(then, now)| *then || *now).collect());
            self.video_layer.left = Some((pixels.len(), drawn));
        }
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
/// width, premultiplied BGRA, and where each goes in the part. Only the
/// window's rows flagged as `drawn` in are looked at: nothing is in the rest.
fn picture_of(black: &[u32], white: &[u32], width: usize, [left, top, w, h]: [i32; 4], drawn: &[bool]) -> Picture {
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
    // by comparing memory: most rows are. The rest here, unless there are
    // a great many: handing rows to other threads waits for them to be
    // woken, which on a busy machine takes longer than the rows do.
    let looked: Vec<usize> = (0..h).filter(|y| drawn.get(top + y) == Some(&true)).collect();
    let span_of = |&y: &usize| {
        let (b, wh) = rows(y);
        if b == none_black.as_slice() && wh == none_white.as_slice() {
            return None;
        }
        let first = (0..w).find(|x| alpha(b[*x], wh[*x]) > 0)?;
        let last = (first..w).rev().find(|x| alpha(b[*x], wh[*x]) > 0).unwrap_or(first);
        Some((first, last + 1))
    };
    let found: Vec<Option<(usize, usize)>> = match looked.len() * w < SHARED_FROM {
        true => looked.iter().map(span_of).collect(),
        false => looked.par_iter().map(span_of).collect(),
    };
    let mut spans: Vec<Option<(usize, usize)>> = vec![None; h];
    for (y, span) in looked.into_iter().zip(found) {
        spans[y] = span;
    }
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
        let row_of = |(at, out): (usize, &mut [u32])| {
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
        };
        match band.len() < SHARED_FROM {
            true => band.chunks_mut(*cw as usize).enumerate().for_each(row_of),
            false => band.par_chunks_mut(*cw as usize).enumerate().for_each(row_of),
        }
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
        let picture = picture_of(&black, &white, width, [0, 0, 4, 2], &[true; 2]);
        // Cut down to the second row, from its second pixel.
        assert_eq!(picture.parts, [[1, 1, 3, 1]]);
        let a = |p: u32| p >> 24;
        assert_eq!(picture.pixels[0], 0xFFFF0000);
        assert!((127..=129).contains(&a(picture.pixels[1])) && (picture.pixels[1] & 0xFF) == a(picture.pixels[1]));
        assert!((63..=65).contains(&a(picture.pixels[2])) && (picture.pixels[2] >> 16 & 0xFF) == 0);
        // Nothing drawn is nothing to show.
        assert!(picture_of(&draw(0)[..4], &draw(0xFFFFFF)[..4], width, [0, 0, 4, 1], &[true]).parts.is_empty());
    }

    #[test]
    fn rows_not_drawn_in_are_not_looked_at() {
        // Something in each of four rows; told only the middle two were
        // drawn in, only they make the picture.
        let mut black = vec![0u32; 16];
        let white_of = |black: &[u32]| black.iter().map(|pixel| if *pixel == 0 { 0xFFFFFF } else { *pixel }).collect::<Vec<_>>();
        for row in 0..4 {
            black[row * 4 + 1] = 0xFF0000;
        }
        let white = white_of(&black);
        assert_eq!(picture_of(&black, &white, 4, [0, 0, 4, 4], &[false, true, true, false]).parts, [[1, 1, 1, 2]]);
        assert_eq!(picture_of(&black, &white, 4, [0, 0, 4, 4], &[true; 4]).parts, [[1, 0, 1, 4]]);
        // Told of no rows, or none drawn in: nothing.
        assert!(picture_of(&black, &white, 4, [0, 0, 4, 4], &[]).parts.is_empty());
        assert!(picture_of(&black, &white, 4, [0, 0, 4, 4], &[false; 4]).parts.is_empty());
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
        let window = self.window.as_ref()?;
        if slot.is_none() {
            // The one the first video was opened in ahead of the window is
            // this player's: put inside the window, rather than made anew.
            let early = self.early_window.take().filter(|_| index == usize::from(self.active));
            *slot = early.and_then(|mut early| early.adopt(window).then_some(early)).or_else(|| x11::VideoWindow::new(window));
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
