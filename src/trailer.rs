//! The grid's trailer of a video, played on a thread of its own. Starting
//! a large file, seeking in it and scaling its frames all take a while, and
//! none of that may hold up moving about the grid.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::frame::{fit_size, Frame};
use crate::mpv::{Player, Role, Signal};

/// How many stretches of a longer video are shown, and for how long each.
/// A video no longer than `WHOLE` seconds is simply played through.
const STRETCHES: usize = 8;
const STRETCH: Duration = Duration::from_millis(2500);
const WHOLE: f64 = 20.0;

/// What the trailer thread has to tell.
pub enum Note {
    /// A frame is waiting to be taken.
    Frame,
    /// How long a video is, in seconds.
    Length(usize, f64),
}

enum Message {
    /// Play this video, in frames fitting these bounds.
    Play(usize, PathBuf, (u32, u32)),
    Stop,
    Player(Signal),
}

/// The video being played and where its trailer has got to.
struct Playing {
    id: usize,
    bounds: (u32, u32),
    length: Option<f64>,
    stretch: usize,
    next_at: Option<Instant>,
}

pub struct Trailers {
    messages: mpsc::Sender<Message>,
    /// The newest frame not yet taken, and the video it is of. A frame
    /// that is not taken in time is replaced, not queued behind.
    latest: Arc<Mutex<Option<(usize, Frame)>>>,
}

impl Trailers {
    /// `notify` is called from the trailer thread; it returns whether
    /// anyone is still listening.
    pub fn new(notify: impl Fn(Note) -> bool + Send + 'static) -> Trailers {
        let (messages, inbox) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let (signals, slot) = (Mutex::new(messages.clone()), latest.clone());
        std::thread::spawn(move || {
            let wake = move |signal| drop(signals.lock().unwrap().send(Message::Player(signal)));
            // Without a player there are no trailers; the grid does without.
            let Ok(player) = Player::new(Role::Preview, wake) else { return };
            player.set("loop-file", "inf");
            let mut playing: Option<Playing> = None;
            // Frames are rendered into this when nobody wants them.
            let mut spare = Vec::new();
            loop {
                let wait = playing.as_ref().and_then(|playing| playing.next_at).map(|at| at.saturating_duration_since(Instant::now()));
                let message = match wait {
                    Some(wait) => inbox.recv_timeout(wait),
                    None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match message {
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(Message::Play(..) | Message::Stop) => {
                        // Only the last of a run of requests matters: the
                        // cursor may have crossed several videos by now.
                        let mut newest = message.ok();
                        let mut signals = Vec::new();
                        while let Ok(later) = inbox.try_recv() {
                            match later {
                                Message::Player(signal) => signals.push(signal),
                                ask => newest = Some(ask),
                            }
                        }
                        playing = match newest {
                            Some(Message::Play(id, path, bounds)) => {
                                player.open(&path);
                                player.set("pause", "no");
                                Some(Playing { id, bounds, length: None, stretch: 0, next_at: None })
                            }
                            _ => {
                                player.set("pause", "yes");
                                player.command_later(&[b"stop"]);
                                *slot.lock().unwrap() = None;
                                None
                            }
                        };
                        if !signals.is_empty() {
                            player.events();
                        }
                    }
                    // Only to empty the queue, so that mpv keeps signalling.
                    Ok(Message::Player(Signal::Events)) => drop(player.events()),
                    Ok(Message::Player(Signal::Frame)) => {
                        // mpv stalls unless each frame it announces is taken.
                        let Some(playing) = &playing else {
                            spare.resize(16 * 9, 0);
                            player.render(&mut spare, 16, 9);
                            continue;
                        };
                        let size = player.number("dwidth").zip(player.number("dheight"));
                        let native = size.map_or((16, 9), |(w, h)| ((w as u32).max(1), (h as u32).max(1)));
                        let (width, height) = fit_size(native, playing.bounds, true);
                        let mut pixels = vec![0; width as usize * height as usize];
                        if player.render(&mut pixels, width, height) {
                            let frame = Frame::still(width, height, native, pixels);
                            let waiting = slot.lock().unwrap().replace((playing.id, frame)).is_some();
                            if !waiting && !notify(Note::Frame) {
                                return;
                            }
                        }
                    }
                }
                // A longer video moves on from stretch to stretch, each from
                // the middle of its part: the very start is often black.
                let Some(playing) = &mut playing else { continue };
                if playing.length.is_none() {
                    playing.length = player.number("duration");
                    if let Some(length) = playing.length {
                        notify(Note::Length(playing.id, length));
                    }
                }
                let Some(length) = playing.length.filter(|length| *length > WHOLE) else { continue };
                if playing.next_at.is_none_or(|at| at <= Instant::now()) {
                    let at = length * (playing.stretch as f64 + 0.5) / STRETCHES as f64;
                    player.command_later(&[b"seek", at.to_string().as_bytes(), b"absolute+keyframes"]);
                    playing.stretch = (playing.stretch + 1) % STRETCHES;
                    playing.next_at = Some(Instant::now() + STRETCH);
                }
            }
        });
        Trailers { messages, latest }
    }

    /// Plays video `id`, dropping whatever was playing.
    pub fn play(&self, id: usize, path: PathBuf, bounds: (u32, u32)) {
        let _ = self.messages.send(Message::Play(id, path, bounds));
    }

    pub fn stop(&self) {
        let _ = self.messages.send(Message::Stop);
    }

    /// The newest frame, if one has come since the last was taken.
    pub fn take(&self) -> Option<(usize, Frame)> {
        self.latest.lock().unwrap().take()
    }
}
