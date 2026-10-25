//! Video playback, by libmpv in a process of its own.
//!
//! mpv decodes, keeps time and plays the audio, and draws each frame in
//! software into memory jog shares with it; jog lays its overlays on that
//! as on a picture. mpv runs in a player process, one for each `Player`:
//! jog started again as `jog --player`, which loads libmpv and does as
//! jog's process asks over a socket (`wire`). A crash in what mpv uses,
//! ffmpeg or a GPU driver, so takes down that process and not jog: the
//! `Player` says so with `PlayerEvent::Crashed`, the file it had open is
//! noted (`crashes`), and jog starts another to carry on with.
//!
//! - `engine`: libmpv itself, in the player process.
//! - `helper`: the player process's side: the engine, asked over the socket.
//! - `host`: jog's side, `Player`: the same calls as the engine has.
//! - `wire`: the socket, what goes over it, and the shared memory.
//! - `crashes`: files a player has crashed on, kept from run to run.

mod crashes;
mod checks;
mod engine;
mod helper;
mod host;
mod processes;
mod wire;

use std::sync::atomic::AtomicBool;

pub use checks::Checks;
pub use crashes::{all_crashes, crashed_on, forget_crash, Crash};
pub use helper::main as player_main;
pub use host::Player;
pub use processes::{crashes_this_run, usage};

/// The argument that starts jog as a player process rather than as jog.
pub const PLAYER_ARG: &str = "--player";

/// Whether videos are drawn by the player on the GPU, into a window of
/// their own inside jog's, as mpv draws its own: decoded and scaled on the
/// GPU, nothing copied back. Otherwise the player draws each frame in
/// software for jog to copy into its window. Set once, at the start: by
/// jog from its options and the display it has, and in a player process
/// from what jog told it.
static GPU_VIDEO: AtomicBool = AtomicBool::new(false);

pub fn gpu_video() -> bool {
    GPU_VIDEO.load(std::sync::atomic::Ordering::Relaxed)
}

/// jog's side: drawn on the GPU when asked, on an X11 display, which has
/// windows inside windows to draw into; never on Wayland, nor in a
/// scripted run, whose pictures are of jog's window alone.
pub fn choose_gpu_video(wanted: bool, scripted: bool) {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()) && std::env::var("WINIT_UNIX_BACKEND").map_or(true, |backend| backend != "x11");
    let x11 = std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty());
    GPU_VIDEO.store(wanted && x11 && !wayland && !scripted, std::sync::atomic::Ordering::Relaxed);
}

/// How a playing video is decoded on the GPU: left there to be drawn, or
/// copied back for the software renderer.
pub fn gpu_decoding() -> &'static str {
    if gpu_video() { "auto" } else { "auto-copy" }
}

/// What a player is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Playing a video. `silent` discards the audio; `gpu` has it decoded
    /// by the GPU, unless `--no-hwdec`.
    Playback { silent: bool, gpu: bool },
    /// Fetching frames to show while the pointer is over the seek bar.
    Preview,
}

impl Role {
    /// As the player process is told it on its command line.
    fn word(self) -> &'static str {
        match self {
            Role::Playback { silent: false, gpu: false } => "play",
            Role::Playback { silent: false, gpu: true } => "play-gpu",
            Role::Playback { silent: true, gpu: false } => "silent",
            Role::Playback { silent: true, gpu: true } => "silent-gpu",
            Role::Preview => "preview",
        }
    }

    fn from_word(word: &str) -> Option<Role> {
        [Role::Playback { silent: false, gpu: false }, Role::Playback { silent: false, gpu: true }, Role::Playback { silent: true, gpu: false }, Role::Playback { silent: true, gpu: true }, Role::Preview]
            .into_iter()
            .find(|role| role.word() == word)
    }

    /// Whether its videos may be decoded by the GPU at all.
    fn gpu(self) -> bool {
        matches!(self, Role::Playback { gpu: true, .. })
    }
}

/// Why the player wants attention. Sent from a thread of jog's that
/// listens to the player process.
#[derive(Clone, Copy, Debug)]
pub enum Signal {
    /// Player events are waiting to be read.
    Events,
    /// A new video frame is due on screen.
    Frame,
}

/// What happened in the player since it was last asked.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PlayerEvent {
    Started,
    /// Frames of the current file can now be rendered.
    Showing,
    Ended,
    /// Playback ran into the end of the file, where it holds the last frame.
    AtEnd,
    /// A watched property has a new value.
    Changed,
    /// The time along has moved on, which it does with every frame.
    Moved,
    /// A command sent with `command_tagged` is done, by its tag: whether
    /// it worked.
    Replied(u64, bool),
    /// The player process is gone: it crashed, or stopped answering and
    /// was ended. Nothing more will come from this player; what it had
    /// open is noted in `crashes`, and another is to be started.
    Crashed,
}

