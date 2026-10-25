//! Checks drawn on the video, as a monitor on a film set draws them:
//! zebra stripes over what is clipped and false color for exposure, as it
//! plays (drawn by the player process: `mpv::checks`); and, on the frame
//! paused on, what of a wide-gamut video is outside BT.709.
//!
//! The last cannot follow playback: the player hands frames over in 8-bit
//! BT.709, where what was outside it has been clipped already. So the
//! frame paused on is decoded again by ffmpeg, made small, in linear
//! light and its own primaries, as floating point, each pixel taken into
//! BT.709 and those that come out with a channel below nothing marked.

use std::path::Path;
use std::process::Stdio;

use rayon::prelude::*;

use super::devtools::Action;
use super::inspect::Hit;
use super::*;
use crate::mpv::Checks;

/// How wide the frame is decoded for the gamut check, at most.
const MASK_WIDTH: u32 = 640;

/// What is outside BT.709 on a frame: a cell each, `width` across.
pub struct GamutMask {
    pub id: usize,
    pub at: f64,
    pub width: u32,
    pub height: u32,
    pub outside: Vec<bool>,
    /// The share of the picture outside, from 0 to 1.
    pub share: f64,
}

/// The matrix from a gamut's linear RGB into BT.709's, if it is one jog
/// knows to be wider: BT.2020, or Display P3.
fn to_709(primaries: &str) -> Option<[[f32; 3]; 3]> {
    // As mpv names them (bt.2020, display-p3), or ffmpeg (bt2020, smpte432).
    match primaries.replace('.', "").as_str() {
        "bt2020" => Some([[1.6605, -0.5876, -0.0728], [-0.1246, 1.1329, -0.0083], [-0.0182, -0.1006, 1.1187]]),
        "smpte432" | "display-p3" => Some([[1.2249, -0.2247, 0.0], [-0.0420, 1.0419, 0.0], [-0.0197, -0.0786, 1.0979]]),
        _ => None,
    }
}

/// Which pixels of the frame at `at` are outside BT.709, decoded by
/// ffmpeg at `width` by `height`, its primaries `primaries`.
pub fn read_gamut(path: &Path, at: f64, (width, height): (u32, u32), primaries: &str) -> Result<(Vec<bool>, f64), String> {
    let matrix = to_709(primaries).ok_or("not a wide gamut")?;
    let output = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{at:.3}")])
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-vf", &format!("scale={width}:{height},zscale=t=linear:npl=100,format=gbrpf32le"), "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("ffmpeg: {error}"))?;
    let pixels = (width * height) as usize;
    if output.stdout.len() != pixels * 12 {
        return Err("ffmpeg gave no frame".into());
    }
    // Planes of 32-bit floats, in the order G, B, R.
    let plane = |index: usize| &output.stdout[index * pixels * 4..(index + 1) * pixels * 4];
    let value = |plane: &[u8], at: usize| f32::from_le_bytes([plane[at * 4], plane[at * 4 + 1], plane[at * 4 + 2], plane[at * 4 + 3]]);
    let (g, b, r) = (plane(0), plane(1), plane(2));
    let outside: Vec<bool> = (0..pixels)
        .into_par_iter()
        .map(|at| {
            let rgb = [value(r, at), value(g, at), value(b, at)];
            // Too dark to tell from noise.
            if rgb.iter().copied().fold(0.0, f32::max) < 0.01 {
                return false;
            }
            // A channel below nothing in BT.709 is a color it cannot make;
            // a little below is rounding, not the picture.
            matrix.iter().any(|row| row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] < -0.005)
        })
        .collect();
    let share = outside.iter().filter(|outside| **outside).count() as f64 / pixels.max(1) as f64;
    Ok((outside, share))
}

impl App {
    /// Turns a check on or off.
    pub(super) fn toggle_check(&mut self, kind: usize) {
        let Some(on) = self.checks.get_mut(kind) else { return };
        *on = !*on;
        let on = *on;
        if kind == 2 {
            self.gamut = None;
            if on && self.wide_primaries().is_none() {
                self.toast("Not wide gamut · nothing outside BT.709");
            }
        }
        self.apply_checks();
        self.tend_gamut();
        self.redraw();
    }

    /// The checks the players draw, told to each.
    pub(super) fn apply_checks(&self) {
        let checks = Checks { zebra: self.checks[0], false_color: self.checks[1] };
        if let Some(player) = &self.player {
            player.set_checks(checks);
        }
        if let Some(spare) = &self.spare {
            spare.player.set_checks(checks);
        }
        // The frame kept for redrawing is without them, or with.
        self.video_kept.set(None);
    }

    /// The primaries of the video on view, when they are wider than
    /// BT.709's.
    fn wide_primaries(&self) -> Option<String> {
        let player = self.player.as_ref()?;
        player.text("video-params/primaries").filter(|primaries| to_709(primaries).is_some())
    }

