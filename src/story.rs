//! Frames from along a video: a quick look through it without playing it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

use crate::frame::Frame;

/// How many frames are taken, evenly spaced through the video.
pub const FRAMES: usize = 10;
/// The frames are shrunk to fit this: enough for a grid cell, and small
/// enough that those of several videos can be kept.
const BOUNDS: (u32, u32) = (480, 270);

/// What the thread fetching frames reports.
pub enum Found {
    /// How long the video is, in seconds.
    Length(usize, Option<f64>),
    /// The frame at one of the `FRAMES` places along the video.
    Frame(usize, usize, Frame),
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
                    // The middle of each tenth: the very start is often black.
                    let at = length.unwrap_or(0.0) * (index as f64 + 0.5) / FRAMES as f64;
                    if let Some(frame) = frame_at(&path, at) {
                        report(Found::Frame(id, index, frame));
                    }
                }
                report(Found::Done(id));
            }
        });
        Stories { jobs, wanted }
    }

    /// Asks for the frames of video `id` at the places in `missing`.
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

/// The keyframe nearest before `seconds` into the video, shrunk. Stopping
/// at a keyframe saves decoding up to the exact moment, which matters little
/// here and can take most of a second in a large video.
fn frame_at(path: &Path, seconds: f64) -> Option<Frame> {
    let scale = format!("scale={}:{}:force_original_aspect_ratio=decrease", BOUNDS.0, BOUNDS.1);
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "1", "-noaccurate_seek", "-ss", &format!("{seconds:.3}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", &scale, "-f", "image2pipe", "-c:v", "bmp", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let image = image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Bmp).ok()?.into_rgb8();
    let (width, height) = image.dimensions();
    let pixels = image.as_raw().chunks_exact(3).map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]));
    Some(Frame::still(width, height, (width, height), pixels.collect()))
}