/// Whether reading a property that takes a while is reported on the
/// terminal: every read waits for the player's core, which is busy for a
/// good while when it opens a file.
pub static DEBUG: AtomicBool = AtomicBool::new(false);

/// Whether videos played are decoded by the GPU, where it can: see
/// `engine::Engine::new`.
pub static HWDEC: AtomicBool = AtomicBool::new(true);

/// The kind of value a watched property has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Flag,
    Number,
    Text,
}

/// A property's value as last told of.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Known {
    Flag(bool),
    Number(f64),
    Text(String),
}

/// Whether a watched property's change is told of as `PlayerEvent::Changed`,
/// which has jog draw again; a quiet one is only kept, for when it is next
/// read. One that changes with every frame and is only shown in passing,
/// in the info overlay, is quiet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tell {
    Loud,
    Quiet,
}

/// The properties watched, with the kind each is. Their places here, from
/// 1, are what mpv tags the change events with, and how the player
/// process tells jog's of a change. Every property jog reads while it
/// draws is here, so that reading it never waits on the player: see
/// `drawing`.
pub(crate) const WATCHED: &[(&str, Kind, Tell)] = &[
    ("eof-reached", Kind::Flag, Tell::Loud),
    ("time-pos", Kind::Number, Tell::Loud),
    ("duration", Kind::Number, Tell::Loud),
    ("pause", Kind::Flag, Tell::Loud),
    ("seeking", Kind::Flag, Tell::Loud),
    ("vo-configured", Kind::Flag, Tell::Loud),
    ("path", Kind::Text, Tell::Loud),
    ("volume", Kind::Number, Tell::Loud),
    ("track-list/count", Kind::Number, Tell::Loud),
    ("video-format", Kind::Text, Tell::Loud),
    ("video-params/gamma", Kind::Text, Tell::Loud),
    ("video-params/primaries", Kind::Text, Tell::Loud),
    ("container-fps", Kind::Number, Tell::Loud),
    ("frame-drop-count", Kind::Number, Tell::Loud),
    ("vo-drop-frame-count", Kind::Number, Tell::Loud),
    ("decoder-frame-drop-count", Kind::Number, Tell::Quiet),
    // The tracks and chapters as a whole, as JSON: read a property at a
    // time, they were a wait on the player for each of ten things about
    // each track, with every drawing of the Tracks tab.
    ("track-list", Kind::Text, Tell::Loud),
    ("chapter-list", Kind::Text, Tell::Loud),
    ("aid", Kind::Text, Tell::Loud),
    ("sid", Kind::Text, Tell::Loud),
    ("sub-delay", Kind::Number, Tell::Loud),
    ("audio-delay", Kind::Number, Tell::Loud),
    // What the info overlay shows of the video and how it plays.
    ("width", Kind::Number, Tell::Quiet),
    ("height", Kind::Number, Tell::Quiet),
    // Turned a quarter, as phones record upright video, its width and
    // height on screen are the other way round.
    ("video-params/rotate", Kind::Number, Tell::Quiet),
    ("video-params/pixelformat", Kind::Text, Tell::Quiet),
    ("video-bitrate", Kind::Number, Tell::Quiet),
    ("hwdec-current", Kind::Text, Tell::Quiet),
    ("file-format", Kind::Text, Tell::Quiet),
    ("estimated-vf-fps", Kind::Number, Tell::Quiet),
    ("audio-codec-name", Kind::Text, Tell::Quiet),
    ("audio-params/channel-count", Kind::Number, Tell::Quiet),
    ("audio-params/samplerate", Kind::Number, Tell::Quiet),
    ("audio-bitrate", Kind::Number, Tell::Quiet),
    ("avsync", Kind::Number, Tell::Quiet),
    ("demuxer-cache-duration", Kind::Number, Tell::Quiet),
];

pub(crate) fn watched(property: &str) -> Option<usize> {
    WATCHED.iter().position(|(name, ..)| *name == property)
}

thread_local! {
    /// Whether this thread is drawing the window just now.
    static DRAWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the time this thread is drawing the window, for as long as the
/// value lives: a player asked for something then, which waits on the
/// player, is told of on the terminal as a fault. Everything read while
/// drawing is to be watched (`WATCHED`), and so known without asking.
pub fn drawing() -> impl Drop {
    struct Drawing(bool);
    impl Drop for Drawing {
        fn drop(&mut self) {
            DRAWING.with(|drawing| drawing.set(self.0));
        }
    }
    Drawing(DRAWING.with(|drawing| drawing.replace(true)))
}

/// Whether this thread is drawing the window just now.
pub(crate) fn is_drawing() -> bool {
    DRAWING.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_read_as_written() {
        for role in [Role::Playback { silent: false, gpu: false }, Role::Playback { silent: false, gpu: true }, Role::Playback { silent: true, gpu: false }, Role::Playback { silent: true, gpu: true }, Role::Preview] {
            assert_eq!(Role::from_word(role.word()), Some(role));
        }
        assert_eq!(Role::from_word("nothing"), None);
    }
}