    /// Paused on a wide-gamut video with the check on: what is outside
    /// BT.709 on the frame, asked for if it is not known. On another
    /// thread: decoding a frame again takes a moment.
    pub(super) fn tend_gamut(&mut self) {
        if !self.checks[2] {
            return;
        }
        let Some(video) = self.video.as_ref().filter(|video| video.showing) else { return };
        let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
        let Some((at, _)) = self.video_time().filter(|_| paused) else { return };
        let Some(primaries) = self.wide_primaries() else { return };
        let id = video.id;
        let known = self.gamut.as_ref().is_some_and(|mask| mask.id == id && (mask.at - at).abs() < 0.02);
        if known || self.gamut_asked.is_some_and(|(asked, when)| asked == id && (when - at).abs() < 0.02) {
            return;
        }
        let native = self.size_of(id).unwrap_or((16, 9));
        let width = MASK_WIDTH.min(native.0.max(2)) / 2 * 2;
        let height = ((f64::from(width) * f64::from(native.1) / f64::from(native.0.max(1))).round() as u32 / 2 * 2).max(2);
        self.gamut_asked = Some((id, at));
        let (path, proxy) = (self.files[id].clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let read = read_gamut(&path, at, (width, height), &primaries).map(|(outside, share)| GamutMask { id, at, width, height, outside, share });
            let _ = proxy.send_event(Event::Gamut(read));
        });
    }

    pub(super) fn gamut_read(&mut self, mask: Result<GamutMask, String>) {
        self.gamut_asked = None;
        match mask {
            Ok(mask) => {
                let share = mask.share;
                self.gamut = Some(mask);
                self.video_kept.set(None);
                self.toast(match share {
                    0.0 => "Nothing outside BT.709 on this frame".to_string(),
                    share if share < 0.001 => "Outside BT.709 · a trace of this frame".to_string(),
                    share => format!("Outside BT.709 · {:.1}% of this frame", share * 100.0),
                });
                self.redraw();
            }
            Err(error) => self.toast(format!("No gamut check · {error}")),
        }
    }

    /// The frame paused on, with what is outside BT.709 tinted over it;
    /// nothing while it plays, or of another frame.
    pub(super) fn draw_gamut(&self, pixels: &mut [u32], window: (u32, u32)) {
        if !self.checks[2] {
            return;
        }
        let Some(mask) = &self.gamut else { return };
        let Some(video) = self.video.as_ref().filter(|video| video.id == mask.id) else { return };
        let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
        let at = self.video_time().map(|(at, _)| at);
        if !paused || at.is_none_or(|at| (at - mask.at).abs() > 0.05) {
            return;
        }
        let native = self.size_of(video.id).unwrap_or((mask.width, mask.height));
        let layout = self.layout_of(native);
        let (w, h) = (window.0 as i32, window.1 as i32);
        let (x0, y0) = (layout.x.max(0), layout.y.max(0));
        let (x1, y1) = ((layout.x + layout.width as i32).min(w), (layout.y + layout.height as i32).min(h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        pixels.par_chunks_mut(w as usize).enumerate().skip(y0 as usize).take((y1 - y0) as usize).for_each(|(y, row)| {
            let my = ((y as i32 - layout.y) as i64 * i64::from(mask.height) / i64::from(layout.height.max(1))).clamp(0, i64::from(mask.height) - 1) as usize;
            for x in x0..x1 {
                let mx = ((x - layout.x) as i64 * i64::from(mask.width) / i64::from(layout.width.max(1))).clamp(0, i64::from(mask.width) - 1) as usize;
                if mask.outside[my * mask.width as usize + mx] {
                    // Magenta, two parts in three over the picture.
                    let pixel = &mut row[x as usize];
                    let blend = |shift: u32, toward: u32| ((*pixel >> shift & 0xFF) / 3 + toward * 2 / 3) << shift;
                    *pixel = blend(16, 0xFF) | blend(8, 0x00) | blend(0, 0xFF);
                }
            }
        });
    }

    /// The Color tab's checks on the picture, as toggles.
    pub(super) fn check_actions(&self) -> Vec<(String, Vec<Action>)> {
        let mark = |on: bool| if on { "●" } else { "○" };
        let mut rows = vec![
            Action { words: format!("{}  Zebra · clipped highlights", mark(self.checks[0])), hit: Hit::Run(Command::Check(0)) },
            Action { words: format!("{}  False color · exposure", mark(self.checks[1])), hit: Hit::Run(Command::Check(1)) },
        ];
        if self.wide_primaries().is_some() {
            rows.push(Action { words: format!("{}  Outside BT.709 · on the frame paused on", mark(self.checks[2])), hit: Hit::Run(Command::Check(2)) });
        }
        vec![("Checks on the picture".to_string(), rows)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bt2020_primaries_are_outside_bt709_and_white_is_not() {
        let m = to_709("bt2020").unwrap();
        let into = |rgb: [f32; 3]| m.map(|row| row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]);
        assert!(into([0.0, 1.0, 0.0]).iter().any(|c| *c < 0.0), "BT.2020's green is beyond BT.709's");
        let white = into([1.0, 1.0, 1.0]);
        assert!(white.iter().all(|c| (c - 1.0).abs() < 0.01), "white stays white: {white:?}");
        assert!(to_709("bt709").is_none() && to_709("bt.709").is_none());
        assert!(to_709("bt.2020").is_some(), "as mpv names it");
    }
}
