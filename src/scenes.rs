//! The scenes of a video: a frame from each of a number of equal parts of
//! it, to get about in it by what is to be seen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::frame::{fit_size, Frame};
use crate::mpv::{Player, PlayerEvent, Role, Signal};
use crate::story::{is_flat, segment_at};

/// How many equal parts a video is shown as.
pub const PARTS: usize = 10;
/// The frames are shrunk to fit this.
const BOUNDS: (u32, u32) = (480, 270);
/// How long the player is given to open a file or get to a place in it.
const PATIENCE: Duration = Duration::from_secs(3);

/// When each part of a video this long starts, in seconds.
pub fn starts(length: f64) -> Vec<f64> {
    (0..PARTS).map(|part| length * part as f64 / PARTS as f64).collect()
}

/// A frame from the part at this place among those of a video.
pub struct Found(pub usize, pub usize, pub Frame);

/// Fetches frames on a thread of its own, one video at a time. Asking for
/// another video drops what is left of the one before.
pub struct Finder {
    jobs: mpsc::Sender<(usize, PathBuf, Vec<f64>)>,
    wanted: Arc<AtomicUsize>,
}

impl Finder {
    /// `report` is called from the fetching thread; it returns whether
    /// anyone is still listening.
    pub fn new(report: impl Fn(Found) -> bool + Send + 'static) -> Finder {
        let (jobs, queue) = mpsc::channel::<(usize, PathBuf, Vec<f64>)>();
        let wanted = Arc::new(AtomicUsize::new(usize::MAX));
        let latest = wanted.clone();
        std::thread::spawn(move || {
            // A player of its own goes from place to place in a file many
            // times faster than starting ffmpeg for each frame does. Where
            // there is none, ffmpeg it is.
            let (signals, inbox) = mpsc::channel();
            let signals = Mutex::new(signals);
            let wake = move |signal| {
                let _ = signals.lock().unwrap_or_else(std::sync::PoisonError::into_inner).send(signal);
            };
            let player = Player::new(Role::Preview, wake).ok();
            for (id, path, starts) in queue {
                let current = || latest.load(Ordering::Relaxed) == id;
                if !current() {
                    continue;
                }
                let opened = player.as_ref().filter(|player| {
                    player.open(&path);
                    arrive(player, &inbox).is_some()
                });
                for (index, start) in starts.into_iter().enumerate() {
                    if !current() {
                        break;
                    }
                    let frame = match opened {
                        Some(player) => frame_at(player, &inbox, start),
                        None => segment_at(&path, start + 0.1).pop(),
                    };
                    if let Some(frame) = frame {
                        if !report(Found(id, index, frame)) {
                            return;
                        }
                    }
                }
            }
        });
        Finder { jobs, wanted }
    }

    /// Asks for a frame from each part of video `id`, which start at
    /// `starts`.
    pub fn want(&self, id: usize, path: PathBuf, starts: Vec<f64>) {
        self.wanted.store(id, Ordering::Relaxed);
        let _ = self.jobs.send((id, path, starts));
    }
}

/// A frame to stand for the part starting at `start`: the keyframe there,
/// or the frame a few seconds in when that is a single colour, as the
/// first frame of a video and of a fade often is.
fn frame_at(player: &Player, inbox: &Receiver<Signal>, start: f64) -> Option<Frame> {
    let go = |seconds: f64, how: &[u8]| {
        player.command_later(&[b"seek", format!("{seconds:.3}").as_bytes(), how]);
        arrive(player, inbox)
    };
    let first = go(start, b"absolute+keyframes")?;
    if !is_flat(&first.pixels) {
        return Some(first);
    }
    // To that very moment: the nearest keyframe is the one just tried.
    go(start + 3.0, b"absolute+exact").or(Some(first))
}

/// Waits for the player to have got where it was last sent, a file or a
/// place in one, and returns the frame it shows there. Every frame it
/// announces on the way is drawn, as it must be for the player to go on.
fn arrive(player: &Player, inbox: &Receiver<Signal>) -> Option<Frame> {
    let until = Instant::now() + PATIENCE;
    let (mut there, mut frame) = (false, None);
    while !(there && frame.is_some()) {
        match inbox.recv_timeout(until.saturating_duration_since(Instant::now())).ok()? {
            Signal::Events => there |= player.events().contains(&PlayerEvent::Showing),
            // Asked to draw again without a new frame, it has nothing to
            // show of where it is going.
            Signal::Frame if !player.frame_waiting() => {}
            Signal::Frame => {
                let size = player.number("dwidth").zip(player.number("dheight"));
                let native = size.map_or((16, 9), |(w, h)| ((w as u32).max(1), (h as u32).max(1)));
                let (width, height) = fit_size(native, BOUNDS, false);
                let mut pixels = vec![0; width as usize * height as usize];
                if player.render(&mut pixels, width, height) {
                    frame = Some(Frame::still(width, height, native, pixels));
                }
            }
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_is_cut_into_equal_parts_from_its_start() {
        let starts = starts(200.0);
        assert_eq!(starts.len(), PARTS);
        assert_eq!((starts[0], starts[1], starts[9]), (0.0, 20.0, 180.0));
    }
}
