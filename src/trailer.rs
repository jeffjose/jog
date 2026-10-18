//! The grid's trailer of a video, played on a thread of its own. Starting
//! a large file, seeking in it and scaling its frames all take a while, and
//! none of that may hold up moving about the grid.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::frame::{fit_size, Frame};
use crate::mpv::{Player, PlayerEvent, Role, Signal};

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
    /// Play this video, in frames fitting these bounds: as a trailer, or
    /// straight on from a number of seconds in.
    Play(usize, PathBuf, (u32, u32), Option<f64>),
    Stop,
    Player(Signal),
}

/// The video being played and where its trailer has got to.
struct Playing {
    id: usize,
    bounds: (u32, u32),
    /// Where it is played on from, when it is not a trailer of the whole.
    from: Option<f64>,
    length: Option<f64>,
    stretch: usize,
    next_at: Option<Instant>,
    /// How many times the player has begun showing this video: once on
    /// opening it, and again after each move to another stretch.
    restarts: usize,
    /// Whether its frames are ones to show. Until the player has opened it
    /// they are of the video before, and in a longer video, until the first
    /// stretch is reached, they are of its opening, which is often black.
    showing: bool,
}

/// Reads the player's events, noting when the video last asked for has
/// begun to show. `opening` counts the files asked for that have not been
/// started on yet: frames only count once none are left.
fn heed(player: &Player, opening: &mut usize, playing: &mut Option<Playing>) {
    for event in player.events() {
        match event {
            PlayerEvent::Started => *opening = opening.saturating_sub(1),
            PlayerEvent::Showing if *opening == 0 => {
                if let Some(playing) = playing {
                    playing.restarts += 1;
                }
            }
            _ => {}
        }
    }
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
            let wake = move |signal| drop(signals.lock().unwrap_or_else(std::sync::PoisonError::into_inner).send(Message::Player(signal)));
            // Without a player there are no trailers; the grid does without.
            let Ok(player) = Player::new(Role::Preview, wake) else { return };
            player.set("loop-file", "inf");
            let mut playing: Option<Playing> = None;
            let mut opening = 0;
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
                        while let Ok(later) = inbox.try_recv() {
                            match later {
                                Message::Player(_) => {}
                                ask => newest = Some(ask),
                            }
                        }
                        // Whatever the player said meanwhile was about the
                        // video before.
                        heed(&player, &mut opening, &mut playing);
                        playing = match newest {
                            Some(Message::Play(id, path, bounds, from)) => {
                                opening += 1;
                                match from {
                                    Some(start) => {
                                        use std::os::unix::ffi::OsStrExt;
                                        let start = format!("start={start:.3}");
                                        player.command_later(&[b"loadfile", path.as_os_str().as_bytes(), b"replace", start.as_bytes()]);
                                    }
                                    None => player.open(&path),
                                }
                                player.set("pause", "no");
                                Some(Playing { id, bounds, from, length: None, stretch: 0, next_at: None, restarts: 0, showing: false })
                            }
                            _ => {
                                player.set("pause", "yes");
                                player.command_later(&[b"stop"]);
                                *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                                None
                            }
                        };
                    }
                    Ok(Message::Player(Signal::Events)) => heed(&player, &mut opening, &mut playing),
                    Ok(Message::Player(Signal::Frame)) => {
                        // mpv stalls unless each frame it announces is taken,
                        // wanted or not: with nothing playing, or while the
                        // frame is still one of the video before.
                        let Some(playing) = playing.as_ref().filter(|playing| playing.showing) else {
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
                            let waiting = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).replace((playing.id, frame)).is_some();
                            if !waiting && !notify(Note::Frame) {
                                return;
                            }
                        }
                    }
                }
                // A longer video moves on from stretch to stretch, each from
                // the middle of its part: the very start is often black.
                let Some(playing) = &mut playing else { continue };
                // Before it has opened, the player still speaks of the
                // video before.
                if playing.restarts == 0 {
                    continue;
                }
                // Played on from somewhere, it has nothing more to do: the
                // player opened it there.
                if playing.from.is_some() {
                    playing.showing = true;
                    continue;
                }
                if playing.length.is_none() {
                    playing.length = player.number("duration");
                    if let Some(length) = playing.length {
                        notify(Note::Length(playing.id, length));
                    }
                }
                let Some(length) = playing.length.filter(|length| *length > WHOLE) else {
                    playing.showing = true;
                    continue;
                };
                playing.showing = playing.restarts >= 2;
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

    /// Plays video `id`, dropping whatever was playing: as a trailer, or
    /// with `from` straight on from that many seconds in.
    pub fn play(&self, id: usize, path: PathBuf, bounds: (u32, u32), from: Option<f64>) {
        let _ = self.messages.send(Message::Play(id, path, bounds, from));
    }

    pub fn stop(&self) {
        let _ = self.messages.send(Message::Stop);
    }

    /// The newest frame, if one has come since the last was taken.
    pub fn take(&self) -> Option<(usize, Frame)> {
        self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }
}
