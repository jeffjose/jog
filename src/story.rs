//! Short stretches from along a video: a quick look through it, in the
//! manner of a trailer, without playing it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

use crate::frame::Frame;

/// How many stretches are taken, evenly spaced through the video.
pub const SEGMENTS: usize = 8;
/// How long each stretch is, in seconds, and how many frames of it are
/// kept. Shown at the same rate, a stretch plays at its true speed.
const SEGMENT_SECONDS: f64 = 1.5;
pub const SEGMENT_FRAMES: usize = 12;
/// The frames are shrunk to fit this. A whole video's worth is some
/// thirty megabytes, so only those of a few videos can be kept.
const BOUNDS: (u32, u32) = (384, 216);

/// What the thread fetching frames reports.
pub enum Found {
    /// How long the video is, in seconds.
    Length(usize, Option<f64>),
    /// The frames of one of the `SEGMENTS` stretches. None, if the video
    /// shows nothing there but a flat colour.
    Segment(usize, usize, Vec<Frame>),
    /// The video has been gone through, or dropped for another.
    Done(usize),
}

/// Fetches frames on a thread of its own, one video at a time. Asking for
/// another video drops what is left of the one before.
pub struct Stories {
    jobs: mpsc::Sender<(usize, PathBuf, Vec<usize>)>,
    wanted: Arc<AtomicUsize>,
}

impl Stories {
    /// `report` is called from the fetching thread; it returns whether
    /// anyone is still listening.
    pub fn new(report: impl Fn(Found) -> bool + Send + 'static) -> Stories {
        let (jobs, queue) = mpsc::channel::<(usize, PathBuf, Vec<usize>)>();
        let wanted = Arc::new(AtomicUsize::new(usize::MAX));
        let latest = wanted.clone();
        std::thread::spawn(move || {
            for (id, path, missing) in queue {
                let current = || latest.load(Ordering::Relaxed) == id;
                if !current() {
                    continue;
                }
                let length = length(&path);
                if !report(Found::Length(id, length)) {
                    return;
                }
                for index in missing {
                    if !current() {
                        break;
                    }
                    // From the middle of each part: a video's very start
                    // and end are often titles or black.
                    let length = length.unwrap_or(0.0);
                    let at = (length * (index as f64 + 0.5) / SEGMENTS as f64).min((length - SEGMENT_SECONDS).max(0.0));
                    let frames = segment_at(&path, at).into_iter().filter(|frame| !is_flat(&frame.pixels)).collect();
                    report(Found::Segment(id, index, frames));
                }
                report(Found::Done(id));
            }
        });
        Stories { jobs, wanted }
    }

    /// Asks for the stretches of video `id` at the places in `missing`.
    pub fn want(&self, id: usize, path: PathBuf, missing: Vec<usize>) {
        self.wanted.store(id, Ordering::Relaxed);
        let _ = self.jobs.send((id, path, missing));
    }
}

/// How long a video is, in seconds, as ffprobe reads it.
pub fn length(path: &Path) -> Option<f64> {
    let arguments = ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"];
    let output = Command::new("ffprobe").args(arguments).arg(path).stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Whether a picture is all but one colour: the black, white or tinted
/// frame a video fades in from, which says nothing about what is in it.
/// A scattering of its pixels is enough to tell.
pub fn is_flat(pixels: &[u32]) -> bool {
    let step = (pixels.len() / 2000).max(1);
    let samples = || pixels.iter().step_by(step);
    let count = samples().count().max(1) as f64;
    (0..3).all(|channel| {
        let level = |pixel: &u32| f64::from(pixel >> (8 * channel) & 0xFF);
        let mean = samples().map(level).sum::<f64>() / count;
        let variance = samples().map(|pixel| (level(pixel) - mean).powi(2)).sum::<f64>() / count;
        variance.sqrt() < FLAT_SPREAD
    })
}

/// How little the levels of a colour may vary, on a scale of 256, for a
/// picture to count as flat.
const FLAT_SPREAD: f64 = 8.0;

/// The pictures in a run of BMP files end to end, as ffmpeg writes them
/// to a pipe.
pub fn bmp_frames(mut bytes: &[u8]) -> Vec<Frame> {
    let mut frames = Vec::new();
    // Each file says how long it is in the four bytes after its "BM".
    while bytes.len() > 6 {
        let size = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]) as usize;
        if size == 0 || size > bytes.len() {
            break;
        }
        let (file, rest) = bytes.split_at(size);
        bytes = rest;
        let Ok(image) = image::load_from_memory_with_format(file, image::ImageFormat::Bmp) else { break };
        let image = image.into_rgb8();
        let (width, height) = image.dimensions();
        let pixels = image.as_raw().chunks_exact(3).map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]));
        frames.push(Frame::still(width, height, (width, height), pixels.collect()));
    }
    frames
}

/// Frames from the stretch of the video starting near `seconds`, shrunk.
/// It starts at the keyframe before, which saves decoding up to the exact
/// moment: that matters little here and can take most of a second.
fn segment_at(path: &Path, seconds: f64) -> Vec<Frame> {
    let rate = SEGMENT_FRAMES as f64 / SEGMENT_SECONDS;
    let filter = format!("fps={rate},scale={}:{}:force_original_aspect_ratio=decrease", BOUNDS.0, BOUNDS.1);
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "2", "-noaccurate_seek", "-ss", &format!("{seconds:.3}")])
        .args(["-t", &SEGMENT_SECONDS.to_string(), "-i"])
        .arg(path)
        .args(["-an", "-frames:v", &SEGMENT_FRAMES.to_string(), "-vf", &filter, "-f", "image2pipe", "-c:v", "bmp", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    output.map(|output| bmp_frames(&output.stdout)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_colour_is_flat_and_a_picture_is_not() {
        assert!(is_flat(&vec![0x000000; 10_000]));
        assert!(is_flat(&vec![0xFFFFFF; 10_000]));
        // A red with a little noise in it, as a compressed fade has.
        let red: Vec<u32> = (0..10_000).map(|at| 0xC80000 + (at % 5) * 0x010101).collect();
        assert!(is_flat(&red));
        let stripes: Vec<u32> = (0..10_000).map(|at| if at / 100 % 2 == 0 { 0x202020 } else { 0xD0D0D0 }).collect();
        assert!(!is_flat(&stripes));
    }
}
