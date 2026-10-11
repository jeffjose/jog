//! The scenes of a video: where its picture changes to something else,
//! and a frame from each, to get about in it by what is to be seen.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

use crate::frame::Frame;
use crate::story::{self, is_flat, segment_at};

/// How unlike the one before a keyframe has to be, from 0 to 1, to begin a
/// scene.
const CHANGE: f64 = 0.1;
/// Scenes shorter than this many seconds are taken with the one before.
const SHORTEST: f64 = 2.0;
/// No more scenes than this are kept: a strip of hundreds is no help.
const MOST: usize = 60;
/// A video with no changes to be found is cut into this many equal parts.
const PARTS: usize = 8;

/// What the thread finding scenes reports.
pub enum Found {
    /// When each scene of a video starts, in seconds.
    Starts(usize, Vec<f64>),
    /// A frame from the scene at this place among them.
    Frame(usize, usize, Frame),
}

/// Finds scenes on a thread of its own, one video at a time. Asking for
/// another video drops what is left of the one before.
pub struct Finder {
    jobs: mpsc::Sender<(usize, PathBuf, Vec<f64>)>,
    wanted: Arc<AtomicUsize>,
}

impl Finder {
    /// `report` is called from the finding thread; it returns whether
    /// anyone is still listening.
    pub fn new(report: impl Fn(Found) -> bool + Send + 'static) -> Finder {
        let (jobs, queue) = mpsc::channel::<(usize, PathBuf, Vec<f64>)>();
        let wanted = Arc::new(AtomicUsize::new(usize::MAX));
        let latest = wanted.clone();
        std::thread::spawn(move || {
            for (id, path, chapters) in queue {
                let current = || latest.load(Ordering::Relaxed) == id;
                if !current() {
                    continue;
                }
                // A video that comes in chapters has said where its scenes are.
                let starts = if chapters.len() > 1 { chapters } else { starts(&path) };
                if !report(Found::Starts(id, starts.clone())) {
                    return;
                }
                for (index, start) in starts.into_iter().enumerate() {
                    if !current() {
                        break;
                    }
                    if let Some(frame) = frame_of(&path, start) {
                        report(Found::Frame(id, index, frame));
                    }
                }
            }
        });
        Finder { jobs, wanted }
    }

    /// Asks for the scenes of video `id`, which are its chapters if it has
    /// any: `chapters` is when each of those starts.
    pub fn want(&self, id: usize, path: PathBuf, chapters: Vec<f64>) {
        self.wanted.store(id, Ordering::Relaxed);
        let _ = self.jobs.send((id, path, chapters));
    }
}

/// When each scene of a video starts. Only its keyframes are looked at,
/// which takes a small part of the time decoding all of it would, and an
/// encoder puts one where the picture changes anyway.
fn starts(path: &Path) -> Vec<f64> {
    let filter = format!("scale=160:-2,select='eq(n\\,0)+gt(scene\\,{CHANGE})',showinfo");
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-loglevel", "info", "-skip_frame", "nokey", "-i"])
        .arg(path)
        .args(["-an", "-vf", &filter, "-vsync", "vfr", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output();
    let said = output.map(|output| String::from_utf8_lossy(&output.stderr).into_owned()).unwrap_or_default();
    let found = spaced(said.lines().filter_map(shown_at).collect());
    if found.len() > 1 {
        return found;
    }
    // One long take: equal parts are better than nothing to go by.
    let length = story::length(path).unwrap_or(0.0);
    (0..PARTS).map(|part| length * part as f64 / PARTS as f64).collect()
}

/// The time on a line the `showinfo` filter wrote about a frame.
fn shown_at(line: &str) -> Option<f64> {
    let (_, rest) = line.split_once("pts_time:")?;
    rest.split_whitespace().next()?.parse().ok()
}

/// The starts that are far enough after the one before, the first always
/// at the very beginning, and thinned out evenly if there are too many.
fn spaced(times: Vec<f64>) -> Vec<f64> {
    let mut kept: Vec<f64> = vec![0.0];
    for time in times {
        if time - kept[kept.len() - 1] >= SHORTEST {
            kept.push(time);
        }
    }
    let step = kept.len().div_ceil(MOST);
    kept.into_iter().step_by(step).collect()
}

/// A frame to stand for the scene starting at `start`: its first, or one
/// from a second in when it opens on a single colour, as a fade does.
fn frame_of(path: &Path, start: f64) -> Option<Frame> {
    // A little way in, so that the keyframe found is this scene's own.
    let first = segment_at(path, start + 0.1).pop()?;
    if !is_flat(&first.pixels) {
        return Some(first);
    }
    segment_at(path, start + 1.0).pop().or(Some(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_from_what_ffmpeg_says() {
        let line = "[Parsed_showinfo_2 @ 0x55] n:   3 pts:  48048 pts_time:12.012  duration: 1 fmt:yuv420p";
        assert_eq!(shown_at(line), Some(12.012));
        assert_eq!(shown_at("frame=   10 fps=0.0 q=-0.0 size=N/A"), None);
    }

    #[test]
    fn scenes_start_at_the_beginning_and_are_not_too_short() {
        assert_eq!(spaced(vec![0.0, 0.5, 4.0, 5.0, 9.5]), [0.0, 4.0, 9.5]);
        assert_eq!(spaced(vec![3.0]), [0.0, 3.0]);
        assert_eq!(spaced(Vec::new()), [0.0]);
        // Too many are thinned out evenly, keeping the first.
        let many = spaced((0..300).map(|n| f64::from(n) * 10.0).collect());
        assert!(many.len() <= MOST && many[0] == 0.0);
    }
}
