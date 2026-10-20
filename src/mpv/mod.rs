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
mod engine;
mod helper;
mod host;
mod wire;

use std::sync::atomic::AtomicBool;

pub use crashes::{crashed_on, Crash};
pub use helper::main as player_main;
pub use host::Player;

/// The argument that starts jog as a player process rather than as jog.
pub const PLAYER_ARG: &str = "--player";

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

/// The properties watched, with the kind each is. Their places here, from
/// 1, are what mpv tags the change events with, and how the player
/// process tells jog's of a change.
pub(crate) const WATCHED: &[(&str, Kind)] = &[
    ("eof-reached", Kind::Flag),
    ("time-pos", Kind::Number),
    ("duration", Kind::Number),
    ("pause", Kind::Flag),
    ("seeking", Kind::Flag),
    ("vo-configured", Kind::Flag),
    ("path", Kind::Text),
    ("volume", Kind::Number),
    ("track-list/count", Kind::Number),
    ("video-format", Kind::Text),
    ("video-params/gamma", Kind::Text),
    ("video-params/primaries", Kind::Text),
    ("container-fps", Kind::Number),
    ("frame-drop-count", Kind::Number),
    ("vo-drop-frame-count", Kind::Number),
];

pub(crate) fn watched(property: &str) -> Option<usize> {
    WATCHED.iter().position(|(name, _)| *name == property)
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
