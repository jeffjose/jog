use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{self, AtomicBool, AtomicU32};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, KeyLocation, ModifiersState, NamedKey};
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Fullscreen, Window, WindowId};

use crate::commands::{self, Applies, Command, MenuItem};
use crate::convert::{self, Format};
use crate::scopes;
use crate::shellpaths;
use crate::ultrahdr::{self, Hdr};
use crate::hdrmeta;
use crate::files::{self, Sort};
use crate::frame::{fit_size, Frame, Transform};
use crate::loader::{Job, Loader, Request};
use crate::meta::{self, Meta};
use crate::mpv::{Player, PlayerEvent, Role, Signal};
use crate::ops;
use crate::options::{LastSlide, Options, Start, SubtitleStyle};
use crate::positions::Positions;
use crate::scenes;
use crate::history;
use crate::sessions::{self, Session};
use crate::triage::{self, Triage, Verdict};
use crate::seen::Seen;
use crate::render;
use crate::rng::Rng;
use crate::search::{Facts, Query};
use crate::story::{self, Found, Stories};
use crate::trailer::{Note, Trailers};
use crate::ui::{self, Canvas, Text};

mod convert_panel;
use convert_panel::{ConvertPanel, PanelSource};
mod section;
use section::{Edge, EntryFrames, Section, StripTab};
mod back;
mod compare;
use crate::view::{Layout, View, Zoom};
use crate::x11;

const ZOOM_STEP: f64 = 1.25;
/// Thumbnails kept ready on each side of the current image.
const THUMB_RADIUS: i64 = 24;
/// How long after the last zoom or pan step the image is redrawn smoothly.
const SETTLE: Duration = Duration::from_millis(150);
/// Sources up to this many pixels are cheap enough to always scale smoothly.
const SMOOTH_ALWAYS: usize = 12_000_000;
/// Magnification of the source frame from which scaling stops being smoothed.
const SHOW_PIXELS_AT: u32 = 4;
const TOAST: Duration = Duration::from_millis(1500);
/// How long after a thumbnail comes in the grid or strip is drawn with
/// it: the ones coming in just behind it get into the same frame.
const THUMBS_REDRAW: Duration = Duration::from_millis(12);
const DEFAULT_SLIDE_DELAY: f64 = 3.0;
/// How often a slideshow checks whether the video it is waiting on is over.
const VIDEO_END_POLL: Duration = Duration::from_millis(200);
/// Seconds that `<` and `>` move through a video.
const SEEK_STEP: f64 = 5.0;
/// How long a window must keep its size before a video is drawn at it.
const RESIZE_SETTLE: Duration = Duration::from_millis(250);
/// Videos shorter than this many seconds always start from the beginning.
const RESUME_MIN_LENGTH: f64 = 60.0;
/// A video left within this many seconds of either end starts over.
const RESUME_MARGIN: f64 = 10.0;
/// How long to wait on the preview player before asking it again.
const PREVIEW_TIMEOUT: Duration = Duration::from_millis(400);
/// How soon a second click must follow the first to make a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// How long the grid takes to slide to another row, and the fewest and
/// most pictures it can be set to show across.
const GRID_SCROLL: Duration = Duration::from_millis(260);
const GRID_COLUMNS: (i32, i32) = (2, 12);
/// How long the grid's cursor must rest on a video before its trailer
/// begins.
const TRAILER_DELAY: Duration = Duration::from_millis(300);
/// How long a thumbnail in the grid takes to give way to its trailer.
const TRAILER_FADE: Duration = Duration::from_millis(350);
/// How many videos' frames are kept at once.
const STORIES_KEPT: usize = 4;
/// How often the directories are listed again while watching.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// The playback speeds that `[` and `]` step through.
const SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
/// How long after a seek the player's reported position cannot be trusted.
const SEEK_SETTLE: Duration = Duration::from_millis(500);
/// How close to its length, in seconds, a video counts as at its end.
const VIDEO_END: f64 = 0.25;
/// How far back past its start, in seconds, a seek has to aim to leave a
/// video for the file before it.
const VIDEO_START: f64 = 0.5;
/// How long a video has to have been playing to count as watched.
const SEEN_AFTER: Duration = Duration::from_secs(5);
/// How long after a video starts showing its tracks are asked about.
const TRACKS_AFTER: Duration = Duration::from_millis(300);
/// How long the window waits for the first image before it is shown.
const FIRST_FRAME_WAIT: Duration = Duration::from_millis(25);
/// A file no larger than this decodes within that wait, by and large.
const QUICK_FILE: u64 = 1500 << 10;
/// How long the first image serves as the window's background at most,
/// should the window manager never say the window is up.
const BACKDROP_AT_MOST: Duration = Duration::from_millis(500);
/// How long the small movements of the interface take: a toast sliding
/// in, the seek bar rising, a cursor going from one cell to the next.
const EASE: Duration = Duration::from_millis(170);
/// How far into its movement something is shown on the frame it sets out
/// on: one frame of a display that shows sixty a second.
const HEAD_START: Duration = Duration::from_millis(16);
/// How long a picture takes to grow out of its cell in the grid, or to
/// shrink back into it.
const GROW: Duration = Duration::from_millis(170);
/// How long the dot on a video takes to fade once it has been watched.
const DOT_FADE: Duration = Duration::from_millis(500);
/// How long the space bar or the mouse button has to be down to count as
/// held, and how many times its speed a video then plays at.
const HOLD_AFTER: Duration = Duration::from_millis(250);
const HOLD_SPEED: f64 = 2.0;
/// How far into a scene, in seconds, going back still means the one before.
const SCENE_RESTART: f64 = 1.5;
/// The usual size of subtitles and how far off the bottom edge they sit,
/// in pixels of a picture 720 high.
const SUBTITLE_SIZE: f64 = 44.0;
const SUBTITLE_MARGIN: i32 = 40;

/// How often the debug overlay asks the player for its figures.
const VIDEO_STATS_EVERY: Duration = Duration::from_millis(250);

const TEXT_COLOR: u32 = 0xEEEEEE;
const DIM_TEXT_COLOR: u32 = 0x9A9A9A;
const MENU_COLOR: u32 = 0x1E1E1E;
const MENU_EDGE_COLOR: u32 = 0x555555;
/// The colour of what is picked out, as `--accent` set it. It is read all
/// over the drawing code, which has no other tie to the options.
static ACCENT: AtomicU32 = AtomicU32::new(0x3E63DD);

fn accent() -> u32 {
    ACCENT.load(atomic::Ordering::Relaxed)
}

/// The colour for words on the accent: white, or near black on an accent
/// light enough that white would be lost on it.
fn on_accent() -> u32 {
    let channel = |shift: u32| f64::from(accent() >> shift & 0xFF);
    let brightness = 0.299 * channel(16) + 0.587 * channel(8) + 0.114 * channel(0);
    if brightness > 160.0 { 0x161616 } else { 0xFFFFFF }
}

/// The colours for the main and the lesser words of a row: on one that is
/// picked out, ones that read on the accent, the lesser a little towards
/// it; elsewhere the usual white and grey. Grey on the accent is mud.
fn row_text(selected: bool) -> (u32, u32) {
    if !selected {
        return (TEXT_COLOR, DIM_TEXT_COLOR);
    }
    let (on, accent) = (on_accent(), accent());
    let blend = |shift: u32| ((on >> shift & 0xFF) * 72 + (accent >> shift & 0xFF) * 28) / 100 << shift;
    (on, blend(16) | blend(8) | blend(0))
}

/// A lighter tint of the accent, for words and thin lines on the dark
/// panels, where the accent itself would be hard to read.
fn accent_soft() -> u32 {
    let toward_white = |shift: u32| {
        let channel = accent() >> shift & 0xFF;
        (channel + (255 - channel) * 45 / 100) << shift
    };
    toward_white(16) | toward_white(8) | toward_white(0)
}
const DANGER_COLOR: u32 = 0xD64545;
const PLACEHOLDER_COLOR: u32 = 0x2A2A2A;
const MARK_COLOR: u32 = 0xFFB020;
/// What a triage has kept is marked in this; what it has rejected, in
/// the colour of danger.
const KEPT_COLOR: u32 = 0x3DD68C;
const GRID_COLOR: u32 = 0x101010;

/// Files converted on another thread: those made, how many could not be,
/// the sizes before and after, and how the last was written.
pub struct Converted {
    created: Vec<PathBuf>,
    failed: usize,
    /// What they were converted to, in a word or two.
    what: String,
    before: u64,
    after: u64,
    how: Option<String>,
    /// Why the last that could not be converted could not.
    error: Option<String>,
    /// A section of a video saved, rather than files converted.
    saving: bool,
}

/// What background threads send to the event loop.
pub enum Event {
    Loaded(Job),
    Converted(Converted),
    /// The Convert panel's picture, decoded, and a try of its settings.
    ConvertSource(usize, Result<Arc<PanelSource>, String>),
    ConvertTrial(u64, Format, Result<convert::Trial, String>),
    ConvertScore(u64, Option<f64>),
    /// How far along converting videos is, in words.
    VideoProgress(String),
    /// A frame for a cell of the Sections tab.
    EntryFrame(scenes::Found),
    /// A signal from one of the two players that take turns playing: the
    /// number says which.
    Video(u8, Signal),
    Player(Result<Player, String>),
    /// The second of the two, which holds the next video ready.
    Spare(Result<Player, String>),
    /// The same pair for the player that fetches seek-bar previews.
    Preview(Signal),
    PreviewPlayer(Result<Player, String>),
    /// The frames before a pause, fetched for stepping back at once: whose
    /// they are, and how long they took.
    BackFrames(u64, VecDeque<(f64, Vec<u32>)>, Duration),
    /// Two pictures to compare, decoded; and how near the second is.
    CompareLoaded(u64, Result<[Arc<image::RgbImage>; 2], String>),
    CompareScore(u64, Option<f64>),
    /// File details read on another thread, for the info and EXIF overlays.
    Meta(usize, Meta),
    /// The font, loaded on another thread while the first image decodes.
    Font(Text),
    /// The files in the directories being watched, after a change.
    Listed(Vec<PathBuf>),
    /// How long a video is, in seconds, for the file list.
    Duration(usize, Option<f64>),
    /// A frame from along a video, or word about fetching them.
    Story(Found),
    /// The scenes of a video, found on another thread.
    Scenes(scenes::Found),
    /// Word from the thread playing the grid's trailer.
    Trailer(Note),
    /// The files of a workspace not yet gone to, listed for the overview:
    /// the workspace by the paths it is of, its place having maybe moved.
    SpaceListed(Vec<PathBuf>, Vec<PathBuf>),
    /// What a video says about its HDR, read on another thread.
    HdrInfo(usize, Option<Arc<hdrmeta::Info>>),
}

/// A file operation that `u` can take back.
enum Undo {
    /// Files written by a copy or a conversion.
    Created(Vec<PathBuf>),
    Moved(Vec<Moved>),
    Renamed { id: usize, from: PathBuf, to: PathBuf },
}

/// What a toast offers to be clicked: to undo what it tells of, or to go
/// to the file it made.
#[derive(Clone, PartialEq, Debug)]
enum ToastLink {
    Undo,
    Show(PathBuf),
    /// Stop the encode it tells of.
    Cancel,
}

/// Why the list is being brought up to date: the folders watched, a
/// refresh asked for, or files jog itself just made, which join quietly
/// and are not gone to.
#[derive(Clone, Copy, PartialEq)]
enum Taking {
    Watching,
    Refreshing,
    Adopting,
}

/// A file moved out of the list: where it sat and where it went.
struct Moved {
    id: usize,
    pos: usize,
    from: PathBuf,
    to: PathBuf,
}

/// A number on its way to another: where it set out from, where it is
/// going and when it set out. It gets there quickly at first and then
/// settles, which reads as prompt.
#[derive(Clone, Copy)]
struct Glide {
    from: f64,
    to: f64,
    since: Instant,
}

impl Glide {
    /// One that is at `value` already.
    fn at(value: f64) -> Glide {
        Glide { from: value, to: value, since: Instant::now() }
    }

    /// How far along it is, from 0 to 1, eased: quicker at first, but not
    /// so much so that the first frame takes a third of the way in one
    /// stride, which reads as a jerk and not as movement.
    fn part(&self, over: Duration) -> f64 {
        let part = (self.since.elapsed().as_secs_f64() / over.as_secs_f64().max(0.001)).min(1.0);
        1.0 - (1.0 - part).powi(2)
    }

    fn now(&self, over: Duration) -> f64 {
        self.from + (self.to - self.from) * self.part(over)
    }

    fn moving(&self, over: Duration) -> bool {
        self.from != self.to && self.since.elapsed() < over
    }

    /// Sets it off for somewhere else, from wherever it has got to.
    fn head_for(&mut self, to: f64, over: Duration) {
        if to != self.to {
            // It set out a frame ago, as far as the frame now being drawn
            // goes: that one already shows it on its way.
            let now = Instant::now();
            *self = Glide { from: self.now(over), to, since: now.checked_sub(HEAD_START).unwrap_or(now) };
        }
    }
}

/// The space bar or the mouse button being down over a video: since when,
/// which of the two it is, where the pointer was, and whether it has been
/// down long enough to hurry the video along.
struct Hold {
    since: Instant,
    by_key: bool,
    at: (f64, f64),
    fast: bool,
}

/// A picture between its cell in the grid and its place in the window:
/// whose it is, where the cell is, when it set out, and which way it goes.
struct Grow {
    id: usize,
    cell: [i32; 4],
    since: Instant,
    opening: bool,
    /// The frame a video was showing in its cell when it set out, which
    /// is nearer what it is growing into than its thumbnail is.
    picture: Option<Arc<Frame>>,
}

/// What happens when a video has played to its end.
#[derive(Clone, Copy, PartialEq)]
enum Looping {
    /// The next file plays, and the last one stays at its end.
    Off,
    /// It plays again.
    File,
    /// The next file plays, and after the last one the first.
    List,
}

/// The player that is not playing, and the video it has been given to
/// hold ready: which file, and how many seconds in.
struct Spare {
    player: Player,
    id: Option<usize>,
    from: f64,
}

/// The video the player has been given, and whether it has produced a
/// frame yet. Until then the still from the loader is shown.
struct Video {
    id: usize,
    showing: bool,
    /// When the player was handed the file.
    started: Instant,
    /// When the last seek was asked for and the time it was aiming at.
    seeking: Option<(Instant, f64)>,
    /// How long the player took to show its first frame.
    start_took: Option<Duration>,
    /// When the oldest seek not yet landed was asked for, and how long the
    /// last one that did land took.
    seek_asked: Option<Instant>,
    seek_took: Option<Duration>,
    /// Whether it was taken over from the spare player, which may have had
    /// its frame ready, and then says nothing more about it.
    held: bool,
    /// How many subtitle and audio tracks it has, once that has been
    /// asked: not while it starts, when asking waits on the player.
    tracks: Option<(usize, usize)>,
    /// When it first showed, and whether it has been counted as watched.
    shown_at: Option<Instant>,
    watched: bool,
    /// When the user last took the video to its end, by seeking there or
    /// stepping forward a frame, or such a seek landed. A seek elsewhere
    /// takes it away.
    to_end: Option<Instant>,
}

/// The last time the image layer was scaled afresh.
struct Scaled {
    took: Duration,
    smooth: bool,
    from: (u32, u32),
    to: (u32, u32),
}

/// A cluster of figures in the debug overlay: what it is about, and each
/// figure's name and value.
type Group = (&'static str, Vec<(&'static str, String)>);

/// The timings behind the debug overlay.
struct Perf {
    /// The file on view and when it was gone to.
    id: Option<usize>,
    arrived: Instant,
    /// How long after that its picture was first on screen.
    shown_in: Option<Duration>,
    scaled: Option<Scaled>,
    /// The last redraw: filling the buffer, then handing it to the display.
    paint: Duration,
    present: Duration,
    /// When the redraws of the last second happened, for their rate.
    paints: VecDeque<Instant>,
    /// How long the player took to draw its last frame.
    render: Duration,
    /// The player's own figures, and when they were read.
    video_lines: Option<(Instant, Vec<Group>)>,
}

impl Perf {
    fn new() -> Perf {
        Perf {
            id: None,
            arrived: Instant::now(),
            shown_in: None,
            scaled: None,
            paint: Duration::ZERO,
            present: Duration::ZERO,
            paints: VecDeque::new(),
            render: Duration::ZERO,
            video_lines: None,
        }
    }

    /// Redraws per second over the last second.
    fn rate(&self) -> Option<f64> {
        let (first, last) = (self.paints.front()?, self.paints.back()?);
        let span = last.duration_since(*first).as_secs_f64();
        (span > 0.0).then(|| (self.paints.len() - 1) as f64 / span)
    }
}

/// A duration as the debug overlay writes it.
fn ms(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 1.0 {
        format!("{:.1}ms", seconds * 1000.0)
    } else {
        format!("{seconds:.2}s")
    }
}

/// The seek bar's track: its left end, length and vertical centre. `unit`
/// is the interface text size, which everything about the bar scales with.
#[derive(Clone, Copy)]
struct SeekBar {
    x: i32,
    width: i32,
    middle: i32,
    unit: i32,
    /// Where the bar's room begins: the window's left edge, or the edge
    /// of the file list when that is up, which the bar stands beside.
    left: i32,
    /// The right edge of the window, and how many buttons stand at it.
    end: i32,
    buttons: i32,
}

impl SeekBar {
    /// The square at the far left holding the play / pause symbol, as its
    /// top-left corner and side.
    fn symbol(&self) -> (i32, i32, i32) {
        self.control(1)
    }

    /// One of the three squares at the far left: the file before, play or
    /// pause, and the file after, counted from 0.
    fn control(&self, which: i32) -> (i32, i32, i32) {
        let side = self.unit * 5 / 4;
        (self.left + self.unit * 3 / 2 + which * (side + self.unit), self.middle - side / 2, side)
    }

    /// Whether a window position is on the symbol, with some slack around
    /// it to make it easy to hit.
    fn on_symbol(&self, x: f64, y: f64) -> bool {
        self.on_control(1, x, y)
    }

    fn on_control(&self, which: i32, x: f64, y: f64) -> bool {
        self.over(self.control(which), x, y)
    }

    /// One of the squares at the far right, counted from the edge inwards.
    fn button(&self, which: i32) -> (i32, i32, i32) {
        let side = self.unit * 5 / 4;
        (self.end - self.unit * 3 / 2 - side - which * (side + self.unit), self.middle - side / 2, side)
    }

    fn on_button(&self, which: i32, x: f64, y: f64) -> bool {
        which < self.buttons && self.over(self.button(which), x, y)
    }

    fn over(&self, (left, top, side): (i32, i32, i32), x: f64, y: f64) -> bool {
        let slack = f64::from(self.unit / 2);
        let within = |at: f64, from: i32| at >= f64::from(from) - slack && at < f64::from(from + side) + slack;
        within(x, left) && within(y, top)
    }

    /// Whether a horizontal position is on the track, give or take a little.
    fn on_track(&self, x: f64) -> bool {
        let slack = f64::from(self.unit);
        x >= f64::from(self.x) - slack && x < f64::from(self.x + self.width) + slack
    }
}

/// Something to do to the video on view.
#[derive(Clone, Copy, PartialEq)]
enum VideoAction {
    Pause,
    /// Seconds to move by.
    Seek(f64),
    /// One frame forward or back.
    Frame(bool),
    /// Percentage points to change the volume by.
    Volume(i32),
    /// The next or previous file.
    File(bool),
}

/// A clickable button and the command it runs.
struct Button {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    command: Command,
}

/// The delete confirmation: the question, the highlighted answer (No to
/// begin with) and the files it is about.
struct Confirm {
    question: String,
    yes: bool,
    ids: Vec<usize>,
    /// Where its Cancel and its Delete button were last drawn: left, top,
    /// width and height.
    drawn: [[i32; 4]; 2],
}

/// A line of text being typed, and where in it the cursor is.
#[derive(Default)]
struct TextField {
    text: Vec<char>,
    cursor: usize,
    /// Where a selection started; it runs from here to the cursor.
    anchor: Option<usize>,
}

impl TextField {
    fn new(text: Vec<char>, cursor: usize) -> TextField {
        TextField { text, cursor, anchor: None }
    }

    fn string(&self) -> String {
        self.text.iter().collect()
    }

    /// The selected stretch, if there is one: where it starts and ends.
    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|anchor| *anchor != self.cursor)?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    /// Takes out what is selected. Returns whether anything was.
    fn cut(&mut self) -> bool {
        let selection = self.selection();
        if let Some((from, to)) = selection {
            self.text.drain(from..to);
            self.cursor = from;
        }
        self.anchor = None;
        selection.is_some()
    }

    fn insert(&mut self, typed: &str) {
        self.cut();
        // What is pasted may run over several lines; this is one.
        for c in typed.chars().filter(|c| !c.is_control()) {
            self.text.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// Where the word before, or after, the cursor ends.
    fn word_edge(&self, forward: bool) -> usize {
        let mut at = self.cursor;
        let space = |at: usize| self.text[at].is_whitespace();
        if forward {
            while at < self.text.len() && space(at) {
                at += 1;
            }
            while at < self.text.len() && !space(at) {
                at += 1;
            }
        } else {
            while at > 0 && space(at - 1) {
                at -= 1;
            }
            while at > 0 && !space(at - 1) {
                at -= 1;
            }
        }
        at
    }

    /// Applies an editing key. Returns whether the text changed.
    ///
    /// Beyond typing: the arrows, Home and End move, by a word with Ctrl
    /// and selecting with Shift; Ctrl+A selects everything; Ctrl+C, Ctrl+X
    /// and Ctrl+V copy, cut and paste; Ctrl+Backspace or Ctrl+W takes out
    /// the word before the cursor, and Ctrl+U everything.
    fn key(&mut self, key: Key<&str>, modifiers: ModifiersState) -> bool {
        let (ctrl, shift) = (modifiers.control_key(), modifiers.shift_key());
        let typing = !ctrl && !modifiers.alt_key() && !modifiers.super_key();
        let before = self.text.clone();
        // Moving the cursor with Shift held stretches the selection to it.
        let move_to = |field: &mut TextField, to: usize| {
            field.anchor = if shift { field.anchor.or(Some(field.cursor)) } else { None };
            field.cursor = to;
        };
        match key {
            Key::Character(letter) if ctrl => match letter.to_lowercase().as_str() {
                "a" => (self.anchor, self.cursor) = (Some(0), self.text.len()),
                "c" | "x" => {
                    let (from, to) = self.selection().unwrap_or((0, self.text.len()));
                    ops::set_clipboard(&self.text[from..to].iter().collect::<String>());
                    if letter.eq_ignore_ascii_case("x") {
                        self.cut();
                    }
                }
                "v" => self.insert(&ops::clipboard().unwrap_or_default()),
                "u" => (self.text, self.cursor, self.anchor) = (Vec::new(), 0, None),
                "w" => {
                    self.anchor = Some(self.word_edge(false));
                    self.cut();
                }
                _ => {}
            },
            Key::Named(NamedKey::Backspace | NamedKey::Delete) if self.cut() => {}
            Key::Named(NamedKey::Backspace) if ctrl => {
                self.anchor = Some(self.word_edge(false));
                self.cut();
            }
            Key::Named(NamedKey::Backspace) if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            Key::Named(NamedKey::Delete) if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            Key::Named(NamedKey::Space) if typing => self.insert(" "),
            Key::Character(typed) if typing => self.insert(typed),
            Key::Named(NamedKey::ArrowLeft) if ctrl => move_to(self, self.word_edge(false)),
            Key::Named(NamedKey::ArrowRight) if ctrl => move_to(self, self.word_edge(true)),
            Key::Named(NamedKey::ArrowLeft) => move_to(self, self.cursor.saturating_sub(1)),
            Key::Named(NamedKey::ArrowRight) => move_to(self, (self.cursor + 1).min(self.text.len())),
            Key::Named(NamedKey::Home) => move_to(self, 0),
            Key::Named(NamedKey::End) => move_to(self, self.text.len()),
            _ => {}
        }
        self.text != before
    }
}

/// The trailer playing in the grid, under the cursor.
struct Trailer {
    id: usize,
    /// The frame last taken from the thread playing it, and when the
    /// first one came.
    frame: Option<Frame>,
    since: Option<Instant>,
    /// When the thread was asked to play it.
    asked: Instant,
}

/// How the box for finding files is brought to a close.
#[derive(Clone, Copy, PartialEq)]
enum PaletteEnd {
    /// Go to the match picked out.
    Go,
    /// Carry on with only the matches.
    Keep,
    /// Drop a search kept earlier and go back to where it was begun.
    All,
    /// Drop it, and stay on the file on view among all the others.
    Here,
    /// Run what is typed on one lot of a triage: the kept, the rejected,
    /// or every file there is.
    OnKept,
    OnRejected,
    OnEvery,
}

/// The box for finding files: what is typed, the files that match it with
/// the closest first, and which of them is picked out.
struct Palette {
    field: TextField,
    matches: Vec<usize>,
    /// Whether what is typed leaves any file out.
    narrowed: bool,
    selected: usize,
    /// Which part has the keyboard: 0 for the text and the matches, then
    /// the buttons in the order `palette_buttons` gives them.
    focus: usize,
    /// Where the buttons were last drawn, for clicks: left, top, width
    /// and height.
    drawn: Vec<(PaletteEnd, [i32; 4])>,
    /// What is listed in place of files when the box is given a command
    /// to find, with `>`, or a time to go to, with `:`: what each is called,
    /// its key, and the command itself.
    commands: Vec<(String, &'static str, Command)>,
    /// The first row on show: the list is scrolled this far down.
    first: usize,
}

impl Palette {
    /// How many rows there are to pick among: commands, or else files.
    fn rows(&self) -> usize {
        if self.commands.is_empty() { self.matches.len() } else { self.commands.len() }
    }

    /// Scrolls the list, of which `rows` show at once, just far enough to
    /// have the row picked out among them.
    fn show_selected(&mut self, rows: usize) {
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + rows {
            self.first = self.selected + 1 - rows;
        }
    }
}

/// Where the go-to box is drawn and which of its matches are in view.
struct PaletteLayout {
    left: i32,
    top: i32,
    width: i32,
    /// Height of the part typed into, and of each match below it.
    head: i32,
    row_height: i32,
    first: usize,
    rows: usize,
    px: u32,
    /// Size of each of the frames shown above it for a video.
    frame: (i32, i32),
}

/// Where a session was left: the file, how far into it for a video, and
/// the search the list was narrowed to.
struct Spot {
    /// Which session it is, and the folder it is of.
    id: String,
    cwd: PathBuf,
    file: PathBuf,
    seconds: f64,
    filter: Option<String>,
}

/// A workspace: a folder, or what was opened, under a name, with how it
/// is listed and what it was showing when it was last left.
struct Space {
    name: String,
    paths: Vec<PathBuf>,
    recursive: bool,
    sort: Option<Sort>,
    reverse: bool,
    /// The digit that goes to it with Ctrl, if it has one.
    key: Option<usize>,
    /// Listed again, and begun at its start, each time it is gone to.
    start: Start,
    filelist: Option<PathBuf>,
    /// Whether it is in the config file, rather than only opened this once.
    saved: bool,
    /// `None` until it has been gone to.
    kept: Option<Kept>,
    /// What is written under its card in the overview, for a session:
    /// when it was left and where.
    caption: Option<String>,
    /// Where a session left off in it, to be landed on when it is first
    /// gone to.
    spot: Option<Spot>,
    /// Whether its files have been asked for, to show in the overview.
    asked: bool,
}

/// How the list stood when a search was kept: the file on view and the
/// one under the grid's cursor, and the order every file was in.
struct FilterFrom {
    /// The search this one was made in, if it was made in one.
    typed: Option<String>,
    viewed: usize,
    focus: usize,
    order: Vec<usize>,
    sort: Option<Sort>,
    reversed: bool,
    shuffled: bool,
}

/// What a workspace was showing when it was left, to come back to.
struct Kept {
    list: Vec<usize>,
    pos: usize,
    cursor: usize,
    filter: Option<(String, Vec<usize>)>,
    /// Where things stood when each search now in force was kept, the
    /// first of them first, to go back to.
    filter_from: Vec<FilterFrom>,
    sort: Option<Sort>,
    reversed: bool,
    shuffled: bool,
}

/// The overview of workspaces: which card is picked out, and how far its
/// slideshow has got.
struct Overview {
    cursor: usize,
    tick: usize,
}

/// How long each picture of the overview's slideshow stays up.
const OVERVIEW_STEP: Duration = Duration::from_millis(1100);

/// The keys that go to the first nine workspaces, as a menu shows them.
const SPACE_KEYS: [&str; 9] = ["Ctrl+1", "Ctrl+2", "Ctrl+3", "Ctrl+4", "Ctrl+5", "Ctrl+6", "Ctrl+7", "Ctrl+8", "Ctrl+9"];

/// The rename box: the file being renamed and the name as typed so far.
struct Prompt {
    id: usize,
    kind: PromptKind,
    field: TextField,
}

/// What a box for typing into is for.
#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    /// A new name for file `id`.
    Rename,
    /// A name to save the folder on view under, as a workspace.
    SaveWorkspace,
    /// A folder to move the target files into, or to copy them into.
    SendTo { moving: bool },
    /// A name for the kept section of the video on view with these ends.
    SectionName { start: f64, end: f64 },
}

pub struct App {
    /// The folder jog was started in, which its sessions are kept by.
    cwd: PathBuf,
    options: Options,
    /// Path of each file id.
    files: Vec<PathBuf>,
    marks: HashSet<usize>,
    undo: Vec<Undo>,
    /// The folder last typed to copy or move files into.
    last_folder: Option<String>,
    /// The workspaces: those of the config file, in its order, and after
    /// them the folder opened by its path, if it is none of them. And
    /// which one is on show.
    spaces: Vec<Space>,
    space: usize,
    /// The one jog was started on, which Ctrl+` comes back to.
    home: usize,
    /// The overview of the workspaces, when it is up, and when its
    /// slideshow is to move on.
    overview: Option<Overview>,
    overview_due: Option<Instant>,
    prompt: Option<Prompt>,
    palette: Option<Palette>,
    /// A command picked in the find box, to be run now that it has closed.
    run_next: Option<Command>,
    /// A search that was kept: what was typed and every file, in order.
    /// `list` then holds only the matches.
    filter: Option<(String, Vec<usize>)>,
    /// Where things stood when each search now in force was kept, the
    /// first of them first, to go back to.
    filter_from: Vec<FilterFrom>,
    /// The order the list was last put in, unless shuffled since, and
    /// whether it runs backwards.
    sort: Option<Sort>,
    reversed: bool,
    shuffled: bool,
    /// Whether the panel of file names is open.
    show_list: bool,
    /// How many pictures the grid shows across, when that has been set
    /// with Ctrl and + or -; and when to draw it next while it slides.
    grid_columns: Option<i32>,
    grid_due: Option<Instant>,
    /// Things on their way: a toast sliding in, the seek bar and the strip
    /// of scenes rising, and the middles of the
    /// filmstrip.
    toast_slide: Cell<Glide>,
    bar_rise: Cell<Glide>,
    scenes_rise: Cell<Glide>,
    strip_slide: Cell<Glide>,
    /// The grid cursor's ring on its way to another cell, and whether it
    /// is to be put straight where it belongs: the grid has just come up.
    ring: [Cell<Glide>; 4],
    ring_snap: Cell<bool>,
    /// The row of the file list the pointer has come onto, lighting up.
    row_fade: Cell<Glide>,
    row_faded: Cell<Option<usize>>,
    /// A menu or a box coming up over the rest, and room for a copy of
    /// what was there before it.
    box_fade: Cell<Glide>,
    fade_buf: Vec<u32>,
    /// Whose picture `scene` holds, and whether it is what the window
    /// last showed.
    scene_id: Option<usize>,
    scene_live: bool,
    /// A picture growing out of its cell in the grid or shrinking into it.
    grow: Option<Grow>,
    /// When videos were found to have been watched, for their dots to fade.
    seen_at: RefCell<HashMap<usize, Instant>>,
    /// When to draw again because something is still moving, and whether
    /// anything was found to be while the frame in hand was drawn.
    anim_due: Option<Instant>,
    /// How long the display shows each frame for, once that has been asked.
    frame_every: Option<Duration>,
    /// When the frame before this one was begun.
    frame_began: Option<Instant>,
    /// Whether a frame has been asked for from here since the last was
    /// drawn, and what the window was last handed, to tell what changes.
    asked: Cell<bool>,
    presented: Vec<u32>,
    animating: Cell<bool>,
    /// Whether the whole window was painted afresh this frame, so that
    /// comparing it with the last would only find everything changed.
    repainted: Cell<bool>,
    /// The first picture as the window's background, until a frame has
    /// reached the window the ordinary way; and whether that has been
    /// done with, and whether the window has had the focus yet, which
    /// is the window manager done putting it up.
    backdrop: Option<(x11::Backdrop, Instant)>,
    backdrop_done: bool,
    focused_once: bool,
    /// The first row the file list was scrolled to with the wheel, along
    /// with the position and list length it was scrolled at: once either
    /// changes, the list goes back to following the file on view.
    list_scroll: Option<(usize, usize, usize)>,
    /// The list position under the pointer, in the file list.
    list_hover: Option<usize>,
    list_button_over: Option<usize>,
    /// The list position of the filmstrip cell under the pointer.
    strip_over: Option<usize>,
    /// How many files along from the one on view the filmstrip has been
    /// moved with the wheel. Going to a file puts it back.
    strip_scroll: i64,
    strip_close_over: bool,
    /// Which of the grid's buttons the pointer is over, counting those
    /// under it and then those at its sides.
    grid_over: Option<usize>,
    /// The debug overlay as last drawn: its buttons, its figures for
    /// copying, and the sizes it has grown to.
    debug_buttons: Vec<Button>,
    debug_last: Vec<Group>,
    debug_keep: DebugKeep,
    /// Video lengths, found out on another thread as their rows come up.
    /// Asked for but not yet known ones are `None`.
    durations: HashMap<usize, Option<f64>>,
    prober: Option<mpsc::Sender<(usize, PathBuf)>>,
    /// Frames from along videos, for looking through one without playing
    /// it: those fetched so far for each of the last few videos looked at,
    /// oldest first in `story_order`.
    stories: Option<Stories>,
    story_frames: HashMap<usize, Vec<Option<Vec<Arc<Frame>>>>>,
    story_order: VecDeque<usize>,
    /// The video whose frames are being fetched.
    story_asked: Option<usize>,
    /// The video the grid is running through, which of its frames is up,
    /// and when the next one is due.
    trailer: Option<Trailer>,
    trailers: Option<Trailers>,
    /// Set while the directories are watched; clearing it stops the thread
    /// that does the watching.
    watch: Option<Arc<AtomicBool>>,
    /// File sizes looked up for searches such as `>10mb`.
    sizes: HashMap<usize, Option<u64>>,
    /// Where videos were left, read the first time one is played.
    positions: Option<Positions>,
    /// The videos watched, read when first asked about, and what has been
    /// found out about the files here so far.
    seen: RefCell<Option<Seen>>,
    seen_known: RefCell<HashMap<usize, bool>>,
    confirm: Option<Confirm>,
    proxy: EventLoopProxy<Event>,
    /// File ids in viewing order. Removing an image drops it from here.
    list: Vec<usize>,
    pos: usize,
    forward: bool,
    loader: Loader,
    /// What the loader was last told, so that it is only told of changes.
    last_request: RefCell<Option<Request>>,
    transforms: HashMap<usize, Transform>,
    /// Size images are fitted to. Using the monitor rather than the window
    /// means resizing or leaving fullscreen never needs a fresh decode.
    monitor: (u32, u32),
    /// What is on screen and the file it came from. Kept while the next
    /// image decodes, so navigation never flashes to black.
    shown: Option<(usize, Arc<Frame>)>,
    error: Option<String>,

    view: View,
    keep_view: bool,
    antialias: bool,
    settle_at: Option<Instant>,
    /// Which picture of an animated image is up, and when the next is due.
    picture: usize,
    picture_due: Option<Instant>,
    /// When to draw again to see whether a font being loaded has come.
    font_due: Option<Instant>,
    /// The video the grid's cursor has come to rest on, and when its
    /// trailer is to begin if the cursor is still there.
    trailer_due: Option<(usize, Instant)>,
    /// The image layer as last rendered, reused when only overlays change.
    scene: Vec<u32>,
    scene_key: Option<SceneKey>,
    /// The video frame as the player last drew it, and the window size and
    /// video it was drawn for; copied from while the player has no newer.
    /// Forgotten when a frame is taken from the player for anything else.
    video_copy: Vec<u32>,
    video_kept: Cell<Option<((u32, u32), Option<usize>)>>,
    /// The filmstrip's pictures at the size its cells take them, by file
    /// id and size, each with the thumbnail it was made from: shrinking
    /// the thumbnails again on every frame is most of drawing the strip.
    strip_thumbs: RefCell<HashMap<(usize, u32, u32), (Arc<Frame>, Arc<Frame>)>>,

    show_info: bool,
    /// Whether the grid names each file under its picture.
    grid_titles: bool,
    show_debug: bool,
    /// Which graphs of the picture's colours stand under the debug
    /// overlay, by `scopes::KINDS`; whether they count by logarithm; what
    /// they last counted, and how long counting and drawing them took.
    scopes: [bool; 5],
    scope_log: bool,
    /// Which rendition of Ultra HDR photos is shown, and which files have
    /// been found to be such photos.
    hdr: Hdr,
    ultra_hdr: RefCell<HashMap<usize, bool>>,
    /// What each video says about its HDR, once read: `None` while it
    /// is being read. And the headroom, in stops, its curves are drawn
    /// for: none, for this screen.
    hdr_infos: HashMap<usize, Option<Arc<hdrmeta::Info>>>,
    hdr_headroom: f64,
    /// Where the choices of rendition in the info panel were drawn.
    info_buttons: Vec<Button>,
    /// The format of each image looked at, by its first bytes.
    formats: RefCell<HashMap<usize, Option<String>>>,
    scope_counts: scopes::Measured,
    scope_cost: Duration,
    perf: Perf,
    show_exif: bool,
    show_actions: bool,
    pointer_visible: bool,
    strip_pinned: bool,
    strip_hover: bool,
    /// Thumbnails are only decoded once the filmstrip has been opened.
    strip_used: bool,
    menu: Option<Menu>,
    grid: Option<Grid>,
    /// Where the buttons under the grid were last drawn, for clicks.
    grid_buttons: Vec<Button>,
    help: bool,
    /// What has been typed into the list of shortcuts, to find one.
    help_search: String,
    snapshot: Option<Snapshot>,
    /// The benchmark, when `--benchmark` runs one; and whether its next
    /// scene is to be played at the first chance.
    bench: Option<Bench>,
    bench_step_due: bool,
    /// Created the first time a video comes up.
    player: Option<Player>,
    /// Set once the player has been asked for, so that happens only once.
    player_asked: bool,
    /// Which of the two players that take turns `player` is, and the other
    /// one, which holds the video that comes next ready to play.
    active: u8,
    spare: Option<Spare>,
    video: Option<Video>,
    /// A second, muted player that fetches the frames shown while the
    /// pointer is over the seek bar, the file it has open, and its picture.
    preview: Option<Player>,
    /// The frames before a pause, when they are to be fetched, and the
    /// thread that fetches them, with the number of the latest ask.
    back: Option<back::BackFrames>,
    back_due: Option<Instant>,
    back_jobs: Option<mpsc::Sender<back::Job>>,
    back_ticket: u64,
    /// Two pictures compared, when they are, and the number of the latest
    /// asked for.
    compare: Option<compare::Compare>,
    compare_ticket: u64,
    preview_id: Option<usize>,
    preview_frame: Option<Frame>,
    /// Where along the seek bar the pointer is, from 0 to 1, while over it.
    hover: Option<f64>,
    /// The time the preview should show, the time last asked of its
    /// player and whether to that exact frame, and since when that request
    /// has been outstanding.
    preview_want: Option<f64>,
    preview_sent: Option<(f64, bool)>,
    preview_busy: Option<Instant>,
    /// Where the next video started is to begin, when a seek carried over
    /// from the file beside it: seconds in, or negative, from its end.
    enter_at: Option<f64>,
    /// Playback speed, kept from one video to the next.
    speed: f64,
    muted: bool,
    /// What a video that has played to its end does, kept from one video
    /// to the next.
    looping: Looping,
    /// What is being held down over a video, and how strong the loupe is
    /// while `v` is held for one.
    /// Whether the video that was on view plays on in its cell while the
    /// grid is up, and its picture there.
    pip: bool,
    pip_live: Option<Frame>,
    pip_in_cell: bool,
    hold: Option<Hold>,
    loupe: Option<f64>,
    /// Whether seeking lands on the very moment asked for or on the
    /// nearest keyframe.
    precise: bool,
    /// Whether the strip of scenes is up, the scenes of the videos looked
    /// at so far, and the thread that finds them.
    /// The search being typed into the heading of the file list.
    /// The shell commands run before, read when first wanted, and the one
    /// just typed into the find box, on its way to being run.
    shell_history: Option<Vec<String>>,
    /// The files viewed in this run, the latest first, for the find box.
    visited: RefCell<Vec<usize>>,
    shell_typed: Option<String>,
    /// Whether the file list stands at the right of the window, and whether
    /// it is pinned there: the picture then has what is left beside it,
    /// where an unpinned list lies over the picture.
    list_right: bool,
    list_pinned: bool,
    /// Whether files are being decided about one after another; what has
    /// been decided, read when first asked about; and what has been found
    /// out about the files here so far.
    triage_on: bool,
    triage: RefCell<Option<Triage>>,
    verdicts: RefCell<HashMap<usize, Option<Verdict>>>,
    /// Where the buttons of a triage were last drawn, for clicks on them.
    triage_buttons: RefCell<Vec<(Command, [i32; 4])>>,
    /// The Convert panel, when it is up.
    convert_panel: Option<ConvertPanel>,
    /// The section marked in each video, by file: still there when the
    /// video is come back to.
    sections: HashMap<usize, Section>,
    /// The strip's tab; the sections kept, by video, read the first time
    /// they are wanted; each video's own chapters, read from the player;
    /// and the frames for the Sections tab's cells.
    strip_tab: StripTab,
    kept_sections: Option<section::KeptSections>,
    /// The first cell in view of a Sections tab too long for the window,
    /// and the entry a right click there was on.
    entry_first: usize,
    entry_target: Option<usize>,
    chapters: HashMap<usize, Vec<(f64, String)>>,
    entry_frames: HashMap<usize, EntryFrames>,
    section_finder: Option<scenes::Finder>,
    /// The names of the commands run from the find box lately, the latest
    /// first: offered first the next time.
    palette_recent: Option<Vec<String>>,
    /// Which of them the pointer is over.
    triage_over: Option<usize>,
    list_search: Option<TextField>,
    /// The row of the file list picked out with the arrows while searching.
    list_cursor: Option<usize>,
    /// The row of the file list a menu was brought up on, which what is
    /// chosen from it is done to.
    row_target: Option<usize>,
    scenes_on: bool,
    /// The scene the pointer is on.
    scene_over: Option<usize>,
    scenes: HashMap<usize, SceneList>,
    scene_finder: Option<scenes::Finder>,
    /// The size the player last drew a video at, when the window last
    /// changed size, and the frame drawn at the old size while it does.
    video_render: Option<(u32, u32)>,
    resized_at: Option<Instant>,
    video_scratch: Vec<u32>,
    /// Whether subtitles show, kept from one video to the next, their size
    /// as a percentage, and how far off the bottom edge they sit.
    subtitles: bool,
    subtitle_size: u32,
    subtitle_margin: i32,
    toast: Option<(String, Instant)>,
    /// What the toast offers to be clicked, and where that was drawn.
    toast_links: Vec<(String, ToastLink)>,
    toast_buttons: Vec<(ToastLink, [i32; 4])>,
    /// While set, the video's time and a larger progress bar are shown.
    time_until: Option<Instant>,
    meta: Option<(usize, Meta)>,
    /// The file whose details were last asked for on another thread.
    meta_asked: Option<usize>,
    text: Option<Text>,
    /// What the player was last told about the view, so that it is only
    /// told again when something changes.
    video_view_sent: RefCell<Option<[String; 4]>>,
    /// Scroll-wheel movement not yet turned into a step, in lines.
    scrolled: f64,

    modifiers: ModifiersState,
    mouse: (f64, f64),
    /// When and where the image was last clicked, to tell a double click.
    last_click: Option<(Instant, (f64, f64))>,
    drag: Option<Drag>,
    slide_delay: Duration,
    slide_at: Option<Instant>,
    rng: Rng,

    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
}

struct SceneKey {
    frame: Arc<Frame>,
    picture: usize,
    layout: Layout,
    smooth: bool,
    window: (u32, u32),
}

impl SceneKey {
    fn matches(&self, other: &SceneKey) -> bool {
        Arc::ptr_eq(&self.frame, &other.frame)
            && (self.picture, self.layout, self.smooth, self.window)
                == (other.picture, other.layout, other.smooth, other.window)
    }
}

enum Drag {
    Pan,
    /// Dragging along the seek bar.
    Scrub,
    /// Dragging an end of the section along the seek bar.
    Section(Edge),
    Zoom { start_x: f64, start_scale: f64, anchor: (f64, f64), moved: bool },
    /// Dragging along the bar under the grid, through its pages.
    Pages,
}

struct Menu {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    px: u32,
    title: Option<String>,
    rows: Vec<MenuRow>,
    selected: Option<usize>,
    /// The menu opened beside the selected item, if it has one.
    sub: Option<Box<Menu>>,
}

struct MenuRow {
    top: i32,
    height: i32,
    item: Option<&'static MenuItem>,
}

impl Menu {
    fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        let (x, y) = (x as i32 - self.x, y as i32 - self.y);
        if x < 0 || x >= self.width {
            return None;
        }
        self.rows.iter().position(|row| row.item.is_some() && y >= row.top && y < row.top + row.height)
    }
}

/// Filmstrip placement: a band along the bottom with fixed-size cells.
#[derive(Clone, Copy)]
struct Strip {
    top: i32,
    height: i32,
    pad: i32,
    cell_w: i32,
    cell_h: i32,
}

impl Strip {
    fn for_height(window_height: u32) -> Strip {
        let height = (window_height as i32 / 12).clamp(110, 220).min(window_height as i32);
        let pad = height / 10;
        let cell_h = height - 2 * pad;
        Strip { top: window_height as i32 - height, height, pad, cell_w: cell_h * 3 / 2, cell_h }
    }

    fn pitch(&self) -> i32 {
        self.cell_w + self.pad
    }
}

/// A scripted run for testing without a visible window: press some keys,
/// wait for decodes, write the rendered frame to a file and quit. Driven by
/// the `JOG_SNAPSHOT` (output `.ppm` path), `JOG_SNAPSHOT_KEYS`,
/// `JOG_SNAPSHOT_POINTER` (`X,Y`) and `JOG_SNAPSHOT_WAIT` (ms) variables.
/// A `{Wait}` in the keys pauses for one waiting period before going on.
/// A benchmark: scenes to play, each some keys and then a wait, and
/// the figures taken while they play, by scene and metric.
struct Bench {
    scenes: VecDeque<(&'static str, &'static str, Duration)>,
    scene: &'static str,
    at: Option<Instant>,
    samples: Vec<((String, String), Vec<f64>)>,
    launched: Instant,
    /// Whether the first picture is up, which is when the scenes begin.
    begun: bool,
}

struct Snapshot {
    path: PathBuf,
    keys: String,
    pointer: (f64, f64),
    wait: Duration,
    at: Option<Instant>,
}

impl Snapshot {
    fn from_env() -> Option<Snapshot> {
        let var = |name: &str| std::env::var(name).ok();
        let pointer = var("JOG_SNAPSHOT_POINTER").and_then(|text| {
            let (x, y) = text.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        });
        let wait = var("JOG_SNAPSHOT_WAIT").and_then(|ms| ms.parse().ok()).unwrap_or(1500);
        Some(Snapshot {
            path: std::env::var_os("JOG_SNAPSHOT")?.into(),
            keys: var("JOG_SNAPSHOT_KEYS").unwrap_or_default(),
            pointer: pointer.unwrap_or((0.0, 0.0)),
            wait: Duration::from_millis(wait),
            at: None,
        })
    }
}

/// The grid view. The cursor is a list position, moved by the arrow keys or
/// the pointer. The grid scrolls by the row to keep it in view, sliding
/// from where it was (`from`, in rows) to where it is going (`target`).
struct Grid {
    cursor: usize,
    from: f64,
    target: f64,
    since: Instant,
    /// Whether the cursor has been moved since the grid came up by
    /// itself, with `--grid`: until then no trailer plays.
    moved: bool,
}

/// Grid placement: equal cells filling the window above a footer line.
#[derive(Clone, Copy)]
struct GridLayout {
    columns: i32,
    rows: i32,
    cell_w: i32,
    cell_h: i32,
    pad: i32,
    left: i32,
    top: i32,
    /// How wide the window is: how far apart two pages lie.
    width: i32,
}

impl GridLayout {
    /// `columns` is how many pictures go across; left out, it follows from
    /// the width of the window.
    fn for_window(window: (u32, u32), columns: Option<i32>) -> GridLayout {
        let (width, height) = (window.0 as i32, window.1 as i32);
        let usable = (height - Self::footer_height(window.1)).max(1);
        let columns = columns.unwrap_or((width / 240).clamp(2, 6)).clamp(GRID_COLUMNS.0, GRID_COLUMNS.1);
        let cell_w = (width / columns).max(1);
        let cell_h = (cell_w * 3 / 4).clamp(1, usable);
        let rows = (usable / cell_h).max(1);
        GridLayout {
            columns,
            rows,
            cell_w,
            cell_h,
            pad: (cell_w / 24).max(2),
            left: (width - columns * cell_w) / 2,
            top: (usable - rows * cell_h) / 2,
            width,
        }
    }

    fn footer_height(window_height: u32) -> i32 {
        Text::line_height(ui_px(window_height)) * 2
    }

    fn per_page(&self) -> usize {
        (self.columns * self.rows) as usize
    }


    fn thumb_size(&self) -> (u32, u32) {
        ((self.cell_w - 2 * self.pad).max(1) as u32, (self.cell_h - 2 * self.pad).max(1) as u32)
    }

    /// The list position of the cell at a window position, with `page` on
    /// show.
    fn cell_at(&self, x: f64, y: f64, page: f64) -> Option<usize> {
        let column = ((x - f64::from(self.left)) / f64::from(self.cell_w)).floor() as i32;
        let row = ((y - f64::from(self.top)) / f64::from(self.cell_h)).floor() as i32;
        let inside = (0..self.columns).contains(&column) && (0..self.rows).contains(&row);
        inside.then(|| page as usize * self.per_page() + (row * self.columns + column) as usize)
    }

    /// The page a list position is on.
    fn page_of(&self, pos: usize) -> f64 {
        (pos / self.per_page()) as f64
    }
}

impl Grid {
    /// A grid showing the page `cursor` is on.
    fn at(cursor: usize, layout: &GridLayout) -> Grid {
        let page = layout.page_of(cursor);
        Grid { cursor, from: page, target: page, since: Instant::now(), moved: true }
    }

    /// Which page is at the window's left just now, and how far on to the
    /// next: on its way from where it was to where it is going, fast at
    /// first and settling in.
    fn scroll(&self) -> f64 {
        let part = (self.since.elapsed().as_secs_f64() / GRID_SCROLL.as_secs_f64()).min(1.0);
        self.from + (self.target - self.from) * (1.0 - (1.0 - part).powi(3))
    }

    fn scrolling(&self) -> bool {
        self.since.elapsed() < GRID_SCROLL && self.from != self.target
    }

    /// Sets the grid sliding to page `target` from where it is now.
    fn scroll_to(&mut self, target: f64) {
        if target != self.target {
            (self.from, self.target, self.since) = (self.scroll(), target, Instant::now());
        }
    }

    /// Slides to the page the cursor is on.
    fn follow(&mut self, layout: &GridLayout) {
        self.scroll_to(layout.page_of(self.cursor));
    }
}

fn ui_px(window_height: u32) -> u32 {
    (window_height / 90).clamp(13, 30)
}

impl App {
    pub fn new(options: Options, files: Vec<PathBuf>, start: usize, loader: Loader, proxy: EventLoopProxy<Event>, cwd: PathBuf) -> Self {
        let slide_delay = Duration::from_secs_f64(options.slideshow_delay.unwrap_or(DEFAULT_SLIDE_DELAY));
        ACCENT.store(options.accent, atomic::Ordering::Relaxed);
        crate::mpv::DEBUG.store(options.debug, atomic::Ordering::Relaxed);
        // The workspaces of the config file; and unless one of them was
        // asked for by name, what was opened by its path as one more.
        // With sessions to choose from, they are the workspaces, and those
        // of the config file are left out of it.
        let choosing = !options.sessions.is_empty();
        let mut spaces: Vec<Space> = (options.workspaces.iter().filter(|_| !choosing))
            .map(|workspace| Space {
                name: workspace.name.clone(),
                paths: vec![workspace.path.clone()],
                recursive: workspace.recursive.unwrap_or(options.listing.recursive),
                sort: workspace.sort,
                reverse: workspace.reverse,
                key: workspace.key,
                start: workspace.start,
                filelist: None,
                saved: true,
                kept: None,
                asked: false,
                caption: None,
                spot: None,
            })
            .collect();
        let space = options.workspace.unwrap_or_else(|| {
            let paths = if options.paths.is_empty() { vec![PathBuf::from(".")] } else { options.paths.clone() };
            // Named after the folder, when it is one folder.
            let folder = match paths.as_slice() {
                [only] if only.is_dir() => only.canonicalize().ok().and_then(|full| Some(full.file_name()?.to_string_lossy().into_owned())),
                _ => None,
            };
            spaces.push(Space {
                name: folder.unwrap_or_else(|| "opened".to_string()),
                paths,
                recursive: options.listing.recursive,
                sort: options.listing.sort,
                reverse: options.listing.reverse,
                key: None,
                start: options.start.unwrap_or(Start::Resume),
                filelist: options.listing.filelist.clone(),
                saved: false,
                kept: None,
                asked: false,
                caption: None,
                spot: None,
            });
            spaces.len() - 1
        });
        // The session opened is the latest; the others follow it, each to
        // be listed, and landed in where it was left, when it is gone to.
        if let Some((latest, others)) = options.sessions.split_first() {
            spaces[space].name = sessions::title(latest);
            spaces[space].caption = Some(sessions::caption(latest, options.all_sessions));
            spaces.extend(others.iter().map(|session| Space {
                name: sessions::title(session),
                caption: Some(sessions::caption(session, options.all_sessions)),
                paths: session.paths.clone(),
                recursive: session.recursive,
                sort: session.sort,
                reverse: session.reverse,
                key: None,
                start: Start::Resume,
                filelist: session.filelist.clone(),
                saved: false,
                kept: None,
                asked: false,
                spot: Some(Spot { id: session.id.clone(), cwd: session.cwd.clone(), file: session.file.clone(), seconds: session.seconds, filter: session.filter.clone() }),
            }));
        }
        App {
            cwd,
            marks: HashSet::new(),
            undo: Vec::new(),
            last_folder: None,
            prompt: None,
            palette: None,
            run_next: None,
            filter: None,
            filter_from: Vec::new(),
            sort: options.listing.sort.filter(|_| !options.listing.randomize),
            reversed: options.listing.reverse,
            shuffled: options.listing.randomize,
            show_list: false,
            grid_columns: None,
            grid_due: None,
            toast_slide: Cell::new(Glide::at(1.0)),
            bar_rise: Cell::new(Glide::at(0.0)),
            scenes_rise: Cell::new(Glide::at(0.0)),
            strip_slide: Cell::new(Glide::at(start as f64)),
            ring: std::array::from_fn(|_| Cell::new(Glide::at(0.0))),
            ring_snap: Cell::new(true),
            row_fade: Cell::new(Glide::at(0.0)),
            row_faded: Cell::new(None),
            box_fade: Cell::new(Glide::at(0.0)),
            fade_buf: Vec::new(),
            scene_id: None,
            scene_live: false,
            grow: None,
            seen_at: RefCell::new(HashMap::new()),
            anim_due: None,
            frame_every: None,
            frame_began: None,
            asked: Cell::new(false),
            presented: Vec::new(),
            animating: Cell::new(false),
            repainted: Cell::new(false),
            backdrop: None,
            backdrop_done: false,
            focused_once: false,
            list_scroll: None,
            list_hover: None,
            list_button_over: None,
            strip_over: None,
            strip_scroll: 0,
            strip_close_over: false,
            grid_over: None,
            debug_buttons: Vec::new(),
            debug_last: Vec::new(),
            debug_keep: DebugKeep::default(),
            durations: HashMap::new(),
            prober: None,
            stories: None,
            story_frames: HashMap::new(),
            story_order: VecDeque::new(),
            story_asked: None,
            trailer: None,
            trailers: None,
            watch: None,
            sizes: HashMap::new(),
            positions: None,
            seen: RefCell::new(None),
            seen_known: RefCell::new(HashMap::new()),
            confirm: None,
            proxy,
            list: (0..files.len()).collect(),
            pos: start,
            forward: true,
            loader,
            last_request: RefCell::new(None),
            transforms: HashMap::new(),
            monitor: (0, 0),
            shown: None,
            error: None,
            view: View { zoom: options.zoom, center: None, enlarge: options.auto_zoom },
            keep_view: options.keep_view,
            antialias: options.antialias,
            settle_at: None,
            picture: 0,
            picture_due: None,
            font_due: None,
            trailer_due: None,
            scene: Vec::new(),
            scene_key: None,
            video_copy: Vec::new(),
            video_kept: Cell::new(None),
            strip_thumbs: RefCell::new(HashMap::new()),
            show_info: options.draw_info,
            grid_titles: options.grid_titles,
            show_debug: false,
            scopes: [false, true, false, true, false],
            scope_log: true,
            hdr: options.hdr,
            ultra_hdr: RefCell::new(HashMap::new()),
            hdr_infos: HashMap::new(),
            hdr_headroom: 0.0,
            info_buttons: Vec::new(),
            formats: RefCell::new(HashMap::new()),
            scope_counts: scopes::Measured::default(),
            scope_cost: Duration::ZERO,
            perf: Perf::new(),
            show_exif: options.draw_exif,
            show_actions: options.draw_actions,
            pointer_visible: !options.hide_pointer,
            strip_pinned: options.filmstrip,
            strip_hover: false,
            strip_used: options.filmstrip,
            menu: None,
            grid: None,
            grid_buttons: Vec::new(),
            help: false,
            help_search: String::new(),
            snapshot: Snapshot::from_env(),
            bench: None,
            bench_step_due: false,
            player: None,
            active: 0,
            spare: None,
            player_asked: false,
            video: None,
            preview: None,
            back: None,
            back_due: None,
            back_jobs: None,
            back_ticket: 0,
            compare: None,
            compare_ticket: 0,
            preview_id: None,
            preview_frame: None,
            hover: None,
            preview_want: None,
            preview_sent: None,
            preview_busy: None,
            enter_at: options.begin_at,
            speed: 1.0,
            muted: false,
            looping: Looping::Off,
            pip: false,
            pip_live: None,
            pip_in_cell: false,
            hold: None,
            loupe: None,
            precise: options.precise_seeking,
            shell_history: None,
            visited: RefCell::new(Vec::new()),
            shell_typed: None,
            list_right: options.list_right,
            list_pinned: options.list_pinned,
            triage_on: false,
            triage: RefCell::new(None),
            verdicts: RefCell::new(HashMap::new()),
            triage_buttons: RefCell::new(Vec::new()),
            convert_panel: None,
            sections: HashMap::new(),
            strip_tab: StripTab::Scenes,
            kept_sections: None,
            entry_first: 0,
            entry_target: None,
            chapters: HashMap::new(),
            entry_frames: HashMap::new(),
            section_finder: None,
            palette_recent: None,
            triage_over: None,
            list_search: None,
            list_cursor: None,
            row_target: None,
            scenes_on: false,
            scene_over: None,
            scenes: HashMap::new(),
            scene_finder: None,
            video_render: None,
            resized_at: None,
            video_scratch: Vec::new(),
            subtitles: options.subtitles,
            subtitle_size: options.subtitle_size,
            subtitle_margin: SUBTITLE_MARGIN,
            toast: None,
            toast_links: Vec::new(),
            toast_buttons: Vec::new(),
            time_until: None,
            meta: None,
            meta_asked: None,
            text: None,
            video_view_sent: RefCell::new(None),
            scrolled: 0.0,
            modifiers: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            last_click: None,
            drag: None,
            slide_delay,
            slide_at: options.slideshow_delay.map(|_| Instant::now() + slide_delay),
            rng: Rng::from_clock(),
            window: None,
            surface: None,
            spaces,
            space,
            home: space,
            overview: None,
            overview_due: None,
            options,
            files,
        }
    }

    fn id(&self) -> usize {
        self.list[self.pos]
    }

    /// Whether timings are being printed to the terminal.
    pub fn debugging(&self) -> bool {
        self.options.debug
    }

    fn path(&self) -> &Path {
        &self.files[self.id()]
    }

    fn name(&self) -> String {
        self.name_of(self.id())
    }

    fn name_of(&self, id: usize) -> String {
        let path = &self.files[id];
        path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
    }

    /// List position that commands act on: the grid cursor when the grid is
    /// up, otherwise the image being viewed.
    fn focus(&self) -> usize {
        // A row of the file list a menu was brought up on comes first.
        let row = self.row_target.filter(|at| *at < self.list.len() && self.grid.is_none());
        row.unwrap_or_else(|| self.grid.as_ref().map_or(self.pos, |grid| grid.cursor))
    }

    /// Files a file command applies to: the marked ones in list order, or
    /// the focused one when nothing is marked.
    fn targets(&self) -> Vec<usize> {
        if self.marks.is_empty() {
            return vec![self.list[self.focus()]];
        }
        self.list.iter().copied().filter(|id| self.marks.contains(id)).collect()
    }

    /// Prints the marked files, for `--print-marked`.
    pub fn print_marked(&self) {
        if self.options.print_marked {
            for id in self.list.iter().filter(|id| self.marks.contains(id)) {
                println!("{}", self.files[*id].display());
            }
        }
    }

    /// Quits, at once. What is to be kept has been written by now or is
    /// written here; shutting the players down in good order takes most
    /// of a second with their sound open, and gains nothing.
    fn leave(&mut self, event_loop: &ActiveEventLoop) {
        if self.snapshot.is_some() {
            return event_loop.exit();
        }
        self.remember_position();
        self.save_session();
        self.print_marked();
        crate::transcode::abandon();
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::process::exit(0);
    }

    fn window_size(&self) -> (u32, u32) {
        self.window.as_ref().map_or((0, 0), |w| w.inner_size().into())
    }

    fn redraw(&self) {
        if let Some(window) = &self.window {
            self.asked.set(true);
            window.request_redraw();
        }
    }

    fn toast(&mut self, message: impl Into<String>) {
        // It slides in from the edge, unless one is already up: then the
        // words change where it stands, so that pressing a key again and
        // again does not set it sliding in each time.
        let showing = self.toast.as_ref().is_some_and(|(_, until)| *until > Instant::now());
        if !showing {
            self.toast_slide.set(Glide { from: 0.0, to: 1.0, since: Instant::now() });
        }
        self.toast = Some((message.into(), Instant::now() + TOAST));
        self.toast_links.clear();
        self.redraw();
    }

    /// A toast with something to click: Undo, or Show for a file made. It
    /// stays up longer, to give time to reach it.
    fn toast_with(&mut self, message: impl Into<String>, links: Vec<(String, ToastLink)>) {
        self.toast(message);
        if let Some((_, until)) = &mut self.toast {
            *until += TOAST;
        }
        self.toast_links = links;
    }

    /// A click on a toast's link: what it offers is done. Whether it was on
    /// one.
    fn toast_click(&mut self, x: f64, y: f64, event_loop: &ActiveEventLoop) -> bool {
        let on = |[left, top, w, h]: [i32; 4]| (left..left + w).contains(&(x as i32)) && (top..top + h).contains(&(y as i32));
        let Some((link, _)) = self.toast_buttons.iter().find(|(_, place)| on(*place)).cloned() else { return false };
        self.toast_buttons.clear();
        match link {
            ToastLink::Undo => self.execute(Command::Undo, event_loop),
            ToastLink::Show(path) => self.show_path(&path),
            ToastLink::Cancel => self.execute(Command::CancelEncode, event_loop),
        }
        true
    }

    /// Goes to a file, by its path: one just made, say.
    fn show_path(&mut self, path: &Path) {
        let Some(id) = self.files.iter().position(|file| file == path) else {
            return self.toast(format!("{} is not in the list", path.display()));
        };
        // A kept search that leaves it out gives way.
        if !self.list.contains(&id) && self.filter.is_some() {
            self.clear_filter(true);
        }
        if let Some(pos) = self.list.iter().position(|other| *other == id) {
            self.go_to(pos, pos > self.pos);
        }
    }

    fn strip_visible(&self) -> bool {
        // The strip of scenes stands where it would.
        if self.scenes_shown() {
            return false;
        }
        // While triaging it is up, to show what has been decided.
        self.strip_pinned || self.strip_hover || self.triage_on
    }

    // ---- Loading -------------------------------------------------------

    /// The images to have decoded around a position in a list: that one,
    /// then its neighbours, favouring the way `forward` says we are
    /// going. Stepping quickly, several are on their way at once, and a
    /// large file takes a few steps' worth of time to decode; but only
    /// as many ahead as there are threads to decode them, or the next
    /// one waits its turn behind those further on.
    fn around(list: &[usize], pos: usize, forward: bool, workers: usize) -> Vec<usize> {
        let (len, pos) = (list.len() as i64, pos as i64);
        let dir = if forward { 1 } else { -1 };
        let ahead = workers.clamp(3, 6) as i64;
        let offsets = [0, 1, -1, 2, 3, -2].into_iter().chain(4..=ahead).filter(|offset| *offset <= ahead);
        let mut wanted = Vec::new();
        for offset in offsets {
            let id = list[(pos + offset * dir).rem_euclid(len) as usize];
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        wanted
    }

    /// What to ask the loader for before there is a window: the images
    /// around the one to start on, fitted to a screen of `screen`.
    pub fn first_request(count: usize, start: usize, screen: (u32, u32), enlarge: bool, hdr: Hdr, workers: usize) -> Request {
        let list: Vec<usize> = (0..count).collect();
        Request { wanted: Self::around(&list, start, true, workers), target: screen, enlarge, hdr, ..Request::default() }
    }

    /// Tells the loader what is needed around the current position.
    fn request(&self) {
        if self.monitor.0 == 0 || self.monitor.1 == 0 {
            return;
        }
        // The file on view goes first among those viewed lately.
        if let Some(&id) = self.list.get(self.pos) {
            let mut visited = self.visited.borrow_mut();
            if visited.first() != Some(&id) {
                visited.retain(|had| *had != id);
                visited.insert(0, id);
                visited.truncate(50);
            }
        }
        let len = self.list.len() as i64;
        let wanted = Self::around(&self.list, self.pos, self.forward, self.loader.workers());
        // Thumbnails are sized for the monitor, like frames, so they survive
        // window resizes; and for the grid's cells, the larger of the two
        // uses, so that the filmstrip and the grid share them.
        let mut thumbs = Vec::new();
        let thumb_size = self.grid_layout(self.monitor).thumb_size();
        if let Some(grid) = &self.grid {
            // The page on show, then the pages either side of it.
            let per_page = self.grid_layout(self.window_size()).per_page() as i64;
            let page = grid.target as i64;
            for page in [page, page + 1, page - 1] {
                let positions = (page * per_page).clamp(0, len)..((page + 1) * per_page).clamp(0, len);
                thumbs.extend(positions.map(|at| self.list[at as usize]));
            }
        } else if self.strip_used {
            for distance in 0..=THUMB_RADIUS {
                let centre = self.strip_centre();
                for at in [centre + distance, centre - distance] {
                    if (0..len).contains(&at) && !thumbs.contains(&self.list[at as usize]) {
                        thumbs.push(self.list[at as usize]);
                    }
                }
            }
        }
        // The overview shows a few pictures of every workspace, and runs
        // through those of the one picked out.
        if let Some(overview) = &self.overview {
            thumbs.clear();
            for which in 0..self.spaces.len() {
                let ids = self.space_ids(which);
                let from = if which == overview.cursor { overview.tick } else { 0 };
                thumbs.extend((0..ids.len().min(6)).map(|step| ids[(from + step) % ids.len()]));
                thumbs.extend(ids.iter().take(4));
            }
            thumbs.dedup();
        }
        // The go-to box shows a small picture beside each match.
        {
            let shown = self.palette_shown();
            thumbs.retain(|id| !shown.contains(id));
            thumbs.splice(0..0, shown);
        }
        let request = Request {
            wanted,
            target: self.monitor,
            enlarge: self.view.enlarge,
            full: self.needs_full().then(|| self.id()),
            thumbs,
            thumb_size,
            hdr: self.hdr,
        };
        // Panning and zooming call this on every pointer move; most of the
        // time nothing has changed and the loader need not be woken.
        if self.last_request.borrow().as_ref() == Some(&request) {
            return;
        }
        self.loader.request(request.clone());
        *self.last_request.borrow_mut() = Some(request);
    }

    /// Whether the view shows more detail than the fitted frame holds.
    fn needs_full(&self) -> bool {
        let Some((id, frame)) = &self.shown else { return false };
        let window = self.window_size();
        if *id != self.id() || frame.is_native() || window.0 == 0 || window.1 == 0 {
            return false;
        }
        // A video's frame is only a still; the player draws the real thing.
        // An animation at full resolution would mean every picture of it.
        if files::is_video(&self.files[*id]) || frame.animation.is_some() {
            return false;
        }
        // The loupe shows the picture's own pixels.
        self.loupe.is_some() || self.layout_of(frame.native).width > frame.width
    }

    /// Syncs the title and displayed frame with the current image.
    fn refresh(&mut self) {
        let id = self.id();
        if self.perf.id != Some(id) {
            self.perf.id = Some(id);
            self.perf.arrived = Instant::now();
            self.perf.shown_in = None;
        }
        match self.loader.fitted(id) {
            Some(Ok(frame)) => {
                // A different image, or a redone frame of this one, starts
                // its animation from the first picture.
                if !self.shown.as_ref().is_some_and(|(shown, old)| *shown == id && Arc::ptr_eq(old, &frame)) {
                    self.picture_due = None;
                }
                self.shown = Some((id, frame));
                self.error = None;
            }
            Some(Err(reason)) => {
                self.shown = None;
                self.error = Some(reason);
            }
            None => {}
        }
        // The frame that just arrived may call for a full-resolution decode.
        self.request();
        self.sync_video();
        // The still may have just arrived, and with it the size to lay out.
        self.apply_video_view();
        if let Some(window) = &self.window {
            window.set_title(&format!("{} [{}/{}] - jog", self.name(), self.pos + 1, self.list.len()));
        }
        self.first_backdrop();
        self.redraw();
    }

    /// Hands the first picture to the display as the window's background,
    /// which the server paints the window with the moment the window
    /// manager shows it: a frame handed over the ordinary way before then
    /// is thrown away, and the next waits on the manager being done.
    fn first_backdrop(&mut self) {
        let Some(window) = self.window.clone() else { return };
        // Once the window is up, a frame reaches it the ordinary way at
        // once, and the background would only be the same picture twice.
        if self.backdrop_done || self.focused_once || self.perf.shown_in.is_some() || self.snapshot.is_some() || self.shown.is_none() {
            return;
        }
        self.backdrop_done = true;
        let started = Instant::now();
        let (width, height) = self.window_size();
        self.backdrop = x11::Backdrop::new(&window, width, height).map(|mut backdrop| {
            self.paint(backdrop.pixels(), (width, height));
            backdrop.show();
            (backdrop, Instant::now())
        });
        if self.options.debug {
            let how = if self.backdrop.is_some() { "as the window's background" } else { "not as the window's background" };
            eprintln!("jog: {:>7.1?} start first frame {how}, {:.1?} since the loop began", started.elapsed(), self.perf.arrived.elapsed());
        }
    }

    fn update_monitor(&mut self) {
        let Some(window) = &self.window else { return };
        // With the screen off there may be no monitor, or a stand-in a
        // pixel across; the window is then the guide to the size wanted.
        let monitor = window.current_monitor().map_or(PhysicalSize::new(0, 0), |m| m.size());
        let inner = window.inner_size();
        let size = PhysicalSize::new(monitor.width.max(inner.width), monitor.height.max(inner.height));
        if size.width > 0 && size.height > 0 && self.monitor != (size.width, size.height) {
            self.monitor = (size.width, size.height);
            self.request();
        }
        // For the next run to decode its first image for, before it has
        // a display to ask.
        crate::screen::remember(self.monitor);
    }

    // ---- Navigation ----------------------------------------------------

    fn go_to(&mut self, pos: usize, forward: bool) {
        if pos == self.pos {
            return;
        }
        self.pos = pos;
        // The filmstrip comes back to the file on view, and has moved
        // under the pointer.
        self.strip_scroll = 0;
        self.strip_over = self.strip_cell_at(self.mouse.0, self.mouse.1);
        self.forward = forward;
        self.error = None;
        self.reset_view();
        if self.slide_at.is_some() {
            self.slide_at = Some(Instant::now() + self.slide_delay);
        }
        self.request();
        self.refresh();
    }

    // ---- Video ---------------------------------------------------------

    /// Starts or stops the player so that it is playing exactly when a video
    /// is the image on view.
    fn sync_video(&mut self) {
        let id = self.id();
        let playing = self.video.as_ref().map(|video| video.id);
        // With the grid up, the video that was on view plays on in a corner.
        let wanted = (self.grid.is_none() || self.pip) && self.overview.is_none() && files::is_video(&self.files[id]);
        if wanted && playing != Some(id) {
            self.start_video(id);
        } else if !wanted && playing.is_some() {
            self.remember_position();
            self.video = None;
            self.hover = None;
            if let Some(player) = &self.player {
                player.stop();
            }
        }
        self.preload_video();
    }

    /// Starts the player on another thread, once; `Event::Player` delivers
    /// it. Loading libmpv and starting mpv takes long enough to notice.
    fn warm_player(&mut self) {
        if std::mem::replace(&mut self.player_asked, true) {
            return;
        }
        let proxy = self.proxy.clone();
        // A scripted test run must not make any sound.
        let silent = self.snapshot.is_some() || self.bench.is_some();
        std::thread::spawn(move || {
            let wake = proxy.clone();
            let role = Role::Playback { silent };
            let player = Player::new(role, move |signal| drop(wake.send_event(Event::Video(0, signal))));
            let playable = player.is_ok();
            let _ = proxy.send_event(Event::Player(player));
            if playable {
                let wake = proxy.clone();
                let preview = Player::new(Role::Preview, move |signal| drop(wake.send_event(Event::Preview(signal))));
                let _ = proxy.send_event(Event::PreviewPlayer(preview));
                let wake = proxy.clone();
                let spare = Player::new(role, move |signal| drop(wake.send_event(Event::Video(1, signal))));
                let _ = proxy.send_event(Event::Spare(spare));
            }
        });
    }

    /// Asks the preview player for the frame at a point along the video.
    fn request_preview(&mut self, fraction: f64) {
        let Some((_, length)) = self.video_time() else { return };
        self.preview_want = Some(fraction * length);
        self.pump_preview();
    }

    /// Sends the preview player the latest spot asked for, unless it is
    /// still fetching the one before. Pointer movement asks far faster than
    /// frames can be fetched; this way the requests in between are skipped.
    fn pump_preview(&mut self) {
        let (Some(preview), Some(video), Some(want)) = (&self.preview, &self.video, self.preview_want) else {
            return;
        };
        // A request that never produced a frame must not block the rest.
        let busy = self.preview_busy.is_some_and(|since| since.elapsed() < PREVIEW_TIMEOUT);
        if busy {
            return;
        }
        // Another video than the one it has open: that one is opened, and
        // the spot is asked for afresh even if it is the same time along
        // as the last one asked of the video before.
        if self.preview_id != Some(video.id) {
            preview.open(&self.files[video.id]);
            self.preview_id = Some(video.id);
            self.preview_frame = None;
            self.preview_sent = None;
        }
        // The nearest keyframe is close enough, and much quicker to reach.
        // Paused, with the pointer still, the keyframe is followed by the
        // exact frame there, what a click would show.
        let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
        let exact = match self.preview_sent {
            Some((sent, exact)) if sent == want => match exact || !paused {
                true => return,
                false => true,
            },
            _ => false,
        };
        let how: &[u8] = if exact { b"absolute+exact" } else { b"absolute+keyframes" };
        preview.command_later(&[b"seek", want.to_string().as_bytes(), how]);
        self.preview_sent = Some((want, exact));
        self.preview_busy = Some(Instant::now());
    }

    fn preview_signal(&mut self, signal: Signal) {
        let Some(preview) = &self.preview else { return };
        if let Signal::Events = signal {
            // Only to empty the queue, so that mpv keeps signalling.
            preview.events();
            return;
        }
        // Size the picture to the video so that no bars are drawn into it.
        let native = self.shown.as_ref().map_or((16, 9), |(_, frame)| frame.native);
        let unit = ui_px(self.window_size().1);
        let (width, height) = fit_size(native, (unit * 16, unit * 12), true);
        let frame = self.preview_frame.get_or_insert_with(|| Frame::still(width, height, native, Vec::new()));
        (frame.width, frame.height) = (width, height);
        frame.pixels.resize(width as usize * height as usize, 0);
        preview.render(&mut frame.pixels, width, height);
        self.preview_busy = None;
        self.pump_preview();
        if self.hover.is_some() {
            self.redraw();
        }
    }

    /// Jumps to a point along the video, from 0 to 1: to that exact frame,
    /// or to the nearest keyframe, which is quick enough to do repeatedly
    /// while dragging.
    fn seek_to(&mut self, fraction: f64, exact: bool) {
        let (Some(player), Some((_, length))) = (&self.player, self.video_time()) else { return };
        let target = fraction.clamp(0.0, 1.0) * length;
        let how: &[u8] = if exact { b"absolute+exact" } else { b"absolute+keyframes" };
        player.command_later(&[b"seek", target.to_string().as_bytes(), how]);
        if let Some(video) = &mut self.video {
            // Where it lands is only known beforehand when that is exact.
            video.seeking = exact.then(|| (Instant::now(), target));
            video.seek_asked.get_or_insert_with(Instant::now);
            video.to_end = (target >= length - VIDEO_END).then(Instant::now);
        }
        self.redraw();
    }

    /// Where the seek bar's track is: inset from the sides and lifted off
    /// the bottom edge, or off the filmstrip when that is pinned open.
    fn seek_bar(&self) -> SeekBar {
        let (width, height) = self.window_size();
        let unit = ui_px(height) as i32;
        let base = if self.strip_pinned || self.triage_on || self.scenes_shown() { Strip::for_height(height).top } else { height as i32 };
        // Room on the left for the three buttons and the elapsed time, and
        // on the right for the duration and the buttons there.
        let (end, buttons) = (width as i32, self.bar_buttons().len() as i32);
        let right = unit * 7 + buttons * unit * 9 / 4;
        // Beside the file list when that is up, not under it.
        let left = self.list_panel().filter(|_| !self.list_right).map_or(0, |panel| panel.width);
        let end = end - self.right_inset();
        SeekBar { x: left + unit * 13, width: (end - left - unit * 13 - right).max(1), middle: base - unit * 2, unit, left, end, buttons }
    }

    /// Tracks the pointer over the seek bar: shows it at full size, and the
    /// frame for the spot under the pointer.
    fn video_pointer(&mut self, x: f64, y: f64) {
        let bar = self.seek_bar();
        // From a little above the bar down to the edge it is lifted off.
        let zone = f64::from(bar.middle - bar.unit * 4)..f64::from(bar.middle + bar.unit * 2);
        let fraction = ((x - f64::from(bar.x)) / f64::from(bar.width)).clamp(0.0, 1.0);
        let hover = zone.contains(&y).then_some(fraction);
        if hover != self.hover {
            self.hover = hover;
            if let Some(fraction) = hover {
                self.request_preview(fraction);
            }
            self.redraw();
        }
    }

    fn player_ready(&mut self, player: Result<Player, String>, event_loop: &ActiveEventLoop) {
        match player {
            Ok(player) => {
                player.set("speed", &self.playing_speed().to_string());
                player.set("mute", if self.muted { "yes" } else { "no" });
                self.player = Some(player);
                self.apply_subtitles();
                *self.video_view_sent.borrow_mut() = None;
                // A video may already be waiting for it.
                self.sync_video();
                // mpv signals only when an event joins an empty queue, and
                // may have queued some before it had anyone to signal.
                self.video_signal(Signal::Events);
            }
            Err(error) => {
                // Without a player, videos would only be entries that cannot
                // be shown. Finding that out here, rather than before the
                // window opens, keeps libmpv's loading time off the start.
                let videos: Vec<(usize, usize)> = (self.list.iter().enumerate())
                    .filter(|(_, id)| files::is_video(&self.files[**id]))
                    .map(|(pos, id)| (*id, pos))
                    .collect();
                let count = videos.len();
                eprintln!("jog: leaving out {count} videos; playing them needs libmpv (the libmpv1 or libmpv2 package): {error}");
                self.toast(format!("Cannot play videos, leaving out {count}: {error}"));
                if count > 0 {
                    self.drop_from_list(&videos, event_loop);
                }
            }
        }
    }

    fn start_video(&mut self, id: usize) {
        self.warm_player();
        if self.player.is_none() {
            return;
        }
        // The video being left, if this replaces one; and its frames kept.
        self.remember_position();
        self.drop_back();
        // Paused on the video being left, the next is opened paused too, so
        // going back and forth across files to look does not set them
        // playing. Stopped at its end is no pause asked for: the next plays.
        let paused = self.video.is_some()
            && self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true) && player.flag("eof-reached") != Some(true));
        // A seek carried over from the file beside this one says where to
        // begin; otherwise it is where the video was left.
        let carried = self.enter_at.take();
        let from = carried.or_else(|| self.resume_from(id));
        let mut held = false;
        let speed = self.playing_speed().to_string();
        if let Some(player) = &mut self.player {
            let started = Instant::now();
            // Seeking one that is held somewhere else takes longer than
            // opening the file afresh, so only one held where this is to
            // begin is taken.
            let start = from.unwrap_or(0.0);
            match self.spare.as_mut().filter(|spare| spare.id == Some(id) && (start - spare.from).abs() < 0.01) {
                // The spare player has it open already, and takes over.
                Some(spare) => {
                    std::mem::swap(player, &mut spare.player);
                    self.active ^= 1;
                    let old = &spare.player;
                    old.stop();
                    spare.id = None;
                    player.set("speed", &speed);
                    player.set("mute", if self.muted { "yes" } else { "no" });
                    if let Some(volume) = old.number("volume") {
                        player.set("volume", &volume.to_string());
                    }
                    *self.video_view_sent.borrow_mut() = None;
                    // What it said while waiting was said to no one.
                    player.events();
                    player.set("pause", if paused { "yes" } else { "no" });
                    held = true;
                }
                None => {
                    player.play(&self.files[id], from, paused);
                    if self.options.debug {
                        eprintln!("jog: {:>7.1?} video asked for", started.elapsed());
                    }
                }
            }
            // Until the player reports for itself, this is where it is.
            // One counted from the end has to wait for the length.
            let seeking = from.filter(|seconds| *seconds >= 0.0).map(|seconds| (started, seconds));
            self.video = Some(Video { id, showing: false, started, seeking, start_took: None, seek_asked: None, seek_took: None, held, tracks: None, shown_at: None, watched: false, to_end: None });
            self.perf.video_lines = None;
            self.apply_video_view();
        }
        if let Some(seconds) = from.filter(|_| carried.is_none()) {
            self.toast(format!("Resumed at {}", clock(seconds)));
        }
        self.apply_loop();
        if held {
            // It may have nothing more to say before its frame is shown.
            self.video_signal(Signal::Events);
        }
    }

    /// Where a video was left, when those places are being remembered.
    fn resume_from(&mut self, id: usize) -> Option<f64> {
        match self.options.resume {
            true => self.positions.get_or_insert_with(Positions::load).get(&self.files[id]),
            false => None,
        }
    }

    /// Gives the spare player the video that comes after the file on view,
    /// or before it when going backwards, to hold ready on its first frame.
    /// Starting a video cold takes long enough to see; this way the one
    /// most likely to be wanted next plays at once.
    fn preload_video(&mut self) {
        let len = self.list.len();
        if self.spare.is_none() || len < 2 || self.grid.is_some() || self.overview.is_some() {
            return;
        }
        let next = self.list[if self.forward { (self.pos + 1) % len } else { (self.pos + len - 1) % len }];
        if !files::is_video(&self.files[next]) || next == self.id() {
            return;
        }
        let from = self.resume_from(next);
        let start = from.unwrap_or(0.0);
        let Some(spare) = self.spare.as_mut().filter(|spare| spare.id != Some(next) || spare.from != start) else { return };
        spare.player.hold(&self.files[next], from);
        (spare.id, spare.from) = (Some(next), start);
    }

    /// The spare player has only to be listened to: mpv signals again only
    /// once its queue is empty, and stalls if a frame it has announced is
    /// not taken.
    fn spare_signal(&mut self, signal: Signal) {
        let Some(spare) = &self.spare else { return };
        match signal {
            Signal::Events => drop(spare.player.events()),
            Signal::Frame => drop(spare.player.render(&mut [0; 64], 8, 8)),
        }
    }

    /// Notes where the video on view has got to, to pick up there next
    /// time. One left near its start or its end starts over, as does a clip
    /// too short to lose one's place in.
    pub fn remember_position(&mut self) {
        let (Some(player), Some(video), true) = (&self.player, &self.video, self.options.resume) else { return };
        // Before its first frame the player has nothing true to report.
        let (Some(position), Some(length), true) = (player.number("time-pos"), player.number("duration"), video.showing)
        else {
            return;
        };
        let worth_it = length >= RESUME_MIN_LENGTH && position >= RESUME_MARGIN && position <= length - RESUME_MARGIN;
        let path = self.files[video.id].clone();
        self.positions.get_or_insert_with(Positions::load).set(&path, worth_it.then_some(position));
    }

    fn video_signal(&mut self, signal: Signal) {
        let Some(player) = &self.player else { return };
        // Read the events even when no video is wanted: mpv only signals
        // again once its queue has been emptied.
        let mut events = if let Signal::Events = signal { player.events() } else { Vec::new() };
        let mut video_started = None;
        // Whether the time along is written anywhere: on the seek bar, in
        // the info line, under the grid, or along the scenes. Elsewhere
        // its moving on with every frame changes nothing in the window.
        let time_shown = self.hover.is_some() || self.time_until.is_some() || self.show_info || self.show_debug || self.grid.is_some() || self.scenes_shown() || self.slide_at.is_some();
        let Some(video) = &mut self.video else { return };
        // One held ready by the spare player announced its frame before it
        // was this player, so that has to be looked for.
        if video.held && !video.showing && player.holds(&self.files[video.id]) {
            events.insert(0, PlayerEvent::Showing);
        }
        let mut at_end = false;
        // Whether there is anything new to draw: a frame, or a property
        // that moved, the time along say. The player also answers each
        // setting made, and a frame drawn for the answer would set it
        // again, the bar on its way up moving the subtitles' margin: a
        // round of frames as fast as the answers come, until it stops.
        let mut changed = matches!(signal, Signal::Frame);
        for event in events {
            if event == PlayerEvent::AtEnd {
                at_end = true;
                continue;
            }
            if event == PlayerEvent::Changed || (event == PlayerEvent::Moved && time_shown) {
                changed = true;
                continue;
            }
            if event == PlayerEvent::Moved {
                continue;
            }
            // The still stays up until the player has a frame to show.
            let showing = event == PlayerEvent::Showing;
            if showing && !video.showing {
                let took = video.started.elapsed();
                video.start_took.get_or_insert(took);
                video_started = Some(took);
                video.shown_at.get_or_insert_with(Instant::now);
                if self.options.debug {
                    eprintln!("jog: {took:>7.1?} video start {}", self.files[video.id].display());
                }
                if self.options.verbose {
                    let codec = player.text("video-format").unwrap_or_default();
                    let rate = player.number("container-fps").map(|rate| format!(" {rate:.2} fps")).unwrap_or_default();
                    let length = player.number("duration").map(|length| format!(", {} long", clock(length)));
                    let from = video.seeking.map(|(_, seconds)| format!(", resumed at {}", clock(seconds)));
                    let (length, from) = (length.unwrap_or_default(), from.unwrap_or_default());
                    eprintln!("jog:     playing in {}: {codec}{rate}{length}{from}", ms(took));
                }
            } else if showing {
                // Every seek ends with the player announcing frames again.
                if let Some(asked) = video.seek_asked.take() {
                    video.seek_took = Some(asked.elapsed());
                    if self.options.debug {
                        eprintln!("jog: {:>7.1?} seek", asked.elapsed());
                    }
                    video.to_end = video.to_end.map(|_| Instant::now());
                }
            }
            video.showing = showing;
        }
        // mpv stalls, for over half a second when changing file, if a frame
        // it has announced is not taken. Redrawing takes it, but that only
        // happens once the video is showing; until then take it here and
        // discard it. The hidden window of a scripted run is drawn like
        // any other, and takes its frames the same way.
        if matches!(signal, Signal::Frame) && !video.showing {
            // At the size it last drew at: another has it set itself up anew.
            let window: (u32, u32) = self.window.as_ref().map_or((0, 0), |w| w.inner_size().into());
            let (width, height) = self.video_render.unwrap_or(window);
            ui::size_for(&mut self.scene, width as usize * height as usize);
            self.scene_key = None;
            player.render(&mut self.scene, width, height);
        }
        // A video that played to its end makes way for the next file. One
        // taken to its end by hand, by seeking or stepping there, stays; and
        // the event may be left over from the file before this one. A seek
        // to short of the end is not that: what plays out after it goes on.
        let by_hand = video.to_end.is_some_and(|at| video.seek_asked.is_some() || at.elapsed() < SEEK_SETTLE);
        // Watched: it has played for a while, or right through. A seek on
        // past the first few seconds counts as well, and is seen to there.
        let over = at_end && player.flag("eof-reached") == Some(true);
        let watched = !video.watched && (over || video.shown_at.is_some_and(|at| at.elapsed() >= SEEN_AFTER));
        video.watched |= watched;
        // Which tracks it has is asked once it has been playing a moment:
        // asked while it starts, each answer waits on the player's core.
        let asked = video.tracks.is_none() && video.shown_at.is_some_and(|at| at.elapsed() >= TRACKS_AFTER);
        if asked {
            video.tracks = Some((tracks_of(player, "sub").len(), tracks_of(player, "audio").len()));
        }
        let (id, showing) = (video.id, video.showing);
        if (showing && (changed || video_started.is_some())) || asked {
            self.redraw();
        }
        if watched {
            self.mark_seen(id);
        }
        if at_end && !by_hand && player.flag("eof-reached") == Some(true) {
            self.play_next();
        }
        if let (Some(took), Some("video")) = (video_started, self.bench_scene()) {
            self.bench_note("video", "start to first frame", took);
        }
    }

    /// Goes on to the next file after a video, unless that is the
    /// slideshow's job, something is open that the user is in the middle
    /// of, or it was the last file. Looping the list goes on from the last
    /// file to the first, and does so even where videos otherwise stay.
    fn play_next(&mut self) {
        let open = self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some();
        let occupied = open || self.drag.is_some() || self.palette.is_some();
        let around = self.looping == Looping::List;
        if self.slide_at.is_some() || occupied || !(around || self.options.autoplay && self.pos + 1 < self.list.len()) {
            return;
        }
        match self.list.len() {
            // The whole list is this one video.
            1 => self.restart_video(),
            _ => self.step(true),
        }
    }

    /// Plays the video from its start, also when it had stopped at its end.
    fn restart_video(&mut self) {
        self.seek_to(0.0, true);
        if let (Some(player), Some(_)) = (&self.player, &self.video) {
            player.set("pause", "no");
        }
    }

    /// Steps on to the next way of looping, and says which it is.
    fn cycle_loop(&mut self) {
        self.looping = match self.looping {
            Looping::Off => Looping::File,
            Looping::File => Looping::List,
            Looping::List => Looping::Off,
        };
        self.apply_loop();
        // One already stopped at its end has to be set going again.
        let ended = self.video.is_some() && self.player.as_ref().is_some_and(|player| player.flag("eof-reached") == Some(true));
        match self.looping {
            Looping::File if ended => self.restart_video(),
            Looping::List if ended => self.play_next(),
            _ => {}
        }
        let (repeat, once) = (commands::icons::REPEAT, commands::icons::REPEAT_1);
        self.toast(match self.looping {
            Looping::Off => format!("{repeat}  Loop off"),
            Looping::File => format!("{once}  Looping this video"),
            Looping::List => format!("{repeat}  Looping the whole list"),
        });
    }

    /// Whether a video has been watched, as far as is remembered.
    fn watched(&self, id: usize) -> bool {
        *self.seen_known.borrow_mut().entry(id).or_insert_with(|| self.seen.borrow_mut().get_or_insert_with(Seen::load).has(&self.files[id]))
    }

    /// Whether to mark a file as one not watched yet: a video that is not
    /// remembered as watched, where that is being kept track of.
    fn is_new(&self, id: usize) -> bool {
        self.options.seen && files::is_video(&self.files[id]) && !self.watched(id)
    }

    /// How strongly a file is marked as not watched yet, from 0 to 1: the
    /// mark of one just watched fades away over a moment.
    fn new_level(&self, id: usize) -> f64 {
        if self.is_new(id) {
            return 1.0;
        }
        let fading = self.seen_at.borrow().get(&id).map(|at| at.elapsed()).filter(|gone| *gone < DOT_FADE && self.options.animations);
        fading.map_or(0.0, |gone| {
            self.animating.set(true);
            1.0 - gone.as_secs_f64() / DOT_FADE.as_secs_f64()
        })
    }

    /// Remembers a video as watched.
    fn mark_seen(&self, id: usize) {
        if self.options.seen && self.seen_known.borrow_mut().insert(id, true) != Some(true) {
            self.seen_at.borrow_mut().insert(id, Instant::now());
            self.seen.borrow_mut().get_or_insert_with(Seen::load).add(&self.files[id]);
        }
    }

    /// The two players that take turns, where they exist.
    fn players(&self) -> impl Iterator<Item = &Player> {
        self.player.iter().chain(self.spare.iter().map(|spare| &spare.player))
    }

    /// Tells both players how subtitles are to look and whether they show.
    /// Those a file styles for itself keep their own look.
    fn apply_subtitles(&self) {
        // mpv's sizes are in pixels of a picture 720 high, and its colours
        // are written `#AARRGGBB`.
        let size = (SUBTITLE_SIZE * f64::from(self.subtitle_size) / 100.0).to_string();
        let margin = self.subtitle_margin.to_string();
        let look: [(&str, &str); 6] = match self.options.subtitle_style {
            SubtitleStyle::Outline => [
                ("sub-border-size", "2.2"),
                ("sub-border-color", "#E6000000"),
                ("sub-shadow-offset", "0"),
                ("sub-shadow-color", "#00000000"),
                ("sub-back-color", "#00000000"),
                ("sub-blur", "0.3"),
            ],
            SubtitleStyle::Shadow => [
                ("sub-border-size", "0.9"),
                ("sub-border-color", "#40000000"),
                ("sub-shadow-offset", "1.4"),
                ("sub-shadow-color", "#C0000000"),
                ("sub-back-color", "#00000000"),
                ("sub-blur", "1.6"),
            ],
            // The shadow's offset is how far the panel reaches past the
            // letters.
            SubtitleStyle::Box => [
                ("sub-border-size", "0"),
                ("sub-border-color", "#00000000"),
                ("sub-shadow-offset", "7"),
                ("sub-shadow-color", "#00000000"),
                ("sub-back-color", "#A6000000"),
                ("sub-blur", "0"),
            ],
        };
        for player in self.players() {
            player.set("sub-font", &self.options.subtitle_font);
            player.set("sub-font-size", &size);
            player.set("sub-color", "#FFF4F4F4");
            player.set("sub-margin-y", &margin);
            player.set("sub-visibility", if self.subtitles { "yes" } else { "no" });
            for (name, value) in look {
                player.set(name, value);
            }
        }
    }

    /// Steps on to the video's next subtitle track, and after the last to
    /// none. Whether subtitles show is kept from one video to the next.
    fn cycle_subtitles(&mut self) {
        let Some(player) = self.player.as_ref().filter(|_| self.video.is_some()) else { return };
        let tracks = tracks_of(player, "sub");
        if tracks.is_empty() {
            return self.toast("This video has no subtitles");
        }
        let showing = tracks.iter().position(|track| track.selected).filter(|_| self.subtitles);
        let next = match showing {
            None => tracks.first(),
            Some(at) => tracks.get(at + 1),
        };
        if let Some(track) = next {
            player.set("sid", &track.id.to_string());
        }
        self.subtitles = next.is_some();
        self.apply_subtitles();
        self.toast(match next {
            Some(track) => format!("{}  {}", commands::icons::CAPTIONS, track.name),
            None => format!("{}  Subtitles off", commands::icons::CAPTIONS_OFF),
        });
    }

    /// Steps on to the video's next audio track, and round to the first.
    fn cycle_audio(&mut self) {
        let Some(player) = self.player.as_ref().filter(|_| self.video.is_some()) else { return };
        let tracks = tracks_of(player, "audio");
        if tracks.len() < 2 {
            return self.toast(if tracks.is_empty() { "This video has no sound" } else { "This video has one audio track" });
        }
        let at = tracks.iter().position(|track| track.selected).map_or(0, |at| (at + 1) % tracks.len());
        player.set("aid", &tracks[at].id.to_string());
        self.toast(format!("{}  {}   {} of {}", commands::icons::AUDIO_LINES, tracks[at].name, at + 1, tracks.len()));
    }

    /// Makes subtitles a step larger or smaller.
    fn size_subtitles(&mut self, larger: bool) {
        let size = if larger { self.subtitle_size + 10 } else { self.subtitle_size.saturating_sub(10) };
        self.subtitle_size = size.clamp(25, 400);
        self.apply_subtitles();
        self.toast(format!("{}  Subtitles {}%", commands::icons::A_LARGE_SMALL, self.subtitle_size));
    }

    /// The buttons at the right of the seek bar, from the edge inwards:
    /// looping, sound on or off, and for a video that has them subtitles
    /// and audio tracks.
    fn bar_buttons(&self) -> Vec<Command> {
        let (subtitles, audio) = self.video.as_ref().and_then(|video| video.tracks).unwrap_or((0, 0));
        let mut buttons = vec![Command::CycleLoop, Command::ToggleMute];
        if subtitles > 0 {
            buttons.push(Command::CycleSubtitles);
        }
        if audio > 1 {
            buttons.push(Command::CycleAudio);
        }
        buttons
    }

    /// Has the spare player hold the file before this one, a seek's length
    /// from its end, once seeking back has come near enough to the start
    /// of this video that the next press may land there.
    fn preload_back(&mut self) {
        let len = self.list.len();
        let near = self.video_time().is_some_and(|(position, _)| position < SEEK_STEP * 2.0);
        if !near || len < 2 {
            return;
        }
        let before = self.list[(self.pos + len - 1) % len];
        if !files::is_video(&self.files[before]) || before == self.id() {
            return;
        }
        let Some(spare) = self.spare.as_mut().filter(|spare| spare.id != Some(before) || spare.from != -SEEK_STEP) else { return };
        spare.player.hold(&self.files[before], Some(-SEEK_STEP));
        (spare.id, spare.from) = (Some(before), -SEEK_STEP);
    }

    /// Tells the player whether a video is to play again when it ends.
    fn apply_loop(&self) {
        if let Some(player) = &self.player {
            player.set("loop-file", if self.looping == Looping::File { "inf" } else { "no" });
        }
        // The section of this video, or none: a video gone on to does not
        // loop the one before's.
        self.apply_section_loop();
    }

    /// Handles the keys that mean something else while a video plays.
    /// Returns whether the key was one of them.
    fn video_key(&mut self, key: Key<&str>, repeat: bool) -> bool {
        let modifiers = self.modifiers;
        if self.player.is_none() || modifiers.alt_key() {
            return false;
        }
        // Escape unmarks a section before it does anything else.
        if key == Key::Named(NamedKey::Escape) && self.section().is_some() {
            return self.clear_section();
        }
        // With the scenes up, the arrows go from one of them to the next.
        if self.scenes_shown() && !modifiers.control_key() && !modifiers.shift_key() {
            let way = match key {
                Key::Named(NamedKey::ArrowRight) => Some(true),
                Key::Named(NamedKey::ArrowLeft) => Some(false),
                _ => None,
            };
            let jumped = match self.strip_tab {
                StripTab::Scenes => way.is_some_and(|forward| self.scene_jump(forward)),
                StripTab::Sections => way.is_some_and(|forward| self.entry_jump(forward)),
            };
            if jumped {
                return true;
            }
        }
        // A held key goes in the same strides as a pressed one.
        let step = SEEK_STEP;
        let arrow = |forward: bool| {
            if modifiers.control_key() {
                VideoAction::File(forward)
            } else if modifiers.shift_key() {
                VideoAction::Frame(forward)
            } else {
                VideoAction::Seek(if forward { step } else { -step })
            }
        };
        let action = match key {
            // The repeats of a space bar held down say nothing new.
            Key::Named(NamedKey::Space) if repeat => return true,
            // Let go at once it pauses or plays; kept down, it hurries the
            // video along. Which, only its release can say.
            Key::Named(NamedKey::Space) => {
                self.begin_hold(true);
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => arrow(true),
            Key::Named(NamedKey::ArrowLeft) => arrow(false),
            // Page Down and Up are the next and the file before, as > and
            // < are; the arrows are for getting about in the video.
            Key::Named(NamedKey::PageDown | NamedKey::PageUp) => return false,
            Key::Named(NamedKey::ArrowUp) => VideoAction::Volume(5),
            Key::Named(NamedKey::ArrowDown) => VideoAction::Volume(-5),
            _ => return false,
        };
        // A held key seeks only as fast as the seeks land. A repeat that
        // comes while one is still on its way, or while a file just gone
        // on to has yet to show, is let go: nothing is left over to play
        // out once the key is released, and each jump is one stride.
        let busy = self.video.as_ref().is_some_and(|video| !video.showing || video.seek_asked.is_some_and(|asked| asked.elapsed() < SEEK_SETTLE));
        if repeat && busy && matches!(action, VideoAction::Seek(_)) {
            return true;
        }
        // The files lie end to end as one timeline: a seek that would run
        // off the end of this video starts the next, and one that would run
        // off its start lands that far from the end of the one before,
        // however the key came to be pressed.
        if let VideoAction::Seek(seconds) = action {
            if let Some(enter) = self.seek_across(seconds) {
                self.enter_at = Some(enter);
                self.step(seconds > 0.0);
                self.enter_at = None;
                // Going on back may run into the file before that one next.
                if seconds < 0.0 {
                    self.preload_back();
                }
                return true;
            }
        }
        self.video_action(action);
        true
    }

    /// Where a seek of this many seconds lands in the file after or before
    /// this one, when it would run off the end or the start of the video:
    /// the start of the next, as zero, or as a negative number of seconds from
    /// the end of the previous. `None` when it stays within this video.
    fn seek_across(&self, seconds: f64) -> Option<f64> {
        let (position, length) = self.video_time()?;
        let ended = self.player.as_ref().is_some_and(|player| player.flag("eof-reached") == Some(true));
        let to = if ended { length } else { position } + seconds;
        if self.list.len() < 2 {
            None
        } else if seconds > 0.0 && to >= length - VIDEO_END {
            Some(0.0)
        } else {
            // Less than this short of the start is the start. Further, and
            // it is the whole of the seek from the end of the file before,
            // which is where that file can be held ready.
            (seconds < 0.0 && to < -VIDEO_START).then_some(-seconds.abs())
        }
    }

    fn video_action(&mut self, action: VideoAction) {
        // A step among the frames kept from before the pause, at once; and
        // anything else is from the frame on show, the main player put on
        // it first.
        match action {
            VideoAction::Frame(forward) if self.back_step(forward) => return,
            VideoAction::Frame(_) | VideoAction::Volume(_) => {}
            _ => self.leave_back(),
        }
        let (Some(player), Some(video)) = (&self.player, &mut self.video) else { return };
        // What pausing or playing turns it into: the player's own word for
        // it comes later, with its events.
        let was_paused = player.flag("pause") == Some(true);
        match action {
            VideoAction::File(forward) => return self.step(forward),
            // Queued, not waited for: the player's core gets to a command
            // only when it is done with what it has in hand, which in the
            // middle of a seek is the rest of the seek.
            VideoAction::Pause => player.command_later(&[b"cycle", b"pause"]),
            VideoAction::Seek(seconds) => {
                // To the nearest keyframe, which the player can show at
                // once, unless the very moment asked for is wanted: getting
                // there means decoding every frame from the keyframe before.
                let how: &[u8] = if self.precise { b"relative+exact" } else { b"relative+keyframes" };
                player.command_later(&[b"seek", seconds.to_string().as_bytes(), how]);
                // mpv has not moved yet when asked straight after a seek, so
                // quick presses build on where the last one was heading.
                let from = match video.seeking {
                    Some((at, target)) if at.elapsed() < SEEK_SETTLE => target,
                    _ => player.number("time-pos").unwrap_or(0.0),
                };
                let length = player.number("duration").unwrap_or(0.0);
                let target = (from + seconds).clamp(0.0, length);
                // Where it lands is only known beforehand when that is
                // exact: a keyframe may be seconds either side, and the
                // seek bar is to show where the video is, not a guess.
                video.seeking = self.precise.then(|| (Instant::now(), target));
                video.to_end = (target >= length - VIDEO_END).then(Instant::now);
                video.seek_asked.get_or_insert_with(Instant::now);
            }
            // Both leave the video paused on the frame they land on.
            VideoAction::Frame(true) => {
                player.command_later(&[b"frame-step"]);
                video.to_end = Some(Instant::now());
            }
            VideoAction::Frame(false) => player.command_later(&[b"frame-back-step"]),
            VideoAction::Volume(change) => {
                player.command(&[b"add", b"volume", change.to_string().as_bytes()]);
                let volume = player.number("volume").unwrap_or(0.0);
                return self.toast(format!("Volume {volume:.0}%"));
            }
        }
        // Pausing says so for a moment, at the top right with the other
        // notes; playing again takes the word away.
        if action == VideoAction::Pause {
            let paused = !was_paused;
            match paused {
                true => self.toast(format!("{}  Paused", commands::icons::PAUSE)),
                false => self.toast = None,
            }
        }
        if matches!(action, VideoAction::Seek(_)) && self.video_time().is_some_and(|(position, _)| position >= SEEN_AFTER.as_secs_f64()) {
            if let Some(id) = self.video.as_ref().map(|video| video.id) {
                self.mark_seen(id);
            }
        }
        // The file a seek may be about to run into is held ready: the one
        // before once the start is near, and otherwise the next.
        match action {
            VideoAction::Seek(seconds) if seconds < 0.0 => self.preload_back(),
            VideoAction::Seek(_) => self.preload_video(),
            _ => {}
        }
        // Bring up the time and the larger progress bar for a moment.
        self.time_until = Some(Instant::now() + TOAST);
        self.redraw();
    }

    /// Moves the playback speed a step up or down the list, or with no
    /// direction back to normal. It applies to every video from then on.
    fn change_speed(&mut self, faster: Option<bool>) {
        let at = SPEEDS.iter().position(|speed| *speed >= self.speed).unwrap_or(SPEEDS.len() - 1);
        self.speed = match faster {
            Some(true) => SPEEDS[(at + 1).min(SPEEDS.len() - 1)],
            Some(false) => SPEEDS[at.saturating_sub(1)],
            None => 1.0,
        };
        if let Some(player) = &self.player {
            player.set("speed", &self.playing_speed().to_string());
        }
        self.toast(format!("Speed {}x", self.speed));
    }

    /// Where playback is and how long the video is, in seconds.
    fn video_time(&self) -> Option<(f64, f64)> {
        let (player, video) = (self.player.as_ref()?, self.video.as_ref()?);
        let length = player.number("duration").filter(|length| *length > 0.0)?;
        // A frame kept from before a pause, on show: its own time. Straight
        // after a seek mpv still reports where it was.
        let position = match (self.back_shown(), video.seeking) {
            (Some((at, _)), _) => at,
            (None, Some((at, target))) if at.elapsed() < SEEK_SETTLE => target,
            (None, _) => player.number("time-pos")?,
        };
        Some((position.clamp(0.0, length), length))
    }

    fn reset_view(&mut self) {
        if !self.keep_view {
            self.view = View { zoom: self.options.zoom, center: None, enlarge: self.view.enlarge };
        }
    }

    fn step(&mut self, forward: bool) {
        let len = self.list.len();
        let pos = if forward { (self.pos + 1) % len } else { (self.pos + len - 1) % len };
        self.go_to(pos, forward);
    }

    /// Moves about 5% of the way through the list.
    fn jump(&mut self, forward: bool) {
        let len = self.list.len() as i64;
        let distance = (len / 20).max(1) * if forward { 1 } else { -1 };
        self.go_to((self.pos as i64 + distance).rem_euclid(len) as usize, forward);
    }

    /// Position of the first image of the next or previous directory.
    fn other_dir(&self, forward: bool) -> Option<usize> {
        let len = self.list.len();
        let dir = |pos: usize| self.files[self.list[pos]].parent();
        let step = |pos: usize| if forward { (pos + 1) % len } else { (pos + len - 1) % len };

        let here = dir(self.pos);
        let mut pos = step(self.pos);
        while pos != self.pos && dir(pos) == here {
            pos = step(pos);
        }
        if pos == self.pos {
            return None;
        }
        if !forward {
            let there = dir(pos);
            while pos > 0 && dir(pos - 1) == there {
                pos -= 1;
            }
        }
        Some(pos)
    }

    fn remove_current(&mut self, event_loop: &ActiveEventLoop) {
        let id = self.list.remove(self.pos);
        self.transforms.remove(&id);
        self.marks.remove(&id);
        if let Some((_, all)) = &mut self.filter {
            all.retain(|other| *other != id);
        }
        if !self.refill(event_loop) {
            return;
        }
        if self.pos >= self.list.len() {
            self.pos = 0;
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
    }

    /// Called when files have left the list. If none of a kept search's
    /// matches remain, goes back to all the files; if no files remain at
    /// all, quits. Returns whether there is still something to show.
    fn refill(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.list.is_empty() {
            return true;
        }
        match self.filter.take() {
            Some((_, all)) if !all.is_empty() => {
                self.list = all;
                self.pos = 0;
                if let Some(grid) = &mut self.grid {
                    grid.cursor = 0;
                }
                true
            }
            _ => {
                event_loop.exit();
                false
            }
        }
    }

    fn advance_slideshow(&mut self, event_loop: &ActiveEventLoop) {
        self.slide_at = Some(Instant::now() + self.slide_delay);
        if self.pos + 1 == self.list.len() {
            match self.options.on_last_slide {
                LastSlide::Resume => {}
                LastSlide::Hold => {
                    self.slide_at = None;
                    return;
                }
                LastSlide::Quit => {
                    event_loop.exit();
                    return;
                }
            }
        }
        self.step(true);
    }

    // ---- View ----------------------------------------------------------

    fn native(&self) -> Option<(u32, u32)> {
        self.shown.as_ref().map(|(_, frame)| frame.native)
    }

    /// Moves an animated image on to its next picture, round to the first
    /// after the last. Pictures that fell due while something else held
    /// the thread up are skipped, so the animation keeps time.
    fn next_picture(&mut self, now: Instant) {
        let Some((_, frame)) = &self.shown else { return self.picture_due = None };
        let Some(animation) = &frame.animation else { return self.picture_due = None };
        let count = animation.frames.len() + 1;
        let mut due = self.picture_due.unwrap_or(now);
        while due <= now {
            self.picture = (self.picture + 1) % count;
            due += picture_delay(frame, self.picture);
        }
        self.picture_due = Some(due);
        self.redraw();
    }

    /// Call after any zoom or pan change.
    fn view_changed(&mut self) {
        self.settle_at = Some(Instant::now() + SETTLE);
        self.request();
        self.apply_video_view();
        self.redraw();
    }

    /// Passes the zoom, pan, rotation and mirroring on to the player, which
    /// does its own scaling. The still for the video, already turned the
    /// same way, supplies the size to lay out.
    fn apply_video_view(&self) {
        let (Some(player), Some(video)) = (&self.player, &self.video) else { return };
        let window = self.window_size();
        let native = self.shown.as_ref().filter(|(id, _)| *id == video.id).map(|(_, frame)| frame.native);
        let (zoom, pan) = match native {
            // In its corner of the grid it is simply all there.
            Some(native) if window.0 > 0 && window.1 > 0 && self.grid.is_none() => {
                let layout = self.layout_of(native);
                // mpv's zoom is a power of two on top of filling the window.
                let filling = fit_size(native, window, true);
                // Less than a pixel either way is the layout's rounding, and
                // not passed on: each change has the player's renderer set
                // itself up again, which for a large video costs ten frames'
                // time, and resizing the window would change it every time.
                let zoom = (f64::from(layout.width) / f64::from(filling.0)).log2();
                let zoom = if layout.width.abs_diff(filling.0) <= 1 { 0.0 } else { zoom };
                // Its pan is the offset from centre as a share of the scaled size.
                let pan = |origin: i32, size: u32, across: u32| {
                    let off = f64::from(origin) + f64::from(size) / 2.0 - f64::from(across) / 2.0;
                    if off.abs() <= 1.0 { 0.0 } else { off / f64::from(size) }
                };
                (zoom, (pan(layout.x, layout.width, window.0), pan(layout.y, layout.height, window.1)))
            }
            _ => (0.0, (0.0, 0.0)),
        };
        let transform = self.transforms.get(&video.id).copied().unwrap_or_default();
        // Done with filters: mpv's own rotate option trips an assertion in
        // its software renderer. A transform mirrors first, then turns.
        let mirror = transform.mirrored().then_some("hflip");
        let turn = match transform.turns() {
            1 => Some("transpose=clock"),
            2 => Some("hflip,vflip"),
            3 => Some("transpose=cclock"),
            _ => None,
        };
        let steps: Vec<&str> = mirror.into_iter().chain(turn).collect();
        let filter = if steps.is_empty() { String::new() } else { format!("lavfi=[{}]", steps.join(",")) };

        // Each property set is a round trip to the player's core, and this
        // runs on every pointer move of a pan, so only send what changed.
        let values = [zoom.to_string(), pan.0.to_string(), pan.1.to_string(), filter];
        let mut sent = self.video_view_sent.borrow_mut();
        for (index, name) in ["video-zoom", "video-pan-x", "video-pan-y", "vf"].into_iter().enumerate() {
            if sent.as_ref().is_none_or(|sent| sent[index] != values[index]) {
                // The player keeps its own HLG filter ahead of the view's.
                match name {
                    "vf" => player.set_filter(&values[index]),
                    _ => player.set(name, &values[index]),
                }
            }
        }
        *sent = Some(values);
    }

    /// Where the video's picture lies in the window: fitted and centred,
    /// as the player draws it. x, y, and its scale.
    fn video_place(size: (u32, u32), window: (u32, u32)) -> (f64, f64, f64) {
        let scale = (f64::from(window.0) / f64::from(size.0.max(1))).min(f64::from(window.1) / f64::from(size.1.max(1)));
        let x = (f64::from(window.0) - f64::from(size.0) * scale) / 2.0;
        let y = (f64::from(window.1) - f64::from(size.1) * scale) / 2.0;
        (x, y, scale)
    }

    fn set_zoom(&mut self, zoom: Zoom) {
        self.view.zoom = zoom;
        self.view.center = None;
        self.view_changed();
    }

    /// Zooms around `anchor`, a window position, or the middle of the window.
    fn zoom_by(&mut self, factor: f64, anchor: Option<(f64, f64)>) {
        let Some(native) = self.native() else { return };
        // In the part of the window the picture has, and measured from it.
        let (left, window) = self.stage();
        let anchor = anchor.map_or((f64::from(window.0) / 2.0, f64::from(window.1) / 2.0), |(x, y)| (x - f64::from(left), y));
        let scale = self.view.scale(native, window) * factor;
        self.view.zoom_to(scale, anchor, native, window);
        self.view_changed();
    }

    /// Whether the image, as zoomed, has parts off to the side of the window.
    fn wider_than_window(&self) -> bool {
        self.native().is_some_and(|native| self.layout_of(native).width > self.stage().1 .0)
    }

    fn pan(&mut self, dx: f64, dy: f64) {
        let Some(native) = self.native() else { return };
        self.view.pan(dx, dy, native, self.stage().1);
        self.view_changed();
    }

    fn transform(&mut self, step: Transform) {
        let id = self.id();
        let total = self.transforms.entry(id).or_default();
        *total = total.then(step);
        self.loader.set_transform(id, *total);
        // Turn what is on screen right away; the loader follows with a
        // properly fitted frame.
        if let Some((shown_id, frame)) = &self.shown {
            if *shown_id == id {
                self.shown = Some((id, Arc::new(frame.transformed(step))));
            }
        }
        self.view.center = None;
        self.view_changed();
    }

    fn toggle_fullscreen(&mut self) {
        let Some(window) = &self.window else { return };
        let fullscreen = window.fullscreen().is_none().then_some(Fullscreen::Borderless(None));
        window.set_fullscreen(fullscreen);
    }

    fn size_window_to_image(&mut self) {
        let (Some(window), Some(native)) = (&self.window, self.native()) else { return };
        if window.fullscreen().is_some() {
            return;
        }
        let scale = match self.view.zoom {
            Zoom::Scale(scale) => scale,
            _ => 1.0,
        };
        let scaled = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
        let (width, height) = fit_size((scaled(native.0), scaled(native.1)), self.monitor, false);
        let _ = window.request_inner_size(PhysicalSize::new(width, height));
    }

    // ---- Commands ------------------------------------------------------

    fn execute(&mut self, command: Command, event_loop: &ActiveEventLoop) {
        use Command::*;
        match command {
            Next => self.step(true),
            Previous => self.step(false),
            First => self.go_to(0, true),
            Last => self.go_to(self.list.len() - 1, false),
            JumpForward => self.jump(true),
            JumpBack => self.jump(false),
            NextDir | PreviousDir => {
                let forward = command == NextDir;
                if let Some(pos) = self.other_dir(forward) {
                    self.go_to(pos, forward);
                }
            }
            ZoomIn => self.zoom_by(ZOOM_STEP, None),
            ZoomOut => self.zoom_by(1.0 / ZOOM_STEP, None),
            ZoomActual => self.set_zoom(Zoom::Scale(1.0)),
            ZoomFit => self.set_zoom(Zoom::Fit),
            ZoomFill => self.set_zoom(Zoom::Fill),
            ToggleEnlarge => {
                self.view.enlarge = !self.view.enlarge;
                self.toast(if self.view.enlarge { "Small images fit the screen" } else { "Small images at actual size" });
                self.view_changed();
            }
            ToggleKeepView => {
                self.keep_view = !self.keep_view;
                self.toast(if self.keep_view { "Keeping zoom from one file to the next" } else { "Zoom starts afresh with each file" });
            }
            // With nothing to scroll sideways, Ctrl+Left and Ctrl+Right move
            // between files, as they do over a video.
            Scroll(dx, 0, false) if !self.wider_than_window() => self.step(dx > 0),
            Scroll(dx, dy, page) => {
                let window = self.window_size();
                let amount = |size: u32, dir: i8| f64::from(size) * f64::from(dir) / if page { 1.0 } else { 10.0 };
                // Scrolling right moves the image left.
                self.pan(-amount(window.0, dx), -amount(window.1, dy));
            }
            RotateRight => self.transform(Transform::CW),
            RotateLeft => self.transform(Transform::CCW),
            Flip => self.transform(Transform::FLIP),
            Mirror => self.transform(Transform::MIRROR),
            SectionStart => self.mark_edge(Edge::A),
            SectionEnd => self.mark_edge(Edge::B),
            SectionClear => {
                self.clear_section();
            }
            SaveSection(clip) => self.save_section(clip),
            KeepSection => self.keep_section(),
            RenameSection => self.ask_section_name(),
            ForgetSection => self.forget_section(),
            CancelEncode => {
                if !crate::transcode::cancel() {
                    self.toast("Nothing to cancel");
                }
            }
            ToggleAntialias => {
                self.antialias = !self.antialias;
                self.toast(if self.antialias { "Smooth scaling on" } else { "Smooth scaling off" });
            }
            Render => {
                self.settle_at = None;
                self.redraw();
            }
            ToggleFullscreen => self.toggle_fullscreen(),
            ToggleFilmstrip => {
                self.strip_pinned = !self.strip_pinned;
                self.strip_used = true;
                self.request();
                self.redraw();
            }
            // The two are different looks at the same file, in the same
            // corner: turning one on turns the other off.
            ToggleInfo => {
                self.show_info = !self.show_info;
                self.show_debug &= !self.show_info;
            }
            ToggleDebug => {
                self.show_debug = !self.show_debug;
                self.show_info &= !self.show_debug;
            }
            ToggleVerbose => {
                self.options.verbose = !self.options.verbose;
                self.toast(if self.options.verbose { "Printing files to the terminal" } else { "No longer printing files" });
            }
            ToggleDebugLog => {
                self.options.debug = !self.options.debug;
                self.loader.set_debug(self.options.debug);
                crate::mpv::DEBUG.store(self.options.debug, atomic::Ordering::Relaxed);
                self.toast(if self.options.debug { "Printing timings to the terminal" } else { "No longer printing timings" });
            }
            ToggleExif => self.show_exif = !self.show_exif,
            ToggleActions => self.show_actions = !self.show_actions,
            TogglePointer => {
                self.pointer_visible = !self.pointer_visible;
                if let Some(window) = &self.window {
                    window.set_cursor_visible(self.pointer_visible);
                }
            }
            ToggleMenu => match self.menu.take() {
                Some(_) => {}
                None => self.open_menu(commands::MENU, None),
            },
            ToggleGrid => self.toggle_grid(),
            ToggleHelp => {
                self.help = !self.help;
                self.help_search.clear();
            }
            ToggleSlideshow => {
                if self.slide_at.take().is_some() {
                    self.toast("Slideshow paused");
                } else {
                    self.slide_at = Some(Instant::now() + self.slide_delay);
                    self.toast(format!("Slideshow, {}s per image", self.slide_delay.as_secs_f64()));
                }
            }
            SizeToImage => self.size_window_to_image(),
            Reload => {
                self.loader.invalidate(self.id());
                self.meta = None;
                self.meta_asked = None;
                let listed = files::relist(&self.options.paths, &self.options.listing);
                self.take_listing(listed, false);
                self.request();
            }
            ToggleWatch => self.set_watch(self.watch.is_none()),
            ToggleList => {
                self.show_list = !self.show_list;
                self.apply_video_view();
                self.list_search = None;
            }
            ListSearch => {
                // The list comes up for it, if it was not there.
                self.show_list = true;
                self.focus_list_search();
            }
            // The picture has a different part of the window either way,
            // which a video has to be told of.
            ListPin => {
                self.list_pinned = !self.list_pinned;
                self.apply_video_view();
            }
            ListFlip => {
                self.list_right = !self.list_right;
                self.apply_video_view();
            }
            // From the grid, the picture under its cursor; from the file
            // list, the row the menu was brought up on.
            OpenFromGrid | PlayFromGrid if self.grid.is_some() => self.open_from_grid(),
            OpenFromGrid | PlayFromGrid => {
                if let Some(at) = self.row_target.take().filter(|at| *at < self.list.len()) {
                    let first = self.list_panel().map(|panel| panel.first);
                    self.go_to(at, at > self.pos);
                    self.list_scroll = first.map(|first| (self.pos, self.list.len(), first));
                }
            }
            GridLarger => self.zoom_grid(true),
            GridSmaller => self.zoom_grid(false),
            ToggleGridTitles => self.grid_titles = !self.grid_titles,
            GridMenu => {
                // At the cell, not wherever the pointer happens to be.
                if let Some(cursor) = self.grid.as_ref().map(|grid| grid.cursor) {
                    let layout = self.grid_layout(self.window_size());
                    let [left, top, width, height] = self.grid_area(&layout, cursor, self.window_size().1);
                    self.menu = Some(self.build_menu(commands::GRID_MENU, None, (left + width / 2, top + height / 2)));
                }
            }
            Scope(kind) => {
                self.scopes[kind] = !self.scopes[kind];
                // A graph asked for brings up the overlay it stands under.
                if self.scopes[kind] {
                    self.show_debug = true;
                    self.show_info = false;
                }
            }
            ScopeLog => self.scope_log = !self.scope_log,
            SetHdr(mode) => {
                self.hdr = mode;
                self.request();
                self.toast(format!("Ultra HDR: {}", mode.name()));
            }
            HdrHeadroom(up) => {
                self.hdr_headroom = (self.hdr_headroom + if up { 0.25 } else { -0.25 }).clamp(0.0, 6.0);
            }
            CycleHdr => {
                self.hdr = self.hdr.next();
                self.request();
                // The setting holds for every Ultra HDR photo from here on;
                // one that is not such a photo is shown as it always is.
                self.toast(match self.is_ultra_hdr(self.id()) {
                    true => format!("Ultra HDR: {}", self.hdr.name()),
                    false => format!("Ultra HDR photos: {} (this one is not one)", self.hdr.name()),
                });
            }
            CopyDebug | CopyDebugJson => {
                // As lines under headings, or as JSON for pasting elsewhere.
                let quoted = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
                let copied = if command == CopyDebug {
                    let cluster = |(title, figures): &Group| {
                        let lines: Vec<String> = figures.iter().map(|(name, value)| format!("  {name:<14}{value}")).collect();
                        format!("{title}\n{}", lines.join("\n"))
                    };
                    self.debug_last.iter().map(cluster).collect::<Vec<_>>().join("\n")
                } else {
                    let cluster = |(title, figures): &Group| {
                        let pairs: Vec<String> = figures.iter().map(|(name, value)| format!("{}: {}", quoted(name), quoted(value))).collect();
                        format!("{}: {{{}}}", quoted(title), pairs.join(", "))
                    };
                    format!("{{{}}}", self.debug_last.iter().map(cluster).collect::<Vec<_>>().join(", "))
                };
                println!("{copied}");
                match self.snapshot.is_some() || ops::set_clipboard(&copied) {
                    true => self.toast("Copied"),
                    false => self.toast("No clipboard tool (xclip, xsel, wl-copy)"),
                }
            }
            ToggleMute => {
                self.muted = !self.muted;
                if let Some(player) = &self.player {
                    player.set("mute", if self.muted { "yes" } else { "no" });
                }
                self.toast(if self.muted { "Muted" } else { "Sound on" });
            }
            Search | Palette => self.open_palette(),
            Remove => self.remove_current(event_loop),
            Trash => self.trash(event_loop),
            // Of a video it is the frame on view that is saved; of a picture,
            // a copy of it.
            Save if self.video.as_ref().is_some_and(|video| video.showing) => self.save_frame(),
            Save => {
                let copy = format!("jog_{}_{}", std::process::id(), self.name());
                if self.dry(|app| vec![format!("save a copy: {} -> {copy}", app.path().display())], "would save a copy") {
                    return;
                }
                match std::fs::copy(self.path(), &copy) {
                    Ok(_) => self.toast(format!("Saved {copy}")),
                    Err(error) => self.toast(format!("Could not save {copy}: {error}")),
                }
            }
            SaveFilelist => {
                let name = format!("jog_{}_filelist", std::process::id());
                // The marked files if there are any, otherwise the whole list.
                let ids = if self.marks.is_empty() { self.list.clone() } else { self.targets() };
                let lines: String = ids.iter().map(|&id| format!("{}\n", self.files[id].display())).collect();
                if self.dry(|_| vec![format!("write the list of {} files to {name}", ids.len())], "would save the file list") {
                    return;
                }
                match std::fs::write(&name, lines) {
                    Ok(()) => self.toast(format!("Saved {name}")),
                    Err(error) => self.toast(format!("Could not save {name}: {error}")),
                }
            }
            Action(slot) => self.run_action(slot),
            Mark => self.toggle_mark(),
            MarkMenu => self.open_mark_menu(),
            SortMenu => self.open_palette_with(">sort "),
            Workspace(to) => self.switch_space(to),
            WorkspaceKey(key) => match self.spaces.iter().position(|space| space.key == Some(key)) {
                Some(to) => self.switch_space(to),
                None => self.toast(format!("No workspace is on Ctrl+{key}")),
            },
            HomeWorkspace => self.switch_space(self.home),
            NextWorkspace => self.switch_space((self.space + 1) % self.spaces.len()),
            PrevWorkspace => self.switch_space((self.space + self.spaces.len() - 1) % self.spaces.len()),
            WorkspaceList => self.open_palette_with("@"),
            Overview => self.open_overview(),
            SaveWorkspace => self.ask_workspace_name(),
            SortBy(sort) => self.sort_by(sort),
            Shuffle => {
                self.shuffle();
                self.toast("Shuffled");
            }
            ReverseOrder => {
                self.reversed = !self.reversed;
                self.reorder(|ids, _| ids.reverse());
                self.toast("Order reversed");
            }
            OnlyMarked if self.marks.is_empty() => self.toast("Nothing is marked"),
            OnlyMarked => {
                // The same as typing it into the search box and keeping it.
                let (matches, _) = self.matching("is:marked");
                let picked = matches.first().copied();
                self.keep_matches("is:marked".to_string(), matches, picked);
            }
            MarkAll => {
                self.marks = self.list.iter().copied().collect();
                self.toast(format!("Marked all {}", self.marks.len()));
            }
            InvertMarks => {
                self.marks = self.list.iter().copied().filter(|id| !self.marks.contains(id)).collect();
                self.toast(format!("{} marked", self.marks.len()));
            }
            ClearMarks => {
                let count = self.marks.len();
                self.marks.clear();
                self.toast(format!("Cleared {count} marks"));
            }
            CopyName => self.copy_text(false),
            CopyViewedName | CopyViewedPath | CopyViewedSize => {
                let name = match command {
                    CopyViewedPath => {
                        let path = &self.files[self.id()];
                        std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()).display().to_string()
                    }
                    CopyViewedSize => self.native().map(|(w, h)| format!("{w}x{h}")).unwrap_or_default(),
                    _ => self.name(),
                };
                println!("[copy] {name}");
                if self.snapshot.is_some() || ops::set_clipboard(&name) {
                    self.toast(format!("Copied {name}"));
                } else {
                    self.toast("No clipboard tool (xclip, xsel, wl-copy)");
                }
            }
            CopyPath => self.copy_text(true),
            CopyShell => self.copy_for_shell(),
            CopyTo => self.send(false, event_loop),
            MoveTo => self.send(true, event_loop),
            CopyToAsk => self.ask_folder(false),
            MoveToAsk => self.ask_folder(true),
            Undo => self.undo(event_loop),
            Rename => self.start_rename(),
            Delete => self.ask_delete(),
            Convert(format) => self.convert(format),
            ConvertPanel => self.open_convert_panel(),
            ConvertVideo(target) => self.convert_videos(target),
            Compare => self.open_compare(),
            SpeedUp => self.change_speed(Some(true)),
            SpeedDown => self.change_speed(Some(false)),
            SpeedReset => self.change_speed(None),
            PlayPause => self.video_action(VideoAction::Pause),
            // Back to the video that is playing, the whole window of it.
            ShowPlaying => self.toggle_grid(),
            SeekForward => self.video_action(VideoAction::Seek(SEEK_STEP)),
            SeekTo(seconds) => {
                if let Some((_, length)) = self.video_time() {
                    self.seek_to(seconds / length, true);
                }
            }
            SeekBack => self.video_action(VideoAction::Seek(-SEEK_STEP)),
            FrameForward => self.video_action(VideoAction::Frame(true)),
            FrameBack => self.video_action(VideoAction::Frame(false)),
            VolumeUp => self.video_action(VideoAction::Volume(5)),
            VolumeDown => self.video_action(VideoAction::Volume(-5)),
            Restart => self.restart_video(),
            CycleLoop => self.cycle_loop(),
            TogglePrecise => {
                self.precise = !self.precise;
                self.toast(if self.precise { "Seeking to the exact moment" } else { "Seeking to the nearest keyframe, which is quicker" });
            }
            ToggleScenes => self.cycle_strip(),
            CycleSubtitles => self.cycle_subtitles(),
            CycleAudio => self.cycle_audio(),
            SubtitlesLarger => self.size_subtitles(true),
            SubtitlesSmaller => self.size_subtitles(false),
            ClearFilter => self.clear_filter(false),
            Shell => self.open_palette_with("!"),
            Triage => {
                self.triage_on = !self.triage_on;
                // Its filmstrip wants its pictures, as one asked for does.
                self.strip_used = true;
                self.request();
                self.apply_video_view();
                self.toast(if self.triage_on { "Triage: Space keeps, x rejects; nothing is done to the files" } else { "Triage left: what was decided is kept" });
            }
            Keep => self.decide(Verdict::Kept),
            Reject => self.decide(Verdict::Rejected),
            OnlyKept | OnlyRejected | OnlyUndecided => {
                let typed = match command {
                    OnlyKept => "is:kept",
                    OnlyRejected => "is:rejected",
                    _ => "is:undecided",
                };
                // The same as typing it into the search box and keeping it.
                let (matches, _) = self.matching(typed);
                match matches.first().copied() {
                    Some(first) => {
                        let picked = Some(self.id()).filter(|id| matches.contains(id)).or(Some(first));
                        self.keep_matches(typed.to_string(), matches, picked);
                    }
                    None => self.toast("There are none of those"),
                }
            }
            RunTyped => {
                if let Some(typed) = self.shell_typed.take() {
                    self.run_shell(&typed);
                }
            }
            OpenRecent(id) => {
                let path = self.files[id].clone();
                self.show_path(&path);
            }
            RunShell(at) => {
                if let Some(command) = self.shell_history.get_or_insert_with(history::load).get(at).cloned() {
                    self.run_shell(&command);
                }
            }
            RepeatShell => match self.shell_history.get_or_insert_with(history::load).first().cloned() {
                Some(command) => self.run_shell(&command),
                None => self.toast("No command has been run yet: ! is for typing one"),
            },
            ShowAllHere => self.clear_filter(true),
            Quit => self.leave(event_loop),
        }
        self.redraw();
    }

    fn run_action(&mut self, slot: usize) {
        let Some(template) = &self.options.actions[slot] else {
            // Enter and the digits are spare keys until actions are set
            // for them; only then is an unset one worth a word.
            if self.options.actions.iter().any(Option::is_some) {
                self.toast(format!("Nothing is set for key {slot}"));
            }
            return;
        };
        let seconds = self.video_time().map_or(0.0, |(position, _)| position);
        let command = expand_action(template, self.path(), seconds);
        if self.dry(|_| vec![command.clone()], &format!("would run action {slot}")) {
            return;
        }
        match std::process::Command::new("sh").arg("-c").arg(&command).spawn() {
            Ok(mut child) => {
                // Reap the child without holding up the viewer.
                std::thread::spawn(move || child.wait());
                self.toast(format!("Action {slot}: {command}"));
            }
            Err(error) => self.toast(format!("Action {slot} failed: {error}")),
        }
    }

    fn is_checked(&self, command: Command) -> bool {
        use Command::*;
        match command {
            ToggleEnlarge => self.view.enlarge,
            ToggleFullscreen => self.window.as_ref().is_some_and(|w| w.fullscreen().is_some()),
            ToggleFilmstrip => self.strip_pinned,
            ToggleInfo => self.show_info,
            ToggleDebug => self.show_debug,
            Scope(kind) => self.scopes[kind],
            ScopeLog => self.scope_log,
            ToggleWatch => self.watch.is_some(),
            Workspace(which) => which == self.space,
            ToggleList => self.show_list,
            ToggleGridTitles => self.grid_titles,
            ToggleMute => self.muted,
            CycleLoop => self.looping != Looping::Off,
            Triage => self.triage_on,
            TogglePrecise => self.precise,
            ToggleScenes => self.scenes_on,
            CycleSubtitles => self.subtitles,
            ListPin => self.list_pinned,
            SortBy(sort) => self.sort == Some(sort),
            ReverseOrder => self.reversed,
            Shuffle => self.shuffled,
            ToggleVerbose => self.options.verbose,
            ToggleDebugLog => self.options.debug,
            ToggleExif => self.show_exif,
            ToggleSlideshow => self.slide_at.is_some(),
            ToggleKeepView => self.keep_view,
            ToggleAntialias => self.antialias,
            ToggleActions => self.show_actions,
            TogglePointer => !self.pointer_visible,
            _ => false,
        }
    }

    // ---- Marks and file operations --------------------------------------

    fn toggle_mark(&mut self) {
        let id = self.list[self.focus()];
        if self.marks.remove(&id) {
            println!("[unmark] {}", self.name_of(id));
        } else {
            self.marks.insert(id);
            println!("[mark] {}", self.name_of(id));
        }
        self.redraw();
    }

    /// Copies file names, or full paths, to the clipboard and the terminal.
    /// Copies the target files' paths on one line for a shell, each
    /// folder's files folded together with braces: to be pasted after
    /// `rm` or `ls`.
    fn copy_for_shell(&mut self) {
        let paths: Vec<PathBuf> = self.targets().into_iter().map(|id| self.files[id].clone()).collect();
        let line = shellpaths::compress(&paths);
        println!("[copy] {line}");
        // Said as it is, and no more of it than fits a corner.
        let shown = match line.chars().count() {
            0..=70 => line.clone(),
            _ => format!("{}…", line.chars().take(68).collect::<String>()),
        };
        // A scripted test run must not replace the user's clipboard.
        if self.snapshot.is_some() || ops::set_clipboard(&line) {
            self.toast(format!("Copied {shown}"));
        } else {
            self.toast("No clipboard tool (xclip, xsel, wl-copy)");
        }
    }

    fn copy_text(&mut self, full_path: bool) {
        let lines: Vec<String> = self
            .targets()
            .into_iter()
            .map(|id| match std::path::absolute(&self.files[id]) {
                Ok(path) if full_path => path.to_string_lossy().into_owned(),
                _ => self.name_of(id),
            })
            .collect();
        for line in &lines {
            println!("[copy] {line}");
        }
        let what = match lines.as_slice() {
            [only] => only.clone(),
            many => format!("{} {}", many.len(), if full_path { "paths" } else { "names" }),
        };
        // A scripted test run must not replace the user's clipboard.
        if self.snapshot.is_some() || ops::set_clipboard(&lines.join("\n")) {
            self.toast(format!("Copied {what}"));
        } else {
            self.toast("No clipboard tool (xclip, xsel, wl-copy)");
        }
    }

    /// Copies or moves the target files into the `-o` folder. Files whose
    /// name is already taken there are left alone.
    fn send(&mut self, moving: bool, event_loop: &ActiveEventLoop) {
        // With no `-o` folder there is nowhere agreed on: ask for one.
        match self.options.output_dir.clone() {
            Some(dir) => self.send_to(dir, moving, event_loop),
            None => self.ask_folder(moving),
        }
    }

    /// Copies or moves the target files into `dir`, made if it is not
    /// there. Files whose name is already taken there are left alone.
    fn send_to(&mut self, dir: PathBuf, moving: bool, event_loop: &ActiveEventLoop) {
        let how = if moving { "move" } else { "copy" };
        let said = |app: &Self| app.targets().iter().map(|id| format!("{how}: {} -> {}", app.files[*id].display(), dir.display())).collect();
        if self.dry(said, &format!("would {how} {} to {}", self.targets().len(), dir.display())) {
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.toast(format!("Cannot use {}: {error}", dir.display()));
            return;
        }

        let (mut sent, mut skipped, mut failed) = (Vec::new(), 0, 0);
        for id in self.targets() {
            let from = self.files[id].clone();
            let Some(name) = from.file_name() else { continue };
            let to = dir.join(name);
            if to.exists() {
                skipped += 1;
                continue;
            }
            let result = if moving { ops::move_file(&from, &to) } else { ops::copy_file(&from, &to) };
            match result {
                Ok(()) => {
                    println!("[{}] {} -> {}", if moving { "move" } else { "copy to" }, from.display(), to.display());
                    let pos = self.list.iter().position(|&other| other == id).unwrap_or(0);
                    sent.push(Moved { id, pos, from, to });
                }
                Err(error) => {
                    eprintln!("jog: {}: {error}", from.display());
                    failed += 1;
                }
            }
        }

        let mut summary = format!("{} {} to {}", if moving { "Moved" } else { "Copied" }, sent.len(), dir.display());
        if skipped > 0 {
            summary.push_str(&format!(", {skipped} already there"));
        }
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed"));
        }
        if sent.is_empty() {
            return self.toast(summary);
        }
        self.toast_with(summary, vec![("Undo".into(), ToastLink::Undo)]);
        if moving {
            let removed: Vec<(usize, usize)> = sent.iter().map(|moved| (moved.id, moved.pos)).collect();
            self.drop_from_list(&removed, event_loop);
            self.undo.push(Undo::Moved(sent));
        } else {
            // The marked images have been dealt with; start a fresh selection.
            self.marks.clear();
            self.undo.push(Undo::Created(sent.into_iter().map(|copy| copy.to).collect()));
        }
    }

    /// Takes files that are gone out of the list, given as `(id, position)`.
    /// The view and the grid cursor stay on the images they were on where
    /// those are still there.
    fn drop_from_list(&mut self, removed: &[(usize, usize)], event_loop: &ActiveEventLoop) {
        let gone = |id: &usize| removed.iter().any(|(gone_id, _)| gone_id == id);
        // Where a list position ends up once the removed files are gone.
        let shifted = |pos: usize| pos - removed.iter().filter(|(_, at)| *at < pos).count();
        let (pos, cursor) = (shifted(self.pos), self.grid.as_ref().map(|grid| shifted(grid.cursor)));

        self.list.retain(|id| !gone(id));
        self.marks.retain(|id| !gone(id));
        if let Some((_, all)) = &mut self.filter {
            all.retain(|id| !gone(id));
        }
        if !self.refill(event_loop) {
            return;
        }
        let last = self.list.len() - 1;
        self.pos = pos.min(last);
        if let (Some(grid), Some(cursor)) = (&mut self.grid, cursor) {
            grid.cursor = cursor.min(last);
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
    }

    /// Which button of the delete confirmation is at a window position:
    /// `true` for Delete, `false` for Cancel.
    fn confirm_button_at(&self, x: f64, y: f64) -> Option<bool> {
        let confirm = self.confirm.as_ref()?;
        let on = |[left, top, width, height]: [i32; 4]| {
            (left..left + width).contains(&(x as i32)) && (top..top + height).contains(&(y as i32))
        };
        confirm.drawn.iter().position(|place| on(*place)).map(|button| button == 1)
    }

    /// What a menu item is called. Copying and moving name the folder
    /// they go to, when `-o` has set one.
    fn menu_label(&self, item: &MenuItem) -> String {
        let output = self.options.output_dir.as_deref().map(short_dir);
        Self::label_for(item, output.as_deref())
    }

    fn label_for(item: &MenuItem, output: Option<&str>) -> String {
        match (item.command, output) {
            (Command::CopyTo, Some(folder)) => format!("Copy to {folder}"),
            (Command::MoveTo, Some(folder)) => format!("Move to {folder}"),
            _ => item.label.to_string(),
        }
    }

    /// Asks before deleting the target files for good.
    fn ask_delete(&mut self) {
        let ids = self.targets();
        let question = match ids.as_slice() {
            [only] => format!("Delete {}?", self.name_of(*only)),
            many => format!("Delete {} marked images?", many.len()),
        };
        self.menu = None;
        self.confirm = Some(Confirm { question, yes: false, ids, drawn: [[0; 4]; 2] });
        self.redraw();
    }

    fn confirm_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let Some(confirm) = &mut self.confirm else { return };
        match key {
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::Tab) => confirm.yes = !confirm.yes,
            Key::Character("y" | "Y") => confirm.yes = true,
            Key::Character("n" | "N") | Key::Named(NamedKey::Escape) => self.confirm = None,
            Key::Named(NamedKey::Enter) => {}
            _ => return,
        }
        // Enter takes the highlighted answer; y is a yes in one press.
        if matches!(key, Key::Named(NamedKey::Enter) | Key::Character("y" | "Y")) {
            if let Some(Confirm { yes: true, ids, .. }) = self.confirm.take() {
                self.delete(&ids, event_loop);
            }
        }
        self.redraw();
    }

    /// Deletes files from disk. This cannot be undone.
    fn delete(&mut self, ids: &[usize], event_loop: &ActiveEventLoop) {
        if self.dry(|app| ids.iter().map(|id| format!("delete: {}", app.files[*id].display())).collect(), &format!("would delete {}", ids.len())) {
            return;
        }
        let (mut removed, mut failed) = (Vec::new(), 0);
        for &id in ids {
            let Some(pos) = self.list.iter().position(|&other| other == id) else { continue };
            match std::fs::remove_file(&self.files[id]) {
                Ok(()) => {
                    println!("[delete] {}", self.files[id].display());
                    removed.push((id, pos));
                }
                Err(error) => {
                    eprintln!("jog: cannot delete {}: {error}", self.files[id].display());
                    failed += 1;
                }
            }
        }
        let mut summary = format!("Deleted {}", removed.len());
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed"));
        }
        self.toast(summary);
        if !removed.is_empty() {
            self.drop_from_list(&removed, event_loop);
        }
    }

    fn undo(&mut self, event_loop: &ActiveEventLoop) {
        if !self.undo.is_empty() && self.dry(|_| vec!["undo the last copy, move, rename or convert".to_string()], "would undo") {
            return;
        }
        match self.undo.pop() {
            None => self.toast("Nothing to undo"),
            Some(Undo::Created(paths)) => {
                let removed = paths.iter().filter(|path| std::fs::remove_file(path).is_ok()).count();
                for path in &paths {
                    println!("[undo] removed {}", path.display());
                }
                self.toast(format!("Undone  ·  {removed} removed"));
                // Those the list had taken in leave it.
                let listed: Vec<(usize, usize)> = (self.list.iter().enumerate())
                    .filter(|(_, id)| paths.contains(&self.files[**id]))
                    .map(|(pos, id)| (*id, pos))
                    .collect();
                if !listed.is_empty() {
                    self.drop_from_list(&listed, event_loop);
                }
            }
            Some(Undo::Moved(moved)) => {
                let viewing = self.id();
                let mut restored = 0;
                // In their original order, so each lands where it was.
                for item in &moved {
                    if ops::move_file(&item.to, &item.from).is_ok() {
                        println!("[undo] {} -> {}", item.to.display(), item.from.display());
                        let at = item.pos.min(self.list.len());
                        if let Some((_, all)) = &mut self.filter {
                            // Next to the neighbour it had among the matches.
                            let before = at.checked_sub(1).map(|before| self.list[before]);
                            let index = before.and_then(|before| all.iter().position(|id| *id == before));
                            all.insert(index.map_or(0, |index| index + 1), item.id);
                        }
                        self.list.insert(at, item.id);
                        restored += 1;
                    } else {
                        eprintln!("jog: cannot move {} back", item.to.display());
                    }
                }
                self.pos = self.list.iter().position(|&id| id == viewing).unwrap_or(self.pos);
                self.toast(format!("Undone  ·  {restored} of {} moved back", moved.len()));
                self.request();
                self.refresh();
            }
            Some(Undo::Renamed { id, from, to }) => {
                if ops::move_file(&to, &from).is_ok() {
                    println!("[undo] {} -> {}", to.display(), from.display());
                    self.set_path(id, from);
                    self.toast(format!("Undone  ·  {}", self.name_of(id)));
                    self.refresh();
                } else {
                    self.toast("Could not undo the rename");
                }
            }
        }
    }

    fn set_path(&mut self, id: usize, path: PathBuf) {
        self.loader.set_path(id, path.clone());
        self.files[id] = path;
    }

    /// Converts the target files on another thread; `Event::Converted`
    /// reports back.
    fn convert(&mut self, format: Format) {
        self.convert_ids(self.targets(), format, self.options.convert);
    }

    /// Converts these files, each turned and mirrored as it is shown.
    fn convert_ids(&mut self, ids: Vec<usize>, format: Format, settings: convert::Settings) {
        let paths: Vec<(PathBuf, Transform)> = ids.into_iter().map(|id| (self.files[id].clone(), self.transforms.get(&id).copied().unwrap_or_default())).collect();
        if self.dry(|_| paths.iter().map(|(path, _)| format!("convert to {}: {}", format.name(), path.display())).collect(), &format!("would convert {}", paths.len())) {
            return;
        }
        self.toast(format!("Converting {} to {} ({})...", paths.len(), format.name(), settings.describe(format)));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let mut done = Converted { created: Vec::new(), failed: 0, what: format.name().to_string(), before: 0, after: 0, how: None, error: None, saving: false };
            for (path, transform) in paths {
                match convert::convert(&path, format, &settings, transform) {
                    Ok((to, how)) => {
                        println!("[convert] {} -> {} ({how})", path.display(), to.display());
                        done.before += std::fs::metadata(&path).map_or(0, |meta| meta.len());
                        done.after += std::fs::metadata(&to).map_or(0, |meta| meta.len());
                        done.created.push(to);
                        done.how = Some(how);
                    }
                    Err(error) => {
                        eprintln!("jog: cannot convert {}: {error}", path.display());
                        done.failed += 1;
                        done.error = Some(short_error(&path, &error.to_string()));
                    }
                }
            }
            let _ = proxy.send_event(Event::Converted(done));
        });
    }

    /// Converts the target videos by ffmpeg, one after another on another
    /// thread, saying how far along each is as it goes.
    fn convert_videos(&mut self, target: crate::transcode::Target) {
        let settings = self.options.convert.video;
        // Each turned and mirrored as it is shown.
        let paths: Vec<(PathBuf, Transform)> = self
            .targets()
            .into_iter()
            .filter(|id| files::is_video(&self.files[*id]))
            .map(|id| (self.files[id].clone(), self.transforms.get(&id).copied().unwrap_or_default()))
            .collect();
        if paths.is_empty() {
            return self.toast("No videos to convert");
        }
        if let Some(problem) = target.problem(&settings) {
            return self.toast(problem);
        }
        let what = target.describe(&settings);
        if self.dry(|_| paths.iter().map(|(path, _)| format!("convert to {what}: {}", path.display())).collect(), &format!("would convert {}", paths.len())) {
            return;
        }
        self.toast(format!("Converting {} to {what}...", paths.len()));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let total = paths.len();
            let began = crate::transcode::generation();
            let mut done = Converted { created: Vec::new(), failed: 0, what: what.clone(), before: 0, after: 0, how: None, error: None, saving: false };
            for (index, (path, transform)) in paths.iter().enumerate() {
                // Called off: the rest are not begun.
                if crate::transcode::generation() != began {
                    done.error = Some(crate::transcode::CANCELLED_WORD.to_string());
                    break;
                }
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let mut said = None;
                let started = Instant::now();
                let result = crate::transcode::transcode(path, &target, &settings, *transform, |share| {
                    // A word only when the whole percentage moves on.
                    let percent = (share * 100.0) as u32;
                    if said != Some(percent) {
                        said = Some(percent);
                        let of = if total > 1 { format!(" ({} of {total})", index + 1) } else { String::new() };
                        let _ = proxy.send_event(Event::VideoProgress(format!("{name}{of}  ·  {}", progress_words(share, started))));
                    }
                });
                match result {
                    Ok(to) => {
                        println!("[convert] {} -> {}", path.display(), to.display());
                        done.before += std::fs::metadata(path).map_or(0, |meta| meta.len());
                        done.after += std::fs::metadata(&to).map_or(0, |meta| meta.len());
                        done.created.push(to);
                    }
                    Err(error) => {
                        if error == crate::transcode::CANCELLED_WORD {
                            done.error = Some(error);
                            continue;
                        }
                        eprintln!("jog: cannot convert {}: {error}", path.display());
                        done.failed += 1;
                        done.error = Some(short_error(path, &error));
                    }
                }
            }
            let _ = proxy.send_event(Event::Converted(done));
        });
    }

    fn converted(&mut self, done: Converted) {
        let cancelled = done.error.as_deref() == Some(crate::transcode::CANCELLED_WORD);
        if cancelled && done.created.is_empty() {
            return self.toast("Cancelled");
        }
        // Nothing made: why, rather than a count of nothing.
        if done.created.is_empty() {
            let why = done.error.unwrap_or_else(|| "see the terminal".to_string());
            return self.toast(format!("{}: {why}", if done.saving { "Not saved" } else { "Not converted" }));
        }
        // What was made joins the list beside what it was made from, and
        // the toast goes to it on a click.
        self.adopt_created(&done.created);
        let links = vec![("Show".into(), ToastLink::Show(done.created[0].clone())), ("Undo".into(), ToastLink::Undo)];
        // A section saved: what and how large.
        if done.saving {
            let made = &done.created[0];
            let size = std::fs::metadata(made).map_or(0, |meta| meta.len());
            self.toast_with(format!("{}  ·  {}", made.file_name().unwrap_or_default().to_string_lossy(), meta::human_size(size)), links);
            self.undo.push(Undo::Created(done.created));
            return;
        }
        let mut summary = match done.created.as_slice() {
            [one] => one.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            all => format!("Converted {} to {}", all.len(), done.what),
        };
        if done.before > 0 {
            let change = (done.after as f64 / done.before as f64 - 1.0) * 100.0;
            summary.push_str(&format!("  ·  {} → {}  {change:+.0}%", meta::human_size(done.before), meta::human_size(done.after)));
        }
        // A JPEG repacked rather than encoded says so.
        if done.how.is_some_and(|how| how.contains("repacked")) {
            summary.push_str("  ·  lossless");
        }
        if done.failed > 0 {
            summary.push_str(&format!("  ·  {} failed", done.failed));
        }
        if cancelled {
            summary.push_str("  ·  the rest cancelled");
        }
        self.toast_with(summary, links);
        self.undo.push(Undo::Created(done.created));
    }

    /// Files just made, by a convert or a saved section, put in the list
    /// right after the file each was made from: the one in the same folder
    /// whose name starts its name.
    fn adopt_created(&mut self, created: &[PathBuf]) {
        let total = self.filter.as_ref().map_or(&self.list, |(_, all)| all);
        let mut listed: Vec<PathBuf> = total.iter().map(|id| self.files[*id].clone()).collect();
        for made in created {
            if listed.contains(made) || !made.exists() {
                continue;
            }
            let stem = made.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let source = listed.iter().rposition(|path| {
                path.parent() == made.parent() && path.file_stem().is_some_and(|own| stem.starts_with(&*own.to_string_lossy()))
            });
            let at = source.map_or(listed.len(), |at| at + 1);
            listed.insert(at, made.clone());
        }
        self.take_listing_as(listed, Taking::Adopting);
    }

    // ---- Rename --------------------------------------------------------

    fn start_rename(&mut self) {
        let id = self.list[self.focus()];
        let text: Vec<char> = self.name_of(id).chars().collect();
        // Start typing before the extension.
        let cursor = text.iter().rposition(|c| *c == '.').filter(|at| *at > 0).unwrap_or(text.len());
        self.prompt = Some(Prompt { id, kind: PromptKind::Rename, field: TextField::new(text, cursor) });
        self.menu = None;
        self.redraw();
    }

    /// Opens a box to type the folder to copy or move the target files
    /// into, starting from the one last typed, or the `-o` folder.
    fn ask_folder(&mut self, moving: bool) {
        let known = self.last_folder.clone().or_else(|| self.options.output_dir.as_deref().map(short_dir));
        let text: Vec<char> = known.unwrap_or_default().chars().collect();
        let id = self.list[self.focus()];
        self.prompt = Some(Prompt { id, kind: PromptKind::SendTo { moving }, field: TextField::new(text.clone(), text.len()) });
        self.menu = None;
        self.redraw();
    }

    fn prompt_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let Some(prompt) = &mut self.prompt else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.prompt = None,
            // Tab finishes a folder's name, from what is typed before the
            // cursor: as far as the folders it could be agree.
            Key::Named(NamedKey::Tab) => self.complete_prompt(),
            Key::Named(NamedKey::Enter) => match prompt.kind {
                PromptKind::Rename => self.finish_rename(),
                PromptKind::SaveWorkspace => {
                    let typed = prompt.field.string().trim().to_string();
                    self.prompt = None;
                    self.save_space(&typed);
                }
                PromptKind::SectionName { start, end } => {
                    let typed = prompt.field.string();
                    self.prompt = None;
                    self.name_section(start, end, &typed);
                }
                PromptKind::SendTo { moving } => {
                    let typed = prompt.field.string().trim().to_string();
                    self.prompt = None;
                    if !typed.is_empty() {
                        self.last_folder = Some(typed.clone());
                        // `@name` is the folder of that workspace.
                        let folder = match typed.strip_prefix('@') {
                            Some(name) => self.spaces.iter().find(|space| space.name == name).and_then(|space| space.paths.first().cloned()),
                            None => Some(crate::options::expand_home(&typed)),
                        };
                        match folder {
                            Some(folder) => self.send_to(folder, moving, event_loop),
                            None => self.toast(format!("There is no workspace named {}", &typed[1..])),
                        }
                    }
                }
            },
            key => drop(prompt.field.key(key, self.modifiers)),
        }
        self.redraw();
    }

    /// Tab in a box for a path: finishes a folder's name from what is
    /// typed before the cursor, as far as the folders it could be agree.
    fn complete_prompt(&mut self) {
        let Some(prompt) = &self.prompt else { return };
        let base = match prompt.kind {
            PromptKind::Rename => self.files[prompt.id].parent().map(Path::to_path_buf).unwrap_or_default(),
            PromptKind::SendTo { .. } => PathBuf::new(),
            PromptKind::SaveWorkspace | PromptKind::SectionName { .. } => return,
        };
        let before: String = prompt.field.text[..prompt.field.cursor].iter().collect();
        if before.starts_with('@') {
            return;
        }
        let Some(done) = complete_folder(&base, &before) else { return self.toast("No match") };
        let Some(prompt) = &mut self.prompt else { return };
        let after = prompt.field.text.split_off(prompt.field.cursor);
        prompt.field.cursor = done.chars().count();
        prompt.field.text = done.chars().chain(after).collect();
        prompt.field.anchor = None;
    }

    /// What Enter would do with what is typed so far, when it is more than
    /// a new name: where a file would go, and whether that folder is to
    /// be made or a file of that name is there.
    fn prompt_hint(&self, prompt: &Prompt) -> Option<String> {
        let typed = prompt.field.string();
        let typed = typed.trim();
        match prompt.kind {
            PromptKind::Rename => {
                let from = &self.files[prompt.id];
                let to = ops::rename_target(from, typed)?;
                let here = from.parent().and_then(|dir| dir.canonicalize().ok());
                let dir = to.parent()?;
                let name = to.file_name()?.to_string_lossy();
                let there = to.exists() && to != from.canonicalize().unwrap_or_default();
                if there {
                    return Some(format!("{name} exists"));
                }
                if Some(dir) == here.as_deref() {
                    return None;
                }
                let new = if dir.exists() { "" } else { "  (new folder)" };
                Some(format!("→ {}/{name}{new}", short_dir(dir)))
            }
            PromptKind::SendTo { .. } if !typed.is_empty() && !typed.starts_with('@') => {
                let dir = crate::options::expand_home(typed);
                let shown = short_dir(&dir.canonicalize().unwrap_or_else(|_| dir.clone()));
                Some(if dir.is_dir() { format!("→ {shown}") } else { format!("→ {shown}  (new folder)") })
            }
            _ => None,
        }
    }

    /// With `--dry-run`, says what would have been done, a line for each
    /// thing in the terminal and a word on screen, and returns true:
    /// whoever asked then does none of it.
    fn dry(&mut self, lines: impl FnOnce(&Self) -> Vec<String>, summary: &str) -> bool {
        if !self.options.dry_run {
            return false;
        }
        for line in lines(self) {
            println!("[dry run] {line}");
        }
        self.toast(format!("Dry run: {summary}"));
        true
    }

    /// Saves the frame a video is showing as a picture, at the video's own
    /// size and without subtitles: named after the video and how far in it
    /// is, in the folder set for frames, or the one files are copied to,
    /// or else beside the video.
    fn save_frame(&mut self) {
        // A frame kept from before a pause is on show: the main player is
        // put on it, and given a moment to get there, so that the frame
        // saved is that one, at the video's own size.
        let kept_at = self.back_shown().map(|(at, _)| at);
        if let Some(at) = kept_at {
            self.leave_back();
            let asked = Instant::now();
            while let Some(player) = self.player.as_ref().filter(|_| asked.elapsed() < Duration::from_secs(3)) {
                if player.number_now("time-pos").is_some_and(|now| (now - at).abs() < 0.008) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // Named for the frame it is: the one kept, by its own time, the
        // player's word for where it is lagging until its events are read.
        let Some((position, _)) = self.video_time() else { return };
        let position = kept_at.unwrap_or(position);
        let path = self.path().to_path_buf();
        let stem = path.file_stem().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
        let whole = position as u64;
        let at = format!("{:02}-{:02}-{:02}.{:03}", whole / 3600, whole / 60 % 60, whole % 60, (position.fract() * 1000.0) as u64);
        let dir = self.options.frame_dir.clone().or_else(|| self.options.output_dir.clone());
        let dir = dir.or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let to = dir.join(format!("{stem}_{at}.{}", self.options.frame_format));
        if self.dry(|_| vec![format!("save the frame: {}", to.display())], "would save the frame") {
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&dir) {
            return self.toast(format!("Cannot use {}: {error}", dir.display()));
        }
        // The player writes it, and says nothing of how that went.
        let Some(player) = &self.player else { return };
        player.command(&[b"screenshot-to-file", to.as_os_str().as_encoded_bytes(), b"video"]);
        match to.exists() {
            true => {
                println!("[frame] {}", to.display());
                self.toast(format!("{}  Saved {}", commands::icons::CAMERA, to.file_name().unwrap_or_default().to_string_lossy()));
            }
            false => self.toast(format!("Could not save the frame to {}", to.display())),
        }
    }

    // ---- Triage ----------------------------------------------------------

    /// What a triage has decided about a file, as far as is remembered.
    fn verdict(&self, id: usize) -> Option<Verdict> {
        *self.verdicts.borrow_mut().entry(id).or_insert_with(|| self.triage.borrow_mut().get_or_insert_with(Triage::load).of(&self.files[id]))
    }

    /// Decides about the file on view and goes on to the next. Deciding
    /// the same again takes the decision back, and stays. Nothing is done
    /// to the file either way.
    fn decide(&mut self, verdict: Verdict) {
        // The one a menu was brought up on, or the grid's cursor is on,
        // where it is not the file on view.
        let id = self.list[self.focus()];
        let on_view = self.grid.is_none() && id == self.id();
        let to = Some(verdict).filter(|_| self.verdict(id) != Some(verdict));
        self.triage.borrow_mut().get_or_insert_with(Triage::load).decide(&self.files[id], to);
        self.verdicts.borrow_mut().insert(id, to);
        if on_view && self.triage_on {
            if to.is_some() && self.pos + 1 < self.list.len() {
                self.step(true);
            }
        } else {
            // With no tally in sight to show it, it is said.
            let name = self.name_of(id);
            self.toast(match to {
                Some(Verdict::Kept) => format!("{}  Kept {name}", commands::icons::CHECK_FILLED),
                Some(Verdict::Rejected) => format!("{}  Rejected {name}", commands::icons::X_FILLED),
                None => format!("Undecided again: {name}"),
            });
        }
        self.redraw();
    }

    /// How many of all the files are kept, rejected and still undecided.
    fn tally(&self) -> (usize, usize, usize) {
        let all = self.everything();
        let count = |wanted: Option<Verdict>| all.iter().filter(|id| self.verdict(**id) == wanted).count();
        (count(Some(Verdict::Kept)), count(Some(Verdict::Rejected)), count(None))
    }

    /// The files a triage has put in one of its three lots, in list order.
    fn lot(&self, wanted: Option<Verdict>) -> Vec<usize> {
        self.everything().into_iter().filter(|id| self.verdict(*id) == wanted).collect()
    }

    /// Which button of a triage is at a place in the window.
    fn triage_button_at(&self, x: f64, y: f64) -> Option<usize> {
        let on = |[left, top, width, height]: [i32; 4]| (left..left + width).contains(&(x as i32)) && (top..top + height).contains(&(y as i32));
        self.triage_buttons.borrow().iter().position(|(_, place)| on(*place))
    }

    /// Looks for files that have come or gone, and says how many did.
    fn triage_refresh(&mut self, event_loop: &ActiveEventLoop) {
        let before = self.everything().len();
        self.execute(Command::Reload, event_loop);
        let now = self.everything().len();
        self.toast(match now.cmp(&before) {
            Ordering::Greater => format!("{}  {} more files", commands::icons::REFRESH, now - before),
            Ordering::Less => format!("{}  {} files fewer", commands::icons::REFRESH, before - now),
            Ordering::Equal => format!("{}  No files have come or gone", commands::icons::REFRESH),
        });
    }

    /// What a triage has at the top of the picture, in a box as the find
    /// box and the menus have. At its ends a button to reject and one to
    /// keep, each with its key, and edged in its colour when it is what
    /// was decided about the file on view. Between them how many are
    /// rejected, beside the one, and kept, beside the other, with a bar
    /// that fills from each end and how many are still to go written over
    /// it. Last, two round ones: to look for files that have come or gone,
    /// and to finish.
    fn draw_triage(&self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        self.triage_buttons.borrow_mut().clear();
        if !self.triage_on || self.grid.is_some() || self.overview.is_some() {
            return;
        }
        let (kept, rejected, undecided) = self.tally();
        let total = (kept + rejected + undecided).max(1);
        let decided = self.verdict(self.id());
        let small = (px * 5 / 6).max(11);
        let (pad, line) = (px as i32 * 2 / 3, Text::line_height(px));
        // How tall a button is, and how far the box stands off them.
        let (side, inset) = (line + pad, pad * 2 / 3);

        let choices = [
            (commands::icons::X_FILLED, "Reject", "x", DANGER_COLOR, Verdict::Rejected, Command::Reject),
            (commands::icons::CHECK_FILLED, "Keep", "Space", KEPT_COLOR, Verdict::Kept, Command::Keep),
        ];
        let widths: [i32; 2] = std::array::from_fn(|which| {
            let (icon, word, key, ..) = choices[which];
            pad + text.width(icon, px) + pad / 2 + text.width(word, px) + pad * 2 / 3 + text.width(key, small) + pad
        });
        let counts = [rejected.to_string(), kept.to_string()];
        let count_widths = [text.width(&counts[0], px), text.width(&counts[1], px)];
        let left_words = format!("{undecided} to go");
        let track = (px as i32 * 9).max(text.width(&left_words, small) + pad);
        let middle = pad + count_widths[0] + pad + track + pad + count_widths[1] + pad;
        let w = inset + widths[0] + middle + widths[1] + pad + side + pad / 2 + side + inset;
        let h = side + 2 * inset;
        let (left, stage) = self.stage();
        let (x, y) = (left + (stage.0 as i32 - w) / 2, px as i32);
        panel(canvas, x, y, w, h, pad);

        let over = self.triage_over;
        let mut buttons = Vec::new();
        let top = y + inset;
        let text_top = top + (side - line) / 2;
        let mut choice = |canvas: &mut Canvas, text: &mut Text, at: i32, which: usize| {
            let (icon, word, key, color, verdict, command) = choices[which];
            let (chosen, hovered) = (decided == Some(verdict), over == Some(buttons.len()));
            let width = widths[which];
            // As the buttons of the find box are: filled in under the
            // pointer, and edged in its own colour once it is chosen.
            let round = pad / 2;
            canvas.round_fill(at, top, width, side, round, if hovered { accent() } else { PLACEHOLDER_COLOR }, 256);
            let (edge, edge_color) = if chosen { (2, color) } else if hovered { (1, accent()) } else { (1, MENU_EDGE_COLOR) };
            canvas.round_outline(at, top, width, side, round, edge, edge_color);
            let (main, dim) = row_text(hovered);
            let mut across = at + pad;
            text.draw(canvas, across, text_top, px, if hovered { main } else { color }, icon);
            across += text.width(icon, px) + pad / 2;
            text.draw(canvas, across, text_top, px, main, word);
            across += text.width(word, px) + pad * 2 / 3;
            text.draw(canvas, across, top + (side - Text::line_height(small)) / 2, small, dim, key);
            buttons.push((command, [at, top, width, side]));
        };
        let mut at = x + inset;
        choice(canvas, text, at, 0);
        at += widths[0];

        // Between them: the count of the rejected, the bar with what is
        // left over it, the count of the kept.
        let mut across = at + pad;
        text.draw(canvas, across, text_top, px, DANGER_COLOR, &counts[0]);
        across += count_widths[0] + pad;
        let thick = (px as i32 / 5).max(3);
        let small_line = Text::line_height(small);
        let words_top = top + (side - small_line - thick - pad / 4) / 2;
        let words_left = across + (track - text.width(&left_words, small)) / 2;
        text.draw(canvas, words_left, words_top, small, DIM_TEXT_COLOR, &left_words);
        let bar_top = words_top + small_line + pad / 4;
        canvas.round_fill(across, bar_top, track, thick, thick / 2, TEXT_COLOR, 40);
        let share = |count: usize| (i64::from(track) * count as i64 / total as i64) as i32;
        if rejected > 0 {
            canvas.round_fill(across, bar_top, share(rejected).max(thick), thick, thick / 2, DANGER_COLOR, 256);
        }
        if kept > 0 {
            let kept_w = share(kept).max(thick);
            canvas.round_fill(across + track - kept_w, bar_top, kept_w, thick, thick / 2, KEPT_COLOR, 256);
        }
        across += track + pad;
        text.draw(canvas, across, text_top, px, KEPT_COLOR, &counts[1]);
        at += middle;
        choice(canvas, text, at, 1);
        at += widths[1] + pad;

        // Last, two round ones, as the file list has: look for files
        // again, and finish.
        for (icon, command) in [(commands::icons::REFRESH, Command::Reload), (commands::icons::X, Command::Triage)] {
            let hovered = over == Some(buttons.len());
            canvas.round_fill(at, top, side, side, side / 2, if hovered { accent() } else { PLACEHOLDER_COLOR }, 256);
            let icon_left = at + (side - text.width(icon, small)) / 2;
            text.draw(canvas, icon_left, top + (side - small_line) / 2, small, if hovered { on_accent() } else { TEXT_COLOR }, icon);
            buttons.push((command, [at, top, side, side]));
            at += side + pad / 2;
        }
        *self.triage_buttons.borrow_mut() = buttons;
    }

    /// The graphs of the picture's colours, on a panel under the debug
    /// overlay: a row of buttons to choose them and how they count, then
    /// each one chosen under its name, then how much of the picture is
    /// at pure black and at pure white. Gives back where the buttons went.
    fn draw_scopes(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32, at: (i32, i32), pointer: (i32, i32)) -> (Vec<Button>, i32) {
        let small = (px * 5 / 6).max(11);
        let (pad, row) = (px as i32, Text::line_height(small) + px as i32 / 6);
        let button_height = row + pad / 3;
        let mut choices: Vec<(&str, Command, bool)> = scopes::KINDS.iter().enumerate().map(|(kind, (name, _))| (*name, Command::Scope(kind), self.scopes[kind])).collect();
        choices.push(("Log", Command::ScopeLog, self.scope_log));
        let widths: Vec<i32> = choices.iter().map(|(label, ..)| text.width(label, small) + pad).collect();
        let buttons_width = widths.iter().sum::<i32>() + pad / 3 * (widths.len() as i32 - 1);
        let graph_width = (px as i32 * 22).max(buttons_width);
        // How tall each graph is: the vectorscope wants a square.
        let tall = |kind: usize| if kind == scopes::VECTOR { graph_width * 3 / 5 } else { graph_width * 2 / 5 };
        let (x, y) = at;
        // Each graph chosen, under its name: where its name and it go.
        let mut graphs = Vec::new();
        let mut top = y + pad + button_height + pad / 2;
        for kind in (0..scopes::KINDS.len()).filter(|kind| self.scopes[*kind]) {
            graphs.push((kind, top, top + row));
            top += row + tall(kind) + pad / 2;
        }
        let (width, height) = (graph_width + 2 * pad, top + row + pad - y);

        // Solid, unlike the panel above, and not filled where a graph will
        // cover it: blending or filling so large a panel each frame would
        // cost more than the graphs do. Only its two ends are rounded.
        let (round, ground) = (pad / 2, 0x0D0D0D);
        canvas.round_fill(x, y, width, 2 * round, round, ground, 256);
        let mut filled = y + round;
        for (kind, _, graph_top) in &graphs {
            canvas.fill(x, filled, width, graph_top - filled, ground);
            canvas.fill(x, *graph_top, pad, tall(*kind), ground);
            canvas.fill(x + pad + graph_width, *graph_top, width - pad - graph_width, tall(*kind), ground);
            filled = graph_top + tall(*kind);
        }
        canvas.fill(x, filled, width, y + height - round - filled, ground);
        canvas.round_fill(x, y + height - 2 * round, width, 2 * round, round, ground, 256);
        canvas.round_outline(x, y, width, height, round, 1, MENU_EDGE_COLOR);

        // The buttons, as those that copy the figures above: filled in
        // while their graph is up, and under the pointer.
        let mut drawn = Vec::new();
        let (mut left, buttons_top) = (x + pad, y + pad);
        for ((label, command, on), button_width) in choices.into_iter().zip(widths) {
            let over = (left..left + button_width).contains(&pointer.0) && (buttons_top..buttons_top + button_height).contains(&pointer.1);
            let lit = on || over;
            canvas.round_fill(left, buttons_top, button_width, button_height, pad / 3, if lit { accent() } else { PLACEHOLDER_COLOR }, 256);
            canvas.round_outline(left, buttons_top, button_width, button_height, pad / 3, 1, if lit { accent() } else { MENU_EDGE_COLOR });
            let words_top = buttons_top + (button_height - Text::line_height(small)) / 2;
            text.draw(canvas, left + pad / 2, words_top, small, if lit { on_accent() } else { TEXT_COLOR }, label);
            drawn.push(Button { x: left, y: buttons_top, width: button_width, height: button_height, command });
            left += button_width + pad / 3;
        }

        for (kind, name_top, graph_top) in graphs {
            text.draw(canvas, x + pad, name_top, small, DIM_TEXT_COLOR, scopes::KINDS[kind].1);
            let stride = canvas.width;
            let mut place = scopes::Place { pixels: canvas.all(), stride, x: x + pad, y: graph_top, w: graph_width, h: tall(kind) };
            match kind {
                scopes::OVERLAID => scopes::histogram(&mut place, &self.scope_counts, self.scope_log, false),
                scopes::ROWS => scopes::histogram(&mut place, &self.scope_counts, self.scope_log, true),
                scopes::WAVEFORM => scopes::waveform(&mut place, &self.scope_counts),
                scopes::PARADE => scopes::parade(&mut place, &self.scope_counts),
                _ => scopes::vector(&mut place, &self.scope_counts),
            }
        }
        // How much has lost its detail at either end: a warning colour
        // past a fiftieth of the picture.
        let (black, white) = self.scope_counts.clipped();
        let mut words_left = x + pad;
        for (words, warn) in [("clipped   black ".to_string(), false), (format!("{black:.1}%"), black > 2.0), ("   white ".to_string(), false), (format!("{white:.1}%"), white > 2.0)] {
            text.draw(canvas, words_left, top, small, if warn { MARK_COLOR } else if words.ends_with('%') { TEXT_COLOR } else { DIM_TEXT_COLOR }, &words);
            words_left += text.width(&words, small);
        }
        (drawn, y + height + pad / 2)
    }

    /// A video's HDR, on a panel under the other debug panels: what it is
    /// and how its metadata is carried, the figures of the frame on view,
    /// and its curves. The 2094-50 ones as hdr-explorer draws them: input
    /// and output as multiples of the reference white, the stored curves
    /// in grey, and in green the curve for the headroom chosen with the
    /// buttons, a dashed line at its peak. Beside them the HDR10+ curve,
    /// as fractions of the scene's brightest and of the screen's peak.
    fn draw_hdr(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32, at: (i32, i32), pointer: (i32, i32)) -> Vec<Button> {
        let Some(info) = self.hdr_info() else { return Vec::new() };
        let badges = self.hdr_badges();
        if badges.is_empty() && !info.is_hdr() {
            return Vec::new();
        }
        let seconds = self.video_time().map_or(0.0, |(position, _)| position);
        let (agtm, plus) = (info.agtm_at(seconds), info.hdr10plus_at(seconds));
        let small = (px * 5 / 6).max(11);
        let (pad, row) = (px as i32, Text::line_height(small) + px as i32 / 6);
        let side = px as i32 * 13;

        // The words: what it is, how it is carried, the figures.
        let mut lines: Vec<(String, u32)> = Vec::new();
        for (what, how) in &info.carried {
            lines.push((format!("{what} in {how}"), DIM_TEXT_COLOR));
        }
        if let Some((peak, floor)) = info.mastering {
            lines.push((format!("Mastering display {peak:.0} nits, black {floor:.4}"), DIM_TEXT_COLOR));
        }
        if let Some((cll, fall)) = info.light {
            lines.push((format!("MaxCLL {cll} nits, MaxFALL {fall} nits"), DIM_TEXT_COLOR));
        }
        if let Some(agtm) = agtm {
            let heads: Vec<String> = agtm.altr.iter().map(|altr| format!("{:.2}", altr.headroom)).collect();
            lines.push((format!("2094-50  graded for {:.2} stops, white {:.0} nits, curves at {} stops", agtm.baseline, agtm.reference_white, heads.join(", ")), TEXT_COLOR));
        }
        if let Some(plus) = plus {
            let [r, g, b] = plus.maxscl;
            lines.push((format!("HDR10+  for a {:.0}-nit screen, brightest {r:.0} / {g:.0} / {b:.0} nits, mean {:.0}", plus.target, plus.average), TEXT_COLOR));
        }
        let graphs = usize::from(agtm.is_some()) + usize::from(plus.is_some());
        let words = lines.iter().map(|(line, _)| text.width(line, small)).max().unwrap_or(0);
        let width = words.max(graphs as i32 * (side + pad) - pad).max(px as i32 * 16) + 2 * pad;
        let graph_rows = if graphs > 0 { row + side + row + pad / 2 } else { 0 };
        let controls = if agtm.is_some() { row + pad / 3 + pad / 2 } else { 0 };
        let height = pad + row + lines.len() as i32 * row + pad / 2 + controls + graph_rows + pad / 2;
        let (x, y) = at;
        canvas.round_fill(x, y, width, height, pad / 2, 0x000000, 190);
        canvas.round_outline(x, y, width, height, pad / 2, 1, MENU_EDGE_COLOR);

        // A heading with the badges.
        let mut left = x + pad;
        text.draw(canvas, left, y + pad, small, accent_soft(), "HDR");
        left += text.width("HDR", small);
        for name in &badges {
            let (run, color) = badge(name);
            left += draw_run(canvas, text, left, y + pad, small, color, &run).0;
        }
        let mut top = y + pad + row;
        for (line, color) in &lines {
            text.draw(canvas, x + pad, top, small, *color, line);
            top += row;
        }
        top += pad / 2;

        // The headroom the 2094-50 curve is for, with buttons to move it.
        let mut drawn = Vec::new();
        if agtm.is_some() {
            let button_height = row + pad / 3;
            let mut left = x + pad;
            for (label, up) in [("−", false), ("+", true)] {
                let button_width = text.width(label, small) + pad;
                let over = (left..left + button_width).contains(&pointer.0) && (top..top + button_height).contains(&pointer.1);
                canvas.round_fill(left, top, button_width, button_height, pad / 3, if over { accent() } else { PLACEHOLDER_COLOR }, 256);
                canvas.round_outline(left, top, button_width, button_height, pad / 3, 1, if over { accent() } else { MENU_EDGE_COLOR });
                text.draw(canvas, left + pad / 2, top + (button_height - Text::line_height(small)) / 2, small, if over { on_accent() } else { TEXT_COLOR }, label);
                drawn.push(Button { x: left, y: top, width: button_width, height: button_height, command: Command::HdrHeadroom(up) });
                left += button_width + pad / 3;
            }
            let what = if self.hdr_headroom == 0.0 { " (SDR, as this screen)".to_string() } else { format!(" ({:.0} times SDR white)", self.hdr_headroom.exp2()) };
            text.draw(canvas, left + pad / 2, top + (button_height - Text::line_height(small)) / 2, small, TEXT_COLOR, &format!("Headroom {:.2} stops{what}", self.hdr_headroom));
            top += button_height + pad / 2;
        }

        let mut left = x + pad;
        let place = |left: i32| [left, top + row, side, side];
        if let Some(agtm) = agtm {
            let headroom = self.hdr_headroom;
            let most = agtm.altr.iter().map(|altr| altr.headroom).fold(agtm.baseline, f64::max).max(headroom);
            let range = most.exp2().ceil().max(2.0);
            text.draw(canvas, left, top, small, DIM_TEXT_COLOR, &format!("2094-50, 0 to {range:.0} times white"));
            let mut lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(|x| x), 0x6A6A6A, true)];
            for altr in &agtm.altr {
                lines.push((Box::new(move |x| hdrmeta::Agtm::output(altr, x)), 0x9A9A9A, false));
            }
            lines.push((Box::new(move |_| headroom.exp2()), 0x3DD68C, true));
            lines.push((Box::new(move |x| agtm.output_at(headroom, x)), 0x3DD68C, false));
            plot(canvas, place(left), (range, range), 1.0, &lines);
            left += side + pad;
        }
        if let Some(plus) = plus {
            text.draw(canvas, left, top, small, DIM_TEXT_COLOR, &format!("HDR10+, for {:.0} nits", plus.target));
            let lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(|x| x), 0x6A6A6A, true), (Box::new(move |x| plus.tone(x)), 0xB48CF0, false)];
            plot(canvas, place(left), (1.0, 1.0), 0.25, &lines);
        }
        drawn
    }

    /// Writes the files of this session out in the three lots of a triage,
    /// if any of them has been decided about, and says where.
    fn write_triage(&self, session: &str) {
        let files: Vec<(PathBuf, Option<Verdict>)> = self.everything().into_iter().map(|id| (self.files[id].clone(), self.verdict(id))).collect();
        if files.iter().all(|(_, verdict)| verdict.is_none()) {
            return;
        }
        if let Some(list) = triage::write_out(session, &files) {
            println!("[triage] {}", list.display());
        }
    }

    // ---- Sessions --------------------------------------------------------

    /// Keeps what is on view as a session of the folder jog was started
    /// in, to be picked up with `--resume`.
    pub fn save_session(&mut self) {
        if self.list.is_empty() || self.cwd.as_os_str().is_empty() {
            return;
        }
        let full = |path: &PathBuf| std::path::absolute(path).unwrap_or_else(|_| path.clone());
        let listing = &self.options.listing;
        let id = sessions::save(Session {
            // Its own if it was picked up; otherwise given by the keeping.
            id: self.options.session_id.clone().unwrap_or_default(),
            cwd: self.cwd.clone(),
            when: sessions::now(),
            paths: self.options.paths.iter().map(full).collect(),
            recursive: listing.recursive,
            sort: self.sort,
            reverse: self.reversed,
            filelist: listing.filelist.as_ref().map(full),
            filter: self.filter.as_ref().map(|(typed, _)| typed.clone()),
            file: full(&self.files[self.id()]),
            pos: self.pos,
            total: self.list.len(),
            seconds: self.video_time().map_or(0.0, |(position, _)| position),
        });
        self.write_triage(&id);
    }

    /// Shows the sessions there are to choose from, as the workspaces
    /// they have been made: the latest is the one already open.
    pub fn choose_session(&mut self) {
        self.open_overview();
    }

    /// Lands on the file a session was left on, in a workspace that has
    /// just been listed for it: narrowed as it was, and for a video as
    /// far in.
    fn take_up(&mut self, spot: Spot) {
        // From here on it is this session that is on view, and kept.
        (self.options.session_id, self.cwd) = (Some(spot.id.clone()), spot.cwd.clone());
        if let Some(typed) = &spot.filter {
            self.start_filter(typed);
        }
        let known = self.files.iter().position(|file| *file == spot.file);
        if let Some(pos) = known.and_then(|id| self.list.iter().position(|other| *other == id)) {
            self.pos = pos;
            if let Some(grid) = &mut self.grid {
                grid.cursor = pos;
            }
            self.enter_at = Some(spot.seconds).filter(|seconds| *seconds >= 1.0);
        }
    }

    // ---- The file list's place ------------------------------------------

    /// The part of the window the picture has: all of it, or, with the
    /// file list pinned, what that leaves beside it. As where it starts
    /// from the left, and its size.
    fn stage(&self) -> (i32, (u32, u32)) {
        let (width, height) = self.window_size();
        match self.list_panel().filter(|_| self.list_pinned) {
            Some(panel) => (if self.list_right { 0 } else { panel.width }, ((width as i32 - panel.width).max(1) as u32, height)),
            None => (0, (width, height)),
        }
    }

    /// Where a picture of this size is in the window: laid out in the
    /// part of it the picture has.
    fn layout_of(&self, native: (u32, u32)) -> crate::view::Layout {
        let (left, size) = self.stage();
        let mut layout = self.view.layout(native, size);
        layout.x += left;
        layout
    }

    /// How far in from the right edge of the window things there stand:
    /// by the width of the file list, when that is on the right.
    fn right_inset(&self) -> i32 {
        self.list_panel().filter(|_| self.list_right).map_or(0, |panel| panel.width)
    }

    // ---- Shell commands -------------------------------------------------

    /// What the find box lists after a `!`: what is typed, to be run as it
    /// stands, and under it the commands run before that have it in them,
    /// the latest first.
    fn shell_matching(&mut self, typed: &str) -> Vec<(String, &'static str, Command)> {
        let typed = typed.trim();
        let wanted = typed.to_lowercase();
        let history = self.shell_history.get_or_insert_with(history::load);
        let before = history.iter().enumerate().filter(|(_, command)| command.as_str() != typed && command.to_lowercase().contains(&wanted));
        let mut rows: Vec<(String, &'static str, Command)> = before.map(|(at, command)| (command.clone(), "", Command::RunShell(at))).collect();
        if !typed.is_empty() {
            rows.insert(0, (typed.to_string(), "Enter", Command::RunTyped));
        }
        rows
    }

    /// Runs a shell command on the marked files, or on the one on view,
    /// once for each, and does not wait for it. In it `%f` is the file's
    /// path, `%n` its name, `%s` its name without the ending, `%d` the
    /// folder it is in and `%t` how many seconds into a video the one on
    /// view is. One that names none of these gets the path after it.
    fn run_shell(&mut self, template: &str) {
        self.run_shell_on(template, self.targets(), false);
    }

    /// Runs a shell command once for each of these files. `in_turn` is for
    /// a great many: one after another, each waited for before the next,
    /// where otherwise they are all set going at once.
    fn run_shell_on(&mut self, template: &str, ids: Vec<usize>, in_turn: bool) {
        let template = template.trim();
        if template.is_empty() {
            return;
        }
        let named = ["%f", "%n", "%s", "%d"].iter().any(|place| template.contains(place));
        let whole = if named { template.to_string() } else { format!("{template} %f") };
        let seconds = self.video_time().map_or(0.0, |(position, _)| position);
        // A dry run still remembers what was typed, to be run in earnest.
        if self.options.dry_run {
            history::remember(self.shell_history.get_or_insert_with(history::load), template);
        }
        if self.dry(|app| ids.iter().map(|id| expand_action(&whole, &app.files[*id], seconds)).collect(), &format!("would run it on {}", ids.len())) {
            return;
        }
        let mut failed = None;
        if in_turn {
            let commands: Vec<String> = ids.iter().map(|id| expand_action(&whole, &self.files[*id], seconds)).collect();
            std::thread::spawn(move || {
                let failures = commands.iter().filter(|command| {
                    let status = std::process::Command::new("sh").arg("-c").arg(command).stdin(std::process::Stdio::null()).status();
                    !status.is_ok_and(|status| status.success())
                });
                let failures = failures.count();
                println!("[run] done with {} files{}", commands.len(), if failures > 0 { format!(", {failures} of them failed") } else { String::new() });
            });
        }
        for id in ids.iter().filter(|_| !in_turn) {
            let command = expand_action(&whole, &self.files[*id], seconds);
            match std::process::Command::new("sh").arg("-c").arg(&command).stdin(std::process::Stdio::null()).spawn() {
                // Reap the child without holding up the viewer.
                Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
                Err(error) => failed = Some(error.to_string()),
            }
        }
        history::remember(self.shell_history.get_or_insert_with(history::load), template);
        // Said as it was typed, and no more of it than fits a corner.
        let short = |text: &str| match text.chars().count() {
            0..=60 => text.to_string(),
            _ => format!("{}…", text.chars().take(58).collect::<String>()),
        };
        self.toast(match (failed, ids.len()) {
            (Some(error), _) => format!("Could not run it: {error}"),
            (None, 1) => format!("{}  {}", commands::icons::TERMINAL, short(template)),
            (None, count) => format!("{}  On {count} files: {}", commands::icons::TERMINAL, short(template)),
        });
    }

    // ---- Searching in the file list ------------------------------------

    /// Whether the heading of the file list is a box to type a search in:
    /// while one is being typed, and while the list is narrowed to one.
    fn list_searching(&self) -> bool {
        self.list_search.is_some() || self.filter.is_some()
    }

    /// Where that box is: across the heading, up to the buttons at its right.
    fn list_field(&self) -> Option<[i32; 4]> {
        let panel = self.list_panel().filter(|_| self.list_searching())?;
        let px = panel.px as i32;
        let side = px * 3 / 2;
        let right = panel.width - 4 * side - px / 2 - px;
        Some([px / 2, px / 2, right - px / 2 - px / 3, side])
    }

    /// Puts the keyboard in the file list's search box, with whatever the
    /// list is narrowed to already there to go on from.
    fn focus_list_search(&mut self) {
        let typed: Vec<char> = self.filter.as_ref().map(|(typed, _)| typed.chars().collect()).unwrap_or_default();
        let end = typed.len();
        self.list_search = Some(TextField::new(typed, end));
        self.redraw();
    }

    fn list_search_key(&mut self, key: Key<&str>) {
        let modifiers = self.modifiers;
        let Some(field) = &mut self.list_search else { return };
        match key {
            // The list stays as it has been narrowed; Escape again widens it.
            Key::Named(NamedKey::Escape) => (self.list_search, self.list_cursor) = (None, None),
            // Enter goes to the row picked out with the arrows, if one was.
            Key::Named(NamedKey::Enter) => {
                let picked = self.list_cursor.take().filter(|at| *at < self.list.len());
                self.list_search = None;
                if let Some(at) = picked {
                    let first = self.list_panel().map(|panel| panel.first);
                    self.go_to(at, at > self.pos);
                    self.list_scroll = first.map(|first| (self.pos, self.list.len(), first));
                }
            }
            // The arrows pick a row out, from the top or from where they left off.
            Key::Named(arrow @ (NamedKey::ArrowDown | NamedKey::ArrowUp)) => {
                let last = self.list.len() - 1;
                let at = match (self.list_cursor, arrow) {
                    (None, NamedKey::ArrowDown) => 0,
                    (None, _) => last,
                    (Some(at), NamedKey::ArrowDown) => (at + 1).min(last),
                    (Some(at), _) => at.saturating_sub(1),
                };
                self.list_cursor = Some(at);
                // It is kept in sight.
                if let Some(panel) = self.list_panel() {
                    let first = panel.first.min(at).max((at + 1).saturating_sub(panel.rows));
                    self.list_scroll = Some((self.pos, self.list.len(), first));
                }
            }
            key => {
                let before = field.string();
                field.key(key, modifiers);
                let typed = field.string();
                if typed != before {
                    self.filter_list(&typed);
                }
            }
        }
        self.redraw();
    }

    /// Narrows the file list to what `typed` finds, as it is typed. The
    /// file on view stays on view and in the list whether it is among them
    /// or not: what is playing is not cut off by a search. With nothing
    /// typed every file is back.
    fn filter_list(&mut self, typed: &str) {
        // Whatever row the arrows had picked out is of the list as it was.
        self.list_cursor = None;
        let typed = typed.trim();
        if typed.is_empty() {
            return self.clear_filter(true);
        }
        let current = self.id();
        let (mut matches, narrowed) = self.matching(typed);
        if !narrowed {
            return;
        }
        if !matches.contains(&current) {
            matches.push(current);
        }
        // Where leaving the search goes back to is where it was begun.
        if self.filter.is_none() {
            let (focus, order) = (self.list[self.focus()], self.everything());
            self.filter_from.push(FilterFrom { typed: None, viewed: current, focus, order, sort: self.sort, reversed: self.reversed, shuffled: self.shuffled });
        }
        self.narrow(typed.to_string(), matches, Some(current));
    }

    // ---- A video playing on in the grid --------------------------------

    /// Where the video that was on view goes on playing while the grid is
    /// up: in its own cell there, when that is on the page showing.
    fn pip_place(&self, window: (u32, u32)) -> Option<[i32; 4]> {
        let video = self.video.as_ref().filter(|_| self.pip && self.grid.is_some())?;
        let native = self.shown.as_ref().filter(|(id, _)| *id == video.id).map_or((16, 9), |(_, frame)| frame.native);
        let area = self.grid_area(&self.grid_layout(window), self.pos, window.1);
        let place = Self::grid_picture(area, native, true);
        (place[0] >= 0 && place[0] + place[2] <= window.0 as i32).then_some(place)
    }

    /// Where, at the left of the line under the grid, the small picture
    /// of the video playing stands: there on every page, whether or not
    /// its own cell is on the one showing.
    fn pip_small(&self, window: (u32, u32)) -> Option<[i32; 4]> {
        let video = self.video.as_ref().filter(|_| self.pip && self.grid.is_some())?;
        let native = self.shown.as_ref().filter(|(id, _)| *id == video.id).map_or((16, 9), |(_, frame)| frame.native);
        let unit = ui_px(window.1) as i32;
        let footer = GridLayout::footer_height(window.1);
        // As tall as the line and the margin the grid leaves free above
        // it: room that nothing else uses, and no cell is covered.
        let tall = footer + self.grid_layout(window).top - unit / 2;
        let (w, h) = fit_size(native, ((window.0 / 6).max(64), tall.max(8) as u32), true);
        Some([unit, window.1 as i32 - unit / 3 - h as i32, w as i32, h as i32])
    }

    /// Takes the video's frame for the grid to draw: at the size of its
    /// cell when that is on the page showing, where it takes the place of
    /// the thumbnail, and otherwise at the size of the small picture under
    /// the grid.
    fn render_pip(&mut self, window: (u32, u32)) {
        let showing = self.video.as_ref().is_some_and(|video| video.showing);
        let cell = self.pip_place(window);
        let place = cell.or_else(|| self.pip_small(window)).filter(|_| showing);
        let (Some([_, _, w, h]), Some(player)) = (place, &self.player) else { return self.pip_live = None };
        let mut pixels = self.pip_live.take().map(|frame| frame.pixels).unwrap_or_default();
        pixels.resize((w * h) as usize, 0);
        // The frame kept for the window is not the one the player has now.
        self.video_kept.set(None);
        if player.render(&mut pixels, w as u32, h as u32) {
            // Its full size is the video's, for when it grows out of the cell.
            let native = self.shown.as_ref().filter(|(id, _)| *id == self.id()).map_or((w as u32, h as u32), |(_, frame)| frame.native);
            self.pip_live = Some(Frame::still(w as u32, h as u32, native, pixels));
            self.pip_in_cell = cell.is_some();
        }
    }

    /// At the left of the line under the grid, the video that is playing:
    /// a small picture of it, a button to pause it or play it, its name
    /// and how far in it is. A click on the picture or the name goes back
    /// to it, the whole window of it. Returns how far across it reaches.
    fn draw_now_playing(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32, room: i32) -> i32 {
        let window = (canvas.width as u32, canvas.height as u32);
        let (Some([x, y, w, h]), Some(video)) = (self.pip_small(window), &self.video) else { return 0 };
        let id = video.id;
        let unit = px as i32;
        match &self.pip_live {
            Some(live) => canvas.image_fast(live, x, y, w, h),
            None => canvas.fill(x, y, w, h, PLACEHOLDER_COLOR),
        }
        // Rounded at the corners, as the pictures of the grid are.
        let round = (unit / 2).max(4);
        canvas.round_off(x, y, w, h, round, GRID_COLOR);
        canvas.round_outline(x - 1, y - 1, w + 2, h + 2, round + 1, 1, MENU_EDGE_COLOR);
        // The symbol is what a press does, as on the seek bar.
        let side = unit * 5 / 4;
        // Four buttons in a row: the file before, pause or play, the file
        // after, and the sound on or off.
        let slot = |which: i32| x + w + unit + which * (side + unit);
        let middle = canvas.height - GridLayout::footer_height(window.1) / 2;
        let (button_x, button_y) = (slot(1), middle - side / 2);
        let sound = if self.muted { commands::icons::VOLUME_X } else { commands::icons::VOLUME_2 };
        for (which, icon, command) in [(0, commands::icons::SKIP_BACK, Command::Previous), (2, commands::icons::SKIP_FORWARD, Command::Next), (3, sound, Command::ToggleMute)] {
            let color = if command == Command::ToggleMute && self.muted { DIM_TEXT_COLOR } else { TEXT_COLOR };
            let icon_left = slot(which) + (side - text.width(icon, px)) / 2;
            text.draw(canvas, icon_left, middle - Text::line_height(px) / 2, px, color, icon);
            self.grid_buttons.push(Button { x: slot(which) - unit / 2, y: button_y - unit / 2, width: side + unit, height: side + unit, command });
        }
        let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
        match paused {
            true => canvas.play_symbol(button_x, button_y, side, TEXT_COLOR),
            false => canvas.pause_symbol(button_x, button_y, side, TEXT_COLOR),
        }
        self.grid_buttons.push(Button { x: button_x - unit / 2, y: button_y - unit / 2, width: side + unit, height: side + unit, command: Command::PlayPause });
        // Its name and the time, as far as there is room beside what is
        // written in the middle of the line.
        let left = slot(4);
        let time = self.video_time().map(|(position, length)| format!("{} / {}", clock(position), clock(length))).unwrap_or_default();
        let small = (px * 5 / 6).max(11);
        let line = canvas.height - GridLayout::footer_height(window.1) / 2 - Text::line_height(px) / 2;
        let time_width = text.width(&time, small);
        // With no room for a few letters of it, none of it.
        let name_room = room - left - time_width - unit * 2;
        let name = if name_room >= unit * 4 { clipped(text, &title_of(&self.files[id]), name_room, px) } else { String::new() };
        text.draw(canvas, left, line, px, TEXT_COLOR, &name);
        let after = left + text.width(&name, px) + if name.is_empty() { 0 } else { unit };
        if after + time_width <= room {
            text.draw(canvas, after, line + (Text::line_height(px) - Text::line_height(small)) / 2, small, DIM_TEXT_COLOR, &time);
        }
        self.grid_buttons.push(Button { x, y, width: w, height: h, command: Command::ShowPlaying });
        self.grid_buttons.push(Button { x: left, y, width: (after - left).max(0), height: h, command: Command::ShowPlaying });
        // How far across it reaches: what is written in the middle of the
        // line starts clear of it.
        after.max(slot(3) + side) + unit
    }

    // ---- Held keys -----------------------------------------------------

    /// The speed a video plays at just now: several times the usual while
    /// a key or the mouse button is held down to hurry it along.
    fn playing_speed(&self) -> f64 {
        self.speed * if self.hold.as_ref().is_some_and(|hold| hold.fast) { HOLD_SPEED } else { 1.0 }
    }

    fn apply_speed(&self) {
        if let Some(player) = &self.player {
            player.set("speed", &self.playing_speed().to_string());
        }
    }

    /// Something was pressed that hurries the video along if it is kept
    /// down: the space bar, or the mouse button on the picture.
    fn begin_hold(&mut self, by_key: bool) {
        if self.video.is_some() {
            self.hold = Some(Hold { since: Instant::now(), by_key, at: self.mouse, fast: false });
        }
    }

    /// Held long enough, it is no tap: the video hurries along from here.
    fn tend_hold(&mut self) {
        let Some(hold) = self.hold.as_mut().filter(|hold| !hold.fast && hold.since.elapsed() >= HOLD_AFTER) else { return };
        hold.fast = true;
        // The mouse button held is this, and not the start of a drag.
        self.drag = None;
        self.apply_speed();
        if let Some(player) = &self.player {
            player.set("pause", "no");
        }
        self.toast(format!("{}  {}x", commands::icons::CHEVRONS_RIGHT, self.playing_speed()));
        // It stays for as long as the hold does.
        if let Some((_, until)) = &mut self.toast {
            *until = Instant::now() + Duration::from_secs(3600);
        }
    }

    /// What was held is let go: the video goes back to its speed, or, let
    /// go at once, the space bar was a tap, which pauses and plays.
    fn end_hold(&mut self, by_key: bool) {
        let Some(hold) = self.hold.take_if(|hold| hold.by_key == by_key) else { return };
        if hold.fast {
            self.apply_speed();
            self.toast = None;
            self.redraw();
        } else if by_key {
            self.video_action(VideoAction::Pause);
        }
    }

    /// Drops a hold without it counting as a tap: the window has lost the
    /// keyboard, or the pointer has moved off with the button down.
    fn cancel_hold(&mut self) {
        if self.hold.take().is_some_and(|hold| hold.fast) {
            self.apply_speed();
            self.toast = None;
            self.redraw();
        }
    }

    fn key_released(&mut self, key: Key<&str>) {
        match key {
            Key::Named(NamedKey::Space) => self.end_hold(true),
            Key::Character("v") if self.loupe.take().is_some() => {
                self.request();
                self.redraw();
            }
            _ => {}
        }
    }

    /// The loupe: while `v` is held, a round glass under the pointer shows
    /// what is there larger. Of a picture it shows the picture's own
    /// pixels, one to one or twice the size it is on screen, whichever is
    /// more; of a video, what is on screen at twice the size. The wheel
    /// makes it stronger or weaker.
    fn draw_loupe(&self, pixels: &mut [u32], window: (u32, u32)) {
        let Some(strength) = self.loupe.filter(|_| self.grid.is_none() && self.overview.is_none()) else { return };
        let (width, height) = (window.0 as i32, window.1 as i32);
        let radius = ui_px(window.1) as i32 * 10;
        let (cx, cy) = (self.mouse.0 as i32, self.mouse.1 as i32);
        let background = self.options.background;
        let frame = self.source().filter(|_| !self.video.as_ref().is_some_and(|video| video.showing));
        // What is looked at, as a block of pixels, where in it the pointer
        // is, and how many pixels of the glass one of its pixels takes.
        // A picture's own pixels are read where they are: copying them
        // would be the whole picture again on every move of the pointer.
        let block;
        let (source, across, down, at, scale): (&[u32], i32, i32, (f64, f64), f64) = match &frame {
            Some(frame) => {
                let layout = self.layout_of(frame.native);
                let shown = f64::from(layout.width) / f64::from(frame.width.max(1));
                let native = f64::from(frame.native.0.max(1)) / f64::from(frame.width.max(1));
                let scale = (shown * 2.0).max(native) * strength;
                let at = (f64::from(cx - layout.x) / shown, f64::from(cy - layout.y) / shown);
                (frame.picture(self.picture), frame.width as i32, frame.height as i32, at, scale)
            }
            None => {
                // The part of the window under the glass, taken before the
                // glass is drawn over it.
                let scale = 2.0 * strength;
                let half = (f64::from(radius) / scale).ceil() as i32 + 1;
                let side = 2 * half + 1;
                let mut taken = vec![background; (side * side) as usize];
                for row in 0..side {
                    let y = cy - half + row;
                    for column in 0..side {
                        let x = cx - half + column;
                        if (0..width).contains(&x) && (0..height).contains(&y) {
                            taken[(row * side + column) as usize] = pixels[(y * width + x) as usize];
                        }
                    }
                }
                block = taken;
                (&block[..], side, side, (f64::from(half), f64::from(half)), scale)
            }
        };
        for dy in -radius..=radius {
            let y = cy + dy;
            if !(0..height).contains(&y) {
                continue;
            }
            for dx in -radius..=radius {
                let (x, far) = (cx + dx, dx * dx + dy * dy);
                if far > radius * radius || !(0..width).contains(&x) {
                    continue;
                }
                // A light rim with a dark line inside it, to stand out from
                // whatever is behind.
                let pixel = if far >= (radius - 2) * (radius - 2) {
                    0xF0F0F0
                } else if far >= (radius - 3) * (radius - 3) {
                    0x000000
                } else {
                    let (sx, sy) = ((at.0 + f64::from(dx) / scale).floor() as i32, (at.1 + f64::from(dy) / scale).floor() as i32);
                    match (0..across).contains(&sx) && (0..down).contains(&sy) {
                        true => source[(sy * across + sx) as usize],
                        false => background,
                    }
                };
                pixels[(y * width + x) as usize] = pixel;
            }
        }
    }

    // ---- Movement ------------------------------------------------------

    /// Where a number that glides has got to on its way to `to`, setting
    /// it off if that is news. With movement turned off it is there at
    /// once. While it is still on its way, another frame is asked for.
    fn glide(&self, glide: &Cell<Glide>, to: f64, over: Duration) -> f64 {
        if !self.options.animations {
            glide.set(Glide::at(to));
            return to;
        }
        let mut gliding = glide.get();
        gliding.head_for(to, over);
        glide.set(gliding);
        if gliding.moving(over) {
            self.animating.set(true);
        }
        gliding.now(over)
    }

    // ---- Scenes --------------------------------------------------------

    /// Whether the strip of the video's scenes is up, in place of the
    /// filmstrip of files.
    fn scenes_shown(&self) -> bool {
        self.scenes_on && self.grid.is_none() && self.overview.is_none() && self.video.is_some()
    }

    /// Whether anything stands along the bottom of the window.
    fn band_shown(&self) -> bool {
        self.strip_visible() || self.scenes_shown()
    }

    /// Asks for a frame from each of the equal parts of the video on view,
    /// once it is showing and unless they have been asked for before.
    /// What the video on view says about its HDR, read on another thread
    /// the first time it is asked for: `None` until then, and for a file
    /// that says nothing.
    fn hdr_info(&mut self) -> Option<Arc<hdrmeta::Info>> {
        let id = self.video.as_ref().map(|video| video.id)?;
        if let Some(known) = self.hdr_infos.get(&id) {
            return known.clone();
        }
        self.hdr_infos.insert(id, None);
        let (path, proxy) = (self.files[id].clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let info = hdrmeta::read(&path).map(Arc::new);
            let _ = proxy.send_event(Event::HdrInfo(id, info));
        });
        None
    }

    /// Badges for what a video is: from the file, and from the player for
    /// what the file does not say, as an HEVC one keeps its colour inside
    /// the stream.
    fn hdr_badges(&mut self) -> Vec<String> {
        let info = self.hdr_info();
        let mut badges = info.as_ref().map(|info| info.badges()).unwrap_or_default();
        let known = info.as_ref().is_some_and(|info| info.transfer.is_some());
        if !known {
            let gamma = self.player.as_ref().and_then(|player| player.text("video-params/gamma"));
            let named = match gamma.as_deref() {
                Some("pq") => Some("PQ"),
                Some("hlg") => Some("HLG"),
                _ => None,
            };
            let plus = info.as_ref().is_some_and(|info| !info.hdr10plus.is_empty());
            match named {
                Some("PQ") if plus => badges.insert(0, "HDR10+".to_string()),
                Some(name) => badges.insert(0, name.to_string()),
                None => {}
            }
            let bt2020 = self.player.as_ref().and_then(|player| player.text("video-params/primaries")).as_deref() == Some("bt.2020");
            if bt2020 && !badges.iter().any(|badge| badge == "BT.2020") {
                badges.push("BT.2020".to_string());
            }
        }
        badges
    }

    fn want_scenes(&mut self) {
        let Some(id) = self.video.as_ref().filter(|video| video.showing).map(|video| video.id) else { return };
        if self.scenes.contains_key(&id) {
            return;
        }
        let Some((_, length)) = self.video_time() else { return };
        let starts = scenes::starts(length);
        self.scenes.insert(id, SceneList { frames: vec![None; starts.len()], starts: Some(starts.clone()) });
        let proxy = self.proxy.clone();
        let finder = self.scene_finder.get_or_insert_with(|| scenes::Finder::new(move |found| proxy.send_event(Event::Scenes(found)).is_ok()));
        finder.want(id, self.files[id].clone(), starts);
    }

    /// The scenes of the video on view, once they are known, and which of
    /// them is playing.
    fn scenes_now(&self) -> Option<(&SceneList, &[f64], usize)> {
        let list = self.scenes.get(&self.video.as_ref()?.id)?;
        let starts = list.starts.as_deref()?;
        let (position, _) = self.video_time()?;
        // A hair early, so that the scene just jumped to is the one playing.
        let playing = starts.iter().rposition(|start| *start <= position + 0.05).unwrap_or(0);
        Some((list, starts, playing))
    }

    /// Goes to the scene after the one playing, or back to the start of
    /// this one and from there to the one before. Past the last scene is
    /// the next file. Returns whether there were scenes to go by.
    fn scene_jump(&mut self, forward: bool) -> bool {
        let Some((_, starts, playing)) = self.scenes_now() else { return false };
        let Some((position, length)) = self.video_time() else { return false };
        let to = match forward {
            true => starts.get(playing + 1).copied(),
            // A moment into a scene, back is to where it began.
            false if position - starts[playing] > SCENE_RESTART || playing == 0 => Some(starts[playing]),
            false => Some(starts[playing - 1]),
        };
        match to {
            Some(start) => {
                self.seek_to(start / length, true);
                self.time_until = Some(Instant::now() + TOAST);
            }
            None => self.step(true),
        }
        true
    }

    /// Where the cell of a scene is along the strip: they stand in a row
    /// in the middle of the window, and stay where they are.
    fn scene_left(&self, strip: &Strip, index: usize, window_width: i32) -> i32 {
        let row = scenes::PARTS as i32 * strip.pitch() - strip.pad;
        (window_width - row) / 2 + index as i32 * strip.pitch()
    }

    /// The scene whose cell a window position is on.
    fn scene_at(&self, x: f64, y: f64) -> Option<usize> {
        if self.strip_tab != StripTab::Scenes {
            return None;
        }
        let (width, height) = self.window_size();
        let strip = Strip::for_height(height);
        let (_, starts, _) = self.scenes_now().filter(|_| self.scenes_shown() && y >= f64::from(strip.top))?;
        (0..starts.len()).find(|&index| {
            let left = self.scene_left(&strip, index, width as i32);
            (left..left + strip.cell_w).contains(&(x as i32))
        })
    }

    fn draw_scenes(&self, canvas: &mut Canvas, strip: Strip) {
        canvas.shade(0, strip.top, canvas.width, strip.height, 64);
        let Some((list, starts, playing)) = self.scenes_now() else { return };
        let top = strip.top + strip.pad;
        let (round, edge) = ((strip.pad * 2 / 3).max(3), (strip.pad / 4).max(2));
        let over = self.scene_at(self.mouse.0, self.mouse.1);
        for index in 0..starts.len() {
            let left = self.scene_left(&strip, index, canvas.width);
            if left + strip.cell_w < 0 || left > canvas.width {
                continue;
            }
            // Under the pointer the scene plays, once the player has it.
            let playing_here = self.trailer.as_ref().filter(|trailer| trailer.id == usize::MAX - index);
            let live = playing_here.and_then(|trailer| trailer.frame.as_ref()).filter(|frame| !frame.pixels.is_empty());
            let still = list.frames.get(index).and_then(Option::as_ref).map(|frame| &**frame);
            let [x, y, w, h] = match live.or(still) {
                Some(frame) => {
                    let (w, h) = fit_size((frame.width, frame.height), (strip.cell_w as u32, strip.cell_h as u32), false);
                    let (w, h) = (w as i32, h as i32);
                    let place = [left + (strip.cell_w - w) / 2, top + (strip.cell_h - h) / 2, w, h];
                    canvas.image_rounded(frame, place[0], place[1], w, h, round);
                    place
                }
                None => {
                    canvas.round_fill(left, top, strip.cell_w, strip.cell_h, round, PLACEHOLDER_COLOR, 256);
                    [left, top, strip.cell_w, strip.cell_h]
                }
            };
            // The scene playing is ringed brightly, the one under the
            // pointer quietly.
            let ring = if index == playing { Some(TEXT_COLOR) } else { (over == Some(index)).then_some(accent_soft()) };
            if let Some(color) = ring {
                canvas.round_outline(x - edge, y - edge, w + 2 * edge, h + 2 * edge, round + edge, edge, color);
            }
        }
    }

    /// The words on the strip of scenes: in the corner of each when it
    /// starts.
    /// Until the video is showing, that they are on their way.
    fn draw_scene_labels(&self, canvas: &mut Canvas, text: &mut Text, strip: &Strip, px: u32) {
        let small = (px * 3 / 4).max(10);
        let (top, pad, line) = (strip.top + strip.pad, (small as i32 / 4).max(2), Text::line_height(small));
        let Some((list, starts, _)) = self.scenes_now() else {
            let words = "Scenes…";
            let left = (canvas.width - text.width(words, px)) / 2;
            return text.draw(canvas, left, strip.top + (strip.height - Text::line_height(px)) / 2, px, DIM_TEXT_COLOR, words);
        };
        for (index, start) in starts.iter().enumerate() {
            let left = self.scene_left(strip, index, canvas.width);
            if left + strip.cell_w < 0 || left > canvas.width {
                continue;
            }
            let at = clock(*start);
            // In the bottom right corner of the picture, which may be
            // narrower than its cell, as a video's length is in the filmstrip.
            let size = list.frames.get(index).and_then(Option::as_ref).map_or((strip.cell_w, strip.cell_h), |frame| {
                let (w, h) = fit_size((frame.width, frame.height), (strip.cell_w as u32, strip.cell_h as u32), false);
                (w as i32, h as i32)
            });
            let (right, bottom) = (left + (strip.cell_w + size.0) / 2, top + (strip.cell_h + size.1) / 2);
            let badge_w = text.width(&at, small) + 2 * pad;
            let (badge_x, badge_y) = (right - badge_w - pad, bottom - line - pad);
            canvas.round_fill(badge_x, badge_y, badge_w, line, pad, 0x000000, 190);
            text.draw(canvas, badge_x + pad, badge_y, small, TEXT_COLOR, &at);
        }
    }

    // ---- Search --------------------------------------------------------

    /// How many files there are in all, whatever the list is narrowed to.
    fn total(&self) -> usize {
        self.filter.as_ref().map_or(self.list.len(), |(_, all)| all.len())
    }

    /// Every file, whatever the list is narrowed to, in the list's order.
    fn everything(&self) -> Vec<usize> {
        self.filter.as_ref().map_or_else(|| self.list.clone(), |(_, all)| all.clone())
    }

    /// The files, out of all of them, that match what was typed, closest
    /// first, and whether anything was typed that narrows them. Names with
    /// a word as typed come before any that merely have its letters
    /// scattered through them, which count only when there are none of
    /// the first kind: in a folder of long names nearly any word is in
    /// most of them that way.
    fn matching(&mut self, typed: &str) -> (Vec<usize>, bool) {
        let query = Query::parse(typed);
        let everything = self.everything();
        if query.is_empty() {
            return (everything, false);
        }
        let mut pass = |loose: bool| -> Vec<(usize, i32)> {
            everything.iter().filter_map(|&id| Some((id, self.score(&query, id, loose)?))).collect()
        };
        let mut scored = pass(false);
        if scored.is_empty() {
            scored = pass(true);
        }
        scored.sort_by_key(|(_, score)| -score);
        (scored.into_iter().map(|(id, _)| id).collect(), true)
    }

    /// Narrows the list from the start to what `typed` would find in the
    /// find box, for `--filter`. Returns whether anything is left to show.
    pub fn start_filter(&mut self, typed: &str) -> bool {
        let (matches, narrowed) = self.matching(typed);
        if narrowed && !matches.is_empty() {
            // The file it would have started on, if that is among them.
            let start = Some(self.id()).filter(|id| matches.contains(id));
            self.keep_matches(typed.trim().to_string(), matches, start);
        }
        !narrowed || !self.list.is_empty() && self.filter.is_some()
    }

    /// Narrows the list to `matches`, in the order the list had them, and
    /// goes to `picked` among them.
    fn keep_matches(&mut self, typed: String, matches: Vec<usize>, picked: Option<usize>) {
        if matches.is_empty() {
            return;
        }
        // What is on view now is what leaving this search goes back to:
        // every file, or the matches of the search this one was made in.
        let within = self.filter.as_ref().map(|(typed, _)| typed.clone());
        if within.as_deref() != Some(typed.as_str()) {
            let (viewed, focus) = (self.id(), self.list[self.focus()]);
            let order = self.everything();
            self.filter_from.push(FilterFrom { typed: within, viewed, focus, order, sort: self.sort, reversed: self.reversed, shuffled: self.shuffled });
        }
        let total = self.total();
        self.narrow(typed, matches, picked);
        self.toast(format!("Showing {} of {total} (Esc goes {})", self.list.len(), self.back_label()));
    }

    /// Puts only `matches` in the list, in the order the list had them,
    /// and goes to `picked` among them.
    fn narrow(&mut self, typed: String, matches: Vec<usize>, picked: Option<usize>) {
        let everything = self.everything();
        let wanted: HashSet<usize> = matches.into_iter().collect();
        let list: Vec<usize> = everything.iter().copied().filter(|id| wanted.contains(id)).collect();
        let at = picked.and_then(|picked| list.iter().position(|id| *id == picked)).unwrap_or(0);
        if self.options.verbose {
            eprintln!("jog: search \"{typed}\" kept: {} of {} files", list.len(), everything.len());
            for (pos, id) in list.iter().enumerate() {
                eprintln!("jog:     [{}] {}", pos + 1, self.files[*id].display());
            }
        }
        self.filter = Some((typed, everything));
        self.set_list(list, at, at);
        self.sync_video();
        self.redraw();
    }

    /// Where leaving the search on view goes back to, in a few words: the
    /// search it was made in, by what was typed for that, or else the file
    /// that was on view, by the start of its name.
    fn back_label(&self) -> String {
        let short = |text: &str| match text.chars().count() {
            0..=18 => text.to_string(),
            _ => format!("{}…", text.chars().take(16).collect::<String>().trim_end()),
        };
        match self.filter_from.last() {
            Some(FilterFrom { typed: Some(typed), .. }) => format!("back to \"{}\"", short(typed)),
            Some(from) => format!("back to {}", short(&self.name_of(if self.grid.is_some() { from.focus } else { from.viewed }))),
            None => "back".to_string(),
        }
    }

    /// Puts a different set of files in the list, with the one at `viewed`
    /// on view and, in the grid, the cursor on the one at `focus`.
    fn set_list(&mut self, list: Vec<usize>, viewed: usize, focus: usize) {
        self.list = list;
        self.pos = viewed.min(self.list.len() - 1);
        if let Some(grid) = &mut self.grid {
            grid.cursor = focus.min(self.list.len() - 1);
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
    }

    /// Leaves the search on view for what it was made in: every file, or
    /// the matches of another search. The files are in the order they had
    /// then, whatever has been done to the matches since, and the one that
    /// was on view then is on view again, to carry on from there. With
    /// `stay` it instead shows every file and stays on the one on view now.
    fn clear_filter(&mut self, stay: bool) {
        let Some((_, mut all)) = self.filter.take() else { return };
        let (mut viewed, mut focus) = (self.id(), self.list[self.focus()]);
        // Staying drops every search there is, back to how it all began.
        let from = match stay {
            true => self.filter_from.drain(..).next(),
            false => self.filter_from.pop(),
        };
        let mut within = None;
        if let Some(from) = from {
            // Files that have turned up since go after the rest.
            let place: HashMap<usize, usize> = from.order.iter().enumerate().map(|(place, id)| (*id, place)).collect();
            all.sort_by_key(|id| place.get(id).copied().unwrap_or(usize::MAX));
            (self.sort, self.reversed, self.shuffled) = (from.sort, from.reversed, from.shuffled);
            // It may have been deleted or moved away in the meantime.
            if !stay && all.contains(&from.viewed) {
                (viewed, focus) = (from.viewed, if all.contains(&from.focus) { from.focus } else { from.viewed });
                within = from.typed;
            }
        }
        let at = |id: usize| all.iter().position(|&other| other == id).unwrap_or(0);
        let (viewed, focus) = (at(viewed), at(focus));
        self.set_list(all, viewed, focus);
        self.sync_video();
        // Back in the search this one was made in, at the same file.
        if let Some(typed) = within {
            let picked = self.list[self.focus()];
            let (matches, narrowed) = self.matching(&typed);
            if narrowed && matches.contains(&picked) {
                self.narrow(typed.clone(), matches, Some(picked));
                return self.toast(format!("Back to \"{typed}\", at {} of {}", self.focus() + 1, self.list.len()));
            }
            self.filter_from.clear();
        }
        if self.options.verbose {
            eprintln!("jog: search cleared: all {} files, at {}", self.list.len(), self.pos + 1);
        }
        self.toast(format!("Showing all files, at {} of {}", self.focus() + 1, self.list.len()));
    }

    fn finish_rename(&mut self) {
        let Some(prompt) = self.prompt.take() else { return };
        let typed = prompt.field.string();
        let name = typed.trim();
        let from = self.files[prompt.id].clone();
        let old_name = self.name_of(prompt.id);
        if name == old_name {
            return;
        }
        // A path moves it as well: to another folder, made if it is not
        // there, under the name at the end.
        let Some(to) = ops::rename_target(&from, name).filter(|_| name != "." && name != "..") else {
            self.toast("Not a usable file name");
            return;
        };
        let here = from.parent().and_then(|dir| dir.canonicalize().ok());
        let moved = to.parent() != here.as_deref();
        if !moved && to.file_name() == from.file_name() {
            return;
        }
        let said = if moved { short_dir(&to) } else { to.file_name().unwrap_or_default().to_string_lossy().into_owned() };
        if self.dry(|_| vec![format!("rename: {} -> {}", from.display(), to.display())], "would rename") {
            return;
        }
        if let Some(dir) = to.parent().filter(|_| moved) {
            if let Err(error) = std::fs::create_dir_all(dir) {
                return self.toast(format!("Cannot use {}: {error}", dir.display()));
            }
        }
        match ops::move_file(&from, &to) {
            Ok(()) => {
                match moved {
                    true => println!("[rename] {} -> {}", from.display(), to.display()),
                    false => println!("[rename] {old_name} -> {said}"),
                }
                self.set_path(prompt.id, to.clone());
                self.undo.push(Undo::Renamed { id: prompt.id, from, to });
                self.toast_with(format!("{} {said}", if moved { "Moved to" } else { "Renamed to" }), vec![("Undo".into(), ToastLink::Undo)]);
                self.refresh();
            }
            Err(error) => self.toast(format!("Cannot rename to {said}: {error}")),
        }
    }

    // ---- Menu ----------------------------------------------------------

    /// Opens a menu of `entries`. One with a title is keyboard-first: it
    /// sits in the middle of the window with its first item selected. One
    /// without opens at the pointer.
    fn open_menu(&mut self, entries: &'static [Option<MenuItem>], title: Option<String>) {
        let at = (self.mouse.0 as i32, self.mouse.1 as i32);
        self.menu = Some(self.build_menu(entries, title, at));
    }

    /// Opens the menu belonging to the selected item, beside it. From the
    /// keyboard its first item is selected, ready for the arrow keys.
    fn open_submenu(&mut self, select_first: bool) {
        let Some(menu) = &self.menu else { return };
        let Some(row) = menu.selected.map(|row| &menu.rows[row]) else { return };
        let Some(item) = row.item.filter(|item| !item.sub.is_empty()) else { return };
        let (left, right, top) = (menu.x, menu.x + menu.width - 1, menu.y + row.top - menu.px as i32 / 2);
        let mut sub = self.build_menu(item.sub, None, (right, top));
        // Where there is no room to the right, it opens to the left.
        if sub.x < right {
            sub.x = (left - sub.width + 1).max(0);
        }
        if select_first {
            sub.selected = sub.rows.iter().position(|row| row.item.is_some());
        }
        if let Some(menu) = &mut self.menu {
            menu.sub = Some(Box::new(sub));
        }
    }

    /// Lays out a menu of `entries` with its corner at `at`, or as near as
    /// the window allows; one with a title goes in the middle instead.
    fn build_menu(&mut self, all_entries: &'static [Option<MenuItem>], title: Option<String>, at: (i32, i32)) -> Menu {
        let (width, height) = self.window_size();
        let px = ui_px(height);

        // Leave out what does not apply to the files in question: zooming a
        // video, say, or pausing a photo. A marked set can hold both kinds.
        let ids = if title.is_some() { self.targets() } else { vec![self.list[self.focus()]] };
        let videos = ids.iter().filter(|id| files::is_video(&self.files[**id])).count();
        let applies = |item: &MenuItem| match item.applies {
            Applies::Always => true,
            Applies::Images => videos < ids.len(),
            Applies::Videos => videos > 0,
            Applies::Filtered => self.filter.is_some(),
        };
        let mut entries: Vec<Option<&'static MenuItem>> = Vec::new();
        for entry in all_entries {
            match entry {
                Some(item) if applies(item) => entries.push(Some(item)),
                // A separator only after an item, so none double up.
                None if matches!(entries.last(), Some(Some(_))) => entries.push(None),
                _ => {}
            }
        }
        if matches!(entries.last(), Some(None)) {
            entries.pop();
        }

        let output = self.options.output_dir.as_deref().map(short_dir);
        let text = self.text.get_or_insert_with(Text::load);
        let (row_height, separator_height, pad) = (px as i32 * 9 / 5, px as i32 * 3 / 5, px as i32 / 2);

        let items = entries.iter().flatten();
        let label_width = items.clone().map(|i| text.width(&Self::label_for(i, output.as_deref()), px)).max().unwrap_or(0);
        let keys_width = items.map(|i| text.width(i.keys, px)).max().unwrap_or(0);
        let title_width = title.as_ref().map_or(0, |title| text.width(title, px));
        // Padding, a gutter for check marks, the label, a gap and the keys.
        let content_width = (px as i32 * 2 + label_width + px as i32 * 2 + keys_width).max(title_width);
        let menu_width = content_width + 2 * pad;

        let mut top = pad + if title.is_some() { row_height } else { 0 };
        let mut rows = Vec::new();
        for &item in &entries {
            let row_height = if item.is_some() { row_height } else { separator_height };
            rows.push(MenuRow { top, height: row_height, item });
            top += row_height;
        }
        let menu_height = top + pad;

        let centred = ((width as i32 - menu_width) / 2, (height as i32 - menu_height) / 2);
        let (x, y) = if title.is_some() { centred } else { at };
        Menu {
            x: x.clamp(0, (width as i32 - menu_width).max(0)),
            y: y.clamp(0, (height as i32 - menu_height).max(0)),
            width: menu_width,
            height: menu_height,
            px,
            selected: title.as_ref().and(rows.iter().position(|row| row.item.is_some())),
            rows,
            title,
            sub: None,
        }
    }

    /// The `X` menu: what can be done with the marked images.
    fn open_mark_menu(&mut self) {
        let title = match self.marks.len() {
            0 => format!("Nothing marked: applies to {}", self.name_of(self.list[self.focus()])),
            1 => "1 marked image".to_string(),
            count => format!("{count} marked images"),
        };
        self.open_menu(commands::MARK_MENU, Some(title));
    }

    /// Moves the target files to the trash.
    fn trash(&mut self, event_loop: &ActiveEventLoop) {
        if self.dry(|app| app.targets().iter().map(|id| format!("move to trash: {}", app.files[*id].display())).collect(), "would move to trash") {
            return;
        }
        let (mut removed, mut failed) = (Vec::new(), 0);
        for id in self.targets() {
            let Some(pos) = self.list.iter().position(|&other| other == id) else { continue };
            let status = std::process::Command::new("gio").arg("trash").arg("--").arg(&self.files[id]).status();
            if status.is_ok_and(|s| s.success()) {
                println!("[trash] {}", self.files[id].display());
                removed.push((id, pos));
            } else {
                failed += 1;
            }
        }
        let mut summary = format!("Moved {} to trash", removed.len());
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed"));
        }
        self.toast(summary);
        if !removed.is_empty() {
            self.drop_from_list(&removed, event_loop);
        }
    }

    fn menu_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let Some(menu) = &mut self.menu else { return };
        let step = |menu: &mut Menu, forward: bool| {
            let len = menu.rows.len();
            let mut at = menu.selected.unwrap_or(if forward { len - 1 } else { 0 });
            loop {
                at = if forward { (at + 1) % len } else { (at + len - 1) % len };
                if menu.rows[at].item.is_some() {
                    break menu.selected = Some(at);
                }
            }
        };
        // The keys go to the menu opened last.
        let opens = menu.sub.is_none() && menu.selected.is_some_and(|row| menu.rows[row].item.is_some_and(|item| !item.sub.is_empty()));
        let inner = menu.sub.is_some();
        let active = match &mut menu.sub {
            Some(sub) => &mut **sub,
            None => menu,
        };
        match key {
            Key::Named(NamedKey::ArrowDown) => step(active, true),
            Key::Named(NamedKey::ArrowUp) => step(active, false),
            Key::Named(NamedKey::ArrowRight) if opens => self.open_submenu(true),
            Key::Named(NamedKey::ArrowLeft | NamedKey::Escape) if inner => menu.sub = None,
            Key::Named(NamedKey::Enter) => self.activate_menu(event_loop),
            Key::Named(NamedKey::Escape | NamedKey::Tab) => self.menu = None,
            _ => {}
        }
        self.redraw();
    }

    /// Runs the selected item and closes the menu, or opens the menu that
    /// the item stands for. With nothing selected it only closes.
    fn activate_menu(&mut self, event_loop: &ActiveEventLoop) {
        let Some(menu) = &self.menu else { return };
        let active = menu.sub.as_deref().unwrap_or(menu);
        match active.selected.and_then(|row| active.rows[row].item) {
            Some(item) if !item.sub.is_empty() => self.open_submenu(true),
            Some(item) => {
                self.menu = None;
                self.execute(item.command, event_loop);
            }
            None => self.menu = None,
        }
        self.redraw();
    }

    // ---- Grid ----------------------------------------------------------

    /// How the grid is laid out in a window of this size.
    fn grid_layout(&self, window: (u32, u32)) -> GridLayout {
        GridLayout::for_window(window, self.grid_columns)
    }

    /// Shows more or fewer pictures across the grid, making each smaller
    /// or larger.
    fn zoom_grid(&mut self, larger: bool) {
        let layout = self.grid_layout(self.window_size());
        let columns = (layout.columns + if larger { -1 } else { 1 }).clamp(GRID_COLUMNS.0, GRID_COLUMNS.1);
        self.grid_columns = Some(columns);
        // Start afresh around the cursor: the rows are not what they were.
        let layout = self.grid_layout(self.window_size());
        if let Some(grid) = &mut self.grid {
            *grid = Grid { moved: grid.moved, ..Grid::at(grid.cursor, &layout) };
        }
        self.toast(format!("{columns} across"));
        self.request();
        self.redraw();
    }

    fn toggle_grid(&mut self) {
        let window = self.window_size();
        let layout = self.grid_layout(window);
        let id = self.id();
        // Out of the grid, the picture on view grows from its cell, or from
        // where it was playing there; into the grid it shrinks to its
        // cell, unless it is a video, which plays on in its cell instead.
        let corner = self.pip_place(window).or_else(|| self.pip_small(window));
        // A video playing in its cell grows from the frame it had got to.
        let live = self.pip_live.take().filter(|_| self.grid.is_some()).map(Arc::new);
        let stays = self.grid.is_none() && self.video.is_some();
        self.grow = match &self.grid {
            Some(_) => corner.or_else(|| self.grid_cell(self.pos, &layout, window)).map(|cell| Grow { id, cell, since: Instant::now(), opening: true, picture: live.clone() }),
            // A video shrinks to its cell as the frame it has got to, and
            // plays on there.
            None if stays => Some(Grow { id, cell: [0; 4], since: Instant::now(), opening: false, picture: self.video_frame(window) }),
            None => Some(Grow { id, cell: [0; 4], since: Instant::now(), opening: false, picture: None }),
        };
        self.pip = stays;
        self.grid = match self.grid {
            Some(_) => None,
            None => Some(Grid::at(self.pos, &layout)),
        };
        self.ring_snap.set(true);
        self.menu = None;
        self.drag = None;
        self.request();
        self.sync_video();
        self.apply_video_view();
        self.redraw();
    }

    /// Where the picture of the file at `pos` is in the grid, if it is on
    /// the page showing.
    fn grid_cell(&self, pos: usize, layout: &GridLayout, window: (u32, u32)) -> Option<[i32; 4]> {
        let area = self.grid_area(layout, pos, window.1);
        let place = match self.loader.thumb(self.list[pos]) {
            Some(thumb) => Self::grid_picture(area, (thumb.width, thumb.height), false),
            None => area,
        };
        (place[0] >= 0 && place[0] + place[2] <= window.0 as i32).then_some(place)
    }

    /// The frame the video on view is showing, as it is in the window: to
    /// shrink into its cell of the grid.
    fn video_frame(&self, window: (u32, u32)) -> Option<Arc<Frame>> {
        let video = self.video.as_ref().filter(|video| video.showing)?;
        let native = self.shown.as_ref().filter(|(id, _)| *id == video.id).map(|(_, frame)| frame.native)?;
        // Only the picture, without the bars beside it.
        let place = self.layout_of(native);
        let (w, h) = (place.width.min(window.0).max(1), place.height.min(window.1).max(1));
        let mut pixels = vec![0; w as usize * h as usize];
        // The frame kept for the window is not the one the player has now.
        self.video_kept.set(None);
        self.player.as_ref()?.render(&mut pixels, w, h).then(|| Arc::new(Frame::still(w, h, native, pixels)))
    }

    /// The picture that grows out of a cell of the grid or shrinks into
    /// one: for a video the frame its cell shows, since the frame it opens
    /// on is as often as not black, and otherwise the picture as it is on
    /// view, or failing that the one from its cell.
    fn grow_picture(&self, id: usize) -> Option<Arc<Frame>> {
        let shown = self.shown.as_ref().filter(|(shown, _)| *shown == id && !files::is_video(&self.files[id]));
        shown.map(|(_, frame)| frame.clone()).or_else(|| self.loader.thumb(id))
    }

    /// The picture growing out of its cell, and where it has got to on its
    /// way to its place in the window; nothing once it is there.
    fn growing(&mut self) -> Option<(Arc<Frame>, [i32; 4])> {
        let grow = self.grow.as_ref().filter(|grow| grow.opening && self.options.animations)?;
        let picture = grow.picture.clone().or_else(|| self.grow_picture(grow.id)).filter(|_| grow.since.elapsed() < GROW && grow.id == self.id());
        let Some(picture) = picture else {
            self.grow = None;
            return None;
        };
        let full = self.layout_of(picture.native);
        let full = [full.x, full.y, full.width as i32, full.height as i32];
        let part = Glide { from: 0.0, to: 1.0, since: grow.since }.part(GROW);
        self.animating.set(true);
        Some((picture, std::array::from_fn(|side| (f64::from(grow.cell[side]) + f64::from(full[side] - grow.cell[side]) * part).round() as i32)))
    }

    /// Draws the picture that was on view shrinking into its cell, over
    /// the grid that has just come up.
    fn draw_shrinking(&mut self, canvas: &mut Canvas, layout: &GridLayout) {
        let Some(grow) = self.grow.as_ref().filter(|grow| !grow.opening && self.options.animations) else { return };
        let window = (canvas.width as u32, canvas.height as u32);
        let pos = self.list.iter().position(|id| *id == grow.id);
        let frame = grow.picture.clone().or_else(|| self.grow_picture(grow.id));
        let (Some(pos), Some(frame), true) = (pos, frame, grow.since.elapsed() < GROW) else {
            self.grow = None;
            return;
        };
        let Some(cell) = self.grid_cell(pos, layout, window) else { return };
        let full = self.layout_of(frame.native);
        let full = [full.x, full.y, full.width as i32, full.height as i32];
        let part = Glide { from: 0.0, to: 1.0, since: grow.since }.part(GROW);
        let [x, y, w, h]: [i32; 4] = std::array::from_fn(|side| (f64::from(full[side]) + f64::from(cell[side] - full[side]) * part).round() as i32);
        canvas.image_fast(&frame, x, y, w, h);
        self.animating.set(true);
    }

    /// Moves the grid cursor by a number of images, stopping at either end
    /// of the list. The page on show follows the cursor.
    fn move_cursor(&mut self, by: i64) {
        let last = self.list.len() as i64 - 1;
        let layout = self.grid_layout(self.window_size());
        let Some(grid) = &mut self.grid else { return };
        grid.cursor = (grid.cursor as i64).saturating_add(by).clamp(0, last) as usize;
        grid.moved = true;
        // At once, so that what happens next finds the right page on show.
        grid.follow(&layout);
        self.request();
        self.redraw();
    }

    /// Puts the grid cursor on the thumbnail under the pointer, if any.
    fn point_cursor(&mut self) -> bool {
        let layout = self.grid_layout(self.window_size());
        let (len, (x, y)) = (self.list.len(), self.mouse);
        let Some(grid) = &mut self.grid else { return false };
        // Where the grid is sliding to, not where it is: a cell picked
        // out mid-slide must not pull the grid back.
        let Some(pos) = layout.cell_at(x, y, grid.target).filter(|pos| *pos < len) else {
            return false;
        };
        if pos != grid.cursor {
            (grid.cursor, grid.moved) = (pos, true);
            self.redraw();
        }
        true
    }

    /// Leaves the grid to view the image under the cursor.
    fn open_from_grid(&mut self) {
        // The picture under the cursor grows out of its cell.
        let window = self.window_size();
        let layout = self.grid_layout(window);
        if let Some(cursor) = self.grid.as_ref().map(|grid| grid.cursor) {
            let id = self.list[cursor];
            self.grow = self.grid_cell(cursor, &layout, window).map(|cell| Grow { id, cell, since: Instant::now(), opening: true, picture: None });
        }
        let Some(grid) = self.grid.take() else { return };
        self.pip = false;
        self.go_to(grid.cursor, grid.cursor >= self.pos);
        self.request();
        self.sync_video();
        self.apply_video_view();
        self.redraw();
    }

    /// Moves the grid cursor one cell. Sideways the rows run on from page
    /// to page: going right from the end of a row lands at the start of
    /// the same row on the next page, which slides in, and left likewise.
    /// Up and down stay within the page.
    fn step_cursor(&mut self, dx: i64, dy: i64) {
        let layout = self.grid_layout(self.window_size());
        let Some(grid) = &self.grid else { return };
        let (columns, per_page) = (i64::from(layout.columns), layout.per_page() as i64);
        let (cursor, last) = (grid.cursor as i64, self.list.len() as i64 - 1);
        let column = cursor % per_page % columns;
        let mut target = cursor + dx + dy * columns;
        if dx > 0 && column == columns - 1 {
            // Nothing beyond the last page; a short one may not reach
            // this row, in which case its last picture is taken.
            if (cursor / per_page + 1) * per_page > last {
                return;
            }
            target = (cursor + per_page - (columns - 1)).min(last);
        } else if dx < 0 && column == 0 {
            target = cursor - per_page + (columns - 1);
        }
        let same_page = target.div_euclid(per_page) == cursor / per_page;
        if (0..=last).contains(&target) && (dy == 0 || same_page) {
            self.move_cursor(target - cursor);
        }
    }

    /// Shuffles the viewing order. Where the list can be seen, in the grid,
    /// the filmstrip or the file list, the file in hand stays where it is
    /// and everything before and after it changes. With only the one file
    /// to be seen, another takes its place and is shown.
    fn shuffle(&mut self) {
        (self.sort, self.reversed, self.shuffled) = (None, false, true);
        let held = self.list[self.focus()];
        let stays = self.grid.is_some() || self.strip_visible() || self.show_list;
        let pos = self.pos;
        let mut rng = std::mem::replace(&mut self.rng, Rng::from_clock());
        self.reorder(|ids, _| {
            let place = ids.iter().position(|id| *id == held);
            rng.shuffle(ids);
            let (Some(place), Some(at)) = (place, ids.iter().position(|id| *id == held)) else { return };
            if stays {
                // Back to the place it had, whatever landed there going
                // where the shuffle had put it.
                ids.swap(place, at);
            } else if place == at {
                // Out of the place it had, so that another has it.
                let other = (place + 1) % ids.len();
                ids.swap(place, other);
            }
        });
        self.rng = rng;
        if !stays {
            // `reorder` followed the file; this goes back to its place.
            self.go_to(pos, true);
        }
    }

    /// Puts the list in a different order, and with it the files a kept
    /// search is hiding. The file on view stays on view, and the grid's
    /// cursor stays on its file.
    fn reorder(&mut self, mut order: impl FnMut(&mut Vec<usize>, &[PathBuf])) {
        let (viewed, focus) = (self.id(), self.list[self.focus()]);
        order(&mut self.list, &self.files);
        if let Some((_, all)) = &mut self.filter {
            order(all, &self.files);
        }
        let at = |id: usize| self.list.iter().position(|&other| other == id).unwrap_or(0);
        let (viewed, focus) = (at(viewed), at(focus));
        self.pos = viewed;
        if let Some(grid) = &mut self.grid {
            grid.cursor = focus;
        }
        self.request();
        self.refresh();
        self.redraw();
    }

    // ---- Workspaces ----------------------------------------------------

    /// Goes to another workspace. What this one was showing is kept for
    /// coming back to: its place, its order, a kept search. The other is
    /// listed the first time it is gone to, and picked up where it was
    /// left after that.
    fn switch_space(&mut self, to: usize) {
        let Some(name) = self.spaces.get(to).map(|space| space.name.clone()) else {
            return self.toast(format!("There is no workspace {}", to + 1));
        };
        if to == self.space {
            return self.toast(format!("@{name}"));
        }
        // Only one that resumes is taken up as it was left; any other is
        // listed anew, to show what is there now.
        let start = self.spaces[to].start;
        let incoming = match self.spaces[to].kept.take().filter(|kept| !kept.list.is_empty() && start == Start::Resume) {
            Some(kept) => kept,
            None => {
                let space = &self.spaces[to];
                let sort = space.sort;
                let listing = files::Listing { recursive: space.recursive, sort, reverse: space.reverse, filelist: space.filelist.clone(), ..files::Listing::default() };
                let found = files::collect(&space.paths, &listing).files;
                if found.is_empty() {
                    return self.toast(format!("@{name} has nothing to show"));
                }
                // Files seen before, in whatever workspace, keep their ids.
                let mut list = self.ids_for(found);
                let reversed = self.spaces[to].reverse;
                // Shuffled anew each time, for one that starts at random.
                if start == Start::Random {
                    self.rng.shuffle(&mut list);
                }
                Kept { list, pos: 0, cursor: 0, filter: None, filter_from: Vec::new(), sort: sort.filter(|_| start != Start::Random), reversed, shuffled: start == Start::Random }
            }
        };
        let leaving = Kept {
            cursor: self.focus(),
            list: std::mem::take(&mut self.list),
            pos: self.pos,
            filter: self.filter.take(),
            filter_from: std::mem::take(&mut self.filter_from),
            sort: self.sort,
            reversed: self.reversed,
            shuffled: self.shuffled,
        };
        self.spaces[self.space].kept = Some(leaving);
        self.space = to;

        // Refreshing and watching go by where the options point.
        let space = &self.spaces[to];
        self.options.paths = space.paths.clone();
        (self.options.listing.recursive, self.options.listing.sort, self.options.listing.reverse) = (space.recursive, space.sort, space.reverse);
        self.options.listing.filelist = space.filelist.clone();

        let last = incoming.list.len() - 1;
        let count = incoming.list.len();
        (self.filter, self.sort, self.reversed, self.shuffled) = (incoming.filter, incoming.sort, incoming.reversed, incoming.shuffled);
        self.filter_from = incoming.filter_from;
        (self.list, self.pos) = (incoming.list, incoming.pos.min(last));
        // A session gone to for the first time is landed in where it was left.
        if let Some(spot) = self.spaces[to].spot.take() {
            self.take_up(spot);
        }
        (self.palette, self.menu, self.prompt, self.overview) = (None, None, None, None);
        (self.strip_scroll, self.strip_over, self.list_scroll, self.list_hover) = (0, None, None, None);
        let layout = self.grid_layout(self.window_size());
        if let Some(grid) = &mut self.grid {
            *grid = Grid::at(incoming.cursor.min(last), &layout);
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
        self.sync_video();
        // A watch goes on, now on this workspace's folders.
        if self.watch.is_some() {
            self.set_watch(true);
        }
        self.toast(format!("@{name}   {count} files"));
        if self.options.verbose {
            eprintln!("jog: workspace @{name}: {count} files");
        }
        self.redraw();
    }

    /// The key that goes to a workspace, as a list shows it: Ctrl with
    /// its number for one of the config file's, Ctrl+` for what was opened
    /// by its path.
    fn space_key(&self, which: usize) -> &'static str {
        match self.spaces[which].saved {
            true => self.spaces[which].key.and_then(|key| SPACE_KEYS.get(key - 1)).copied().unwrap_or(""),
            false if which == self.home => "Ctrl+`",
            false => "",
        }
    }

    /// The workspaces whose names match what follows an `@` in the find
    /// box, as rows to pick from: each with its folder and its key, the
    /// one on show ticked. And, for a folder that was opened by its path,
    /// a row to save it as one.
    fn spaces_matching(&mut self, typed: &str) -> Vec<(String, &'static str, Command)> {
        let query = Query::parse(typed);
        let wanted = |name: &str, loose: bool| query.is_empty() || query.score(&Facts { name, video: false, marked: false, seen: None, verdict: None, size: None }, loose).is_some();
        let row = |(at, space): (usize, &Space)| {
            let folder = space.paths.first().map(|path| short_dir(path)).unwrap_or_default();
            let unsaved = if space.saved { "" } else { "   (not saved)" };
            (format!("{}   {folder}{unsaved}", space.name), self.space_key(at), Command::Workspace(at))
        };
        let pass = |loose: bool| -> Vec<(String, &'static str, Command)> {
            self.spaces.iter().enumerate().filter(|(_, space)| wanted(&space.name, loose)).map(row).collect()
        };
        let mut rows = pass(false);
        if rows.is_empty() {
            rows = pass(true);
        }
        let here = &self.spaces[self.space];
        if !here.saved && query.is_empty() {
            rows.push((format!("Save {} as a workspace...", here.name), "", Command::SaveWorkspace));
        }
        rows
    }

    /// Opens a box for the name to save the folder on view under, as a
    /// workspace. Only a single folder can be one.
    fn ask_workspace_name(&mut self) {
        let here = &self.spaces[self.space];
        if here.saved {
            return self.toast(format!("@{} is a workspace already", here.name));
        }
        if !matches!(here.paths.as_slice(), [only] if only.is_dir()) {
            return self.toast("Only a single folder can be saved as a workspace");
        }
        let text: Vec<char> = here.name.chars().collect();
        let id = self.list[self.focus()];
        self.prompt = Some(Prompt { id, kind: PromptKind::SaveWorkspace, field: TextField::new(text.clone(), text.len()) });
        (self.menu, self.palette) = (None, None);
        self.redraw();
    }

    /// Writes the folder on view into the config file as a workspace.
    fn save_space(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        if self.spaces.iter().any(|space| space.saved && space.name == name) {
            return self.toast(format!("There is a workspace named {name} already"));
        }
        if self.dry(|_| vec![format!("add the workspace {name} to the config file")], "would save the workspace") {
            return;
        }
        let Some(folder) = self.spaces[self.space].paths.first().and_then(|path| path.canonicalize().ok()) else {
            return self.toast("Cannot tell where this folder is");
        };
        match crate::options::save_workspace(name, &folder) {
            Ok(file) => {
                let space = &mut self.spaces[self.space];
                (space.name, space.saved) = (name.to_string(), true);
                // It takes the first digit that no other has.
                let taken: Vec<usize> = self.spaces.iter().filter_map(|space| space.key).collect();
                self.spaces[self.space].key = (1..=9).find(|key| !taken.contains(key));
                self.toast(format!("Saved as @{name} in {}", short_dir(&file)));
            }
            Err(error) => self.toast(format!("Could not save it: {error}")),
        }
    }

    /// The ids of some files, those not seen before being taken on. Files
    /// seen before, in whatever workspace, keep the ids they have.
    fn ids_for(&mut self, found: Vec<PathBuf>) -> Vec<usize> {
        let known: HashMap<PathBuf, usize> = self.files.iter().cloned().enumerate().map(|(id, path)| (path, id)).collect();
        let mut list = Vec::with_capacity(found.len());
        for path in found {
            list.push(known.get(&path).copied().unwrap_or_else(|| {
                self.loader.add_file(path.clone());
                self.files.push(path);
                self.files.len() - 1
            }));
        }
        list
    }

    /// The files of a workspace, so far as they are known: of the one on
    /// show, the list; of another, what it was showing when it was left
    /// or, not yet gone to, what the overview has had listed.
    fn space_ids(&self, which: usize) -> &[usize] {
        match which == self.space {
            true => &self.list,
            false => self.spaces[which].kept.as_ref().map_or(&[], |kept| &kept.list),
        }
    }

    /// Brings up the overview: every workspace as a card. Those not yet
    /// gone to are listed on other threads, so that their cards have
    /// pictures to show; a large folder may take a moment.
    fn open_overview(&mut self) {
        self.overview = Some(Overview { cursor: self.space, tick: 0 });
        (self.menu, self.palette, self.overview_due) = (None, None, None);
        for which in 0..self.spaces.len() {
            let space = &mut self.spaces[which];
            if which == self.space || space.kept.is_some() || std::mem::replace(&mut space.asked, true) {
                continue;
            }
            let listing = files::Listing { recursive: space.recursive, sort: space.sort, reverse: space.reverse, filelist: space.filelist.clone(), ..files::Listing::default() };
            let (paths, proxy) = (space.paths.clone(), self.proxy.clone());
            let key = paths.clone();
            std::thread::spawn(move || drop(proxy.send_event(Event::SpaceListed(key, files::collect(&paths, &listing).files))));
        }
        self.sync_video();
        self.request();
        self.redraw();
    }

    /// Closes the overview, going to a workspace if one was picked.
    fn leave_overview(&mut self, to: Option<usize>) {
        (self.overview, self.overview_due) = (None, None);
        match to.filter(|to| *to != self.space) {
            Some(to) => self.switch_space(to),
            None => {
                self.sync_video();
                self.request();
                self.redraw();
            }
        }
    }

    /// Forgets a session in the overview, and takes its card away. The one
    /// open stays: it would be kept again on quitting.
    fn forget_session(&mut self, which: usize) {
        if which == self.space {
            return self.toast("This one is open");
        }
        let Some(id) = self.spaces.get(which).and_then(|space| space.spot.as_ref()).map(|spot| spot.id.clone()) else {
            return self.toast("Only sessions can be forgotten here");
        };
        if let Err(error) = sessions::forget(&id) {
            return self.toast(format!("Not forgotten: {error}"));
        }
        self.spaces.remove(which);
        if self.space > which {
            self.space -= 1;
        }
        if let Some(overview) = &mut self.overview {
            overview.cursor = overview.cursor.min(self.spaces.len() - 1);
            overview.tick = 0;
        }
        self.toast(format!("Forgot {id}"));
        self.request();
        self.redraw();
    }

    /// Handles a key in the overview. Returns whether it was one of its.
    fn overview_key(&mut self, key: Key<&str>) -> bool {
        let columns = self.overview_layout().0;
        let last = self.spaces.len() - 1;
        let Some(overview) = &mut self.overview else { return false };
        let to = match key {
            Key::Named(NamedKey::Escape) | Key::Character("G" | "g" | "q") => {
                self.leave_overview(None);
                return true;
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                let picked = overview.cursor;
                self.leave_overview(Some(picked));
                return true;
            }
            // Delete forgets the session picked out.
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                let picked = overview.cursor;
                self.forget_session(picked);
                return true;
            }
            // A digit goes straight to that workspace.
            Key::Character(digit) if !self.modifiers.control_key() && digit.parse::<usize>().is_ok_and(|n| self.spaces.iter().any(|space| space.key == Some(n))) => {
                let wanted = digit.parse::<usize>().ok();
                let picked = self.spaces.iter().position(|space| space.key == wanted).unwrap_or(0);
                self.leave_overview(Some(picked));
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => (overview.cursor + 1).min(last),
            Key::Named(NamedKey::ArrowLeft) => overview.cursor.saturating_sub(1),
            Key::Named(NamedKey::ArrowDown) => (overview.cursor + columns).min(last),
            Key::Named(NamedKey::ArrowUp) => overview.cursor.checked_sub(columns).unwrap_or(overview.cursor),
            Key::Named(NamedKey::Home) => 0,
            Key::Named(NamedKey::End) => last,
            _ => return false,
        };
        if to != overview.cursor {
            (overview.cursor, overview.tick) = (to, 0);
            self.overview_due = None;
            self.request();
        }
        self.redraw();
        true
    }

    /// How the overview's cards are laid out: how many across, and for
    /// each workspace where its picture goes: left, top, width, height.
    /// Under each picture there is room for its name and a line about it.
    fn overview_layout(&self) -> (usize, Vec<[i32; 4]>) {
        let (width, height) = self.window_size();
        let (width, height, px) = (width as i32, height as i32, ui_px(height) as i32);
        let count = self.spaces.len() as i32;
        let columns = if count <= 3 { count } else { (f64::from(count).sqrt().ceil() as i32).max(1) };
        let rows = (count + columns - 1) / columns;
        let (gap, words) = (px * 3, Text::line_height(px as u32) * 3);
        // As large as the window allows, both ways, up to a size that
        // still reads as a card.
        let across = (width * 85 / 100 - gap * (columns - 1)) / columns;
        let down = ((height * 80 / 100 - gap * (rows - 1)) / rows - words) * 16 / 10;
        let card_w = across.min(down).min(px * 40).max(px * 6);
        let card_h = card_w * 10 / 16;
        let left = (width - columns * card_w - (columns - 1) * gap) / 2;
        let top = (height - rows * (card_h + words) - (rows - 1) * gap) / 2 + px;
        let place = |which: i32| {
            // A last row that is not full sits in the middle.
            let (column, row) = (which % columns, which / columns);
            let in_row = if row == rows - 1 { count - row * columns } else { columns };
            let indent = (columns - in_row) * (card_w + gap) / 2;
            [left + indent + column * (card_w + gap), top + row * (card_h + words + gap), card_w, card_h]
        };
        (columns as usize, (0..count).map(place).collect())
    }

    /// Which workspace's card is at a window position.
    fn overview_card_at(&self, x: f64, y: f64) -> Option<usize> {
        let words = Text::line_height(ui_px(self.window_size().1)) * 3;
        let (_, cards) = self.overview_layout();
        cards.iter().position(|[left, top, width, height]| (*left..left + width).contains(&(x as i32)) && (*top..top + height + words).contains(&(y as i32)))
    }

    /// Draws the overview: each workspace as a card of four of its
    /// pictures, with its name, its key and how many files it has. The
    /// card picked out shows one picture at a time instead, going through
    /// what is in it.
    fn draw_overview(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let Some((cursor, tick)) = self.overview.as_ref().map(|overview| (overview.cursor, overview.tick)) else { return };
        let (_, cards) = self.overview_layout();
        let (pad, line) = (px as i32, Text::line_height(px));
        let round = pad;

        // Sessions to choose from are shown the same way, and named so.
        let what = if self.spaces.iter().any(|space| space.caption.is_some()) { "Sessions" } else { "Workspaces" };
        let heading = format!("{}  {what}", commands::icons::FOLDERS);
        let big = px * 3 / 2;
        let first_top = cards.iter().map(|card| card[1]).min().unwrap_or(0);
        let heading_left = (canvas.width - text.width(&heading, big)) / 2;
        text.draw(canvas, heading_left, (first_top - Text::line_height(big) - 2 * pad).max(pad), big, TEXT_COLOR, &heading);

        for (which, &[left, top, width, height]) in cards.iter().enumerate() {
            let ids = self.space_ids(which).to_vec();
            canvas.round_fill(left, top, width, height, round, PLACEHOLDER_COLOR, 256);
            // Scaled to fit its place, in the middle of it.
            let picture = |canvas: &mut Canvas, id: usize, [x, y, w, h]: [i32; 4]| {
                let Some(thumb) = self.loader.thumb(id) else { return false };
                let (fit_w, fit_h) = fit_size((thumb.width, thumb.height), (w as u32, h as u32), true);
                canvas.image(&thumb, x + (w - fit_w as i32) / 2, y + (h - fit_h as i32) / 2, fit_w as i32, fit_h as i32);
                true
            };
            if which == cursor && !ids.is_empty() {
                // The one picked out runs through its pictures; one not
                // yet decoded leaves the one before it up.
                let shown = (0..ids.len().min(6)).map(|back| ids[(tick + ids.len() - back) % ids.len()]).any(|id| picture(canvas, id, [left, top, width, height]));
                if !shown {
                    picture(canvas, ids[0], [left, top, width, height]);
                }
            } else {
                // Four of them, two by two.
                let gap = (pad / 3).max(2);
                let (cell_w, cell_h) = ((width - gap) / 2, (height - gap) / 2);
                for (at, id) in ids.iter().take(4).enumerate() {
                    let (column, row) = (at as i32 % 2, at as i32 / 2);
                    picture(canvas, *id, [left + column * (cell_w + gap), top + row * (cell_h + gap), cell_w, cell_h]);
                }
            }
            canvas.round_off(left, top, width, height, round, GRID_COLOR);
            // Ringed brightly when picked out; quietly, in the accent,
            // when it is the workspace that was on show.
            if which == cursor || which == self.space {
                let edge = (pad / 5).max(2);
                let color = if which == cursor { TEXT_COLOR } else { accent() };
                canvas.round_outline(left - 2 * edge, top - 2 * edge, width + 4 * edge, height + 4 * edge, round + 2 * edge, edge, color);
            }

            let space = &self.spaces[which];
            let name = clipped(text, &space.name, width, px * 5 / 4);
            let name_top = top + height + pad / 2;
            let name_left = left + (width - text.width(&name, px * 5 / 4)) / 2;
            text.draw(canvas, name_left, name_top, px * 5 / 4, if which == cursor { TEXT_COLOR } else { DIM_TEXT_COLOR }, &name);
            let files = match ids.len() {
                0 if which != self.space => "listing\u{2026}".to_string(),
                1 => "1 file".to_string(),
                count => format!("{count} files"),
            };
            let key = Some(self.space_key(which)).filter(|key| !key.is_empty()).map(|key| format!("{key}   ")).unwrap_or_default();
            let about = space.caption.clone().unwrap_or_else(|| format!("{key}{files}{}", if space.saved { "" } else { "   not saved" }));
            let about = clipped(text, &about, width, px);
            let about_left = left + (width - text.width(&about, px)) / 2;
            text.draw(canvas, about_left, name_top + line * 5 / 4, px, DIM_TEXT_COLOR, &about);
        }
        let hint = match what {
            "Sessions" => "Enter to open  ·  Delete to forget  ·  Esc to close",
            _ => "Enter to open  ·  Esc to close",
        };
        let small = (px * 5 / 6).max(11);
        let hint_left = (canvas.width - text.width(hint, small)) / 2;
        text.draw(canvas, hint_left, canvas.height - Text::line_height(small) - pad, small, DIM_TEXT_COLOR, hint);

        // The slideshow moves on while there is more than one to show.
        if self.space_ids(cursor).len() > 1 && self.overview_due.is_none() {
            self.overview_due = Some(Instant::now() + OVERVIEW_STEP);
        }
    }

    // ---- Refresh and watch ---------------------------------------------

    /// Starts or stops watching the directories. A thread lists them once
    /// a second and reports when what it finds has changed.
    pub fn set_watch(&mut self, on: bool) {
        if let Some(watching) = self.watch.take() {
            watching.store(false, atomic::Ordering::Relaxed);
        }
        if on {
            let watching = Arc::new(AtomicBool::new(true));
            self.watch = Some(watching.clone());
            let (paths, proxy) = (self.options.paths.clone(), self.proxy.clone());
            let listing = files::Listing {
                recursive: self.options.listing.recursive,
                filelist: self.options.listing.filelist.clone(),
                ..files::Listing::default()
            };
            std::thread::spawn(move || {
                let mut known = files::relist(&paths, &listing);
                while watching.load(atomic::Ordering::Relaxed) {
                    std::thread::sleep(WATCH_EVERY);
                    let listed = files::relist(&paths, &listing);
                    if listed != known && watching.load(atomic::Ordering::Relaxed) {
                        known.clone_from(&listed);
                        if proxy.send_event(Event::Listed(listed)).is_err() {
                            return;
                        }
                    }
                }
            });
            // Whatever changed before the watching began.
            let listed = files::relist(&self.options.paths, &self.options.listing);
            self.take_listing(listed, true);
        }
        self.toast(if on { "Watching for new files" } else { "No longer watching" });
    }

    /// Brings the list up to date with `listed`, the files now in the
    /// directories: those not seen before join it where the order puts
    /// them, and those no longer on disk leave it. Marks, the place in the
    /// list and a kept search all stay. When watching, a file arriving
    /// just past the end being viewed is gone to.
    fn take_listing(&mut self, listed: Vec<PathBuf>, watching: bool) {
        self.take_listing_as(listed, if watching { Taking::Watching } else { Taking::Refreshing });
    }

    fn take_listing_as(&mut self, listed: Vec<PathBuf>, taking: Taking) {
        let watching = taking != Taking::Refreshing;
        // Not under an open search box, whose own lists would go stale.
        if self.palette.is_some() {
            return;
        }
        // What a triage decided is looked up again for the files as they
        // are numbered now.
        self.verdicts.borrow_mut().clear();
        let seen: HashSet<&Path> = self.files.iter().map(PathBuf::as_path).collect();
        let fresh: Vec<PathBuf> = listed.iter().filter(|path| !seen.contains(path.as_path())).cloned().collect();
        let exists = |id: &usize| self.files[*id].exists();
        let total = self.filter.as_ref().map_or(&self.list, |(_, all)| all);
        let gone: HashSet<usize> = total.iter().copied().filter(|id| !exists(id)).collect();
        // With every file gone there would be nothing to show: leave them.
        let gone = if gone.len() == total.len() && fresh.is_empty() { HashSet::new() } else { gone };
        if fresh.is_empty() && gone.is_empty() {
            if !watching {
                self.toast("Refreshed: nothing new");
            }
            return;
        }

        let (viewed, focus) = (self.id(), self.list[self.focus()]);
        let (was_first, was_last) = (self.pos == 0, self.pos + 1 == self.list.len());
        let first_new = self.files.len();
        let query = self.filter.as_ref().map(|(typed, _)| Query::parse(typed));
        for path in fresh.iter().cloned() {
            let id = self.files.len();
            self.loader.add_file(path.clone());
            self.files.push(path);
            // Among a kept search's matches only if it matches too.
            let matches = query.as_ref().is_none_or(|query| self.score(query, id, false).is_some());
            if let Some((_, all)) = &mut self.filter {
                all.push(id);
            }
            if matches {
                self.list.push(id);
            }
        }
        self.sizes.clear();

        // Leave what has gone out, unless that would leave nothing.
        let keep = |list: &mut Vec<usize>| {
            if list.iter().any(|id| !gone.contains(id)) {
                list.retain(|id| !gone.contains(id));
            }
        };
        keep(&mut self.list);
        if let Some((_, all)) = &mut self.filter {
            keep(all);
        }
        self.marks.retain(|id| !gone.contains(id));

        // Put the newcomers where the order has them. A shuffled list has
        // no such place, so there they stay at the end.
        let (sort, reversed, shuffled) = (self.sort, self.reversed, self.shuffled);
        let place: HashMap<&Path, usize> = listed.iter().enumerate().map(|(at, path)| (path.as_path(), at)).collect();
        let order = |ids: &mut Vec<usize>, files: &[PathBuf]| {
            match sort {
                _ if shuffled => return,
                Some(sort) => files::sort_ids(ids, files, sort),
                None => ids.sort_by_key(|id| place.get(files[*id].as_path()).copied().unwrap_or(usize::MAX)),
            }
            if reversed {
                ids.reverse();
            }
        };
        order(&mut self.list, &self.files);
        if let Some((_, all)) = &mut self.filter {
            order(all, &self.files);
        }

        // Stay on the file that was on view, or where it was if it went.
        let at = |id: usize, fallback: usize| self.list.iter().position(|&other| other == id).unwrap_or(fallback);
        let last = self.list.len() - 1;
        let (mut pos, cursor) = (at(viewed, self.pos.min(last)), at(focus, self.focus().min(last)));
        let is_new = |pos: usize| self.list[pos] >= first_new;
        if taking == Taking::Watching && self.grid.is_none() {
            if was_last && is_new(last) {
                pos = last;
            } else if was_first && is_new(0) && sort == Some(Sort::Newest) && !reversed {
                pos = 0;
            }
        }
        let moved = self.list[pos] != viewed;
        self.pos = pos;
        if let Some(grid) = &mut self.grid {
            grid.cursor = cursor;
        }
        if moved {
            self.error = None;
            self.reset_view();
        }

        let mut parts = Vec::new();
        if !fresh.is_empty() {
            parts.push(format!("{} new", fresh.len()));
        }
        if !gone.is_empty() {
            parts.push(format!("{} gone", gone.len()));
        }
        if self.options.verbose {
            fresh.iter().for_each(|path| eprintln!("jog: new file {}", path.display()));
            eprintln!("jog: refreshed: {}, {} files now", parts.join(", "), self.total());
        }
        if taking != Taking::Adopting {
            self.toast(format!("{}: {}", if watching { "Watch" } else { "Refreshed" }, parts.join(", ")));
        }
        self.request();
        self.refresh();
        self.redraw();
    }

    // ---- Go to ---------------------------------------------------------

    /// Opens the box for finding files, on `/` and Ctrl+K. A search kept
    /// earlier comes back as it was typed, ready to be changed.
    /// Opens the box with something already typed: `>sort `, say, to
    /// pick among the ways of sorting by typing a few letters.
    fn open_palette_with(&mut self, typed: &str) {
        self.open_palette();
        if let Some(palette) = &mut self.palette {
            palette.field = TextField::new(typed.chars().collect(), typed.chars().count());
        }
        self.palette_changed();
    }

    fn open_palette(&mut self) {
        let text: Vec<char> = self.filter.as_ref().map(|(typed, _)| typed.chars().collect()).unwrap_or_default();
        let field = TextField::new(text.clone(), text.len());
        self.menu = None;
        self.palette = Some(Palette { field, matches: Vec::new(), narrowed: false, selected: 0, focus: 0, drawn: Vec::new(), commands: Vec::new(), first: 0 });
        self.palette_changed();
        // Start on the file on view, where it is among the matches.
        let current = self.list[self.focus()];
        if let Some(palette) = &mut self.palette {
            palette.selected = palette.matches.iter().position(|id| *id == current).unwrap_or(0);
        }
        let rows = self.palette_layout().map_or(1, |layout| layout.rows.max(1));
        if let Some(palette) = &mut self.palette {
            palette.show_selected(rows);
        }
    }

    /// Finds the matches for what the box now says, out of every file:
    /// all of them, in order, with nothing typed, otherwise the closest
    /// match first.
    fn palette_changed(&mut self) {
        let Some(palette) = &self.palette else { return };
        let typed = palette.field.string();
        // Nothing typed: the files viewed lately, then the commands.
        let recent = typed.is_empty().then(|| [self.recent_files(), self.recent_commands()].concat()).filter(|recent| !recent.is_empty());
        let (matches, narrowed, commands) = match typed.chars().next() {
            // Nothing typed yet: the commands run from here lately, the
            // latest first, to run again at once. A letter finds files.
            None if recent.is_some() => (Vec::new(), false, recent.unwrap_or_default()),
            // `>` and then a number is a size to find files by.
            Some('>') if !typed[1..].starts_with(|c: char| c.is_ascii_digit()) => (Vec::new(), false, self.commands_matching(&typed[1..])),
            Some(':') => self.place_named(&typed[1..]),
            Some('@') => (Vec::new(), false, self.spaces_matching(&typed[1..])),
            Some('!') => (Vec::new(), false, self.shell_matching(&typed[1..])),
            _ => {
                let (matches, narrowed) = self.matching(&typed);
                (matches, narrowed, Vec::new())
            }
        };
        if let Some(palette) = &mut self.palette {
            (palette.matches, palette.narrowed, palette.selected, palette.focus) = (matches, narrowed, 0, 0);
            palette.first = 0;
            palette.commands = commands;
        }
        self.request();
        self.redraw();
    }

    /// The commands whose names match what follows a `>` in the box, the
    /// closest first: everything in the menu that applies to the file on
    /// view, those in a submenu named with it, as in "Sort: Newest first".
    fn commands_matching(&mut self, typed: &str) -> Vec<(String, &'static str, Command)> {
        let video = files::is_video(&self.files[self.list[self.focus()]]);
        let mut all = Vec::new();
        let mut groups: Vec<(&str, &'static [Option<MenuItem>])> = vec![("", commands::MENU)];
        while let Some((group, items)) = groups.pop() {
            for item in items.iter().flatten() {
                let applies = match item.applies {
                    Applies::Always => true,
                    Applies::Images => !video,
                    Applies::Videos => video,
                    Applies::Filtered => self.filter.is_some(),
                };
                match item.command {
                    _ if !applies => {}
                    _ if !item.sub.is_empty() => groups.push((item.label, item.sub)),
                    // The box itself is not something to run from it.
                    Command::Search | Command::Palette => {}
                    command if group.is_empty() => all.push((self.menu_label(item), item.keys, command)),
                    command => all.push((format!("{group}: {}", self.menu_label(item)), item.keys, command)),
                }
            }
        }
        all.sort_by(|a, b| a.0.cmp(&b.0));
        // Those run lately come first, the latest first.
        let recent = self.recent_labels().to_vec();
        let lately = |label: &str| recent.iter().position(|had| had == label).unwrap_or(usize::MAX);
        all.sort_by_key(|entry| lately(&entry.0));
        let query = Query::parse(typed);
        if query.is_empty() {
            return all;
        }
        let score = |name: &str, loose: bool| query.score(&Facts { name, video: false, marked: false, seen: None, verdict: None, size: None }, loose);
        let pass = |loose: bool| -> Vec<(i32, (String, &'static str, Command))> {
            all.iter().filter_map(|entry| Some((score(&entry.0, loose)?, entry.clone()))).collect()
        };
        let mut scored = pass(false);
        if scored.is_empty() {
            scored = pass(true);
        }
        // Those run lately that match come first, the latest first; then
        // the rest, the closest match first.
        scored.sort_by_key(|(score, entry)| (lately(&entry.0), -score));
        scored.into_iter().map(|(_, entry)| entry).collect()
    }

    /// The names of the commands run from the box lately, the latest
    /// first, read from the state folder the first time.
    fn recent_labels(&mut self) -> &[String] {
        self.palette_recent.get_or_insert_with(|| {
            let path = crate::positions::state_dir().map(|dir| dir.join("palette-recent"));
            let text = path.and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
            text.lines().filter(|line| !line.is_empty()).map(str::to_string).collect()
        })
    }

    /// The commands run lately that still apply here, as the box lists
    /// commands, the latest first.
    fn recent_commands(&mut self) -> Vec<(String, &'static str, Command)> {
        let recent = self.recent_labels().to_vec();
        if recent.is_empty() {
            return Vec::new();
        }
        let all = self.commands_matching("");
        recent.iter().filter_map(|label| all.iter().find(|entry| entry.0 == *label).cloned()).collect()
    }

    /// The files viewed lately, the one on view aside, as the box lists
    /// commands: a few, the latest first.
    fn recent_files(&self) -> Vec<(String, &'static str, Command)> {
        let current = self.list.get(self.pos).copied();
        let visited = self.visited.borrow();
        let others = visited.iter().filter(|id| Some(**id) != current && self.files[**id].exists());
        others.take(5).map(|&id| (self.name_of(id), "", Command::OpenRecent(id))).collect()
    }

    /// Puts a command first among those run lately, and keeps the list.
    fn remember_command(&mut self, label: &str) {
        const KEPT: usize = 12;
        self.recent_labels();
        let recent = self.palette_recent.get_or_insert_with(Vec::new);
        recent.retain(|had| had != label);
        recent.insert(0, label.to_string());
        recent.truncate(KEPT);
        if let Some(dir) = crate::positions::state_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("palette-recent"), recent.join("\n") + "\n");
        }
    }

    /// What follows a `:` in the box names a place: `42` is the 42nd file
    /// of the list, `1:30` or `90s` a time in the video on view, and `50%`
    /// that far through the video, or through the list when no video is
    /// on view. Given back as the box's matches, whether they narrow the
    /// list (never), and its commands.
    fn place_named(&mut self, typed: &str) -> (Vec<usize>, bool, Vec<(String, &'static str, Command)>) {
        let typed = typed.trim();
        let length = self.video_time().map(|(_, length)| length);
        let seek = |seconds: f64| vec![(format!("Go to {} in this video", clock(seconds)), "", Command::SeekTo(seconds))];
        let file = |app: &App, number: usize| vec![app.list[number.clamp(1, app.list.len()) - 1]];
        let number = |text: &str| text.parse::<f64>().ok().filter(|number| *number >= 0.0);
        if let Some(percent) = typed.strip_suffix('%').and_then(number) {
            let part = (percent / 100.0).min(1.0);
            return match length {
                Some(length) => (Vec::new(), false, seek(length * part)),
                None => (file(self, (part * self.list.len() as f64).ceil() as usize), false, Vec::new()),
            };
        }
        // Hours, minutes and seconds, or minutes and seconds, or seconds.
        let parts: Option<Vec<f64>> = typed.trim_end_matches('s').split(':').map(number).collect();
        let seconds = parts.filter(|parts| parts.len() <= 3).map(|parts| parts.iter().fold(0.0, |total, part| total * 60.0 + part));
        let timed = typed.contains(':') || typed.ends_with('s');
        match (seconds, length) {
            (Some(seconds), Some(length)) if timed => (Vec::new(), false, seek(seconds.min(length))),
            (Some(place), _) if !timed => (file(self, place as usize), false, Vec::new()),
            _ => (Vec::new(), false, Vec::new()),
        }
    }

    /// The buttons at the right of the box, in order: carry on with only
    /// the matches, when what is typed narrows the list, and go back to
    /// every file, when a search was kept before.
    fn palette_buttons(&self) -> Vec<(PaletteEnd, String)> {
        let Some(palette) = &self.palette else { return Vec::new() };
        let mut buttons = Vec::new();
        // For a command, once a triage has decided about anything: the
        // lots it can be run on in place of the file on view.
        if palette.field.text.first() == Some(&'!') {
            let (kept, rejected, undecided) = self.tally();
            if kept + rejected > 0 {
                buttons.push((PaletteEnd::OnKept, format!("On {kept} kept")));
                buttons.push((PaletteEnd::OnRejected, format!("On {rejected} rejected")));
                buttons.push((PaletteEnd::OnEvery, format!("On all {}", kept + rejected + undecided)));
            }
            return buttons;
        }
        if palette.narrowed && !palette.matches.is_empty() {
            buttons.push((PaletteEnd::Keep, format!("Show only {}", palette.matches.len())));
        }
        if let Some((_, all)) = &self.filter {
            let back = self.back_label();
            buttons.push((PaletteEnd::All, format!("{}{}", back[..1].to_uppercase(), &back[1..])));
            buttons.push((PaletteEnd::Here, format!("Show all {}, stay here", all.len())));
        }
        buttons
    }

    fn palette_key(&mut self, key: Key<&str>) {
        let rows = self.palette_layout().map_or(1, |layout| layout.rows.max(1));
        let buttons = self.palette_buttons();
        let Some(palette) = &mut self.palette else { return };
        palette.focus = palette.focus.min(buttons.len());
        let last = palette.rows().saturating_sub(1);
        let at_end = palette.field.cursor == palette.field.text.len();
        // Moving among the matches takes the keyboard back from a button.
        let pick = |palette: &mut Palette, to: usize| (palette.selected, palette.focus) = (to.min(last), 0);
        match key {
            Key::Named(NamedKey::Escape) => self.palette = None,
            Key::Named(NamedKey::Enter) => {
                let end = palette.focus.checked_sub(1).map_or(PaletteEnd::Go, |button| buttons[button].0);
                self.finish_palette(end);
            }
            // Tab goes round: from the text to each button in turn and
            // back, Shift+Tab the other way. Enter presses the one it is on.
            Key::Named(NamedKey::Tab) => {
                let stops = buttons.len() + 1;
                let back = self.modifiers.shift_key();
                palette.focus = (palette.focus + if back { stops - 1 } else { 1 }) % stops;
            }
            // Right from the end of the text goes on to the buttons, and
            // left comes back.
            Key::Named(NamedKey::ArrowRight) if at_end || palette.focus > 0 => {
                palette.focus = (palette.focus + 1).min(buttons.len());
            }
            Key::Named(NamedKey::ArrowLeft) if palette.focus > 0 => palette.focus -= 1,
            Key::Named(NamedKey::ArrowDown) => pick(palette, palette.selected + 1),
            Key::Named(NamedKey::ArrowUp) => pick(palette, palette.selected.saturating_sub(1)),
            Key::Named(NamedKey::PageDown) => pick(palette, palette.selected + rows),
            Key::Named(NamedKey::PageUp) => pick(palette, palette.selected.saturating_sub(rows)),
            key => {
                // Typing goes to the text wherever the keyboard was.
                palette.focus = 0;
                if palette.field.key(key, self.modifiers) {
                    return self.palette_changed();
                }
            }
        }
        // The list moves only as far as it must to keep the pick in view.
        if let Some(palette) = &mut self.palette {
            palette.show_selected(rows);
        }
        self.request();
        self.redraw();
    }

    /// Closes the box: going to the match picked out in it, carrying on
    /// with only its matches, or going back to every file.
    fn finish_palette(&mut self, end: PaletteEnd) {
        let Some(palette) = self.palette.take() else { return };
        // A command is left for whoever closed the box to run, with what
        // running one needs.
        if let (PaletteEnd::Go, Some((label, _, command))) = (end, palette.commands.get(palette.selected)) {
            // What was typed is kept for it, the `!` aside.
            if *command == Command::RunTyped {
                self.shell_typed = Some(palette.field.string().chars().skip(1).collect());
            }
            // A command from the menu's, remembered to be offered first
            // next time.
            if matches!(palette.field.text.first(), None | Some('>')) && !matches!(command, Command::OpenRecent(_)) {
                self.remember_command(label);
            }
            self.run_next = Some(*command);
            return self.redraw();
        }
        // A command for a lot of a triage: the one typed, or the one picked
        // out from those run before, on each file of the lot in turn.
        let lot = match end {
            PaletteEnd::OnKept => Some(self.lot(Some(Verdict::Kept))),
            PaletteEnd::OnRejected => Some(self.lot(Some(Verdict::Rejected))),
            PaletteEnd::OnEvery => Some(self.everything()),
            _ => None,
        };
        if let Some(ids) = lot {
            let template = match palette.commands.get(palette.selected) {
                Some((_, _, Command::RunShell(at))) => self.shell_history.get_or_insert_with(history::load).get(*at).cloned().unwrap_or_default(),
                _ => palette.field.string().chars().skip(1).collect(),
            };
            match ids.is_empty() {
                true => self.toast("There are none of those to run it on"),
                false => self.run_shell_on(&template, ids, true),
            }
            return self.redraw();
        }
        let picked = palette.matches.get(palette.selected).copied();
        match end {
            PaletteEnd::Keep => {
                let typed = palette.field.string().trim().to_string();
                self.keep_matches(typed, palette.matches, picked);
            }
            PaletteEnd::All => self.clear_filter(false),
            PaletteEnd::Here => self.clear_filter(true),
            PaletteEnd::OnKept | PaletteEnd::OnRejected | PaletteEnd::OnEvery => {}
            PaletteEnd::Go => {
                let Some(id) = picked else { return self.redraw() };
                // One that a kept search has hidden brings every file back.
                if !self.list.contains(&id) {
                    self.clear_filter(true);
                }
                let pos = self.list.iter().position(|other| *other == id).unwrap_or(self.pos);
                // From the grid, the file is opened as Enter on its cell would.
                if let Some(grid) = &mut self.grid {
                    grid.cursor = pos;
                    return self.open_from_grid();
                }
                self.go_to(pos, pos > self.pos);
            }
        }
        self.redraw();
    }

    /// The go-to box sits in the upper middle of the window, with as many
    /// matches under it as fit, the one picked out among them.
    fn palette_layout(&self) -> Option<PaletteLayout> {
        let palette = self.palette.as_ref()?;
        let (width, height) = self.window_size();
        let px = ui_px(height);
        let line = Text::line_height(px);
        let (head, row_height) = (line + 2 * px as i32, line * 2);
        // Above it there is room for a row of frames from the video picked
        // out, as wide as the window allows.
        let frame_width = (width as i32 * 9 / 10 / story::SEGMENTS as i32).min(px as i32 * 14);
        let frame = (frame_width, frame_width * 9 / 16);
        // The whole of it, with the frames above and as many matches as fit
        // below, sits in the middle; what is typed then stays where it is
        // however many matches there are.
        let above = frame.1 + px as i32;
        let fit = ((height as i32 * 3 / 4 - above - head) / row_height).clamp(1, 10) as usize;
        let top = (height as i32 - above - head - fit as i32 * row_height) / 2 + above;
        let rows = fit.min(palette.rows());
        let first = palette.first.min(palette.rows().saturating_sub(rows));
        let box_width = (px as i32 * 48).min(width as i32);
        Some(PaletteLayout { left: (width as i32 - box_width) / 2, top, width: box_width, head, row_height, first, rows, px, frame })
    }

    /// The files whose matches the go-to box is showing.
    fn palette_shown(&self) -> Vec<usize> {
        let (Some(palette), Some(layout)) = (&self.palette, self.palette_layout()) else { return Vec::new() };
        palette.matches.iter().skip(layout.first).take(layout.rows).copied().collect()
    }

    /// The row of the go-to box at a window position, counted through all
    /// of its matches.
    fn palette_row_at(&self, x: f64, y: f64) -> Option<usize> {
        let layout = self.palette_layout()?;
        let (x, y) = (x as i32 - layout.left, y as i32 - layout.top - layout.head);
        let row = y.div_euclid(layout.row_height);
        ((0..layout.width).contains(&x) && (0..layout.rows as i32).contains(&row)).then(|| layout.first + row as usize)
    }

    fn draw_palette(&self, canvas: &mut Canvas, text: &mut Text, layout: &PaletteLayout) -> Vec<(PaletteEnd, [i32; 4])> {
        let Some(palette) = &self.palette else { return Vec::new() };
        let (px, pad) = (layout.px, layout.px as i32);
        let line = Text::line_height(px);
        let height = layout.head + layout.rows as i32 * layout.row_height + if layout.rows > 0 { pad / 2 } else { 0 };
        panel(canvas, layout.left, layout.top, layout.width, height, pad * 2 / 3);

        // Above the box, for a video: frames from along it, side by side.
        if let Some(frames) = self.story_subject().and_then(|id| self.story_frames.get(&id)) {
            let gap = (pad / 4).max(2);
            let (cell_w, cell_h) = layout.frame;
            let strip_width = (cell_w + gap) * frames.len() as i32 + gap;
            let (strip_left, strip_top) = ((canvas.width - strip_width) / 2, layout.top - cell_h - 2 * gap - pad / 2);
            panel(canvas, strip_left, strip_top, strip_width, cell_h + 2 * gap, pad / 2);
            for (index, frame) in frames.iter().enumerate() {
                let (cell_left, cell_top) = (strip_left + gap + index as i32 * (cell_w + gap), strip_top + gap);
                // One frame stands for each stretch: the middle one.
                match frame.as_ref().and_then(|segment| segment.get(segment.len() / 2)) {
                    Some(frame) => {
                        let (w, h) = fit_size((frame.width, frame.height), (cell_w as u32, cell_h as u32), true);
                        let (w, h) = (w as i32, h as i32);
                        canvas.image(frame, cell_left + (cell_w - w) / 2, cell_top + (cell_h - h) / 2, w, h);
                    }
                    None => canvas.fill(cell_left, cell_top, cell_w, cell_h, PLACEHOLDER_COLOR),
                }
                canvas.round_off(cell_left, cell_top, cell_w, cell_h, pad / 3, MENU_COLOR);
            }
        }

        let label = match palette.field.text.first() {
            None if !palette.commands.is_empty() => "Recent:   ",
            _ if !palette.commands.is_empty() && palette.field.text.first() == Some(&':') => "Go to:   ",
            Some('>') if palette.matches.is_empty() => "Run:   ",
            Some(':') => "Go to:   ",
            Some('@') => "Workspace:   ",
            Some('!') => "Run command:   ",
            _ => "Find:   ",
        };
        let typed = palette.field.string();
        let before_cursor: String = palette.field.text[..palette.field.cursor].iter().collect();
        let note = match palette.rows() {
            0 => "no match".to_string(),
            count => format!("{} of {count}", palette.selected + 1),
        };
        let (x, y) = (layout.left + pad, layout.top + pad);
        let typed_left = x + text.width(label, px);
        text.draw(canvas, x, y, px, DIM_TEXT_COLOR, label);
        draw_selection(canvas, text, &palette.field, typed_left, y, px);
        text.draw(canvas, typed_left, y, px, TEXT_COLOR, &typed);
        if palette.focus == 0 {
            canvas.fill(typed_left + text.width(&before_cursor, px), y, (pad / 10).max(2), line, TEXT_COLOR);
        }
        // At the right, from the edge inwards: the buttons, then the count.
        let mut right = layout.left + layout.width - pad;
        let mut drawn = Vec::new();
        let buttons = self.palette_buttons();
        for (at, (end, label)) in buttons.iter().enumerate().rev() {
            let focused = palette.focus == at + 1;
            let (button_w, button_h) = (text.width(label, px) + pad, line + pad / 2);
            let (button_x, button_y) = (right - button_w, y - pad / 4);
            let fill = if focused { accent() } else { PLACEHOLDER_COLOR };
            canvas.round_fill(button_x, button_y, button_w, button_h, pad / 3, fill, 256);
            canvas.round_outline(button_x, button_y, button_w, button_h, pad / 3, 1, if focused { TEXT_COLOR } else { MENU_EDGE_COLOR });
            text.draw(canvas, button_x + pad / 2, y, px, if focused { on_accent() } else { TEXT_COLOR }, label);
            drawn.push((*end, [button_x, button_y, button_w, button_h]));
            right = button_x - pad / 2;
        }
        let note_left = right - pad / 2 - text.width(&note, px);
        text.draw(canvas, note_left, y, px, DIM_TEXT_COLOR, &note);

        // Each match: a small picture, the name, and a video's length.
        let thumb = ((layout.row_height - pad / 4) * 16 / 9, layout.row_height - pad / 4);
        for row in 0..layout.rows {
            let at = layout.first + row;
            let top = layout.top + layout.head + row as i32 * layout.row_height;
            if at == palette.selected {
                canvas.round_fill(layout.left + pad / 3, top, layout.width - 2 * (pad / 3), layout.row_height, pad / 2, accent(), 256);
            }
            // Words on the row picked out are in colours that read on it.
            let (main, dim) = row_text(at == palette.selected);
            // A command: what it is called, a mark if it is on, and its key.
            if let Some((name, keys, command)) = palette.commands.get(at) {
                let text_top = top + (layout.row_height - line) / 2;
                if self.is_checked(*command) {
                    let mark = pad / 2;
                    canvas.round_fill(layout.left + pad, top + (layout.row_height - mark) / 2, mark, mark, mark / 4, main, 256);
                }
                text.draw(canvas, layout.left + 2 * pad, text_top, px, main, name);
                let keys_left = layout.left + layout.width - pad - text.width(keys, px);
                text.draw(canvas, keys_left, text_top, px, dim, keys);
                continue;
            }
            let id = palette.matches[at];
            let (cell_left, cell_top) = (layout.left + pad, top + pad / 8);
            match self.loader.thumb(id) {
                Some(frame) => {
                    let (w, h) = fit_size((frame.width, frame.height), (thumb.0 as u32, thumb.1 as u32), true);
                    canvas.image(&frame, cell_left + (thumb.0 - w as i32) / 2, cell_top + (thumb.1 - h as i32) / 2, w as i32, h as i32);
                }
                None => canvas.fill(cell_left, cell_top, thumb.0, thumb.1, PLACEHOLDER_COLOR),
            }
            let text_top = top + (layout.row_height - line) / 2;
            let length = self.durations.get(&id).copied().flatten().map(clock).unwrap_or_default();
            let length_width = text.width(&length, px);
            let right = layout.left + layout.width - pad;
            text.draw(canvas, right - length_width, text_top, px, dim, &length);
            let name_left = cell_left + thumb.0 + pad;
            let name = clipped(text, &self.name_of(id), right - length_width - pad - name_left, px);
            let color = if self.marks.contains(&id) && at != palette.selected { MARK_COLOR } else { main };
            text.draw(canvas, name_left, text_top, px, color, &name);
        }
        drawn
    }

    /// How well file `id` matches a search, or `None` if it does not. With
    /// `loose`, letters scattered through the name count too.
    fn score(&mut self, query: &Query, id: usize, loose: bool) -> Option<i32> {
        let path = &self.files[id];
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
        let size = match query.wants_size() {
            true => *self.sizes.entry(id).or_insert_with(|| std::fs::metadata(path).ok().map(|meta| meta.len())),
            false => None,
        };
        let seen = (query.wants_seen() && files::is_video(path)).then(|| self.watched(id));
        query.score(&Facts { name: &name, video: files::is_video(path), marked: self.marks.contains(&id), seen, verdict: self.verdict(id), size }, loose)
    }

    fn sort_by(&mut self, sort: Sort) {
        (self.sort, self.shuffled) = (Some(sort), false);
        let reversed = self.reversed;
        self.reorder(|ids, files| {
            files::sort_ids(ids, files, sort);
            if reversed {
                ids.reverse();
            }
        });
        let label = commands::SORT.iter().flatten().find(|item| item.command == Command::SortBy(sort)).map_or("", |item| item.label);
        self.toast(format!("Sorted: {}{}", label.to_lowercase(), if reversed { ", reversed" } else { "" }));
    }

    fn grid_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let layout = self.grid_layout(self.window_size());
        match key {
            Key::Named(NamedKey::Escape) => return self.toggle_grid(),
            Key::Named(NamedKey::Tab) => return self.execute(Command::GridMenu, event_loop),
            // With a video playing on, the space bar pauses and plays it,
            // as it does with the video on view.
            Key::Named(NamedKey::Space) if self.pip && self.video.is_some() => return self.video_action(VideoAction::Pause),
            Key::Named(NamedKey::ArrowUp) => return self.step_cursor(0, -1),
            Key::Named(NamedKey::ArrowDown) => return self.step_cursor(0, 1),
            // Ctrl with + or - makes the pictures larger or smaller.
            Key::Character("+" | "=") if self.modifiers.control_key() => return self.zoom_grid(true),
            Key::Character("-" | "_") if self.modifiers.control_key() => return self.zoom_grid(false),
            // Here `i` is for the names under the pictures.
            Key::Character("i") => {
                self.grid_titles = !self.grid_titles;
                return self.redraw();
            }
            _ => {}
        }
        // Ctrl+K is the one key here that a modifier changes the meaning of.
        if commands::for_key(key.clone(), self.modifiers, false) == Some(Command::Palette) {
            return self.execute(Command::Palette, event_loop);
        }
        match commands::for_key(key, ModifiersState::empty(), false) {
            Some(Command::Next) => self.step_cursor(1, 0),
            Some(Command::Previous) => self.step_cursor(-1, 0),
            Some(Command::Shuffle) => self.execute(Command::Shuffle, event_loop),
            Some(Command::JumpForward) => self.move_cursor(layout.per_page() as i64),
            Some(Command::JumpBack) => self.move_cursor(-(layout.per_page() as i64)),
            Some(Command::First) => self.move_cursor(i64::MIN),
            Some(Command::Last) => self.move_cursor(i64::MAX),
            Some(Command::Action(0)) => self.open_from_grid(),
            Some(
                command @ (Command::ToggleGrid
                | Command::ToggleHelp
                | Command::ToggleFullscreen
                | Command::Mark
                | Command::CopyName
                | Command::CopyPath
                | Command::CopyTo
                | Command::MoveTo
                | Command::Undo
                | Command::Rename
                | Command::Delete
                | Command::Trash
                | Command::MarkMenu
                | Command::SaveFilelist
                | Command::Search
                | Command::ClearFilter
                | Command::ShowAllHere
                | Command::SortMenu
                | Command::Quit),
            ) => self.execute(command, event_loop),
            _ => {}
        }
    }

    // ---- Input ---------------------------------------------------------

    fn key_pressed(&mut self, key: Key<&str>, repeat: bool, numpad: bool, event_loop: &ActiveEventLoop) {
        if self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() {
            self.row_target = None;
        }
        if self.help {
            // What is typed looks for a shortcut; Escape empties that, and
            // then, or Enter, or `?` before anything is typed, closes it.
            let typing = !self.modifiers.control_key() && !self.modifiers.alt_key();
            match key {
                Key::Named(NamedKey::Escape) if !self.help_search.is_empty() => self.help_search.clear(),
                Key::Named(NamedKey::Backspace) => drop(self.help_search.pop()),
                Key::Named(NamedKey::Space) if !self.help_search.is_empty() => self.help_search.push(' '),
                Key::Character("?") if self.help_search.is_empty() => self.help = false,
                Key::Character(typed) if typing => self.help_search.push_str(typed),
                Key::Named(NamedKey::Escape | NamedKey::Enter) => self.help = false,
                _ => {}
            }
            return self.redraw();
        }
        if self.confirm.is_some() {
            return self.confirm_key(key, event_loop);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key, event_loop);
        }
        if self.convert_panel.is_some() {
            return self.panel_key(key, event_loop);
        }
        if self.compare.is_some() {
            return self.compare_key(key);
        }
        if self.overview.is_some() && self.palette.is_none() && self.overview_key(key.clone()) {
            return;
        }
        if self.palette.is_some() {
            self.palette_key(key);
            // A command picked in the box is run once the box has closed.
            if let Some(command) = self.run_next.take() {
                self.execute(command, event_loop);
            }
            return;
        }
        // Going to another workspace works from anywhere but a text box.
        let to_space = commands::for_key(key.clone(), self.modifiers, numpad);
        if let Some(command @ (Command::WorkspaceKey(_) | Command::NextWorkspace | Command::PrevWorkspace | Command::HomeWorkspace)) = to_space {
            self.menu = None;
            return self.execute(command, event_loop);
        }
        if self.menu.is_some() {
            return self.menu_key(key, event_loop);
        }
        // What is typed goes into the file list's search while that has
        // the keyboard, and with the list up `/` is what gives it to it.
        if self.list_search.is_some() && self.grid.is_none() {
            return self.list_search_key(key);
        }
        if matches!(key, Key::Character("/")) && self.modifiers.is_empty() && self.list_panel().is_some() {
            return self.focus_list_search();
        }
        if self.grid.is_some() {
            return self.grid_key(key, event_loop);
        }
        // Escape leaves a kept search before it leaves the program.
        if self.filter.is_some() && key == Key::Named(NamedKey::Escape) {
            return self.clear_filter(false);
        }
        // While triaging, the space bar keeps the file on view and `x`
        // rejects it; everything else is as it always is.
        if self.triage_on && self.modifiers.is_empty() {
            let verdict = match key {
                Key::Named(NamedKey::Space) => Some(Verdict::Kept),
                Key::Character("x") => Some(Verdict::Rejected),
                _ => None,
            };
            if let Some(verdict) = verdict {
                if !repeat {
                    self.decide(verdict);
                }
                return;
            }
        }
        // `v` held is the loupe, there until the key is let go.
        if matches!(key, Key::Character("v")) && self.modifiers.is_empty() {
            if !repeat {
                self.loupe = Some(1.0);
                self.request();
                self.redraw();
            }
            return;
        }
        if self.video.is_some() && self.video_key(key.clone(), repeat) {
            return;
        }
        if let Some(command) = commands::for_key(key, self.modifiers, numpad) {
            if !repeat || command.repeats() {
                self.execute(command, event_loop);
            }
        }
    }

    fn mouse_moved(&mut self, x: f64, y: f64) {
        let (dx, dy) = (x - self.mouse.0, y - self.mouse.1);
        self.mouse = (x, y);
        // The button held down and then moved is a drag, not a hold; and
        // the loupe goes where the pointer does.
        if self.hold.as_ref().is_some_and(|hold| !hold.by_key && !hold.fast && (x - hold.at.0).hypot(y - hold.at.1) > 6.0) {
            self.hold = None;
        }
        if self.loupe.is_some() {
            self.redraw();
        }
        // Over the Convert panel the pointer moves the divider and lights
        // up what it is on; nothing behind the panel sees it.
        if self.convert_panel.is_some() || self.compare.is_some() {
            return self.redraw();
        }
        // The scene under the pointer is picked out, and plays.
        let over = self.scene_at(x, y);
        if over != self.scene_over {
            self.scene_over = over;
            self.redraw();
        }

        // In the delete confirmation the button under the pointer is the
        // one picked out, Delete turning red as it is.
        if self.confirm.is_some() {
            let over = self.confirm_button_at(x, y);
            if let (Some(confirm), Some(yes)) = (&mut self.confirm, over) {
                if confirm.yes != yes {
                    confirm.yes = yes;
                    self.redraw();
                }
            }
            return;
        }

        // In the find box the match under the pointer is picked out, as
        // Up and Down would pick it. Only when the pointer has really
        // moved: a list scrolling under a still pointer picks nothing.
        // In the overview the card under the pointer is the one picked
        // out, and so the one running through its pictures.
        if self.overview.is_some() && self.palette.is_none() {
            let over = self.overview_card_at(x, y).filter(|_| (dx, dy) != (0.0, 0.0));
            if let (Some(overview), Some(which)) = (&mut self.overview, over) {
                if overview.cursor != which {
                    (overview.cursor, overview.tick) = (which, 0);
                    self.overview_due = None;
                    self.request();
                    self.redraw();
                }
            }
            return;
        }
        if self.palette.is_some() {
            let row = self.palette_row_at(x, y).filter(|_| (dx, dy) != (0.0, 0.0));
            if let (Some(palette), Some(row)) = (&mut self.palette, row) {
                if palette.selected != row {
                    (palette.selected, palette.focus) = (row, 0);
                    self.request();
                    self.redraw();
                }
            }
            return;
        }

        if let Some(menu) = &mut self.menu {
            // Moving the pointer elsewhere leaves a keyboard selection alone.
            if let Some(sub) = &mut menu.sub {
                let hovered = sub.row_at(x, y);
                if hovered.is_some() {
                    if hovered != sub.selected {
                        sub.selected = hovered;
                        self.redraw();
                    }
                    return;
                }
            }
            let hovered = menu.row_at(x, y);
            if hovered.is_some() && hovered != menu.selected {
                menu.selected = hovered;
                // Resting on an item that has a menu of its own opens it.
                menu.sub = None;
                self.open_submenu(false);
                self.redraw();
            }
            return;
        }
        if self.grid.is_some() {
            // Dragging along the page bar goes through the pages, and
            // picks nothing out on the way.
            if matches!(self.drag, Some(Drag::Pages)) {
                return self.drag_pages(x);
            }
            self.point_cursor();
            // The buttons light up under the pointer.
            let inside = |b: &Button| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32));
            let arrows = self.grid_arrows();
            let close = self.grid_close();
            let over = self.grid_buttons.iter().chain(arrows.iter().map(|(place, _)| place)).chain(close.iter()).position(inside);
            if over != self.grid_over {
                self.grid_over = over;
                self.redraw();
            }
            return;
        }
        let hover = self.list_row_at(x, y);
        // The buttons at the head of the file list light up as well.
        let on_button = self.list_buttons().iter().position(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32)));
        if hover != self.list_hover || on_button != self.list_button_over {
            (self.list_hover, self.list_button_over) = (hover, on_button);
            self.redraw();
        }
        let over = self.strip_cell_at(x, y);
        // The buttons of a triage light up under the pointer.
        let on_triage = self.triage_button_at(x, y);
        if on_triage != self.triage_over {
            self.triage_over = on_triage;
            self.redraw();
        }
        // The cross above the strip lights up under the pointer too.
        let on_close = self.strip_close().is_some_and(|place| (place.x..place.x + place.width).contains(&(x as i32)) && (place.y..place.y + place.height).contains(&(y as i32)));
        if over != self.strip_over || on_close != self.strip_close_over {
            (self.strip_over, self.strip_close_over) = (over, on_close);
            self.redraw();
        }
        match &mut self.drag {
            Some(Drag::Pan) => self.pan(dx, dy),
            // Only in the grid, which is dealt with above.
            Some(Drag::Pages) => {}
            Some(Drag::Zoom { start_x, start_scale, anchor, moved }) => {
                *moved = true;
                let (scale, anchor) = (*start_scale * ((x - *start_x) / 300.0).exp(), *anchor);
                if let Some(native) = self.native() {
                    let (left, stage) = self.stage();
                    self.view.zoom_to(scale, (anchor.0 - f64::from(left), anchor.1), native, stage);
                    self.view_changed();
                }
            }
            Some(Drag::Scrub) => {
                let bar = self.seek_bar();
                let fraction = ((x - f64::from(bar.x)) / f64::from(bar.width)).clamp(0.0, 1.0);
                self.hover = Some(fraction);
                self.request_preview(fraction);
                self.seek_to(fraction, false);
            }
            Some(Drag::Section(edge)) => {
                let edge = *edge;
                let bar = self.seek_bar();
                self.drag_edge(edge, (x - f64::from(bar.x)) / f64::from(bar.width));
            }
            // Over a video the bottom of the window belongs to the seek bar,
            // so the filmstrip does not also open there; `t` still pins it.
            None if self.video.is_some() => {
                self.set_strip_hover(false);
                self.video_pointer(x, y);
            }
            None => {
                let height = self.window_size().1;
                // A thin strip at the bottom edge opens the filmstrip; once
                // open, it stays while the pointer is anywhere over it.
                // The button that closes it sits just above it.
                let reach = ui_px(height) as i32 * 2;
                let zone = if self.band_shown() { Strip::for_height(height).height + reach } else { (height as i32 / 50).max(6) };
                self.set_strip_hover(y >= f64::from(height as i32 - zone));
            }
        }
    }

    fn set_strip_hover(&mut self, hover: bool) {
        if hover == self.strip_hover {
            return;
        }
        self.strip_hover = hover;
        if hover && !self.strip_used {
            self.strip_used = true;
            self.request();
        }
        self.redraw();
    }

    fn mouse_button(&mut self, button: MouseButton, pressed: bool, event_loop: &ActiveEventLoop) {
        let (x, y) = self.mouse;
        // The row a menu was brought up on is done with once the menu and
        // whatever it asked are.
        if pressed && self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() {
            self.row_target = None;
            self.entry_target = None;
        }
        // A right click on a kept section in the Sections tab: what can be
        // done with it.
        if button == MouseButton::Right && pressed && self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() {
            if let Some(index) = self.entry_at(x, y).filter(|index| self.entries().get(*index).is_some_and(|entry| entry.kept)) {
                self.entry_target = Some(index);
                self.open_menu(commands::SECTION_MENU, None);
                return self.redraw();
            }
        }
        if self.help {
            self.help = !pressed;
            return self.redraw();
        }
        if button == MouseButton::Left && pressed && self.confirm.is_none() && self.toast_click(x, y, event_loop) {
            return self.redraw();
        }
        if self.confirm.is_some() {
            // A click on one of its buttons presses it; elsewhere, nothing.
            if button == MouseButton::Left && pressed {
                match self.confirm_button_at(x, y) {
                    Some(true) => {
                        if let Some(confirm) = self.confirm.take() {
                            self.delete(&confirm.ids, event_loop);
                        }
                    }
                    Some(false) => self.confirm = None,
                    None => {}
                }
                self.redraw();
            }
            return;
        }
        if self.convert_panel.is_some() {
            if button == MouseButton::Left && pressed {
                self.panel_click(x, y);
            }
            return;
        }
        if self.compare.is_some() {
            if button == MouseButton::Left && pressed {
                self.compare_click(x, y);
            }
            return;
        }
        if self.overview.is_some() && self.palette.is_none() {
            // A click on a card goes to that workspace.
            if button == MouseButton::Left && pressed {
                if let Some(which) = self.overview_card_at(x, y) {
                    self.leave_overview(Some(which));
                }
            }
            return;
        }
        if self.palette.is_some() {
            // A click on a match goes to it; one anywhere else closes the box.
            if button == MouseButton::Left && pressed {
                let on = |[left, top, width, height]: [i32; 4]| {
                    (left..left + width).contains(&(x as i32)) && (top..top + height).contains(&(y as i32))
                };
                let drawn = self.palette.as_ref().map(|palette| palette.drawn.clone()).unwrap_or_default();
                let button = drawn.into_iter().find(|(_, place)| on(*place)).map(|(end, _)| end);
                let row = self.palette_row_at(x, y);
                match (&mut self.palette, button, row) {
                    (Some(_), Some(end), _) => self.finish_palette(end),
                    (Some(palette), None, Some(row)) => {
                        palette.selected = row;
                        self.finish_palette(PaletteEnd::Go);
                        if let Some(command) = self.run_next.take() {
                            self.execute(command, event_loop);
                        }
                    }
                    _ => self.palette = None,
                }
                self.redraw();
            }
            return;
        }
        // A click on one of the buttons of a triage presses it.
        let on_triage = self.triage_button_at(x, y).map(|at| self.triage_buttons.borrow()[at].0);
        if let Some(command) = on_triage.filter(|_| self.triage_on && self.grid.is_none() && self.menu.is_none() && self.prompt.is_none()) {
            if button == MouseButton::Left && pressed {
                match command {
                    Command::Reload => self.triage_refresh(event_loop),
                    command => self.execute(command, event_loop),
                }
            }
            return;
        }
        if self.grid.is_some() && self.menu.is_none() {
            let hit = |b: &&Button| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32));
            match (button, pressed) {
                (MouseButton::Left, true) => {
                    // The buttons under the grid, then those at its sides
                    // for the pages either way, then the pictures.
                    let arrow = self.grid_arrows().into_iter().find(|(place, _)| hit(&place)).map(|(_, forward)| forward);
                    if self.grid_close().as_ref().is_some_and(|place| hit(&place)) {
                        return self.toggle_grid();
                    }
                    if let Some(command) = self.grid_buttons.iter().find(hit).map(|button| button.command) {
                        self.execute(command, event_loop);
                    } else if let Some(forward) = arrow {
                        let per_page = self.grid_layout(self.window_size()).per_page() as i64;
                        self.move_cursor(if forward { per_page } else { -per_page });
                    } else if self.grid_bar().is_some_and(|bar| bar.holds((x, y))) {
                        // A click on the bar goes to the page at that spot;
                        // keeping the button down drags through them.
                        self.drag = Some(Drag::Pages);
                        self.drag_pages(x);
                    } else if self.point_cursor() {
                        self.open_from_grid();
                    }
                }
                (MouseButton::Left, false) => self.drag = None,
                // A right click on a picture brings up what can be done
                // with it.
                (MouseButton::Right, true) => match self.point_cursor() {
                    true => self.open_menu(commands::GRID_MENU, None),
                    // Off the pictures, it is the grid as a whole.
                    false => self.open_menu(commands::GRID_AROUND_MENU, None),
                },
                _ => {}
            }
            return;
        }
        match (button, pressed) {
            (MouseButton::Left, true) if self.menu.is_some() => {
                if let Some(menu) = &mut self.menu {
                    // A click outside both menus selects nothing: it closes them.
                    let inner = menu.sub.as_mut().map(|sub| (sub.row_at(x, y), sub));
                    match inner {
                        Some((Some(row), sub)) => sub.selected = Some(row),
                        _ => {
                            menu.sub = None;
                            menu.selected = menu.row_at(x, y);
                        }
                    }
                }
                self.activate_menu(event_loop);
            }
            (MouseButton::Left, true) if self.hover.is_some() => {
                let bar = self.seek_bar();
                if bar.on_control(0, x, y) {
                    self.step(false);
                } else if bar.on_control(2, x, y) {
                    self.step(true);
                } else if let Some(command) = (0..).zip(self.bar_buttons()).find(|(which, _)| bar.on_button(*which, x, y)).map(|(_, command)| command) {
                    self.execute(command, event_loop);
                } else if bar.on_symbol(x, y) {
                    self.video_action(VideoAction::Pause);
                } else if let Some(edge) = self.section_edge_at(&bar, x) {
                    // An end of the section is taken hold of, to move it.
                    self.drag = Some(Drag::Section(edge));
                } else if bar.on_track(x) {
                    // Click to jump there; keep the button down to scrub.
                    self.seek_to(self.hover.unwrap_or(0.0), self.precise);
                    self.drag = Some(Drag::Scrub);
                }
            }
            // The buttons on the debug overlay copy its figures.
            // The choices of rendition in the info panel.
            (MouseButton::Left, true) if self.info_buttons.iter().any(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32))) => {
                let hit = self.info_buttons.iter().find(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32)));
                if let Some(command) = hit.map(|button| button.command) {
                    self.execute(command, event_loop);
                }
            }
            (MouseButton::Left, true) if self.debug_buttons.iter().any(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32))) => {
                let hit = self.debug_buttons.iter().find(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32)));
                if let Some(command) = hit.map(|button| button.command) {
                    self.execute(command, event_loop);
                }
            }
            (MouseButton::Left, true) => {
                let height = self.window_size().1;
                let strip = Strip::for_height(height);
                let panel = self.list_panel().filter(|panel| panel.holds(x, y));
                // The file list's own parts are measured from where it stands.
                let within = x as i32 - self.list_panel().map_or(0, |panel| panel.left);
                let inside = |b: &Button| (b.x..b.x + b.width).contains(&within) && (b.y..b.y + b.height).contains(&(y as i32));
                // A click in the search box of the file list puts the
                // keyboard there, and one anywhere else takes it away.
                let in_field = self.list_field().is_some_and(|[fx, fy, w, h]| (fx..fx + w).contains(&within) && (fy..fy + h).contains(&(y as i32)));
                if !in_field {
                    self.list_search = None;
                }
                if in_field {
                    self.focus_list_search();
                } else if let Some(command) = self.list_buttons().iter().find(|button| inside(button)).map(|button| button.command) {
                    // One of the buttons at the head of the file list.
                    self.execute(command, event_loop);
                } else if let Some(pos) = self.list_star_at(x, y) {
                    // The star at the end of a row marks that file, or
                    // takes its mark away.
                    let id = self.list[pos];
                    if !self.marks.remove(&id) {
                        self.marks.insert(id);
                    }
                    self.redraw();
                } else if panel.is_some() {
                    // A click on a name goes to that file.
                    if let Some(pos) = self.list_row_at(x, y) {
                        // The list and the strip stay as they are: the one
                        // clicked is not brought to the middle.
                        let first = self.list_panel().map(|panel| panel.first);
                        let centre = self.strip_centre();
                        self.go_to(pos, pos > self.pos);
                        self.list_scroll = first.map(|first| (self.pos, self.list.len(), first));
                        self.strip_scroll = centre - self.pos as i64;
                    }
                } else if self.strip_close().is_some_and(|place| (place.x..place.x + place.width).contains(&(x as i32)) && (place.y..place.y + place.height).contains(&(y as i32))) {
                    (self.strip_pinned, self.strip_hover, self.strip_over) = (false, false, None);
                    self.redraw();
                } else if self.scenes_shown() && y >= f64::from(strip.top) && self.strip_click(x, y, ui_px(height)) {
                    // A tab, or a section marked from the Sections tab.
                } else if self.scenes_shown() && y >= f64::from(strip.top) {
                    // A click on a scene plays from where it starts.
                    let start = self.scene_at(x, y).and_then(|index| Some(self.scenes_now()?.1[index]));
                    if let (Some(start), Some((_, length))) = (start, self.video_time()) {
                        self.seek_to(start / length, true);
                        // The seek bar comes up to show how far in that is.
                        self.time_until = Some(Instant::now() + TOAST);
                    }
                } else if self.strip_visible() && y >= f64::from(strip.top) {
                    if let Some(pos) = self.strip_cell_at(x, y) {
                        // The list and the strip stay as they are: the one
                        // clicked is not brought to the middle.
                        let first = self.list_panel().map(|panel| panel.first);
                        let centre = self.strip_centre();
                        self.go_to(pos, pos > self.pos);
                        self.list_scroll = first.map(|first| (self.pos, self.list.len(), first));
                        self.strip_scroll = centre - self.pos as i64;
                    }
                } else {
                    // A second click in the same spot does what `f` does.
                    let near = |(at, (cx, cy)): (Instant, (f64, f64))| {
                        at.elapsed() < DOUBLE_CLICK && (x - cx).abs() < 8.0 && (y - cy).abs() < 8.0
                    };
                    if self.last_click.take().is_some_and(near) {
                        self.toggle_fullscreen();
                    } else {
                        self.last_click = Some((Instant::now(), (x, y)));
                        self.drag = Some(Drag::Pan);
                        // Kept down on a video, it hurries it along.
                        self.begin_hold(false);
                    }
                }
            }
            (MouseButton::Middle, true) => {
                if let Some(native) = self.native() {
                    let start_scale = self.view.scale(native, self.stage().1);
                    self.drag = Some(Drag::Zoom { start_x: x, start_scale, anchor: (x, y), moved: false });
                }
            }
            (MouseButton::Left | MouseButton::Middle, false) => {
                self.end_hold(false);
                match self.drag.take() {
                    Some(Drag::Zoom { moved: false, .. }) => self.execute(Command::ZoomActual, event_loop),
                    Some(Drag::Section(edge)) => self.drop_edge(edge),
                    _ => {}
                }
            }
            // On a row of the file list, the menu is of that file.
            (MouseButton::Right, true) => match self.list_row_at(x, y) {
                Some(at) => {
                    self.row_target = Some(at);
                    self.open_menu(commands::LIST_MENU, None);
                    self.redraw();
                }
                None => self.execute(Command::ToggleMenu, event_loop),
            },
            _ => {}
        }
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            MouseScrollDelta::PixelDelta(position) => position.y / 50.0,
        };
        // With the loupe up, the wheel makes it stronger or weaker.
        if let Some(strength) = &mut self.loupe {
            *strength = (*strength * 1.25f64.powf(lines)).clamp(0.5, 8.0);
            return self.redraw();
        }
        if lines == 0.0 || self.menu.is_some() {
            return;
        }
        if self.convert_panel.is_some() {
            return self.panel_wheel(lines);
        }
        if self.compare.is_some() {
            return self.compare_wheel(lines);
        }
        // In the find box it scrolls the list of matches, and what then
        // lies under the pointer is the one picked out. It does not step
        // through them one at a time: the pointer and the arrows do that.
        if self.overview.is_some() && self.palette.is_none() {
            return;
        }
        if self.palette.is_some() {
            let rows = self.palette_layout().map_or(1, |layout| layout.rows.max(1));
            if let Some(palette) = &mut self.palette {
                let furthest = palette.rows().saturating_sub(rows) as i64;
                palette.first = (palette.first as i64 - (lines * 3.0).round() as i64).clamp(0, furthest) as usize;
                // A pick that has gone out of view is brought to its edge.
                palette.selected = palette.selected.clamp(palette.first, palette.first + rows - 1);
            }
            let under = self.palette_row_at(self.mouse.0, self.mouse.1);
            if let (Some(palette), Some(row)) = (&mut self.palette, under) {
                palette.selected = row;
            }
            self.request();
            return self.redraw();
        }
        // Over the Sections tab it scrolls a row too long for the window.
        if self.over_entries(self.mouse.1) {
            return self.scroll_entries(-(lines.round() as i64).clamp(-3, 3));
        }
        // Over the filmstrip it moves the strip along, to look at what is
        // further off without leaving the file on view. A click goes there.
        if self.strip_cell_at(self.mouse.0, self.mouse.1).is_some() {
            let last = self.list.len() as i64 - 1;
            let centre = (self.strip_centre() - (lines * 2.0).round() as i64).clamp(0, last);
            self.strip_scroll = centre - self.pos as i64;
            self.strip_over = self.strip_cell_at(self.mouse.0, self.mouse.1);
            self.request();
            return self.redraw();
        }
        // Over the file list the wheel moves the list, not the file on view.
        let (x, y) = self.mouse;
        if let Some(panel) = self.list_panel().filter(|panel| panel.holds(x, y)) {
            let rows = (lines * 3.0).round() as i64;
            let last = (self.list.len() - panel.rows) as i64;
            let first = (panel.first as i64 - rows).clamp(0, last) as usize;
            self.list_scroll = Some((self.pos, self.list.len(), first));
            self.list_hover = self.list_row_at(x, y);
            return self.redraw();
        }
        if self.modifiers.control_key() && self.grid.is_none() {
            self.scrolled = 0.0;
            return self.zoom_by(ZOOM_STEP.powf(lines), Some(self.mouse));
        }
        // A touchpad reports many small movements per gesture; stepping on
        // each would race through the list. Step once per whole line.
        if (self.scrolled < 0.0) != (lines < 0.0) {
            self.scrolled = 0.0;
        }
        self.scrolled += lines;
        let steps = self.scrolled.trunc();
        self.scrolled -= steps;
        for _ in 0..steps.abs() as u32 {
            if self.grid.is_some() {
                // A page at a time.
                let per_page = self.grid_layout(self.window_size()).per_page() as i64;
                self.move_cursor(if steps < 0.0 { per_page } else { -per_page });
            } else {
                self.step(steps < 0.0);
            }
        }
    }

    // ---- Snapshots -----------------------------------------------------

    /// Once the first image is up, presses the scripted keys and schedules
    /// the snapshot. Does nothing in a normal run.
    fn start_snapshot(&mut self, event_loop: &ActiveEventLoop) {
        let Some(snapshot) = &mut self.snapshot else { return };
        if snapshot.at.is_some() {
            return;
        }
        self.mouse = snapshot.pointer;
        self.snapshot_step(event_loop);
    }

    /// Presses the scripted keys up to the next `{Wait}`, which stands for
    /// one waiting period, and schedules what follows.
    fn snapshot_step(&mut self, event_loop: &ActiveEventLoop) {
        let Some(snapshot) = &mut self.snapshot else { return };
        snapshot.at = Some(Instant::now() + snapshot.wait);
        let script = std::mem::take(&mut snapshot.keys);
        let (keys, later) = script.split_once("{Wait}").unwrap_or((&script, ""));
        snapshot.keys = later.to_string();
        self.play_keys(keys, event_loop);
    }

    /// Presses the keys of a script: plain characters are typed; `{Enter}`,
    /// `{Tab}`, `{F2}` and so on press the named key.
    fn play_keys(&mut self, keys: &str, event_loop: &ActiveEventLoop) {
        let mut rest = keys;
        while let Some(c) = rest.chars().next() {
            let (mut repeating, mut pressing, mut releasing) = (false, true, true);
            let named = rest.strip_prefix('{').and_then(|inner| inner.split_once('}'));
            let key = match named {
                Some((name, after)) => {
                    rest = after;
                    // `{Down+Space}` only presses a key and `{Up+Space}` only
                    // lets it go, for one that is held.
                    (pressing, releasing) = (!name.starts_with("Up+"), !name.starts_with("Down+"));
                    // `{Shift+Right}` and `{Ctrl+Right}` hold a modifier, and
                    // `{Held+Right}` is the key repeating from being held.
                    repeating = name.starts_with("Held+");
                    let (held, name) = match name.split_once('+') {
                        Some(("Shift", name)) => (ModifiersState::SHIFT, name),
                        Some(("Ctrl", name)) => (ModifiersState::CONTROL, name),
                        Some(("Held" | "Down" | "Up", name)) => (ModifiersState::empty(), name),
                        _ => (ModifiersState::empty(), name),
                    };
                    self.modifiers = held;
                    // `{Move:X,Y}` moves the pointer and `{Click}` clicks.
                    if let Some((x, y)) = name.strip_prefix("Move:").and_then(|to| to.split_once(',')) {
                        self.mouse_moved(x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
                        continue;
                    }
                    // `{Wheel:N}` turns the wheel N lines, up being positive.
                    if let Some(lines) = name.strip_prefix("Wheel:") {
                        self.mouse_wheel(MouseScrollDelta::LineDelta(0.0, lines.parse().unwrap_or(0.0)));
                        continue;
                    }
                    // `{RightClick}` is the other button.
                    if name == "RightClick" {
                        self.mouse_button(MouseButton::Right, true, event_loop);
                        self.mouse_button(MouseButton::Right, false, event_loop);
                        continue;
                    }
                    if name == "Click" {
                        self.mouse_button(MouseButton::Left, true, event_loop);
                        self.mouse_button(MouseButton::Left, false, event_loop);
                        continue;
                    }
                    // `{Press}` and `{Release}`, with moves between them,
                    // drag.
                    if name == "Press" || name == "Release" {
                        self.mouse_button(MouseButton::Left, name == "Press", event_loop);
                        continue;
                    }
                    match name {
                        "Enter" => Key::Named(NamedKey::Enter),
                        "Escape" => Key::Named(NamedKey::Escape),
                        "Tab" => Key::Named(NamedKey::Tab),
                        "Backspace" => Key::Named(NamedKey::Backspace),
                        "Delete" => Key::Named(NamedKey::Delete),
                        "Left" => Key::Named(NamedKey::ArrowLeft),
                        "Right" => Key::Named(NamedKey::ArrowRight),
                        "Up" => Key::Named(NamedKey::ArrowUp),
                        "Down" => Key::Named(NamedKey::ArrowDown),
                        "Space" => Key::Named(NamedKey::Space),
                        "PageDown" => Key::Named(NamedKey::PageDown),
                        "PageUp" => Key::Named(NamedKey::PageUp),
                        "Home" => Key::Named(NamedKey::Home),
                        "End" => Key::Named(NamedKey::End),
                        "F2" => Key::Named(NamedKey::F2),
                        "F5" => Key::Named(NamedKey::F5),
                        // `{Ctrl+k}`: a letter with a modifier held.
                        letter if letter.chars().count() == 1 => Key::Character(letter),
                        _ => continue,
                    }
                }
                None => {
                    let (typed, after) = rest.split_at(c.len_utf8());
                    rest = after;
                    Key::Character(typed)
                }
            };
            if pressing {
                self.key_pressed(key.clone(), repeating, false, event_loop);
            }
            // A key typed is let go again at once.
            if releasing && !repeating {
                self.key_released(key);
            }
            self.modifiers = ModifiersState::empty();
        }
        self.mouse_moved(self.mouse.0, self.mouse.1);
    }

    // ---- Benchmark -----------------------------------------------------

    /// Sets up the benchmark: a script of scenes played through the files
    /// once the first picture is up, each timed, with the figures printed
    /// at the end. `launched` is when the process began.
    pub fn start_benchmark(&mut self, launched: Instant) {
        let mut scenes: VecDeque<(&'static str, &'static str, Duration)> = VecDeque::new();
        let mut scene = |name, keys, ms| scenes.push_back((name, keys, Duration::from_millis(ms)));
        // On through the files at a reader's pace, so that each is decoded
        // ahead; then faster than they decode; then back over ones decoded.
        for _ in 0..15 {
            scene("step", "{Right}", 250);
        }
        for _ in 0..40 {
            scene("fast", "{Right}", 40);
        }
        scene("", "", 1500);
        for _ in 0..15 {
            scene("back", "{Left}", 250);
        }
        scene("", "", 400);
        for _ in 0..3 {
            scene("zoom", "{Up}", 350);
        }
        for _ in 0..3 {
            scene("zoom", "{Down}", 350);
        }
        scene("zoom", "0", 400);
        scene("grid", "g", 2000);
        scene("grid", "{PageDown}", 800);
        scene("grid", "{PageDown}", 800);
        scene("grid", "{PageUp}", 800);
        scene("grid", "g", 600);
        if self.files.iter().any(|file| files::is_video(file)) {
            scene("video", "@video", 3000);
            scene("", "{Right}", 500);
        }
        self.bench = Some(Bench { scenes, scene: "", at: None, samples: Vec::new(), launched, begun: false });
    }

    /// Records a figure for the benchmark, under a scene and a metric.
    fn bench_note(&mut self, scene: &str, metric: &str, took: Duration) {
        let Some(bench) = &mut self.bench else { return };
        let key = (scene.to_string(), metric.to_string());
        let ms = took.as_secs_f64() * 1000.0;
        match bench.samples.iter_mut().find(|(k, _)| *k == key) {
            Some((_, values)) => values.push(ms),
            None => bench.samples.push((key, vec![ms])),
        }
    }

    /// The scene the benchmark is in, if it is on.
    fn bench_scene(&self) -> Option<&'static str> {
        self.bench.as_ref().filter(|bench| bench.begun).map(|bench| bench.scene)
    }

    /// Plays the next scene of the benchmark, or ends it.
    fn bench_step(&mut self, event_loop: &ActiveEventLoop) {
        let Some(bench) = &mut self.bench else { return };
        bench.begun = true;
        let Some((scene, keys, wait)) = bench.scenes.pop_front() else { return self.finish_benchmark(event_loop) };
        bench.scene = scene;
        bench.at = Some(Instant::now() + wait);
        match keys {
            // To the first video in the list.
            "@video" => {
                if let Some(pos) = self.list.iter().position(|id| files::is_video(&self.files[*id])) {
                    self.go_to(pos, true);
                }
            }
            keys => self.play_keys(keys, event_loop),
        }
    }

    /// Prints the benchmark's figures and quits.
    fn finish_benchmark(&mut self, event_loop: &ActiveEventLoop) {
        let Some(bench) = self.bench.take() else { return };
        let (width, height) = self.window_size();
        let backend = match self.window.as_ref().and_then(|window| window.display_handle().ok()).map(|handle| handle.as_raw()) {
            Some(winit::raw_window_handle::RawDisplayHandle::Wayland(_)) => "Wayland",
            Some(winit::raw_window_handle::RawDisplayHandle::Xlib(_) | winit::raw_window_handle::RawDisplayHandle::Xcb(_)) => "X11",
            _ => "an unknown display",
        };
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        println!("jog {} benchmark: {} files, a {width}x{height} window on {backend}, {cores} cores", env!("CARGO_PKG_VERSION"), self.list.len());
        println!();
        println!("{:<24} {:<32} {:>8} {:>8} {:>8} {:>5}", "scene", "metric", "median", "p90", "max", "n");
        // Scenes in the order they were played; within one, the metrics
        // in the order a reader wants them.
        const METRICS: [&str; 8] = [
            "launch to first picture",
            "key to picture",
            "decode, read and fit",
            "start to first frame",
            "scale the picture, rough",
            "scale the picture, smooth",
            "draw the page",
            "redraw",
        ];
        let mut samples = bench.samples;
        let scenes: Vec<String> = samples.iter().map(|((scene, _), _)| scene.clone()).fold(Vec::new(), |mut seen, scene| {
            if !seen.contains(&scene) {
                seen.push(scene);
            }
            seen
        });
        let rank = |(scene, metric): &(String, String)| (scenes.iter().position(|s| s == scene), METRICS.iter().position(|m| m == metric));
        samples.sort_by_key(|(key, _)| rank(key));
        let mut last_scene = String::new();
        for ((scene, metric), values) in &samples {
            let mut sorted = values.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let at = |part: f64| sorted[((sorted.len() as f64 * part) as usize).min(sorted.len() - 1)];
            let shown = if *scene == last_scene { "" } else { scene.as_str() };
            last_scene.clone_from(scene);
            println!("{shown:<24} {metric:<32} {:>8} {:>8} {:>8} {:>5}", ms(Duration::from_secs_f64(at(0.5) / 1000.0)), ms(Duration::from_secs_f64(at(0.9) / 1000.0)), ms(Duration::from_secs_f64(sorted[sorted.len() - 1] / 1000.0)), sorted.len());
        }
        let shown = samples.iter().find(|((scene, metric), _)| scene == "fast" && metric == "key to picture").map_or(0, |(_, values)| values.len());
        println!();
        println!("Stepping every 40 ms, {shown} of 40 presses had their picture up before the next press.");
        let (thumbs, _) = self.loader.stats(0).thumbs_kept;
        println!("Thumbnails decoded for the grid: {thumbs}.");
        if let Some(peak) = std::fs::read_to_string("/proc/self/status").ok().and_then(|status| {
            status.lines().find(|line| line.starts_with("VmHWM:")).and_then(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
        }) {
            println!("Peak memory {} MB.", peak / 1024);
        }
        event_loop.exit();
    }

    /// Writes what would be on screen to the snapshot file and quits.
    fn finish_snapshot(&mut self, event_loop: &ActiveEventLoop) {
        if self.snapshot.as_ref().is_some_and(|snapshot| !snapshot.keys.is_empty()) {
            return self.snapshot_step(event_loop);
        }
        let Some(snapshot) = self.snapshot.take() else { return };
        if let (Some(player), true) = (&self.player, self.options.debug) {
            let count = |property| player.number(property).unwrap_or(0.0);
            let (position, late, dropped) = (count("time-pos"), count("frame-drop-count"), count("vo-drop-frame-count"));
            eprintln!("jog: video at {position:.1}s, {late} frames decoded late, {dropped} not shown");
        }
        let window = self.window_size();
        let mut pixels = vec![0u32; window.0 as usize * window.1 as usize];
        self.settle_at = None;
        self.paint(&mut pixels, window);
        self.draw_loupe(&mut pixels, window);

        let mut ppm = format!("P6 {} {} 255\n", window.0, window.1).into_bytes();
        ppm.extend(pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
        if let Err(error) = std::fs::write(&snapshot.path, ppm) {
            eprintln!("jog: cannot write {}: {error}", snapshot.path.display());
        }
        event_loop.exit();
    }

    // ---- Drawing -------------------------------------------------------

    fn draw(&mut self) {
        // While the first image is the window's background and the
        // window manager has not yet put the window up, a frame handed
        // over would be thrown away, and the handing over only delays
        // the server in putting the window up.
        if let Some((_, since)) = &self.backdrop {
            if !self.focused_once && since.elapsed() < BACKDROP_AT_MOST {
                return;
            }
        }
        let (Some(window), Some(mut surface)) = (self.window.clone(), self.surface.take()) else { return };
        let size = window.inner_size();
        // A frame nobody here asked for is the system's: the window has
        // been uncovered or mapped, and wants all of itself drawn.
        let ours = self.asked.take();
        if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            let entered = Instant::now();
            if surface.resize(width, height).is_ok() {
                // This waits for the display to have taken the frame before.
                if let Ok(mut buffer) = surface.buffer_mut() {
                    let started = Instant::now();
                    let waited = started.duration_since(entered);
                    self.paint(&mut buffer, (size.width, size.height));
                    self.draw_loupe(&mut buffer, (size.width, size.height));
                    // Whether the window still shows what was last handed
                    // it, and so only what differs need be told of; not
                    // when the whole picture was just painted afresh.
                    let kept = ours && buffer.age() == 1 && self.presented.len() == buffer.len() && !self.repainted.take();
                    // Something is still on its way somewhere: draw again soon.
                    let moving = self.animating.take();
                    if moving {
                        // No oftener than the display shows a frame.
                        let every = *self.frame_every.get_or_insert_with(|| {
                            let rate = window.current_monitor().and_then(|monitor| monitor.refresh_rate_millihertz());
                            Duration::from_secs_f64(1000.0 / f64::from(rate.filter(|rate| *rate > 0).unwrap_or(60_000)))
                        });
                        self.anim_due = Some((started + every).max(Instant::now() + Duration::from_millis(1)));
                    }
                    let painted = started.elapsed();
                    // The display is handed only what has changed: taking
                    // the whole of a large window is what takes it longest,
                    // several times as long as drawing the frame did.
                    let parts = match kept {
                        true => Some(ui::changed(&buffer, &mut self.presented, size.width as usize)),
                        false => {
                            ui::copy_all(&mut self.presented, &buffer);
                            None
                        }
                    };
                    let told = started.elapsed().saturating_sub(painted);
                    window.pre_present_notify();
                    let presenting = Instant::now();
                    let share = match &parts {
                        Some(parts) => {
                            let rects: Vec<softbuffer::Rect> = parts
                                .iter()
                                .filter_map(|[x, y, w, h]| Some(softbuffer::Rect { x: *x, y: *y, width: NonZeroU32::new(*w)?, height: NonZeroU32::new(*h)? }))
                                .collect();
                            let _ = buffer.present_with_damage(&rects);
                            (parts.iter().map(|part| f64::from(part[2]) * f64::from(part[3])).sum::<f64>() + 0.0) / (f64::from(size.width) * f64::from(size.height))
                        }
                        None => {
                            let _ = buffer.present();
                            1.0
                        }
                    };
                    let presented = presenting.elapsed();
                    // What a frame really cost: waiting for the display to
                    // be done with the last, drawing this one, telling what
                    // changed in it, handing that over and how much of the
                    // window it was, and how long after the last it began.
                    if self.options.debug {
                        let gap = self.frame_began.map(|before| started.duration_since(before)).unwrap_or_default();
                        let what = if moving { "moving" } else { "still " };
                        eprintln!(
                            "jog: draw  {what}  wait {waited:>7.1?}  paint {painted:>7.1?}  compare {told:>7.1?}  present {presented:>7.1?}  sent {:>5.1}%  since last {gap:>7.1?}",
                            share * 100.0
                        );
                    }
                    self.frame_began = Some(started);
                    self.painted(started, painted);
                    if let Some(scene) = self.bench_scene().filter(|scene| !scene.is_empty()) {
                        self.bench_note(scene, "redraw", painted);
                    }
                    // A frame has reached the window the ordinary way, and
                    // the window is up: the background is no longer needed.
                    if self.focused_once {
                        self.backdrop = None;
                    }
                }
            }
        }
        self.surface = Some(surface);
    }

    /// Notes how long a redraw took, and whether it was the first to show
    /// the file on view.
    fn painted(&mut self, started: Instant, painted: Duration) {
        let now = Instant::now();
        let perf = &mut self.perf;
        perf.paint = painted;
        perf.present = now.duration_since(started).saturating_sub(painted);
        perf.paints.push_back(now);
        while perf.paints.front().is_some_and(|at| now.duration_since(*at) > Duration::from_secs(1)) {
            perf.paints.pop_front();
        }
        let up = self.grid.is_none() && self.shown.as_ref().is_some_and(|(id, _)| *id == self.id());
        if up && self.perf.shown_in.is_none() {
            let took = now.duration_since(self.perf.arrived);
            self.perf.shown_in = Some(took);
            if self.options.verbose {
                self.report_shown(took);
            }
            // For the benchmark: how long the picture was in coming, and
            // how long its decode was. The first picture starts it off.
            if let Some(bench) = &self.bench {
                let id = self.id();
                let made = self.loader.stats(id).fitted;
                match bench.begun {
                    false => {
                        self.bench_note("start", "launch to first picture", bench.launched.elapsed());
                        self.bench_step_due = true;
                    }
                    true => {
                        let scene = bench.scene;
                        if matches!(scene, "step" | "fast" | "back" | "video") {
                            self.bench_note(scene, "key to picture", took);
                        }
                        // Going back is over pictures decoded already.
                        if matches!(scene, "step" | "fast" | "video") {
                            if let Some(made) = made.filter(|made| made.how.is_empty()) {
                                self.bench_note(scene, "decode, read and fit", made.read + made.fit);
                            }
                        }
                    }
                }
            }
            // The overlay was drawn before these figures were known.
            if self.show_debug {
                self.redraw();
            }
        }
    }

    /// For `--verbose`: a line about the file that has just come up.
    fn report_shown(&self, took: Duration) {
        let id = self.id();
        let mut line = format!("jog: [{}/{}] {}", self.pos + 1, self.list.len(), self.files[id].display());
        if let Some((width, height)) = self.native() {
            line.push_str(&format!("  {width}x{height}"));
        }
        if let Some(size) = std::fs::metadata(&self.files[id]).ok().map(|meta| meta.len()) {
            line.push_str(&format!("  {}", meta::human_size(size)));
        }
        line.push_str(&format!("  shown in {}", ms(took)));
        // A frame decoded ahead of time cost nothing on arrival.
        if let Some(made) = self.loader.stats(id).fitted {
            let how = if made.how.is_empty() { String::new() } else { format!(", {}", made.how) };
            line.push_str(&format!(" (read {}, fit {}{how})", ms(made.read), ms(made.fit)));
        }
        eprintln!("{line}");
    }

    /// The frame to draw from: the full-resolution one when it is loaded.
    fn source(&self) -> Option<Arc<Frame>> {
        let (id, frame) = self.shown.as_ref()?;
        let full = if *id == self.id() { self.loader.full(*id) } else { None };
        Some(full.unwrap_or_else(|| frame.clone()))
    }

    fn paint(&mut self, pixels: &mut [u32], window: (u32, u32)) {
        // The overview of workspaces takes the whole window.
        if self.overview.is_some() {
            self.scene_live = false;
            ui::fill_all(pixels, GRID_COLOR);
            return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
        }
        // Fresh out of the grid, the picture grows from the cell it had
        // there before anything else of it is drawn.
        if self.grid.is_none() {
            if let Some((picture, [x, y, w, h])) = self.growing() {
                ui::fill_all(pixels, self.options.background);
                Canvas::new(pixels, window.0, window.1).image_fast(&picture, x, y, w, h);
                self.scene_live = false;
                return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
            }
        }
        let playing = self.grid.is_none() && self.video.as_ref().is_some_and(|video| video.showing);
        // A frame kept from before a pause, stepped back to: drawn as it
        // was kept, at this very size, in place of the main player's.
        if playing {
            let len = window.0 as usize * window.1 as usize;
            if let Some((_, kept)) = self.back_shown().filter(|(_, kept)| kept.len() == len) {
                ui::copy_into(pixels, kept);
                self.video_kept.set(None);
                self.repainted.set(true);
                self.scene_live = false;
                return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
            }
        }
        let player = self.player.as_ref().filter(|_| playing);
        let started = Instant::now();
        if let Some(player) = player {
            // Asked to draw at a size it has not drawn at, the player sets
            // itself up afresh, which for a large video is ten times as long
            // as the frame: while the window is being resized, every frame. So
            // it draws at the size it had, and the frame is scaled here, until
            // the window has kept one size for a moment.
            let resizing = self.resized_at.is_some_and(|at| at.elapsed() < RESIZE_SETTLE);
            let size = self.video_render.filter(|size| resizing && *size != window && size.0 > 0 && size.1 > 0).unwrap_or(window);
            // The player draws a new frame straight into the window. A
            // frame drawn over again, for the pointer moving over the bar
            // or a word coming up, is copied from a copy kept of it: a
            // fraction of what the player takes to scale it to a large
            // window, and the display is then told only of what moved
            // over it. The copy is made the first time it is wanted,
            // so that playback with nothing over it pays nothing for it.
            let len = window.0 as usize * window.1 as usize;
            let showing = self.video.as_ref().map(|video| video.id);
            let kept = self.video_kept.get() == Some((window, showing)) && self.video_copy.len() == len;
            let drawn = if size != window {
                self.video_kept.set(None);
                self.repainted.set(true);
                ui::size_for(&mut self.video_scratch, size.0 as usize * size.1 as usize);
                let drawn = player.render(&mut self.video_scratch, size.0, size.1);
                if drawn {
                    let (x, y, scale) = Self::video_place(size, window);
                    let at = [x as i32, y as i32, (f64::from(size.0) * scale) as i32, (f64::from(size.1) * scale) as i32];
                    ui::fill_all(pixels, 0);
                    let whole = [0.0, 0.0, f64::from(size.0), f64::from(size.1)];
                    let _ = render::draw_crop(pixels, window.0 as usize, &self.video_scratch, size, whole, at);
                }
                drawn
            } else if player.frame_waiting() {
                self.video_render = Some(window);
                self.video_kept.set(None);
                self.repainted.set(true);
                player.render(pixels, window.0, window.1)
            } else if !kept {
                self.video_render = Some(window);
                ui::size_for(&mut self.video_copy, len);
                let drawn = player.render(&mut self.video_copy, window.0, window.1);
                self.video_kept.set(drawn.then_some((window, showing)));
                if drawn {
                    ui::copy_into(pixels, &self.video_copy);
                }
                drawn
            } else {
                ui::copy_into(pixels, &self.video_copy);
                true
            };
            if drawn {
                self.perf.render = started.elapsed();
                self.scene_live = false;
                let overlays = Instant::now();
                self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
                if self.options.debug {
                    eprintln!("jog: {:>7.1?} draw  video  overlays {:>7.1?}", self.perf.render, overlays.elapsed());
                }
                return;
            }
        }
        let source = if self.grid.is_some() { None } else { self.source() };
        match source {
            None if self.grid.is_some() => {
                let started = Instant::now();
                // The grid slides after its cursor; while it does, it is
                // drawn again and again.
                let layout = self.grid_layout(window);
                let mut moved = false;
                if let Some(grid) = &mut self.grid {
                    let was = grid.target;
                    grid.follow(&layout);
                    moved = grid.target != was;
                    if grid.scrolling() {
                        self.grid_due = Some(Instant::now() + Duration::from_millis(8));
                    }
                }
                // Other rows are coming into view: their pictures are wanted.
                if moved {
                    self.request();
                }
                ui::fill_all(pixels, GRID_COLOR);
                self.scene_live = false;
                self.render_pip(window);
                self.draw_grid(&mut Canvas::new(pixels, window.0, window.1));
                self.draw_shrinking(&mut Canvas::new(pixels, window.0, window.1), &layout);
                if self.options.debug {
                    eprintln!("jog: {:>7.1?} draw  grid", started.elapsed());
                }
                if self.bench_scene() == Some("grid") {
                    self.bench_note("grid", "draw the page", started.elapsed());
                }
            }
            Some(frame) => {
                let layout = self.layout_of(frame.native);
                let shown = self.shown.as_ref().map(|(id, _)| *id);
                // Big sources are scaled crudely while zooming or panning,
                // then smoothly once the view has settled.
                let settled = self.settle_at.is_none() || frame.pixels.len() <= SMOOTH_ALWAYS;
                // Past a few times magnification, show the actual pixels.
                let magnified = layout.width >= frame.width * SHOW_PIXELS_AT;
                let smooth = self.antialias && settled && !magnified;
                // An animation's clock starts when its first picture is drawn.
                if frame.animation.is_some() && self.picture_due.is_none() {
                    self.picture = 0;
                    self.picture_due = Some(Instant::now() + picture_delay(&frame, 0));
                }
                let picture = if frame.animation.is_some() { self.picture } else { 0 };
                let key = SceneKey { frame, picture, layout, smooth, window };
                let same = self.scene_key.as_ref().is_some_and(|current| current.matches(&key));
                // A picture shown at the size it was made, which most are,
                // is copied straight into the window: scaling it would be
                // a copy anyway, and a copy kept to copy from is only the
                // same copy twice over. A scaled picture is kept, so that
                // a frame drawn over it again does not scale it again.
                let direct = (key.frame.width, key.frame.height) == (layout.width, layout.height);
                let background = self.options.background;
                let started = Instant::now();
                if direct {
                    render::draw_image(pixels, window, &key.frame, picture, layout, smooth, background);
                } else if !same {
                    ui::size_for(&mut self.scene, pixels.len());
                    render::draw_image(&mut self.scene, window, &key.frame, picture, layout, smooth, background);
                }
                if !same {
                    self.scene_id = shown;
                    let took = started.elapsed();
                    if self.options.debug {
                        let how = if smooth { "smooth" } else { "nearest" };
                        eprintln!("jog: {took:>7.1?} draw  {how} {}x{} -> {}x{}", key.frame.width, key.frame.height, layout.width, layout.height);
                    }
                    if self.bench_scene() == Some("zoom") {
                        self.bench_note("zoom", if smooth { "scale the picture, smooth" } else { "scale the picture, rough" }, took);
                    }
                    let (from, to) = ((key.frame.width, key.frame.height), (layout.width, layout.height));
                    self.perf.scaled = Some(Scaled { took, smooth, from, to });
                    self.scene_key = Some(key);
                    self.repainted.set(true);
                }
                if !direct {
                    ui::copy_into(pixels, &self.scene);
                }
                self.scene_live = true;
            }
            None => {
                self.scene_live = false;
                ui::fill_all(pixels, self.options.background);
            }
        }
        self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
    }

    fn draw_overlays(&mut self, canvas: &mut Canvas) {
        let (width, height) = (canvas.width, canvas.height);
        let px = ui_px(height as u32);
        let margin = px as i32;
        self.tend_story();
        self.tend_trailer();
        // The grid replaces the image and everything describing it.
        let viewing = self.grid.is_none() && self.overview.is_none();
        // The graphs of the picture's colours are counted from it as it is
        // on the screen, before anything is drawn over it: only the part of
        // it in view, which zoomed in is the part looked at.
        let graphing = viewing && self.show_debug && self.scopes.contains(&true);
        let mut graph_cost = Duration::ZERO;
        if graphing {
            let shown = self.shown.as_ref().filter(|(id, _)| *id == self.id()).map(|(_, frame)| frame.native);
            let (left, stage) = self.stage();
            let [x, y, w, h] = match shown.map(|native| self.layout_of(native)) {
                Some(layout) => [layout.x, layout.y, layout.width as i32, layout.height as i32],
                None => [left, 0, stage.0 as i32, stage.1 as i32],
            };
            let (x0, y0) = (x.max(0), y.max(0));
            let (x1, y1) = ((x + w).min(width), (y + h).min(height));
            let rect = [x0 as usize, y0 as usize, (x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize];
            let counting = Instant::now();
            self.scope_counts.count(canvas.all(), width as usize, rect, self.scopes);
            graph_cost = counting.elapsed();
        }
        let strip = (viewing && self.strip_visible()).then(|| Strip::for_height(height as u32));
        if let Some(strip) = strip {
            self.draw_strip(canvas, strip);
        }
        // The scenes of the video on view stand there instead, when asked for.
        let scenes_level = self.glide(&self.scenes_rise, if self.scenes_shown() { 1.0 } else { 0.0 }, EASE);
        let scene_strip = (scenes_level > 0.01 && viewing && self.video.is_some()).then(|| {
            // It rises from the bottom edge and sinks back into it.
            let strip = Strip::for_height(height as u32);
            Strip { top: strip.top + ((1.0 - scenes_level) * f64::from(strip.height)) as i32, ..strip }
        });
        if let Some(strip) = scene_strip {
            match self.strip_tab {
                StripTab::Scenes => {
                    self.want_scenes();
                    self.draw_scenes(canvas, strip);
                }
                StripTab::Sections => self.draw_entries(canvas, strip),
            }
        }
        let bottom = strip.or(scene_strip).map_or(height, |s| s.top) - margin;

        let marked = viewing && self.marks.contains(&self.id());
        if marked {
            // A bar along the top edge flags a marked image.
            canvas.fill(0, 0, width, (px as i32 / 3).max(4), MARK_COLOR);
        }
        // The lines at the top left, each a run of words in their colours:
        // what matters bright, what describes it muted.
        let (soft, dim) = (accent_soft(), DIM_TEXT_COLOR);
        let mut top_left: Vec<Vec<(String, u32)>> = Vec::new();
        // Said all the while, so that nothing done is taken for done.
        if self.options.dry_run {
            top_left.push(vec![("Dry run: nothing is changed, only said in the terminal".to_string(), MARK_COLOR)]);
        }
        if let Some((typed, all)) = self.filter.as_ref().filter(|_| self.show_info) {
            let found = format!("{} {typed}", commands::icons::SEARCH);
            top_left.push(vec![(found, soft), (format!("   {} of {}", self.list.len(), all.len()), TEXT_COLOR), ("   Esc to go back".to_string(), dim)]);
        }
        if viewing && !self.marks.is_empty() {
            let count = format!("{} {} marked", commands::icons::STAR, self.marks.len());
            let line = if marked { vec![(count, MARK_COLOR), ("   this one too".to_string(), dim)] } else { vec![(count, dim)] };
            top_left.push(line);
        }
        if viewing && self.show_info {
            // The name is there to be clicked, which copies it; the folder
            // before it, dimmer, copies the whole path.
            let folder = self.files[self.id()].parent().filter(|dir| !dir.as_os_str().is_empty()).map(short_dir);
            let mut line = vec![(format!("{LINK}{}", self.name()), TEXT_COLOR), (format!("   {} / {}", self.pos + 1, self.list.len()), soft)];
            if let Some(folder) = folder {
                line.insert(0, (format!("{LINK}{}/", folder.trim_end_matches('/')), dim));
            }
            // A video not watched yet says so, until it has been.
            if self.is_new(self.id()) {
                line.push(("   new".to_string(), accent_soft()));
            }
            // Which workspace this is, when there are several to be in.
            if self.spaces.len() > 1 {
                line.insert(0, (format!("@{}   ", self.spaces[self.space].name), soft));
            }
            // Beside the name, badges for what the file is: an image's
            // format and whether it is an Ultra HDR photo; a video's codec
            // and HDR formats.
            let ultra = self.video.is_none() && self.is_ultra_hdr(self.id());
            let mut badges: Vec<String> = Vec::new();
            match self.video.is_some() {
                true => {
                    badges.extend(self.codec_name());
                    badges.extend(self.hdr_badges());
                }
                false => {
                    badges.extend(self.format_of(self.id()));
                    if ultra {
                        badges.push("Ultra HDR".to_string());
                    }
                }
            }
            let before_badges = line.len();
            for name in &badges {
                line.push((if line.len() > before_badges { "" } else { " " }.to_string(), dim));
                line.push(badge(name));
            }
            top_left.push(line);
            // The dimensions, to be clicked, which copies them.
            let (dims, rest) = self.info_line();
            let mut details = vec![(rest, dim)];
            if let Some(dims) = dims {
                details.insert(0, (format!("{LINK}{dims}"), dim));
            }
            if ultra {
                // The three renditions, to be clicked: the one on view filled.
                details.push(("   ".to_string(), soft));
                for mode in [Hdr::Sdr, Hdr::Full, Hdr::GainMap] {
                    let marker = if mode == self.hdr { CHOSEN } else { CHOICE };
                    details.push((format!("{marker}{}", mode.name()), TEXT_COLOR));
                }
            }
            top_left.push(details);
            if let Some((position, length)) = self.video_time() {
                let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
                let sign = if paused { commands::icons::PAUSE } else { commands::icons::PLAY };
                let mut line = vec![(format!("{sign} {}", clock(position)), TEXT_COLOR), (format!(" / {}", clock(length)), dim)];
                if self.speed != 1.0 {
                    line.push((format!("   {}x", self.speed), soft));
                }
                match self.looping {
                    Looping::Off => {}
                    Looping::File => line.push((format!("   {} this video", commands::icons::REPEAT_1), soft)),
                    Looping::List => line.push((format!("   {} the list", commands::icons::REPEAT), soft)),
                }
                if self.muted {
                    line.push((format!("   {} muted", commands::icons::VOLUME_X), MARK_COLOR));
                }
                top_left.push(line);
            }
        }
        let debug = if viewing && self.show_debug { self.debug_groups() } else { Vec::new() };
        // The seek bar: a rounded track lifted off the bottom edge, with a
        // handle and the times at either end. It is out of the way during
        // playback, and appears while the pointer is at the bottom of the
        // window and for a moment after a seek, a frame step or a pause.
        let mut time = Vec::new();
        let mut time_bottom = bottom;
        // Text placed by an anchor: ending at it, centred on it, or starting at it.
        let mut labels: Vec<(i32, i32, Ordering, String)> = Vec::new();
        // The buttons at the right of the bar: where, which icon, how bright.
        let mut buttons: Vec<(i32, i32, &str, u32)> = Vec::new();
        let mut subtitle_margin = SUBTITLE_MARGIN;
        let bar_wanted = viewing && (self.hover.is_some() || self.time_until.is_some());
        // It rises from the bottom edge and sinks back into it.
        let bar_level = self.glide(&self.bar_rise, if bar_wanted { 1.0 } else { 0.0 }, EASE);
        // Where the bar stands on a strip it comes up from behind it. What
        // the strip shows is kept while the bar is on its way, and put back
        // over whatever of the bar was drawn across it.
        let onto = ((self.strip_pinned || self.triage_on) && strip.is_some()) || scene_strip.is_some();
        let band_top = Strip::for_height(height as u32).top;
        let behind = (onto && bar_level > 0.01 && bar_level < 0.999).then(|| canvas.copy(0, band_top, width, height - band_top));
        if let Some((position, length)) = (bar_level > 0.01).then(|| self.video_time()).flatten() {
            let mut bar = self.seek_bar();
            // Up from the bottom edge, or from behind the strip it stands
            // on: the same way either way.
            let travel = if onto { bar.unit * 5 } else { bar.unit * 6 };
            bar.middle += ((1.0 - bar_level) * f64::from(travel)) as i32;
            let unit = bar.unit;
            let line = Text::line_height(px);
            let thickness = unit * 2 / 3;
            let top = bar.middle - thickness / 2;
            // Darken the picture behind it, more and more towards the
            // bottom edge.
            let shade_top = bar.middle - unit * 6;
            // Down to what the bar stands on: the window's edge, or a strip
            // along it, which is not to be darkened with the picture.
            canvas.fade_down(bar.left, shade_top, width - bar.left, (bar.middle + unit * 2).min(height) - shade_top, 70);
            // The handle is where the video is, and jumps with it.
            let done = (f64::from(bar.width) * position / length) as i32;
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 72, bar.width);
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 256, done.max(thickness / 2));
            // A notch in the bar where each scene starts, while they are up.
            if let Some((_, starts, _)) = self.scenes_now().filter(|_| self.scenes_shown()) {
                for start in &starts[1..] {
                    let at = bar.x + (f64::from(bar.width) * start / length) as i32;
                    canvas.fill(at - 1, top, 2, thickness, 0x000000);
                }
            }
            let flags = self.draw_section(canvas, &bar, top, thickness, length, &mut labels, px);
            let handle = unit * 5 / 4;
            canvas.pill(bar.x + done - handle / 2, bar.middle - handle / 2, handle, handle, TEXT_COLOR, 256, handle);
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
            // Either side of it: the file before and the file after.
            for (control, icon) in [(0, commands::icons::SKIP_BACK), (2, commands::icons::SKIP_FORWARD)] {
                let (left, top, side) = bar.control(control);
                labels.push((left + side / 2, top + (side - line) / 2, Ordering::Equal, icon.to_string()));
            }
            let (symbol_left, symbol_top, side) = bar.symbol();
            // The symbol is what a press on it does: pause while playing,
            // and play while paused.
            if !paused {
                canvas.pause_symbol(symbol_left, symbol_top, side, TEXT_COLOR);
            } else {
                canvas.play_symbol(symbol_left, symbol_top, side, TEXT_COLOR);
            }
            labels.push((bar.x - unit, bar.middle - line / 2, Ordering::Less, clock(position)));
            labels.push((bar.x + bar.width + unit, bar.middle - line / 2, Ordering::Greater, clock(length)));
            for (which, command) in (0..).zip(self.bar_buttons()) {
                let (left, top, side) = bar.button(which);
                let (icon, on) = match command {
                    Command::CycleLoop => match self.looping {
                        Looping::Off => (commands::icons::REPEAT, false),
                        Looping::File => (commands::icons::REPEAT_1, true),
                        Looping::List => (commands::icons::REPEAT, true),
                    },
                    Command::ToggleMute if self.muted => (commands::icons::VOLUME_X, false),
                    Command::ToggleMute => (commands::icons::VOLUME_2, true),
                    Command::CycleSubtitles if self.subtitles => (commands::icons::CAPTIONS, true),
                    Command::CycleSubtitles => (commands::icons::CAPTIONS_OFF, false),
                    _ => (commands::icons::AUDIO_LINES, true),
                };
                buttons.push((left + side / 2, top + (side - line) / 2, icon, if on { TEXT_COLOR } else { DIM_TEXT_COLOR }));
            }
            // Subtitles stand clear of the bar while it is up.
            subtitle_margin = (height - (bar.middle - unit * 3)) * 720 / height.max(1);
            time_bottom = bar.middle - unit * 4;

            if let Some(fraction) = self.hover.filter(|_| bar.on_track(self.mouse.0)) {
                // A tick where the pointer is, the time there, and above
                // that the frame from that part of the video.
                let at = bar.x + (f64::from(bar.width) * fraction) as i32;
                canvas.fill(at - 1, top - unit / 3, 2, thickness + unit * 2 / 3, MARK_COLOR);
                // Over a section's flag, the time stands a line higher than
                // the flag's letter.
                let over_flag = flags.iter().any(|flag| (flag - at).abs() < line * 2);
                let label_top = bar.middle - unit * 2 - line / 2 - if over_flag { line } else { 0 };
                labels.push((at.clamp(unit * 4, width - unit * 4), label_top, Ordering::Equal, clock(fraction * length)));
                // The video has changed under a pointer that has not moved:
                // the frame kept is of the one before, and is not shown;
                // the one for this video is asked for.
                if self.preview_id != self.video.as_ref().map(|video| video.id) {
                    self.preview_frame = None;
                    self.request_preview(fraction);
                }
                if let Some(frame) = &self.preview_frame {
                    let (w, h) = (frame.width as i32, frame.height as i32);
                    let left = (at - w / 2).clamp(unit, (width - unit - w).max(unit));
                    let picture_top = label_top - unit / 2 - h;
                    let round = unit / 2;
                    canvas.image_rounded(frame, left, picture_top, w, h, round);
                    canvas.round_outline(left - 2, picture_top - 2, w + 4, h + 4, round + 2, 2, MENU_EDGE_COLOR);
                }
            } else if self.time_until.is_some() {
                time.push(format!("{} / {}", clock(position), clock(length)));
            }
        }
        let exif = if viewing && self.show_exif {
            self.current_meta().map(|meta| meta.exif.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let footer: Vec<String> = self.grid_footer().into_iter().collect();
        let mut actions = Vec::new();
        if viewing && self.show_actions {
            let defined = self.options.actions.iter().enumerate();
            actions.extend(defined.filter_map(|(slot, a)| Some(format!("{slot}: {}", a.as_ref()?))));
            if actions.is_empty() {
                actions.push("No actions defined (see --action)".to_string());
            }
        }
        let toast: Vec<String> = self.toast.iter().map(|(message, _)| message.clone()).collect();
        let error: Vec<String> = match (&self.error, &self.shown) {
            (Some(reason), None) if viewing => vec![format!("Cannot show {}", self.name()), reason.clone()],
            _ => Vec::new(),
        };

        let blocks = [&exif, &actions, &toast, &error, &footer, &time];
        let nothing_to_write = blocks.iter().all(|lines| lines.is_empty()) && labels.is_empty() && top_left.is_empty() && debug.is_empty();
        let panel = if viewing { self.list_panel() } else { None };
        let boxes = self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some() || panel.is_some() || self.convert_panel.is_some() || self.compare.is_some();
        // The filmstrip has words of its own: the lengths of its videos.
        let boxes = boxes || self.palette.is_some() || strip.is_some() || scene_strip.is_some() || self.overview.is_some() || self.triage_on;
        if nothing_to_write && !boxes && !self.help {
            // Nothing drawn means no debug overlay, and none of its buttons.
            self.debug_buttons.clear();
            return;
        }
        let mut text = self.text.take().unwrap_or_else(Text::load);
        self.draw_overview(canvas, &mut text, px);
        self.draw_grid_labels(canvas, &mut text, px);
        self.draw_grid_footer(canvas, &mut text, &footer, px);
        // What sits on the left moves over to make room for the file list.
        let left = margin + panel.as_ref().filter(|_| !self.list_right).map_or(0, |panel| panel.width);
        // What sits on the right moves over likewise when the list is there.
        let width = width - self.right_inset();
        if let Some(strip) = &scene_strip {
            if self.strip_tab == StripTab::Scenes {
                self.draw_scene_labels(canvas, &mut text, strip, px);
            }
            self.draw_strip_words(canvas, &mut text, strip, px);
        }
        if let Some(strip) = &strip {
            self.draw_strip_lengths(canvas, &mut text, strip, px);
            self.draw_strip_close(canvas, &mut text, px);
            self.draw_strip_tip(canvas, &mut text, strip, px);
        }
        if let Some(panel) = &panel {
            for pos in panel.first..panel.first + panel.rows {
                self.want_duration(self.list[pos]);
            }
            // Drawn as though it stood at the left edge, on a sheet of its
            // own that starts as what is under it, and laid where it stands.
            let (across, down) = (panel.width, canvas.height);
            let mut sheet = canvas.copy(panel.left, 0, across, down);
            if sheet.len() == (across * down) as usize {
                self.draw_list(&mut Canvas::new(&mut sheet, across as u32, down as u32), &mut text, panel);
                canvas.dissolve(panel.left, 0, across, down, &sheet, 256);
            }
        }
        let (below, choices) = rich_block(canvas, &mut text, &top_left, px, (left, margin));
        // The renditions of an Ultra HDR photo, to be clicked.
        self.info_buttons = choices
            .into_iter()
            .filter_map(|(name, [x, y, width, height])| {
                // The file's name, which a click copies.
                if name == self.name() {
                    return Some(Button { x, y, width, height, command: Command::CopyViewedName });
                }
                // The folder before it, which copies the whole path, and
                // the dimensions.
                if name.ends_with('/') {
                    return Some(Button { x, y, width, height, command: Command::CopyViewedPath });
                }
                if name.contains(" x ") && name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Some(Button { x, y, width, height, command: Command::CopyViewedSize });
                }
                let mode = [Hdr::Sdr, Hdr::Full, Hdr::GainMap].into_iter().find(|mode| mode.name() == name)?;
                Some(Button { x, y, width, height, command: Command::SetHdr(mode) })
            })
            .collect();
        let pointer = (self.mouse.0 as i32, self.mouse.1 as i32);
        let (debug_buttons, debug_bottom) = draw_debug(canvas, &mut text, &debug, px, (left, below), &mut self.debug_keep, pointer);
        self.debug_buttons = debug_buttons;
        self.debug_last = debug;
        let mut below_debug = debug_bottom;
        if graphing {
            let drawing = Instant::now();
            let (graph_buttons, graphs_bottom) = self.draw_scopes(canvas, &mut text, px, (left, debug_bottom), pointer);
            self.debug_buttons.extend(graph_buttons);
            self.scope_cost = graph_cost + drawing.elapsed();
            below_debug = graphs_bottom;
        }
        // Under them, a video's HDR: its formats and its curves.
        if viewing && self.show_debug && self.video.is_some() {
            let hdr_buttons = self.draw_hdr(canvas, &mut text, px, (left, below_debug), pointer);
            self.debug_buttons.extend(hdr_buttons);
        }
        text_block(canvas, &mut text, &exif, px, |_, h| (left, bottom - h));
        text_block(canvas, &mut text, &actions, px, |w, h| (width - margin - w, bottom - h));
        // A toast slides in from the right edge, and off that way again
        // as its time runs out.
        let toast_in = self.glide(&self.toast_slide, 1.0, EASE);
        let toast_left = self.toast.as_ref().map_or(Duration::ZERO, |(_, until)| until.saturating_duration_since(Instant::now()));
        let toast_out = match self.toast.is_some() && self.options.animations && toast_left < EASE {
            true => {
                self.animating.set(true);
                (1.0 - toast_left.as_secs_f64() / EASE.as_secs_f64()).powi(2)
            }
            false => 0.0,
        };
        let toast_away = (1.0 - toast_in) + toast_out;
        self.toast_buttons.clear();
        match (&self.toast, self.toast_links.is_empty()) {
            (Some((message, _)), false) => {
                // One line: the words, the file's name in them there to be
                // clicked when it offers Show, then its links as buttons.
                let shows = self.toast_links.iter().any(|(_, link)| matches!(link, ToastLink::Show(_)));
                let mut line: Vec<(String, u32)> = vec![(if shows { format!("{LINK}{message}") } else { message.clone() }, TEXT_COLOR), ("  ".to_string(), TEXT_COLOR)];
                line.extend(self.toast_links.iter().map(|(label, _)| (format!("{CHOICE}{label}"), TEXT_COLOR)));
                let pad = px as i32 / 2;
                let w = line.iter().map(|(run, _)| run_width(&mut text, run, px)).sum::<i32>() + 3 * pad;
                let x = width - margin - w + (toast_away * f64::from(w + margin)) as i32;
                let (_, choices) = rich_block(canvas, &mut text, &[line], px, (x, margin));
                for (name, place) in choices {
                    let link = match self.toast_links.iter().find(|(label, _)| *label == name) {
                        Some((_, link)) => link.clone(),
                        // The words themselves: Show.
                        None => match self.toast_links.iter().find(|(_, link)| matches!(link, ToastLink::Show(_))) {
                            Some((_, link)) => link.clone(),
                            None => continue,
                        },
                    };
                    self.toast_buttons.push((link, place));
                }
            }
            _ => text_block(canvas, &mut text, &toast, px, |w, _| (width - margin - w + (toast_away * f64::from(w + margin)) as i32, margin)),
        }
        self.draw_triage(canvas, &mut text, px);
        // Twice the usual size, centred just above the seek bar.
        text_block(canvas, &mut text, &time, px, |w, h| ((width - w) / 2, time_bottom - h));
        for (centre, top, icon, color) in buttons {
            let left = centre - text.width(icon, px) / 2;
            text.draw(canvas, left, top, px, color, icon);
        }
        if subtitle_margin != self.subtitle_margin {
            self.subtitle_margin = subtitle_margin;
            for player in self.players() {
                player.set("sub-margin-y", &subtitle_margin.to_string());
            }
        }
        for (anchor, top, side, label) in &labels {
            let label_width = text.width(label, px);
            let left = match side {
                Ordering::Less => anchor - label_width,
                Ordering::Equal => anchor - label_width / 2,
                Ordering::Greater => *anchor,
            };
            text.draw(canvas, left, *top, px, TEXT_COLOR, label);
        }
        if let Some(behind) = &behind {
            canvas.dissolve(0, band_top, width, height - band_top, behind, 256);
            // Its words went with it, and are written again.
            if let Some(strip) = &scene_strip {
                if self.strip_tab == StripTab::Scenes {
                    self.draw_scene_labels(canvas, &mut text, strip, px);
                }
                self.draw_strip_words(canvas, &mut text, strip, px);
            }
            if let Some(strip) = &strip {
                self.draw_strip_lengths(canvas, &mut text, strip, px);
                self.draw_strip_close(canvas, &mut text, px);
            }
        }
        text_block(canvas, &mut text, &error, px, |w, h| ((width - w) / 2, (height - h) / 2));
        // A menu or a box comes up over a moment: what was there is kept
        // and laid back over it, less and less. It is well on its way from
        // the first frame, so that nothing seems to wait.
        let boxed = self.menu.is_some() || self.prompt.is_some() || self.palette.is_some() || self.confirm.is_some() || self.help || self.convert_panel.is_some();
        let box_level = self.glide(&self.box_fade, if boxed { 1.0 } else { 0.0 }, EASE);
        let mut behind = std::mem::take(&mut self.fade_buf);
        let fading = boxed && box_level < 0.99;
        if fading {
            ui::copy_all(&mut behind, canvas.all());
        }
        if let Some(menu) = &self.menu {
            self.draw_menu(canvas, &mut text, menu);
        }
        if let Some(prompt) = &self.prompt {
            let count = self.targets().len();
            let files = if count == 1 { self.name_of(prompt.id) } else { format!("{count} files") };
            let (icon, title, hint) = match prompt.kind {
                PromptKind::Rename => (commands::icons::PENCIL, "Rename".to_string(), "Enter to rename  ·  a path moves it  ·  Tab completes"),
                PromptKind::SaveWorkspace => (commands::icons::FOLDER_PLUS, "Save this folder as a workspace named".to_string(), "Enter to add it to the config file"),
                PromptKind::SectionName { start, end } => (commands::icons::PENCIL, format!("Name {} – {}", clock(start), clock(end)), "Enter to save  ·  empty clears it"),
                PromptKind::SendTo { moving: true } => (commands::icons::FOLDER_INPUT, format!("Move {files} to"), "Enter to move  ·  Tab completes"),
                PromptKind::SendTo { moving: false } => (commands::icons::FOLDER_PLUS, format!("Copy {files} to"), "Enter to copy  ·  Tab completes"),
            };
            // What Enter would do, as it is typed.
            let hint = self.prompt_hint(prompt).unwrap_or_else(|| hint.to_string());
            draw_prompt(canvas, &mut text, icon, &title, &prompt.field, &hint, px);
        }
        if self.convert_panel.is_some() {
            self.draw_convert_panel(canvas, &mut text, px);
        }
        if self.compare.is_some() {
            self.draw_compare(canvas, &mut text, px);
        }
        if let Some(layout) = self.palette_layout() {
            for id in self.palette_shown() {
                self.want_duration(id);
            }
            let drawn = self.draw_palette(canvas, &mut text, &layout);
            if let Some(palette) = &mut self.palette {
                palette.drawn = drawn;
            }
        }
        if let Some(confirm) = &self.confirm {
            let drawn = draw_confirm(canvas, &mut text, confirm, px);
            if let Some(confirm) = &mut self.confirm {
                confirm.drawn = drawn;
            }
        }
        if self.help {
            draw_help(canvas, &mut text, px, &self.help_search);
        }
        if fading {
            ui::dissolve_all(canvas.all(), &behind, ((1.0 - box_level) * 0.6 * 256.0) as u32);
        }
        self.fade_buf = behind;
        // Letters whose font is still loading show as boxes for now.
        if text.waiting() {
            self.font_due = Some(Instant::now() + Duration::from_millis(150));
        }
        self.text = Some(text);
    }

    /// The list positions whose cells are in view, or partly so while the
    /// grid slides: the page on show and the one coming in beside it.
    fn grid_shown(&self, layout: &GridLayout) -> std::ops::Range<usize> {
        let scroll = self.grid.as_ref().map_or(0.0, Grid::scroll);
        let first = scroll.floor().max(0.0) as usize * layout.per_page();
        first.min(self.list.len())..(first + 2 * layout.per_page()).min(self.list.len())
    }

    /// The space a grid cell gives its picture: left, top, width and
    /// height. With titles on, a line under it is kept for the name.
    fn grid_area(&self, layout: &GridLayout, pos: usize, window_height: u32) -> [i32; 4] {
        let scroll = self.grid.as_ref().map_or(0.0, Grid::scroll);
        let cell = (pos % layout.per_page()) as i32;
        let (column, row) = (cell % layout.columns, cell / layout.columns);
        let title = if self.grid_titles { Text::line_height(ui_px(window_height)) + layout.pad / 2 } else { 0 };
        // Pages lie side by side; the one on show is at the window's left.
        let slid = ((layout.page_of(pos) - scroll) * f64::from(layout.width)).round() as i32;
        let left = layout.left + column * layout.cell_w + layout.pad + slid;
        let top = layout.top + row * layout.cell_h + layout.pad;
        [left, top, layout.cell_w - 2 * layout.pad, (layout.cell_h - 2 * layout.pad - title).max(1)]
    }

    /// Where in its space a picture of this size is drawn: scaled to fit,
    /// in the middle.
    fn grid_picture(area: [i32; 4], size: (u32, u32), enlarge: bool) -> [i32; 4] {
        let [left, top, width, height] = area;
        let (w, h) = fit_size(size, (width as u32, height as u32), enlarge);
        let (w, h) = (w as i32, h as i32);
        [left + (width - w) / 2, top + (height - h) / 2, w, h]
    }

    fn draw_grid(&self, canvas: &mut Canvas) {
        let Some(grid) = &self.grid else { return };
        let layout = self.grid_layout((canvas.width as u32, canvas.height as u32));
        let edge = (layout.pad / 4).max(2);
        let radius = (layout.cell_w / 36).max(4);

        // The plain thumbnails go down first, all in one pass over the
        // rows of the window on every core; the cells with more to them
        // are drawn over that one by one.
        let shown: Vec<(usize, usize, Option<Arc<Frame>>)> =
            self.grid_shown(&layout).map(|pos| (pos, self.list[pos], self.loader.thumb(self.list[pos]))).collect();
        let plain = |pos: usize, id: usize| {
            let trailer = self.trailer.as_ref().is_some_and(|trailer| trailer.id == id && pos == grid.cursor && trailer.frame.as_ref().is_some_and(|frame| !frame.pixels.is_empty()));
            let live = self.pip_live.is_some() && self.pip && self.pip_in_cell && pos == self.pos;
            !trailer && !live
        };
        let blits: Vec<(&Frame, [i32; 4])> = shown
            .iter()
            .filter(|(pos, id, thumb)| thumb.is_some() && plain(*pos, *id))
            .map(|(pos, _, thumb)| {
                let thumb = thumb.as_deref().expect("filtered to the thumbnails there are");
                let area = self.grid_area(&layout, *pos, canvas.height as u32);
                (thumb, Self::grid_picture(area, (thumb.width, thumb.height), false))
            })
            .collect();
        canvas.images(&blits);

        let mut cursor_place = None;
        for (pos, id, thumb) in &shown {
            let (pos, id) = (*pos, *id);
            let area = self.grid_area(&layout, pos, canvas.height as u32);
            // A video under the cursor plays as a trailer.
            let trailer = self.trailer.as_ref().filter(|trailer| trailer.id == id && pos == grid.cursor);
            // The video that was on view plays on in its cell, sound and all.
            let live = self.pip_live.as_ref().filter(|_| self.pip && self.pip_in_cell && pos == self.pos);
            let since = trailer.filter(|_| live.is_none()).and_then(|trailer| trailer.since);
            let trailer = live.or(trailer.and_then(|trailer| trailer.frame.as_ref()).filter(|frame| !frame.pixels.is_empty()));
            let [x, y, w, h] = match (trailer, thumb) {
                (Some(frame), _) => {
                    let place = Self::grid_picture(area, (frame.width, frame.height), true);
                    // The thumbnail gives way to the trailer's first frames
                    // gradually, rather than being cut off by them.
                    let left = since.map_or(0.0, |since| 1.0 - since.elapsed().as_secs_f64() / TRAILER_FADE.as_secs_f64());
                    let fading = thumb.as_ref().filter(|_| left > 0.0).map(|thumb| {
                        canvas.image(thumb, place[0], place[1], place[2], place[3]);
                        canvas.copy(place[0], place[1], place[2], place[3])
                    });
                    canvas.image(frame, place[0], place[1], place[2], place[3]);
                    if let Some(was) = fading {
                        canvas.dissolve(place[0], place[1], place[2], place[3], &was, (left * 256.0) as u32);
                    }
                    place
                }
                // Drawn already, with the others.
                (None, Some(thumb)) => Self::grid_picture(area, (thumb.width, thumb.height), false),
                (None, None) => {
                    canvas.fill(area[0], area[1], area[2], area[3], PLACEHOLDER_COLOR);
                    area
                }
            };
            canvas.round_off(x, y, w, h, radius, GRID_COLOR);
            if self.marks.contains(&id) {
                let dot = layout.pad;
                canvas.round_fill(x + dot / 2, y + dot / 2, dot, dot, dot / 2, MARK_COLOR, 256);
            }
            // The cursor is ringed brightly, the image being viewed quietly.
            if pos == grid.cursor {
                // Drawn last, on its way from the cell it was on.
                cursor_place = Some([x, y, w, h]);
            } else if pos == self.pos {
                let color = accent();
                let gap = edge * 2;
                canvas.round_outline(x - gap, y - gap, w + 2 * gap, h + 2 * gap, radius + gap, edge, color);
            }
        }
        // The cursor's ring glides over from the cell it was on. While a
        // page slides, and when the grid has just come up, it is simply
        // where it belongs.
        if let Some(place) = cursor_place {
            let snap = grid.scrolling() || self.ring_snap.take();
            let [x, y, w, h] = std::array::from_fn(|side| {
                let to = f64::from(place[side]);
                if snap {
                    self.ring[side].set(Glide::at(to));
                }
                self.glide(&self.ring[side], to, EASE).round() as i32
            });
            let gap = edge * 2;
            canvas.round_outline(x - gap, y - gap, w + 2 * gap, h + 2 * gap, radius + gap, edge, TEXT_COLOR);
        }
    }

    /// The bar above the grid's footer that shows where among the pages the
    /// one on show is, when there is more than one page.
    fn grid_bar(&self) -> Option<PageBar> {
        let (width, height) = self.window_size();
        let layout = self.grid_layout((width, height));
        let pages = self.list.len().div_ceil(layout.per_page());
        let thick = (ui_px(height) as i32 / 3).max(4);
        let track = width as i32 / 3;
        let y = height as i32 - GridLayout::footer_height(height) - thick;
        (self.grid.is_some() && pages > 1).then_some(PageBar { x: (width as i32 - track) / 2, y, width: track, height: thick, pages })
    }

    /// Goes to the page at a spot along the page bar, keeping the cursor in
    /// the cell it was in.
    fn drag_pages(&mut self, x: f64) {
        let (Some(bar), Some(grid)) = (self.grid_bar(), &self.grid) else { return };
        let per_page = self.grid_layout(self.window_size()).per_page();
        let along = ((x - f64::from(bar.x)) / f64::from(bar.width)).clamp(0.0, 1.0);
        let page = ((along * bar.pages as f64) as usize).min(bar.pages - 1);
        let target = (page * per_page + grid.cursor % per_page).min(self.list.len() - 1);
        let by = target as i64 - grid.cursor as i64;
        if by != 0 {
            self.move_cursor(by);
        }
    }

    /// The round button at the grid's top right that leaves it.
    fn grid_close(&self) -> Option<Button> {
        self.grid.as_ref()?;
        let (width, height) = self.window_size();
        let size = ui_px(height) as i32 * 2;
        // In from the corner a little, both ways.
        Some(Button { x: width as i32 - size - size, y: size, width: size, height: size, command: Command::ToggleGrid })
    }


    /// The round buttons at the grid's left and right edges, for the page
    /// before and the page after: where each is, and whether it goes
    /// forward. Only those with a page to go to.
    fn grid_arrows(&self) -> Vec<(Button, bool)> {
        let Some(grid) = &self.grid else { return Vec::new() };
        let (width, height) = self.window_size();
        let layout = self.grid_layout((width, height));
        let pages = self.list.len().div_ceil(layout.per_page());
        let size = ui_px(height) as i32 * 3;
        let y = (height as i32 - GridLayout::footer_height(height) - size) / 2;
        let button = |x: i32| Button { x, y, width: size, height: size, command: Command::Next };
        let mut arrows = Vec::new();
        if grid.target >= 1.0 {
            arrows.push((button(size / 4), false));
        }
        if (grid.target as usize) + 1 < pages {
            arrows.push((button(width as i32 - size - size / 4), true));
        }
        arrows
    }

    /// The words on the grid: under each picture its title, when those are
    /// on, and in the corner of each video how long it is.
    fn draw_grid_labels(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let Some(cursor) = self.grid.as_ref().filter(|_| self.overview.is_none()).map(|grid| grid.cursor) else { return };
        let layout = self.grid_layout((canvas.width as u32, canvas.height as u32));
        let line = Text::line_height(px);
        for pos in self.grid_shown(&layout) {
            let id = self.list[pos];
            let area = self.grid_area(&layout, pos, canvas.height as u32);
            let size = self.loader.thumb(id).map(|thumb| (thumb.width, thumb.height));
            let [x, y, w, h] = size.map_or(area, |size| Self::grid_picture(area, size, false));
            // A dot in the corner of a video not watched yet.
            let new = self.new_level(id);
            if new > 0.0 {
                let (dot, inset) = ((px as i32 * 2 / 3).max(6), (px as i32 / 2).max(4));
                canvas.round_fill(x + inset - 2, y + inset - 2, dot + 4, dot + 4, dot / 2 + 2, 0x000000, (150.0 * new) as u32);
                canvas.round_fill(x + inset, y + inset, dot, dot, dot / 2, accent_soft(), (256.0 * new) as u32);
            }
            // What a triage has decided about it, in the other corner: a
            // tick on green for kept, a cross on red for rejected.
            if let Some(verdict) = self.verdict(id) {
                let (big, inset) = (px * 5 / 4, (px as i32 / 2).max(4));
                let (icon, color) = if verdict == Verdict::Kept { (commands::icons::CHECK_FILLED, KEPT_COLOR) } else { (commands::icons::X_FILLED, DANGER_COLOR) };
                let side = text.width(icon, big);
                let (left, top) = (x + w - side - inset, y + inset);
                // Dark behind it, so that the tick reads on any picture.
                canvas.round_fill(left + side / 8, top + side / 8, side - side / 4, side - side / 4, side / 2, 0x000000, 256);
                text.draw(canvas, left, top + (side - Text::line_height(big)) / 2, big, color, icon);
            }
            self.want_duration(id);
            if let Some(length) = self.durations.get(&id).copied().flatten().map(clock) {
                let pad = px as i32 / 3;
                let (badge_w, badge_h) = (text.width(&length, px) + 2 * pad, line + pad / 2);
                let (badge_x, badge_y) = (x + w - badge_w - pad, y + h - badge_h - pad);
                canvas.round_fill(badge_x, badge_y, badge_w, badge_h, pad, 0x000000, 190);
                text.draw(canvas, badge_x + pad, badge_y + pad / 4, px, TEXT_COLOR, &length);
            }
            if self.grid_titles {
                let title = clipped(text, &title_of(&self.files[id]), area[2], px);
                let left = area[0] + (area[2] - text.width(&title, px)) / 2;
                let color = if pos == cursor { TEXT_COLOR } else { DIM_TEXT_COLOR };
                text.draw(canvas, left, area[1] + area[3] + layout.pad / 2, px, color, &title);
            }
        }
        // A bar above the footer shows where among the pages this one is.
        if let (Some(grid), Some(bar)) = (&self.grid, self.grid_bar()) {
            let thumb = (bar.width / bar.pages as i32).max(px as i32 * 2).min(bar.width);
            let along = (grid.scroll() / (bar.pages - 1) as f64).clamp(0.0, 1.0);
            let thumb_left = bar.x + (f64::from(bar.width - thumb) * along).round() as i32;
            // Brighter with the pointer on it, or while it is dragged.
            let held = self.drag.as_ref().is_some_and(|drag| matches!(drag, Drag::Pages)) || bar.holds(self.mouse);
            canvas.round_fill(bar.x, bar.y, bar.width, bar.height, bar.height / 2, TEXT_COLOR, if held { 70 } else { 40 });
            canvas.round_fill(thumb_left, bar.y, thumb, bar.height, bar.height / 2, if held { accent_soft() } else { TEXT_COLOR }, if held { 256 } else { 200 });
        }
        // A cross at the top right leaves the grid.
        if let Some(place) = self.grid_close() {
            let over = (place.x..place.x + place.width).contains(&(self.mouse.0 as i32)) && (place.y..place.y + place.height).contains(&(self.mouse.1 as i32));
            canvas.round_fill(place.x, place.y, place.width, place.height, place.width / 2, if over { accent() } else { 0x000000 }, if over { 256 } else { 170 });
            canvas.round_outline(place.x, place.y, place.width, place.height, place.width / 2, 1, MENU_EDGE_COLOR);
            let cross = commands::icons::X;
            let inset = ((place.width - text.width(cross, px)) / 2, (place.height - Text::line_height(px)) / 2);
            text.draw(canvas, place.x + inset.0, place.y + inset.1, px, if over { on_accent() } else { TEXT_COLOR }, cross);
        }
        // The buttons for the pages either way, brighter under the pointer.
        let (mouse_x, mouse_y) = (self.mouse.0 as i32, self.mouse.1 as i32);
        for (place, forward) in self.grid_arrows() {
            let over = (place.x..place.x + place.width).contains(&mouse_x) && (place.y..place.y + place.height).contains(&mouse_y);
            canvas.round_fill(place.x, place.y, place.width, place.height, place.width / 2, if over { accent() } else { 0x000000 }, if over { 256 } else { 170 });
            canvas.round_outline(place.x, place.y, place.width, place.height, place.width / 2, 1, MENU_EDGE_COLOR);
            let (arrow, big) = (if forward { commands::icons::CHEVRON_RIGHT } else { commands::icons::CHEVRON_LEFT }, px * 3 / 2);
            let inset = ((place.width - text.width(arrow, big)) / 2, (place.height - Text::line_height(big)) / 2);
            text.draw(canvas, place.x + inset.0, place.y + inset.1, big, if over { on_accent() } else { TEXT_COLOR }, arrow);
        }
    }

    /// Draws the line under the grid. While images are marked it also has
    /// the count and a row of buttons for acting on them: each an icon,
    /// what it does, and its key.
    fn draw_grid_footer(&mut self, canvas: &mut Canvas, text: &mut Text, footer: &[String], px: u32) {
        self.grid_buttons.clear();
        let Some(label) = footer.first() else { return };
        let (pad, gap) = (px as i32 / 2, px as i32 * 2);
        let height = Text::line_height(px) + pad;
        let actions = if self.marks.is_empty() { &[][..] } else { commands::GRID_ACTIONS };
        let count = format!("{} marked", self.marks.len());
        let small = (px * 5 / 6).max(11);

        // A button's parts: the icon, what it does, and the key for it.
        let parts = |text: &mut Text, label: &'static str, command: Command| {
            let (name, key) = label.split_once("  ").unwrap_or((label, ""));
            let icon = commands::command_icon(command);
            let widths = (text.width(icon, px), text.width(name, px), text.width(key, small));
            (icon, name, key, widths, 2 * pad + widths.0 + pad / 2 + widths.1 + pad + widths.2)
        };
        let mut width = text.width(label, px);
        if !actions.is_empty() {
            width += gap + text.width(&count, px) + gap - pad;
            width += actions.iter().map(|&(label, command)| parts(text, label, command).4 + pad).sum::<i32>();
        }
        let footer_height = GridLayout::footer_height(canvas.height as u32);
        // Nothing of the pictures shows behind this line.
        canvas.fill(0, canvas.height - footer_height, canvas.width, footer_height, GRID_COLOR);
        // At its left, the video that plays on while the grid is up.
        let taken = self.draw_now_playing(canvas, text, px, (canvas.width - width) / 2 - px as i32);
        // In a narrow window that would run under it: then it starts beside
        // it, and is cut short at the window's edge.
        let mut x = ((canvas.width - width) / 2).max(taken);
        let label = &clipped(text, label, canvas.width - x - pad, px);
        let y = canvas.height - footer_height + (footer_height - height) / 2;

        text.draw(canvas, x, y + pad / 2, px, TEXT_COLOR, label);
        if actions.is_empty() {
            return;
        }
        x += text.width(label, px) + gap;
        text.draw(canvas, x, y + pad / 2, px, MARK_COLOR, &count);
        x += text.width(&count, px) + gap;
        let (mouse_x, mouse_y) = (self.mouse.0 as i32, self.mouse.1 as i32);
        for &(label, command) in actions {
            let (icon, name, key, widths, width) = parts(text, label, command);
            let over = (x..x + width).contains(&mouse_x) && (y..y + height).contains(&mouse_y);
            // The one that cannot be taken back is marked as such.
            let danger = command == Command::Delete;
            let fill = if over && danger { DANGER_COLOR } else if over { accent() } else { PLACEHOLDER_COLOR };
            canvas.round_fill(x, y, width, height, height / 2, fill, 256);
            canvas.round_outline(x, y, width, height, height / 2, 1, if over { fill } else { MENU_EDGE_COLOR });
            let mut at = x + pad;
            let (main, dim) = row_text(over && !danger);
            text.draw(canvas, at, y + pad / 2, px, if danger && !over { DANGER_COLOR } else { main }, icon);
            at += widths.0 + pad / 2;
            text.draw(canvas, at, y + pad / 2, px, main, name);
            at += widths.1 + pad;
            let key_top = y + (height - Text::line_height(small)) / 2;
            text.draw(canvas, at, key_top, small, if over && danger { TEXT_COLOR } else { dim }, key);
            self.grid_buttons.push(Button { x, y, width, height, command });
            x += width + pad;
        }
    }

    /// The line under the grid: the image under the cursor and the page.
    fn grid_footer(&self) -> Option<String> {
        let grid = self.grid.as_ref().filter(|_| self.overview.is_none())?;
        let path = &self.files[self.list[grid.cursor]];
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
        let layout = self.grid_layout(self.window_size());
        let pages = self.list.len().div_ceil(layout.per_page()).max(1);
        let mut footer = format!("{name}   [{}/{}]", grid.cursor + 1, self.list.len());
        if pages > 1 {
            footer.push_str(&format!("      Page {} of {pages}", grid.cursor / layout.per_page() + 1));
        }
        Some(footer)
    }

    /// Whether a file is an Ultra HDR photo, found from its first bytes
    /// once and remembered.
    fn is_ultra_hdr(&self, id: usize) -> bool {
        if let Some(known) = self.ultra_hdr.borrow().get(&id) {
            return *known;
        }
        let path = &self.files[id];
        // The thread that decoded the file looked, and reading the file
        // here would hold up the frame; only a file not decoded yet is read.
        let found = match self.loader.sniffed(id) {
            Some((_, ultra_hdr)) => ultra_hdr,
            None => {
                files::is_jpeg(path) && {
                    use std::io::Read;
                    let mut head = Vec::new();
                    std::fs::File::open(path).and_then(|file| file.take(256 << 10).read_to_end(&mut head)).is_ok() && ultrahdr::is_ultra_hdr(&head)
                }
            }
        };
        self.ultra_hdr.borrow_mut().insert(id, found);
        found
    }

    /// An image's format, found from its first bytes once and remembered.
    fn format_of(&self, id: usize) -> Option<String> {
        if let Some(known) = self.formats.borrow().get(&id) {
            return known.clone();
        }
        let path = &self.files[id];
        // As the thread that decoded the file found it, when it has; a
        // RAW file's name is from its ending, with nothing read.
        let found = match self.loader.sniffed(id).filter(|_| !files::is_raw(path)) {
            Some((format, _)) => format.map(str::to_string),
            None => files::format_name(path),
        };
        self.formats.borrow_mut().insert(id, found.clone());
        found
    }

    /// The codec of the video on view, as it is usually written.
    fn codec_name(&self) -> Option<String> {
        let codec = self.player.as_ref()?.text("video-format")?;
        Some(match codec.as_str() {
            "hevc" => "HEVC".to_string(),
            "h264" => "H.264".to_string(),
            "av1" => "AV1".to_string(),
            "vp9" => "VP9".to_string(),
            "vp8" => "VP8".to_string(),
            "mpeg2video" => "MPEG-2".to_string(),
            other => other.to_uppercase(),
        })
    }

    /// The info overlay's second line: the dimensions, apart so that they
    /// can be clicked, then the zoom and the size on disk.
    fn info_line(&mut self) -> (Option<String>, String) {
        let size = self.current_meta().and_then(|meta| meta.size).map(meta::human_size);
        let Some(native) = self.native() else { return (None, size.unwrap_or_default()) };
        let percent = self.view.scale(native, self.stage().1) * 100.0;
        let mut line = format!("   {percent:.0}%");
        if let Some(size) = size {
            line.push_str(&format!("   {size}"));
        }
        (Some(format!("{} x {}", native.0, native.1)), line)
    }

    /// The debug overlay, as groups of figures each with a name: what the
    /// player reports while a video is showing, otherwise what decoding
    /// and drawing the image cost.
    fn debug_groups(&mut self) -> Vec<Group> {
        let perf = &self.perf;
        let mut drawing = vec![("paint", ms(perf.paint)), ("present", ms(perf.present))];
        // What the graphs under the overlay cost, counting and drawing.
        if self.scopes.contains(&true) {
            drawing.push(("graphs", ms(self.scope_cost)));
        }
        if self.video.as_ref().is_some_and(|video| video.showing) {
            // Asking the player on every frame would get in its way, and
            // the figures would change too fast to read.
            let fresh = perf.video_lines.as_ref().is_some_and(|(at, _)| at.elapsed() < VIDEO_STATS_EVERY);
            if !fresh {
                let groups = self.video_debug_groups();
                self.perf.video_lines = Some((Instant::now(), groups));
            }
            let perf = &self.perf;
            let mut groups = perf.video_lines.as_ref().map(|(_, groups)| groups.clone()).unwrap_or_default();
            // The player holds a frame back until it is due, so only a
            // paused video shows what drawing one costs by itself.
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
            drawing.insert(0, (if paused { "render" } else { "render + wait" }, ms(perf.render)));
            if let Some(rate) = perf.rate() {
                drawing.push(("drawn", format!("{rate:.1} fps")));
            }
            groups.push(("Drawing", drawing));
            return groups;
        }

        let stats = self.loader.stats(self.id());
        let mut groups = Vec::new();
        if let (Some(made), Some(native)) = (stats.fitted, self.native()) {
            let megapixels = f64::from(native.0) * f64::from(native.1) / 1e6;
            let mut decode = Vec::new();
            if made.how.is_empty() {
                let rate = megapixels / made.read.as_secs_f64().max(1e-6);
                decode.push(("total", ms(made.read + made.fit)));
                decode.push(("read", ms(made.read)));
                decode.push(("fit", ms(made.fit)));
                decode.push(("speed", format!("{rate:.0} MP/s")));
            } else {
                decode.push(("total", format!("{} ({})", ms(made.fit), made.how)));
            }
            decode.push(("size", format!("{megapixels:.1} MP")));
            // Decoded ahead as a neighbour, or only once it was gone to.
            decode.push(("when", if made.at <= perf.arrived { "prefetched" } else { "on demand" }.to_string()));
            if let Some(full) = stats.full {
                let how = if full.how.is_empty() { String::new() } else { format!(" ({})", full.how) };
                decode.push(("full size", format!("{}{how}", ms(full.read + full.fit))));
            }
            groups.push(("Decode", decode));
        }
        if let Some(scaled) = &perf.scaled {
            let (from, to) = (scaled.from, scaled.to);
            drawing.insert(0, ("scale", format!("{} {}", ms(scaled.took), if scaled.smooth { "smooth" } else { "nearest" })));
            drawing.insert(1, ("from", format!("{} x {}", from.0, from.1)));
            drawing.insert(2, ("to", format!("{} x {}", to.0, to.1)));
        }
        if let Some(took) = perf.shown_in {
            drawing.push(("shown in", ms(took)));
        }
        groups.push(("Drawing", drawing));
        let kept = |(count, bytes): (usize, usize)| format!("{count} ({})", meta::human_size(bytes as u64));
        let memory = vec![("frames", kept(stats.fitted_kept)), ("thumbnails", kept(stats.thumbs_kept)), ("decoding", stats.busy.to_string())];
        groups.push(("Memory", memory));
        groups
    }

    /// What the player says about the video: the file, then how playback
    /// is keeping up.
    fn video_debug_groups(&self) -> Vec<Group> {
        let (Some(player), Some(video)) = (&self.player, &self.video) else { return Vec::new() };
        let number = |property| player.number(property);
        let mut file = Vec::new();
        file.push(("codec", player.text("video-format").unwrap_or_default()));
        if let Some((w, h)) = number("width").zip(number("height")) {
            file.push(("size", format!("{w:.0} x {h:.0}")));
        }
        if let Some(format) = player.text("video-params/pixelformat") {
            file.push(("pixels", format));
        }
        if let Some(rate) = number("container-fps") {
            file.push(("rate", format!("{rate:.2} fps")));
        }
        if let Some(bits) = number("video-bitrate").filter(|bits| *bits > 0.0) {
            file.push(("bitrate", format!("{:.1} Mbps", bits / 1e6)));
        }
        let decoder = match player.text("hwdec-current").as_deref() {
            None | Some("no" | "") => "CPU".to_string(),
            Some(decoder) => decoder.to_string(),
        };
        // The player names every format its reader for this one takes, as
        // `mov,mp4,m4a`: the file's own ending says which, when it is one.
        if let Some(formats) = player.text("file-format") {
            let ending = self.files[video.id].extension().and_then(|ending| ending.to_str()).map(str::to_lowercase);
            let named = formats.split(',').find(|format| Some(*format) == ending.as_deref());
            let container = named.or(formats.split(',').next()).unwrap_or_default().to_string();
            file.push(("container", container));
        }
        if let Some(length) = number("duration") {
            file.push(("length", clock(length)));
        }
        file.push(("decoder", decoder));
        if let Some(took) = video.start_took {
            file.push(("start", ms(took)));
        }

        let mut audio = Vec::new();
        match player.text("audio-codec-name") {
            Some(codec) => audio.push(("codec", codec)),
            None => audio.push(("codec", "none".to_string())),
        }
        if let Some(channels) = number("audio-params/channel-count") {
            audio.push(("channels", format!("{channels:.0}")));
        }
        if let Some(rate) = number("audio-params/samplerate") {
            audio.push(("rate", format!("{:.1} kHz", rate / 1000.0)));
        }
        if let Some(bits) = number("audio-bitrate").filter(|bits| *bits > 0.0) {
            audio.push(("bitrate", format!("{:.0} kbps", bits / 1000.0)));
        }

        let mut playback = Vec::new();
        let count = |property| number(property).unwrap_or(0.0);
        playback.push(("late", format!("{:.0}", count("frame-drop-count"))));
        playback.push(("dropped", format!("{:.0}", count("vo-drop-frame-count"))));
        if let Some(sync) = number("avsync") {
            playback.push(("A-V", format!("{:+.0} ms", sync * 1000.0)));
        }
        if let Some(ahead) = number("demuxer-cache-duration") {
            playback.push(("read ahead", format!("{ahead:.1} s")));
        }
        if let Some(took) = video.seek_took {
            playback.push(("seek", ms(took)));
        }
        if self.speed != 1.0 {
            playback.push(("speed", format!("{}x", self.speed)));
        }
        vec![("Video", file), ("Audio", audio), ("Playback", playback)]
    }

    /// Details of the current file, or `None` while they are being read.
    /// Reading them means opening the file, which on a slow disk would
    /// hold up drawing, so it happens on another thread.
    fn current_meta(&mut self) -> Option<&Meta> {
        let id = self.id();
        if self.meta.as_ref().is_some_and(|(meta_id, _)| *meta_id == id) {
            return self.meta.as_ref().map(|(_, meta)| meta);
        }
        if self.meta_asked != Some(id) {
            self.meta_asked = Some(id);
            let (path, proxy) = (self.files[id].clone(), self.proxy.clone());
            std::thread::spawn(move || drop(proxy.send_event(Event::Meta(id, meta::read(&path)))));
        }
        None
    }

    /// Where in the window the filmstrip's cell for a list position is:
    /// its left edge. The file on view sits in the middle.
    /// The list position in the middle of the filmstrip: the file on view,
    /// unless the wheel has moved the strip along to look further afield.
    fn strip_centre(&self) -> i64 {
        (self.pos as i64 + self.strip_scroll).clamp(0, self.list.len() as i64 - 1)
    }

    /// The small round button above the filmstrip's right end that
    /// closes it.
    fn strip_close(&self) -> Option<Button> {
        let (width, height) = self.window_size();
        let strip = Strip::for_height(height);
        let size = ui_px(height) as i32 * 3 / 2;
        // Not while a triage has it up: it is part of that.
        let shown = self.grid.is_none() && self.strip_visible() && !self.triage_on;
        // In the corner of the strip itself: over a video the seek bar and
        // its buttons stand just above it.
        shown.then_some(Button { x: width as i32 - size - size / 2, y: strip.top + size / 3, width: size, height: size, command: Command::ToggleFilmstrip })
    }

    fn strip_left(&self, strip: &Strip, pos: usize, window_width: i32) -> i32 {
        let middle = window_width / 2 - strip.cell_w / 2;
        // The strip slides along to a new middle rather than jumping to it.
        let centre = self.glide(&self.strip_slide, self.strip_centre() as f64, EASE);
        middle + ((pos as f64 - centre) * f64::from(strip.pitch())).round() as i32
    }

    /// The list position of the filmstrip cell at a window position.
    fn strip_cell_at(&self, x: f64, y: f64) -> Option<usize> {
        let (width, height) = self.window_size();
        let strip = Strip::for_height(height);
        if self.grid.is_some() || !self.strip_visible() || y < f64::from(strip.top) {
            return None;
        }
        let first_cell = f64::from(width as i32 / 2 - strip.cell_w / 2 - strip.pad / 2);
        let pos = self.strip_centre() + ((x - first_cell) / f64::from(strip.pitch())).floor() as i64;
        (0..self.list.len() as i64).contains(&pos).then_some(pos as usize)
    }

    /// A thumbnail at the size a filmstrip cell takes it, made once and
    /// kept; the thumbnail as it is when the cell is no smaller.
    fn strip_thumb(&self, id: usize, thumb: &Arc<Frame>, (w, h): (u32, u32)) -> Arc<Frame> {
        if w >= thumb.width && h >= thumb.height {
            return thumb.clone();
        }
        let mut kept = self.strip_thumbs.borrow_mut();
        // The one kept is for this very thumbnail, not one since redone.
        if let Some((_, small)) = kept.get(&(id, w, h)).filter(|(from, _)| Arc::ptr_eq(from, thumb)) {
            return small.clone();
        }
        // A couple of hundred cells' worth is some megabytes; past that it
        // is started afresh rather than grown without end.
        if kept.len() >= 200 {
            kept.clear();
        }
        let small = Arc::new(crate::decode::shrink(thumb, (w, h)).unwrap_or_else(|_| Frame::still(thumb.width, thumb.height, thumb.native, thumb.pixels.clone())));
        kept.insert((id, w, h), (thumb.clone(), small.clone()));
        small
    }

    fn draw_strip(&self, canvas: &mut Canvas, strip: Strip) {
        canvas.shade(0, strip.top, canvas.width, strip.height, 64);
        let reach = i64::from(canvas.width / 2 / strip.pitch() + 1);
        let top = strip.top + strip.pad;
        let round = (strip.pad * 2 / 3).max(3);
        let edge = (strip.pad / 4).max(2);

        for offset in -reach..=reach {
            let pos = self.strip_centre() + offset;
            if !(0..self.list.len() as i64).contains(&pos) {
                continue;
            }
            let pos = pos as usize;
            let left = self.strip_left(&strip, pos, canvas.width);
            // Where the picture goes in its cell, for what is drawn round it.
            let [x, y, w, h] = match self.loader.thumb(self.list[pos]) {
                Some(thumb) => {
                    let bounds = (strip.cell_w as u32, strip.cell_h as u32);
                    let (w, h) = fit_size((thumb.width, thumb.height), bounds, false);
                    let small = self.strip_thumb(self.list[pos], &thumb, (w, h));
                    let (w, h) = (w as i32, h as i32);
                    let place = [left + (strip.cell_w - w) / 2, top + (strip.cell_h - h) / 2, w, h];
                    canvas.image_rounded(&small, place[0], place[1], w, h, round);
                    place
                }
                None => {
                    canvas.round_fill(left, top, strip.cell_w, strip.cell_h, round, PLACEHOLDER_COLOR, 256);
                    [left, top, strip.cell_w, strip.cell_h]
                }
            };
            if self.marks.contains(&self.list[pos]) {
                let dot = strip.pad;
                canvas.round_fill(x + dot / 3, y + dot / 3, dot, dot, dot / 2, MARK_COLOR, 256);
            }
            // The file on view is ringed brightly, the one under the
            // pointer quietly.
            let ring = if pos == self.pos { Some(TEXT_COLOR) } else { (self.strip_over == Some(pos)).then_some(accent_soft()) };
            if let Some(color) = ring {
                canvas.round_outline(x - edge, y - edge, w + 2 * edge, h + 2 * edge, round + edge, edge, color);
            }
        }
    }

    /// The cross in the corner of the filmstrip, which closes it.
    fn draw_strip_close(&self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let Some(place) = self.strip_close() else { return };
        let over = (place.x..place.x + place.width).contains(&(self.mouse.0 as i32)) && (place.y..place.y + place.height).contains(&(self.mouse.1 as i32));
        canvas.round_fill(place.x, place.y, place.width, place.height, place.width / 2, if over { accent() } else { 0x000000 }, if over { 256 } else { 170 });
        let (cross, small) = (commands::icons::X, (px * 5 / 6).max(11));
        let inset = ((place.width - text.width(cross, small)) / 2, (place.height - Text::line_height(small)) / 2);
        text.draw(canvas, place.x + inset.0, place.y + inset.1, small, if over { on_accent() } else { TEXT_COLOR }, cross);
    }

    /// In the corner of each video in the filmstrip, how long it is, as the
    /// grid has it.
    fn draw_strip_lengths(&mut self, canvas: &mut Canvas, text: &mut Text, strip: &Strip, px: u32) {
        let small = (px * 3 / 4).max(10);
        let reach = i64::from(canvas.width / 2 / strip.pitch() + 1);
        let (top, pad) = (strip.top + strip.pad, (small as i32 / 4).max(2));
        for offset in -reach..=reach {
            let pos = self.strip_centre() + offset;
            if !(0..self.list.len() as i64).contains(&pos) {
                continue;
            }
            let id = self.list[pos as usize];
            // A dot in the corner of a video not watched yet.
            let new = self.new_level(id);
            if new > 0.0 {
                let (dot, inset) = ((small as i32 / 2).max(5), (small as i32 / 3).max(3));
                let left = self.strip_left(strip, pos as usize, canvas.width) + (strip.cell_w - self.loader.thumb(id).map_or(strip.cell_w, |thumb| {
                    fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false).0 as i32
                })) / 2;
                canvas.round_fill(left + inset - 1, top + inset - 1, dot + 2, dot + 2, dot / 2 + 1, 0x000000, (150.0 * new) as u32);
                canvas.round_fill(left + inset, top + inset, dot, dot, dot / 2, accent_soft(), (256.0 * new) as u32);
            }
            // What a triage has decided about it, in its top right corner:
            // a tick or a cross on a filled circle, as in the grid.
            if let Some(verdict) = self.verdict(id) {
                let (icon, color) = if verdict == Verdict::Kept { (commands::icons::CHECK_FILLED, KEPT_COLOR) } else { (commands::icons::X_FILLED, DANGER_COLOR) };
                let across = self.loader.thumb(id).map_or(strip.cell_w, |thumb| fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false).0 as i32);
                let down = self.loader.thumb(id).map_or(strip.cell_h, |thumb| fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false).1 as i32);
                let (side, inset) = (text.width(icon, px), (small as i32 / 3).max(3));
                let left = self.strip_left(strip, pos as usize, canvas.width) + (strip.cell_w + across) / 2 - side - inset;
                let badge_top = top + (strip.cell_h - down) / 2 + inset;
                // Dark behind it, so that the tick reads on any picture.
                canvas.round_fill(left + side / 8, badge_top + side / 8, side - side / 4, side - side / 4, side / 2, 0x000000, 256);
                text.draw(canvas, left, badge_top + (side - Text::line_height(px)) / 2, px, color, icon);
            }
            self.want_duration(id);
            let Some(length) = self.durations.get(&id).copied().flatten().map(clock) else { continue };
            // The corner of the picture, which may be narrower than its cell.
            let left = self.strip_left(strip, pos as usize, canvas.width);
            let size = self.loader.thumb(id).map_or((strip.cell_w as u32, strip.cell_h as u32), |thumb| {
                fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false)
            });
            let (w, h) = (size.0 as i32, size.1 as i32);
            let (right, bottom) = (left + (strip.cell_w + w) / 2, top + (strip.cell_h + h) / 2);
            let (badge_w, badge_h) = (text.width(&length, small) + 2 * pad, Text::line_height(small));
            let (badge_x, badge_y) = (right - badge_w - pad, bottom - badge_h - pad);
            canvas.round_fill(badge_x, badge_y, badge_w, badge_h, pad, 0x000000, 190);
            text.draw(canvas, badge_x + pad, badge_y, small, TEXT_COLOR, &length);
        }
    }

    /// Over the filmstrip cell under the pointer: what the file is called
    /// and where in the list it is.
    fn draw_strip_tip(&mut self, canvas: &mut Canvas, text: &mut Text, strip: &Strip, px: u32) {
        let Some(pos) = self.strip_over else { return };
        let id = self.list[pos];
        let tip = format!("{}   {}/{}", title_of(&self.files[id]), pos + 1, self.list.len());
        let pad = px as i32 / 2;
        let tip = clipped(text, &tip, canvas.width - 4 * pad, px);
        let (width, height) = (text.width(&tip, px) + 2 * pad, Text::line_height(px) + pad);
        let centre = self.strip_left(strip, pos, canvas.width) + strip.cell_w / 2;
        let left = (centre - width / 2).clamp(pad, (canvas.width - width - pad).max(pad));
        let top = strip.top - height - pad / 2;
        panel(canvas, left, top, width, height, pad);
        text.draw(canvas, left + pad, top + pad / 2, px, TEXT_COLOR, &tip);
    }

    /// Where the file list goes, when it is open: down the left side, above
    /// the filmstrip, with the file on view kept in the middle.
    fn list_panel(&self) -> Option<ListPanel> {
        if !self.show_list || self.grid.is_some() {
            return None;
        }
        let (width, height) = self.window_size();
        let px = ui_px(height);
        let bottom = if self.band_shown() { Strip::for_height(height).top } else { height as i32 };
        let row_height = Text::line_height(px) + px as i32 / 2;
        // Above the rows: a heading, and under it the buttons for the order.
        let top = Text::line_height(px) * 2 + px as i32 * 3;
        let rows = (((bottom - top) / row_height).max(1) as usize).min(self.list.len());
        let scrolled = self.list_scroll.filter(|(pos, len, _)| (*pos, *len) == (self.pos, self.list.len()));
        let first = scrolled.map_or(self.pos.saturating_sub(rows / 2), |(_, _, first)| first).min(self.list.len() - rows);
        let panel_width = (px as i32 * 30).min(width as i32 / 2);
        let left = if self.list_right { width as i32 - panel_width } else { 0 };
        Some(ListPanel { left, width: panel_width, bottom, top, row_height, first, rows, px })
    }

    /// Asks for the length of a video, once; `Event::Duration` brings it.
    /// One thread asks ffprobe about each in turn.
    fn want_duration(&mut self, id: usize) {
        if !files::is_video(&self.files[id]) || self.durations.contains_key(&id) {
            return;
        }
        self.durations.insert(id, None);
        let sender = self.prober.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel::<(usize, PathBuf)>();
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                for (id, path) in receiver {
                    let arguments = ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"];
                    let output = std::process::Command::new("ffprobe").args(arguments).arg(&path).output();
                    let seconds = output.ok().and_then(|output| String::from_utf8_lossy(&output.stdout).trim().parse().ok());
                    if proxy.send_event(Event::Duration(id, seconds)).is_err() {
                        return;
                    }
                }
            });
            sender
        });
        let _ = sender.send((id, self.files[id].clone()));
    }

    /// The video whose frames the go-to box lays out: the match picked out.
    fn story_subject(&self) -> Option<usize> {
        let palette = self.palette.as_ref()?;
        let id = *palette.matches.get(palette.selected).filter(|_| palette.commands.is_empty())?;
        files::is_video(&self.files[id]).then_some(id)
    }

    /// What a trailer is played of: in the grid, the video under its cursor.
    fn trailer_subject(&self) -> Option<usize> {
        // With the strip of scenes up, the scene under the pointer: it goes
        // by a number no file has, counted down from the largest there is.
        if self.scenes_shown() {
            return self.scene_at(self.mouse.0, self.mouse.1).map(|index| usize::MAX - index);
        }
        let grid = self.grid.as_ref().filter(|grid| grid.moved && self.palette.is_none() && self.overview.is_none())?;
        let id = self.list[grid.cursor];
        // Not of the one that is playing in its cell as it is.
        let playing = self.pip && self.video.as_ref().is_some_and(|video| video.id == id);
        (files::is_video(&self.files[id]) && !playing).then_some(id)
    }

    /// Has the trailer thread play the video under the grid's cursor, and
    /// stop when the cursor leaves it. Called on each redraw. Nothing here
    /// waits on the player: the thread does all of that.
    /// With `--verbose` or `--debug`, says on the terminal what has become
    /// of a trailer: waited for, skipped, started, showing or stopped.
    fn trailer_note(&self, what: &str, id: usize, more: &str) {
        if self.options.verbose || self.options.debug {
            // A scene of the video on view goes by its place in the strip.
            let name = if id < self.files.len() { self.name_of(id) } else { format!("scene {}", usize::MAX - id + 1) };
            eprintln!("jog: [trailer] {what:<8} {name}{more}");
        }
    }

    fn tend_trailer(&mut self) {
        let subject = self.trailer_subject();
        if self.trailer.as_ref().map(|trailer| trailer.id) == subject {
            self.trailer_due = None;
            return;
        }
        if let Some(was) = self.trailer.take() {
            if let Some(trailers) = &self.trailers {
                trailers.stop();
            }
            self.trailer_note("stopped", was.id, "");
        }
        // The cursor left a video it had not rested on long enough.
        if let Some((due, _)) = self.trailer_due.filter(|(due, _)| Some(*due) != subject) {
            self.trailer_note("skipped", due, " (the cursor moved on)");
        }
        let Some(id) = subject else { return self.trailer_due = None };
        // Not for a video the cursor is only passing over: opening one
        // is work, and a trailer flashing up for a moment is a nuisance.
        match self.trailer_due {
            Some((due, at)) if due == id && at <= Instant::now() => self.trailer_due = None,
            Some((due, _)) if due == id => return,
            _ => {
                self.trailer_note("waiting", id, &format!(" {} for the cursor to rest", ms(TRAILER_DELAY)));
                return self.trailer_due = Some((id, Instant::now() + TRAILER_DELAY));
            }
        }
        let proxy = self.proxy.clone();
        let window = self.window.as_ref().map_or((0, 0), |w| w.inner_size().into());
        // A scene of the video on view plays on from where it starts, in
        // its cell of the strip; a video in the grid as a trailer of the
        // whole of it, in its cell there.
        let scene = self.scenes_now().and_then(|(_, starts, _)| starts.get(usize::MAX - id).copied()).zip(self.video.as_ref().map(|video| video.id));
        let (file, bounds, from) = match scene {
            Some((start, file)) => {
                let strip = Strip::for_height(window.1);
                (file, (strip.cell_w as u32, strip.cell_h as u32), Some(start))
            }
            None if id < self.files.len() => (id, self.grid_layout(window).thumb_size(), None),
            None => return,
        };
        let trailers = self.trailers.get_or_insert_with(|| Trailers::new(move |note| proxy.send_event(Event::Trailer(note)).is_ok()));
        trailers.play(id, self.files[file].clone(), bounds, from);
        self.trailer = Some(Trailer { id, frame: None, since: None, asked: Instant::now() });
        self.trailer_note("started", id, "");
    }

    /// Sees to it that the frames of the video picked out in the go-to box
    /// are on their way. Called on each redraw.
    fn tend_story(&mut self) {
        let Some(id) = self.story_subject() else { return };
        if !self.story_frames.contains_key(&id) {
            // Frames are kept for the last few videos only.
            self.story_order.push_back(id);
            if self.story_order.len() > STORIES_KEPT {
                let oldest = self.story_order.pop_front();
                oldest.map(|oldest| self.story_frames.remove(&oldest));
            }
            self.story_frames.insert(id, vec![None; story::SEGMENTS]);
        }
        let frames = &self.story_frames[&id];
        let missing: Vec<usize> = (0..story::SEGMENTS).filter(|index| frames[*index].is_none()).collect();
        if !missing.is_empty() && self.story_asked != Some(id) {
            self.story_asked = Some(id);
            let proxy = self.proxy.clone();
            let stories = self.stories.get_or_insert_with(|| Stories::new(move |found| proxy.send_event(Event::Story(found)).is_ok()));
            stories.want(id, self.files[id].clone(), missing);
        }
    }

    /// The list position of the file-list row at a window position.
    fn list_row_at(&self, x: f64, y: f64) -> Option<usize> {
        let panel = self.list_panel().filter(|panel| panel.holds(x, y))?;
        let row = (y as i32 - panel.top).div_euclid(panel.row_height);
        (0..panel.rows as i32).contains(&row).then(|| panel.first + row as usize)
    }


    /// The buttons at the right of the file list's heading: sort, shuffle,
    /// and close.
    fn list_buttons(&self) -> Vec<Button> {
        let Some(panel) = self.list_panel() else { return Vec::new() };
        let px = panel.px as i32;
        // The cross, at the right of the heading.
        let side = px * 3 / 2;
        let mut buttons = vec![Button { x: panel.width - side - px / 2, y: px / 2, width: side, height: side, command: Command::ToggleList }];
        // Beside it, the search: a button that opens the box to type in,
        // or, with the list narrowed, the one that widens it again.
        let beside = if self.list_searching() { Command::ShowAllHere } else { Command::ListSearch };
        buttons.push(Button { x: panel.width - 2 * side - px / 2 - px / 3, y: px / 2, width: side, height: side, command: beside });
        // Then the pin, which stands the list beside the picture and not
        // over it, and the one that moves it to the other side.
        for (at, command) in [(2, Command::ListPin), (3, Command::ListFlip)] {
            buttons.push(Button { x: panel.width - (at + 1) * side - px / 2 - at * px / 3, y: px / 2, width: side, height: side, command });
        }
        // The orders, sharing the width of the row under the heading.
        let orders = [Command::SortBy(Sort::Name), Command::SortBy(Sort::Newest), Command::SortBy(Sort::Smallest), Command::Shuffle, Command::ReverseOrder];
        let (gap, height) = (px / 3, Text::line_height(panel.px) + px / 4);
        let width = (panel.width - px - gap * (orders.len() as i32 - 1)) / orders.len() as i32;
        let top = panel.top - px / 2 - px / 2 - height;
        buttons.extend(orders.into_iter().enumerate().map(|(at, command)| Button { x: px / 2 + at as i32 * (width + gap), y: top, width, height, command }));
        buttons
    }

    /// The list position whose star, at the right end of its row, is at a
    /// window position.
    fn list_star_at(&self, x: f64, y: f64) -> Option<usize> {
        let panel = self.list_panel()?;
        let star = (panel.width - panel.px as i32 * 2..panel.width).contains(&(x as i32 - panel.left));
        self.list_row_at(x, y).filter(|_| star)
    }

    fn draw_list(&self, canvas: &mut Canvas, text: &mut Text, panel: &ListPanel) {
        let px = panel.px;
        let pad = px as i32 / 2;
        // The pointer, as measured from where the list stands.
        let mouse = (self.mouse.0 as i32 - panel.left, self.mouse.1 as i32);
        // Pinned, it is beside the picture and has a ground of its own;
        // otherwise it lies over the picture, which shows through darkly.
        match self.list_pinned {
            true => canvas.fill(0, 0, panel.width, panel.bottom, GRID_COLOR),
            false => canvas.shade(0, 0, panel.width, panel.bottom, 36),
        }
        // A line down the side that faces the picture.
        canvas.fill(if self.list_right { 0 } else { panel.width - 1 }, 0, 1, panel.bottom, MENU_EDGE_COLOR);

        // The heading: how many files, and what they are narrowed to, with
        // a cross at its right to close the list. Under it a row of
        // buttons for the order: the one in force filled in, like the
        // buttons under the grid in look, and lighting up under the pointer.
        let buttons = self.list_buttons();
        let small = (px * 5 / 6).max(11);
        match self.list_field() {
            // The search box: a magnifying glass, what is typed, and at its
            // right how many files that leaves of how many.
            Some([x, y, w, h]) => {
                let typing = self.list_search.as_ref();
                canvas.round_fill(x, y, w, h, h / 2, 0x111111, 256);
                let (edge, color) = if typing.is_some() { (2, accent()) } else { (1, MENU_EDGE_COLOR) };
                canvas.round_outline(x, y, w, h, h / 2, edge, color);
                let glass = commands::icons::SEARCH;
                let line = y + (h - Text::line_height(px)) / 2;
                text.draw(canvas, x + pad, line, px, DIM_TEXT_COLOR, glass);
                let left = x + pad + text.width(glass, px) + pad;
                let count = self.filter.as_ref().map(|(_, all)| format!("{} of {}", self.list.len(), all.len())).unwrap_or_default();
                let count_left = x + w - pad - text.width(&count, small);
                text.draw(canvas, count_left, y + (h - Text::line_height(small)) / 2, small, DIM_TEXT_COLOR, &count);
                let room = count_left - left - pad;
                match typing {
                    Some(field) => {
                        draw_selection(canvas, text, field, left, line, px);
                        let typed = clipped(text, &field.string(), room, px);
                        text.draw(canvas, left, line, px, TEXT_COLOR, &typed);
                        let before: String = field.text[..field.cursor].iter().collect();
                        let cursor = left + text.width(&before, px).min(room);
                        canvas.fill(cursor, line, (px as i32 / 10).max(2), Text::line_height(px), TEXT_COLOR);
                    }
                    None => {
                        let typed = self.filter.as_ref().map(|(typed, _)| typed.clone()).unwrap_or_default();
                        let typed = clipped(text, &typed, room, px);
                        text.draw(canvas, left, line, px, TEXT_COLOR, &typed);
                    }
                }
            }
            None => {
                let heading = format!("{} files", self.list.len());
                let heading = clipped(text, &heading, panel.width - px as i32 * 10, px);
                text.draw(canvas, pad * 2, px as i32 * 3 / 4, px, TEXT_COLOR, &heading);
            }
        }
        canvas.fill(pad, panel.top - pad / 2, panel.width - 2 * pad, 1, MENU_EDGE_COLOR);
        for button in &buttons {
            let over = (button.x..button.x + button.width).contains(&mouse.0) && (button.y..button.y + button.height).contains(&mouse.1);
            let on = self.is_checked(button.command);
            if matches!(button.command, Command::ToggleList | Command::ListSearch | Command::ShowAllHere | Command::ListPin | Command::ListFlip) {
                // The cross is a round button, as the grid's is.
                canvas.round_fill(button.x, button.y, button.width, button.height, button.width / 2, if over || on { accent() } else { PLACEHOLDER_COLOR }, 256);
                let cross = match button.command {
                    Command::ListSearch => commands::icons::SEARCH,
                    Command::ShowAllHere => commands::icons::LIST_X,
                    Command::ListPin => commands::icons::PIN,
                    Command::ListFlip => commands::icons::REPLACE,
                    _ => commands::icons::X,
                };
                let inset = ((button.width - text.width(cross, small)) / 2, (button.height - Text::line_height(small)) / 2);
                text.draw(canvas, button.x + inset.0, button.y + inset.1, small, if over { on_accent() } else { TEXT_COLOR }, cross);
                continue;
            }
            let lit = over || on;
            canvas.round_fill(button.x, button.y, button.width, button.height, button.height / 2, if lit { accent() } else { PLACEHOLDER_COLOR }, if over && !on { 200 } else { 256 });
            canvas.round_outline(button.x, button.y, button.width, button.height, button.height / 2, 1, if lit { accent() } else { MENU_EDGE_COLOR });
            let (icon, word) = match button.command {
                Command::SortBy(Sort::Newest) => (commands::icons::CLOCK, "Newest"),
                Command::SortBy(Sort::Smallest) => (commands::icons::WEIGHT, "Size"),
                Command::SortBy(_) => (commands::icons::A_TO_Z, "Name"),
                Command::Shuffle => (commands::icons::SHUFFLE, "Shuffle"),
                _ => (commands::icons::ARROW_UP_DOWN, "Reverse"),
            };
            // The word goes when there is no room for it beside the icon.
            let icon_width = text.width(icon, small);
            let word = if icon_width + pad / 2 + text.width(word, small) + pad <= button.width { word } else { "" };
            let content = icon_width + if word.is_empty() { 0 } else { pad / 2 + text.width(word, small) };
            let (left, top) = (button.x + (button.width - content) / 2, button.y + (button.height - Text::line_height(small)) / 2);
            let color = if lit { on_accent() } else { TEXT_COLOR };
            text.draw(canvas, left, top, small, color, icon);
            text.draw(canvas, left + icon_width + pad / 2, top, small, color, word);
        }


        let numbers = text.width(&self.list.len().to_string(), px);
        let star_room = px as i32 * 2;
        for row in 0..panel.rows {
            let pos = panel.first + row;
            let id = self.list[pos];
            let top = panel.top + row as i32 * panel.row_height;
            let (current, hovered) = (pos == self.pos, self.list_hover == Some(pos));
            // The row the arrows have picked out in a search is ringed and
            // lit, plainly enough to be found at a glance.
            let picked = self.list_cursor == Some(pos) || self.row_target == Some(pos);
            if current {
                canvas.round_fill(pad / 2, top, panel.width - pad, panel.row_height, pad, accent(), 256);
            } else if hovered {
                // It lights up over a moment rather than at once.
                if self.row_faded.replace(Some(pos)) != Some(pos) {
                    self.row_fade.set(Glide::at(0.0));
                }
                let lit = self.glide(&self.row_fade, 1.0, EASE);
                canvas.round_fill(pad / 2, top, panel.width - pad, panel.row_height, pad, TEXT_COLOR, (28.0 * lit) as u32);
            }
            if picked {
                if !current {
                    canvas.round_fill(pad / 2, top, panel.width - pad, panel.row_height, pad, TEXT_COLOR, 56);
                }
                canvas.round_outline(pad / 2, top, panel.width - pad, panel.row_height, pad, 2, if current { TEXT_COLOR } else { accent_soft() });
            }
            let line = top + (panel.row_height - Text::line_height(px)) / 2;
            let (main, dim) = row_text(current);
            // At the right end a star: lit on a marked file, and offered,
            // faintly, on the row under the pointer, where a click marks it.
            let marked = self.marks.contains(&id);
            if marked || hovered {
                let star = commands::icons::STAR;
                let color = if marked && !current { MARK_COLOR } else if marked { main } else { dim };
                let star_left = panel.width - star_room + (star_room - pad - text.width(star, px)) / 2;
                text.draw(canvas, star_left, line, px, color, star);
            }
            // A video's length sits before it, once it is known.
            let length = self.durations.get(&id).copied().flatten().map(clock).unwrap_or_default();
            let length_width = if length.is_empty() { 0 } else { text.width(&length, px) + pad };
            text.draw(canvas, panel.width - star_room - length_width, line, px, dim, &length);
            // Before that, what a triage has decided about it: a tick for
            // kept, a cross for rejected.
            let verdict = self.verdict(id);
            let verdict_width = if verdict.is_some() { text.width(commands::icons::CHECK_FILLED, px) + pad } else { 0 };
            if let Some(verdict) = verdict {
                let (icon, color) = if verdict == Verdict::Kept { (commands::icons::CHECK_FILLED, KEPT_COLOR) } else { (commands::icons::X_FILLED, DANGER_COLOR) };
                text.draw(canvas, panel.width - star_room - length_width - verdict_width, line, px, if current { main } else { color }, icon);
            }
            let length_width = length_width + verdict_width;
            // A dot at the left edge for a video not watched yet.
            let new = self.new_level(id);
            if new > 0.0 {
                let dot = (px as i32 / 3).max(4);
                canvas.round_fill(pad * 5 / 4 - dot / 2, top + (panel.row_height - dot) / 2, dot, dot, dot / 2, if current { main } else { accent_soft() }, (256.0 * new) as u32);
            }
            // Its number in the list, set against the right of a column
            // wide enough for the largest, and then its name.
            let number = (pos + 1).to_string();
            let number_left = pad * 2 + numbers - text.width(&number, px);
            text.draw(canvas, number_left, line, px, dim, &number);
            let name_left = pad * 2 + numbers + pad;
            let name = clipped(text, &self.name_of(id), panel.width - star_room - length_width - name_left - pad, px);
            text.draw(canvas, name_left, line, px, main, &name);
        }
    }

    fn draw_menu(&self, canvas: &mut Canvas, text: &mut Text, menu: &Menu) {
        let px = menu.px;
        let pad = px as i32 / 2;
        panel(canvas, menu.x, menu.y, menu.width, menu.height, pad);
        if let Some(title) = &menu.title {
            text.draw(canvas, menu.x + pad, menu.y + pad, px, DIM_TEXT_COLOR, title);
        }

        for (index, row) in menu.rows.iter().enumerate() {
            let top = menu.y + row.top;
            let Some(item) = row.item else {
                canvas.fill(menu.x + pad, top + row.height / 2, menu.width - 2 * pad, 1, MENU_EDGE_COLOR);
                continue;
            };
            if menu.selected == Some(index) {
                canvas.round_fill(menu.x + pad / 2, top, menu.width - pad, row.height, pad / 2, accent(), 256);
            }
            let line = top + (row.height - Text::line_height(px)) / 2;
            // In the gutter: a tick on what is switched on, otherwise the
            // item's own small picture.
            let (main, dim) = row_text(menu.selected == Some(index));
            let (sign, color) = if self.is_checked(item.command) { (commands::icons::CHECK, main) } else { (commands::icon(item), dim) };
            let gutter = px as i32 * 2;
            let sign_left = menu.x + pad + (gutter - pad - text.width(sign, px)) / 2;
            text.draw(canvas, sign_left, line, px, color, sign);
            text.draw(canvas, menu.x + pad + gutter, line, px, main, &self.menu_label(item));
            let keys_left = menu.x + menu.width - pad - text.width(item.keys, px);
            text.draw(canvas, keys_left, line, px, dim, item.keys);
        }
        if let Some(sub) = &menu.sub {
            self.draw_menu(canvas, text, sub);
        }
    }
}

/// The grid's page bar: where its track is and how many pages it spans.
struct PageBar {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    pages: usize,
}

impl PageBar {
    /// Whether a window position is on the bar, or near enough above and
    /// below it: the bar itself is too thin to aim at.
    fn holds(&self, (x, y): (f64, f64)) -> bool {
        let reach = self.height * 2;
        (self.x..self.x + self.width).contains(&(x as i32)) && (self.y - reach..self.y + self.height + reach).contains(&(y as i32))
    }
}

/// The file list's place and what it shows: `rows` names from list
/// position `first`, each `row_height` tall, starting `top` down.
struct ListPanel {
    /// Where it starts from the left of the window.
    left: i32,
    width: i32,
    bottom: i32,
    top: i32,
    row_height: i32,
    first: usize,
    rows: usize,
    px: u32,
}

impl ListPanel {
    /// Whether a window position is on the list.
    fn holds(&self, x: f64, y: f64) -> bool {
        (self.left..self.left + self.width).contains(&(x as i32)) && (y as i32) < self.bottom
    }
}

/// What to call a file under its picture: its name without the extension.
/// A name with a season and episode in it, such as
/// `Some.Show.S02E05.1080p.x265.mkv`, becomes `Some Show S02E05`: what
/// follows the episode is the release's small print.
fn title_of(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or(path.as_os_str()).to_string_lossy();
    let letters: Vec<char> = stem.chars().collect();
    let digits = |from: usize| letters[from.min(letters.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
    // `S`, digits, `E`, digits, not in the middle of a word.
    let episode = (0..letters.len()).find_map(|at| {
        let starts = matches!(letters[at], 'S' | 's') && (at == 0 || !letters[at - 1].is_alphanumeric());
        let season = digits(at + 1);
        let e = at + 1 + season;
        let marked = letters.get(e).is_some_and(|c| matches!(c, 'E' | 'e'));
        (starts && season > 0 && marked && digits(e + 1) > 0).then(|| e + 1 + digits(e + 1))
    });
    match episode {
        Some(end) => {
            let kept: String = letters[..end].iter().map(|c| if matches!(c, '.' | '_') { ' ' } else { *c }).collect();
            kept.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        None => stem.into_owned(),
    }
}

/// A folder as it is written in a menu: `~` for the home directory, and
/// only its ends when it is a long one.
/// An error about a file, short enough for a toast: the file's own folder
/// left out of the paths in it.
/// How far an encode is, and once it has gone on long enough to tell,
/// about how long it has left: "42%  ·  1:05 left".
fn progress_words(share: f64, began: Instant) -> String {
    let percent = (share * 100.0) as u32;
    let gone = began.elapsed().as_secs_f64();
    if share < 0.03 || gone < 2.0 {
        return format!("{percent}%");
    }
    let left = gone / share * (1.0 - share);
    format!("{percent}%  ·  {} left", clock(left.ceil()))
}

fn short_error(path: &Path, error: &str) -> String {
    match path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => error.replace(&format!("{}/", dir.display()), ""),
        None => error.to_string(),
    }
}

/// `typed` with the folder name it ends in finished, as far as the
/// folders there that start so agree, and a `/` after it when only one
/// does. A relative path is taken from `base`. None when no folder fits.
fn complete_folder(base: &Path, typed: &str) -> Option<String> {
    let (lead, start) = typed.rsplit_once('/').map_or(("", typed), |(lead, start)| (lead, start));
    let lead = if typed.contains('/') { format!("{lead}/") } else { String::new() };
    let dir = match lead.as_str() {
        "" => base.to_path_buf(),
        _ if lead.starts_with('/') || lead.starts_with('~') => crate::options::expand_home(&lead),
        _ => base.join(&lead),
    };
    let dir = if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir };
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(start) && (start.starts_with('.') || !name.starts_with('.')))
        .collect();
    names.sort();
    let first = names.first()?;
    if names.len() == 1 {
        return Some(format!("{lead}{first}/"));
    }
    // As far as they all agree.
    let mut common = first.clone();
    for name in &names[1..] {
        let agree = common.chars().zip(name.chars()).take_while(|(a, b)| a == b).count();
        common = common.chars().take(agree).collect();
    }
    Some(format!("{lead}{common}"))
}

fn short_dir(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let shown = match home.as_ref().and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    };
    let parts: Vec<&str> = shown.split('/').collect();
    if shown.chars().count() <= 36 || parts.len() <= 4 {
        return shown;
    }
    // The start and the last two folders say where it is well enough.
    format!("{}/\u{2026}/{}/{}", parts[..2].join("/"), parts[parts.len() - 2], parts[parts.len() - 1])
}

/// `line`, cut short with an ellipsis if it is wider than `width`.
fn clipped(text: &mut Text, line: &str, width: i32, px: u32) -> String {
    if text.width(line, px) <= width {
        return line.to_string();
    }
    let mut kept: Vec<char> = line.chars().collect();
    loop {
        kept.pop();
        let shorter: String = kept.iter().chain(&['\u{2026}']).collect();
        if kept.is_empty() || text.width(&shorter, px) <= width {
            return shorter;
        }
    }
}

/// The background of a menu or box: a dark panel with rounded corners and
/// a thin edge.
fn panel(canvas: &mut Canvas, x: i32, y: i32, width: i32, height: i32, radius: i32) {
    canvas.round_fill(x, y, width, height, radius, MENU_COLOR, 256);
    canvas.round_outline(x, y, width, height, radius, 1, MENU_EDGE_COLOR);
}

/// Begins a run of words that is drawn as a badge: small, on a rounded
/// ground of the run's colour.
const BADGE: char = '\u{1}';

/// Begin a run that is a choice to be clicked, drawn as the buttons of
/// the debug overlay are: the second for the one chosen, filled in.
const CHOICE: char = '\u{2}';
const CHOSEN: char = '\u{3}';
/// Begins a run drawn as it is, but there to be clicked: the file's name
/// in the info overlay, which copies it.
const LINK: char = '\u{4}';

/// The colour of a badge for what a picture or video is.
fn badge_color(name: &str) -> u32 {
    match name {
        "HDR10+" => 0x8E5BD6,
        "2094-50" => 0x2FA36B,
        "HLG" => 0xD9822B,
        "HDR10" | "PQ" => 0x3E7BD6,
        "Ultra HDR" => 0xC9A227,
        "HEVC" | "AV1" | "VP9" | "H.264" | "VP8" | "MPEG-2" => 0x505A66,
        "JPEG XL" | "AVIF" | "HEIC" | "HEIF" => 0x4C6A8C,
        _ => 0x6A6A6A,
    }
}

/// A run for a badge with this name.
fn badge(name: &str) -> (String, u32) {
    (format!("{BADGE}{name}"), badge_color(name))
}

/// How wide a run is drawn: a badge or a choice with its ground and the
/// gap after it.
fn run_width(text: &mut Text, run: &str, px: u32) -> i32 {
    if let Some(words) = run.strip_prefix(LINK) {
        return text.width(words, px);
    }
    match run.strip_prefix([BADGE, CHOICE, CHOSEN]) {
        Some(name) => text.width(name, (px * 4 / 5).max(10)) + px as i32 * 3 / 2,
        None => text.width(run, px),
    }
}

/// Draws a run, a badge or a choice as one; gives back how wide it was
/// and, for a choice, where its button is.
fn draw_run(canvas: &mut Canvas, text: &mut Text, x: i32, y: i32, px: u32, color: u32, run: &str) -> (i32, Option<[i32; 4]>) {
    if let Some(words) = run.strip_prefix(LINK) {
        text.draw(canvas, x, y, px, color, words);
        let width = text.width(words, px);
        return (width, Some([x, y, width, Text::line_height(px)]));
    }
    let Some(name) = run.strip_prefix([BADGE, CHOICE, CHOSEN]) else {
        text.draw(canvas, x, y, px, color, run);
        return (text.width(run, px), None);
    };
    let small = (px * 4 / 5).max(10);
    let (pad, line) = (px as i32 / 2, Text::line_height(px));
    let (w, h) = (text.width(name, small) + pad, Text::line_height(small) + pad / 3);
    let (left, top) = (x + pad / 2, y + (line - h) / 2);
    let place = match run.chars().next() {
        Some(BADGE) => {
            canvas.round_fill(left, top, w, h, h / 3, color, 200);
            None
        }
        // A choice, as the debug overlay's buttons: the one chosen filled.
        first => {
            let chosen = first == Some(CHOSEN);
            canvas.round_fill(left, top, w, h, h / 3, if chosen { accent() } else { PLACEHOLDER_COLOR }, 256);
            canvas.round_outline(left, top, w, h, h / 3, 1, if chosen { accent() } else { MENU_EDGE_COLOR });
            Some([left, top, w, h])
        }
    };
    let words = if run.starts_with(CHOSEN) { on_accent() } else { TEXT_COLOR };
    text.draw(canvas, left + pad / 2, top + (h - Text::line_height(small)) / 2, small, words, name);
    (run_width(text, run, px), place)
}

/// Draws lines of a function each on a small graph: a dark square with a
/// faint grid every `tick`, the axes running from 0 to `max`. A line is a
/// function from x to y, its colour, and whether it is dashed.
#[allow(clippy::type_complexity)]
fn plot(canvas: &mut Canvas, [x, y, w, h]: [i32; 4], max: (f64, f64), tick: f64, lines: &[(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)]) {
    canvas.fill(x, y, w, h, 0x0B0B0B);
    let mut at = tick;
    while at < max.0.max(max.1) - 1e-9 {
        if at < max.0 {
            canvas.fill(x + (at / max.0 * f64::from(w)) as i32, y, 1, h, 0x262626);
        }
        if at < max.1 {
            canvas.fill(x, y + h - 1 - (at / max.1 * f64::from(h)) as i32, w, 1, 0x262626);
        }
        at += tick;
    }
    let thick = (w / 150).max(2);
    for (line, color, dashed) in lines {
        let mut last: Option<i32> = None;
        for column in 0..w {
            let value = line((f64::from(column) + 0.5) / f64::from(w) * max.0);
            if !value.is_finite() || (*dashed && (column / (thick * 4)) % 2 == 1) {
                last = None;
                continue;
            }
            let row = h - 1 - (value / max.1 * f64::from(h)).round() as i32;
            // Joined to the last column's, so that a steep part has no gaps.
            let (from, to) = match last {
                Some(last) => (last.min(row), last.max(row)),
                None => (row, row),
            };
            last = Some(row);
            let (from, to) = (from.max(0), to.min(h - 1));
            if from <= to {
                canvas.fill(x + column, y + from, thick, to - from + thick.min(h - to), *color);
            }
        }
    }
}

/// Draws lines made of runs of words, each run in its own colour, on a
/// darkened panel with its top-left corner at `at`. Returns where the
/// next thing down should start, and where each choice in it went, by
/// its name.
fn rich_block(canvas: &mut Canvas, text: &mut Text, lines: &[Vec<(String, u32)>], px: u32, at: (i32, i32)) -> (i32, Vec<(String, [i32; 4])>) {
    if lines.is_empty() {
        return (at.1, Vec::new());
    }
    let pad = px as i32 / 2;
    let line_height = Text::line_height(px);
    let widths: Vec<i32> = lines.iter().map(|line| line.iter().map(|(run, _)| run_width(text, run, px)).sum()).collect();
    let width = widths.into_iter().max().unwrap_or(0) + 3 * pad;
    let height = line_height * lines.len() as i32 + 2 * pad;
    canvas.round_fill(at.0, at.1, width, height, pad, 0x000000, 176);
    let mut choices = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let mut x = at.0 + 3 * pad / 2;
        for (run, color) in line {
            let (wide, place) = draw_run(canvas, text, x, at.1 + pad + index as i32 * line_height, px, *color, run);
            if let Some(place) = place {
                choices.push((run.chars().skip(1).collect(), place));
            }
            x += wide;
        }
    }
    (at.1 + height + pad, choices)
}

/// The sizes the debug overlay has grown to: its figures change all the
/// time, and a panel that changed width with each of them would jitter.
/// It only ever gets wider or taller, until it shows other clusters.
#[derive(Default)]
struct DebugKeep {
    titles: Vec<&'static str>,
    columns: Vec<(i32, i32)>,
    rows: i32,
}

/// Draws the debug overlay: a panel of clusters side by side, each a
/// heading over its figures, the names muted and the values bright, and
/// under them buttons to copy the lot. Returns where the buttons went.
fn draw_debug(canvas: &mut Canvas, text: &mut Text, groups: &[Group], px: u32, at: (i32, i32), keep: &mut DebugKeep, pointer: (i32, i32)) -> (Vec<Button>, i32) {
    if groups.is_empty() {
        return (Vec::new(), at.1);
    }
    let small = (px * 5 / 6).max(11);
    let (pad, row) = (px as i32, Text::line_height(small) + px as i32 / 6);
    // Each cluster is as wide as its widest name and its widest value.
    let mut columns: Vec<(i32, i32)> = groups
        .iter()
        .map(|(title, figures)| {
            let names = figures.iter().map(|(name, _)| text.width(name, small)).max().unwrap_or(0);
            let values = figures.iter().map(|(_, value)| text.width(value, small)).max().unwrap_or(0);
            (names, values.max(text.width(title, small) - names - pad))
        })
        .collect();
    let mut rows = groups.iter().map(|(_, figures)| figures.len()).max().unwrap_or(0) as i32 + 1;
    let titles: Vec<&'static str> = groups.iter().map(|(title, _)| *title).collect();
    if keep.titles == titles {
        for (now, was) in columns.iter_mut().zip(&keep.columns) {
            *now = (now.0.max(was.0), now.1.max(was.1));
        }
        rows = rows.max(keep.rows);
    }
    (keep.titles, keep.columns, keep.rows) = (titles, columns.clone(), rows);

    let buttons = [(commands::icons::COPY, "Copy", Command::CopyDebug), (commands::icons::BRACES, "Copy as JSON", Command::CopyDebugJson)];
    let button_height = row + pad / 3;
    let width = columns.iter().map(|(names, values)| names + pad + values).sum::<i32>() + pad * 2 * groups.len() as i32;
    let height = rows * row + button_height + 3 * pad;
    canvas.round_fill(at.0, at.1, width, height, pad / 2, 0x000000, 190);
    canvas.round_outline(at.0, at.1, width, height, pad / 2, 1, MENU_EDGE_COLOR);

    let mut x = at.0 + pad;
    for ((title, figures), (names, values)) in groups.iter().zip(columns) {
        let mut y = at.1 + pad;
        text.draw(canvas, x, y, small, accent_soft(), title);
        canvas.fill(x, y + row - row / 6, names + pad + values, 1, MENU_EDGE_COLOR);
        y += row;
        for (name, value) in figures {
            text.draw(canvas, x, y, small, DIM_TEXT_COLOR, name);
            text.draw(canvas, x + names + pad, y, small, TEXT_COLOR, value);
            y += row;
        }
        x += names + pad + values + 2 * pad;
    }

    // The buttons, along the bottom.
    let mut drawn = Vec::new();
    let (mut x, y) = (at.0 + pad, at.1 + height - pad - button_height);
    for (icon, label, command) in buttons {
        let words = format!("{icon}  {label}");
        let button_width = text.width(&words, small) + pad;
        let over = (x..x + button_width).contains(&pointer.0) && (y..y + button_height).contains(&pointer.1);
        canvas.round_fill(x, y, button_width, button_height, pad / 3, if over { accent() } else { PLACEHOLDER_COLOR }, 256);
        canvas.round_outline(x, y, button_width, button_height, pad / 3, 1, if over { accent() } else { MENU_EDGE_COLOR });
        let words_top = y + (button_height - Text::line_height(small)) / 2;
        text.draw(canvas, x + pad / 2, words_top, small, if over { on_accent() } else { TEXT_COLOR }, &words);
        drawn.push(Button { x, y, width: button_width, height: button_height, command });
        x += button_width + pad / 2;
    }
    (drawn, at.1 + height + pad / 2)
}

/// Draws lines of text on a darkened panel. `place` is given the panel size
/// and returns its top-left corner.
fn text_block(canvas: &mut Canvas, text: &mut Text, lines: &[String], px: u32, place: impl Fn(i32, i32) -> (i32, i32)) {
    if lines.is_empty() {
        return;
    }
    let pad = px as i32 / 2;
    let line_height = Text::line_height(px);
    let width = lines.iter().map(|line| text.width(line, px)).max().unwrap_or(0) + 2 * pad;
    let height = line_height * lines.len() as i32 + 2 * pad;
    let (x, y) = place(width, height);
    canvas.round_fill(x, y, width, height, pad, 0x000000, 176);
    for (index, line) in lines.iter().enumerate() {
        text.draw(canvas, x + pad, y + pad + index as i32 * line_height, px, TEXT_COLOR, line);
    }
}

/// Draws the delete confirmation in the middle of the window: a warning
/// sign and the question, above a Cancel and a Delete button. The one that
/// Enter would press is filled in, Delete in red.
fn draw_confirm(canvas: &mut Canvas, text: &mut Text, confirm: &Confirm, px: u32) -> [[i32; 4]; 2] {
    let mut drawn = [[0; 4]; 2];
    let pad = px as i32;
    let line_height = Text::line_height(px);
    let (sign, sign_px) = (commands::icons::TRIANGLE_ALERT, px * 3 / 2);
    let sign_width = text.width(sign, sign_px) + pad;
    let note = "This cannot be undone.";
    let button_width = px as i32 * 7;
    let button_height = line_height + pad;
    let buttons_width = 2 * button_width + pad;
    let words = text.width(&confirm.question, px).max(text.width(note, px));
    let width = ((sign_width + words).max(buttons_width) + 3 * pad).min(canvas.width);
    let height = 2 * line_height + button_height + 4 * pad;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);

    panel(canvas, left, top, width, height, pad);
    let words_left = left + 3 * pad / 2 + sign_width;
    text.draw(canvas, left + 3 * pad / 2, top + 3 * pad / 2, sign_px, DANGER_COLOR, sign);
    text.draw(canvas, words_left, top + 3 * pad / 2, px, TEXT_COLOR, &confirm.question);
    text.draw(canvas, words_left, top + 3 * pad / 2 + line_height, px, DIM_TEXT_COLOR, note);

    // The buttons sit at the right, as the last thing read.
    let buttons_top = top + height - 3 * pad / 2 - button_height;
    let buttons_left = left + width - 3 * pad / 2 - buttons_width;
    let buttons = [("Cancel", !confirm.yes, accent()), ("Delete", confirm.yes, DANGER_COLOR)];
    for (index, (label, selected, color)) in buttons.into_iter().enumerate() {
        let button_left = buttons_left + index as i32 * (button_width + pad);
        let round = pad / 2;
        if selected {
            canvas.round_fill(button_left, buttons_top, button_width, button_height, round, color, 256);
        } else {
            canvas.round_fill(button_left, buttons_top, button_width, button_height, round, PLACEHOLDER_COLOR, 256);
            canvas.round_outline(button_left, buttons_top, button_width, button_height, round, 1, MENU_EDGE_COLOR);
        }
        let label_left = button_left + (button_width - text.width(label, px)) / 2;
        text.draw(canvas, label_left, buttons_top + pad / 2, px, if selected && index == 0 { on_accent() } else { TEXT_COLOR }, label);
        drawn[index] = [button_left, buttons_top, button_width, button_height];
    }
    drawn
}

/// Draws the rename box, centred with its bottom edge at `bottom`.
/// `label` names what is being typed and `note`, at the right, says
/// something about it, such as how many files match.
/// Shades the selected stretch of a text field whose text starts at
/// `(x, y)`, for the text to be drawn over.
fn draw_selection(canvas: &mut Canvas, text: &mut Text, field: &TextField, x: i32, y: i32, px: u32) {
    let Some((from, to)) = field.selection() else { return };
    let mut upto = |at: usize| text.width(&field.text[..at].iter().collect::<String>(), px);
    let (left, right) = (upto(from), upto(to));
    canvas.fill(x + left, y, right - left, Text::line_height(px), accent());
}

/// Draws a box to type in, in the middle of the window: an icon and what
/// is being asked for, the text in a field of its own, and under it a
/// word on what Enter does.
fn draw_prompt(canvas: &mut Canvas, text: &mut Text, icon: &str, title: &str, field: &TextField, hint: &str, px: u32) {
    let typed = field.string();
    let before_cursor: String = field.text[..field.cursor].iter().collect();
    let pad = px as i32;
    let line_height = Text::line_height(px);
    let small = (px * 5 / 6).max(11);
    let heading = format!("{icon}  {title}");
    let hint = format!("{hint}  ·  Esc to cancel");
    let widest = text.width(&typed, px).max(text.width(&heading, px)).max(text.width(&hint, small));
    let width = (widest + 5 * pad).max(px as i32 * 32).min(canvas.width);
    let field_height = line_height + pad;
    let height = 2 * line_height + field_height + 7 * pad / 2;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);

    panel(canvas, left, top, width, height, pad);
    text.draw(canvas, left + 3 * pad / 2, top + pad, px, TEXT_COLOR, &heading);

    // The field: darker than the panel, edged in the accent as the thing
    // that has the keyboard.
    let (field_left, field_top, field_width) = (left + 3 * pad / 2, top + pad + line_height + pad / 2, width - 3 * pad);
    canvas.round_fill(field_left, field_top, field_width, field_height, pad / 2, 0x111111, 256);
    canvas.round_outline(field_left, field_top, field_width, field_height, pad / 2, 2, accent());
    let (text_left, text_top) = (field_left + pad / 2, field_top + pad / 2);
    draw_selection(canvas, text, field, text_left, text_top, px);
    text.draw(canvas, text_left, text_top, px, TEXT_COLOR, &typed);
    let cursor = text_left + text.width(&before_cursor, px);
    canvas.fill(cursor, text_top, (px as i32 / 10).max(2), line_height, TEXT_COLOR);

    text.draw(canvas, left + 3 * pad / 2, field_top + field_height + pad / 2, small, DIM_TEXT_COLOR, &hint);
}

/// The keys of one line of the shortcuts, each with how wide it is drawn
/// and whether as a key cap. A few words together describe something
/// that is not a key, and are written plainly.
fn help_keys<'a>(text: &mut Text, keys: &'a str, px: u32) -> Vec<(&'a str, i32, bool)> {
    let pad = px as i32 / 2;
    let parts = keys.split("  ").filter(|key| !key.is_empty());
    parts.map(|key| if key.contains(' ') { (key, text.width(key, px), false) } else { (key, (text.width(key, px) + pad).max(px as i32 * 3 / 2), true) }).collect()
}

/// Draws the list of shortcuts as a panel in the middle of the window:
/// columns of sections, each key on a cap of its own.
fn draw_help(canvas: &mut Canvas, text: &mut Text, px: u32, search: &str) {
    // What is typed is looked for, each word of it, in a shortcut's keys,
    // what it does and the heading it is under. Those without it stay
    // where they are, faint, so that nothing moves about.
    let words: Vec<String> = search.to_lowercase().split_whitespace().map(str::to_string).collect();
    let matches = |title: &str, keys: &str, label: &str| {
        let all = format!("{title} {keys} {label}").to_lowercase();
        words.iter().all(|word| all.contains(word.as_str()))
    };
    let found: usize = commands::HELP.iter().flat_map(|sections| sections.iter()).map(|(title, entries)| entries.iter().filter(|(keys, label)| matches(title, keys, label)).count()).sum();
    let small = (px * 5 / 6).max(11);
    let row = Text::line_height(px) + px as i32 / 3;
    let (pad, gap, between) = (px as i32 * 3 / 2, px as i32, px as i32 / 3);
    let keys_width = |text: &mut Text, keys: &str| -> i32 {
        let parts = help_keys(text, keys, small);
        parts.iter().map(|(_, width, _)| width + between).sum::<i32>() - between
    };
    // Each column is as wide as its widest keys plus its widest description.
    let columns: Vec<(i32, i32, i32)> = commands::HELP
        .iter()
        .map(|sections| {
            let entries = sections.iter().flat_map(|(_, entries)| entries.iter());
            let keys = entries.clone().map(|(keys, _)| keys_width(text, keys)).max().unwrap_or(0);
            let labels = entries.map(|(_, label)| text.width(label, px)).max().unwrap_or(0);
            let rows: usize = sections.iter().map(|(_, entries)| entries.len() + 2).sum();
            (keys, labels, rows as i32 - 1)
        })
        .collect();
    let width = columns.iter().map(|(keys, labels, _)| keys + gap + labels).sum::<i32>()
        + 2 * pad * (columns.len() as i32 - 1)
        + 2 * pad;
    let (title, hint) = ("Keyboard shortcuts", "Type to search.  Esc closes this.  > in the find box lists every command.");
    let head = row * 2;
    let height = head + columns.iter().map(|c| c.2).max().unwrap_or(0) * row + row + 2 * pad;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);
    panel(canvas, left, top, width, height, px as i32);
    text.draw(canvas, left + pad, top + pad, px * 5 / 4, TEXT_COLOR, title);
    let hint_width = text.width(hint, small);
    text.draw(canvas, left + width - pad - hint_width, top + height - pad - row / 2, small, DIM_TEXT_COLOR, hint);
    // The box what is typed goes in, at the right of the heading.
    let line = Text::line_height(px);
    let (box_w, box_h) = (px as i32 * 22, line + px as i32 * 2 / 3);
    let (box_x, box_y) = (left + width - pad - box_w, top + pad - px as i32 / 6);
    canvas.round_fill(box_x, box_y, box_w, box_h, px as i32 / 2, PLACEHOLDER_COLOR, 256);
    canvas.round_outline(box_x, box_y, box_w, box_h, px as i32 / 2, 1, if words.is_empty() { MENU_EDGE_COLOR } else { accent_soft() });
    let glass = commands::icons::SEARCH;
    let typed_left = box_x + px as i32 * 2 / 3 + text.width(glass, px) + px as i32 / 2;
    let typed_top = box_y + (box_h - line) / 2;
    text.draw(canvas, box_x + px as i32 * 2 / 3, typed_top, px, DIM_TEXT_COLOR, glass);
    match search.is_empty() {
        true => text.draw(canvas, typed_left, typed_top, px, DIM_TEXT_COLOR, "Type to search"),
        false => text.draw(canvas, typed_left, typed_top, px, TEXT_COLOR, search),
    }
    let caret = typed_left + if search.is_empty() { 0 } else { text.width(search, px) };
    canvas.fill(caret, typed_top, (px as i32 / 10).max(2), line, TEXT_COLOR);
    if !words.is_empty() {
        let count = if found == 0 { "no match".to_string() } else { format!("{found} found") };
        let count_left = box_x + box_w - px as i32 * 2 / 3 - text.width(&count, small);
        text.draw(canvas, count_left, box_y + (box_h - Text::line_height(small)) / 2, small, DIM_TEXT_COLOR, &count);
    }

    let mut x = left + pad;
    for (sections, (keys_width, labels_width, _)) in commands::HELP.iter().zip(columns) {
        let mut y = top + pad + head;
        for (title, entries) in sections.iter() {
            let any = entries.iter().any(|(keys, label)| matches(title, keys, label));
            text.draw(canvas, x, y, px, if any { accent_soft() } else { MENU_EDGE_COLOR }, title);
            canvas.fill(x, y + row - row / 5, keys_width + gap + labels_width, 1, MENU_EDGE_COLOR);
            y += row;
            for (keys, label) in entries.iter() {
                let lit = matches(title, keys, label);
                if lit && !words.is_empty() {
                    canvas.round_fill(x - px as i32 / 3, y, keys_width + gap + labels_width + px as i32 * 2 / 3, row, px as i32 / 3, TEXT_COLOR, 22);
                }
                let mut key_x = x;
                for (key, width, cap) in help_keys(text, keys, small) {
                    let key_top = y + (row - Text::line_height(small)) / 2;
                    if !lit {
                        let inset = if cap { (width - text.width(key, small)) / 2 } else { 0 };
                        text.draw(canvas, key_x + inset, key_top, small, MENU_EDGE_COLOR, key);
                    } else if cap {
                        let (cap_top, cap_height) = (y + row / 10, row - row / 5);
                        canvas.round_fill(key_x, cap_top, width, cap_height, px as i32 / 4, PLACEHOLDER_COLOR, 256);
                        canvas.round_outline(key_x, cap_top, width, cap_height, px as i32 / 4, 1, MENU_EDGE_COLOR);
                        let inset = (width - text.width(key, small)) / 2;
                        text.draw(canvas, key_x + inset, key_top, small, TEXT_COLOR, key);
                    } else {
                        text.draw(canvas, key_x, key_top, small, DIM_TEXT_COLOR, key);
                    }
                    key_x += width + between;
                }
                let label_top = y + (row - Text::line_height(px)) / 2;
                text.draw(canvas, x + keys_width + gap, label_top, px, if lit { TEXT_COLOR } else { MENU_EDGE_COLOR }, label);
                y += row;
            }
            y += row;
        }
        x += keys_width + gap + labels_width + 2 * pad;
    }
}

/// How long picture `index` of an animated frame stays up.
fn picture_delay(frame: &Frame, index: usize) -> Duration {
    let delays = frame.animation.as_ref().map(|animation| &animation.delays[..]).unwrap_or_default();
    delays.get(index).or(delays.first()).copied().unwrap_or(Duration::from_millis(100))
}

/// A playback time as `m:ss`, or `h:mm:ss` from an hour up.
fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The scenes of one video: when each of its equal parts starts, and a
/// frame from each as
/// they come in.
struct SceneList {
    starts: Option<Vec<f64>>,
    frames: Vec<Option<Arc<Frame>>>,
}

/// One of a video's audio or subtitle tracks.
struct Track {
    id: i64,
    /// Its language and title, as far as the file gives them.
    name: String,
    selected: bool,
}

/// The tracks of one kind, `audio` or `sub`, in the file a player has open.
fn tracks_of(player: &Player, kind: &str) -> Vec<Track> {
    let count = player.number("track-list/count").unwrap_or(0.0) as usize;
    let track = |n: usize| {
        let field = |name: &str| player.text(&format!("track-list/{n}/{name}"));
        if field("type")? != kind {
            return None;
        }
        let id = player.number(&format!("track-list/{n}/id"))? as i64;
        let named: Vec<String> = [field("lang"), field("title")].into_iter().flatten().collect();
        let name = if named.is_empty() { format!("Track {id}") } else { named.join("  ") };
        Some(Track { id, name, selected: player.flag(&format!("track-list/{n}/selected")) == Some(true) })
    };
    (0..count).filter_map(track).collect()
}

/// Fills in an action template: `%f` is the path and `%n` the file name,
/// both quoted for the shell, and `%%` a literal percent sign.
fn expand_action(template: &str, path: &Path, seconds: f64) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
    let mut command = String::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            command.push(c);
            continue;
        }
        match chars.next() {
            Some('f') => command.push_str(&quote(&path.to_string_lossy())),
            Some('n') => command.push_str(&quote(&name)),
            Some('s') => command.push_str(&quote(&path.file_stem().unwrap_or(path.as_os_str()).to_string_lossy())),
            Some('d') => command.push_str(&quote(&path.parent().unwrap_or(Path::new(".")).to_string_lossy())),
            Some('t') => command.push_str(&format!("{seconds:.3}")),
            Some('%') => command.push('%'),
            Some(other) => command.extend(['%', other]),
            None => command.push('%'),
        }
    }
    command
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Start decoding before the window exists, so the first image is
        // ready by the time the window is mapped.
        let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next());
        if let Some(size) = monitor.map(|m| m.size()) {
            self.monitor = (size.width, size.height);
            self.request();
        }

        let mut attributes = Window::default_attributes()
            .with_title("jog")
            .with_decorations(!self.options.borderless);
        if self.snapshot.is_some() {
            // Never shown, so it takes the size a fullscreen window would get.
            let size = self.options.geometry.unwrap_or(self.monitor);
            attributes = attributes.with_visible(false).with_inner_size(PhysicalSize::new(size.0, size.1));
        } else if self.options.fullscreen {
            // Made the size of the screen and marked fullscreen while
            // still hidden, then shown: the window manager then puts it up
            // fullscreen in one go. Shown first and marked after, it is
            // put up at a size of the manager's own, resized to the work
            // area and only then to the screen, a burst of events that the
            // first frame has to wait behind.
            let (width, height) = self.monitor;
            attributes = attributes
                .with_visible(false)
                .with_inner_size(PhysicalSize::new(width.max(1), height.max(1)))
                .with_fullscreen(Some(Fullscreen::Borderless(None)));
        } else {
            let fallback = (self.monitor.0 * 3 / 5, self.monitor.1 * 3 / 5);
            let (width, height) = self.options.geometry.unwrap_or(fallback);
            attributes = attributes.with_inner_size(PhysicalSize::new(width.max(1), height.max(1)));
        }
        let creating = Instant::now();
        let window = Arc::new(event_loop.create_window(attributes).expect("failed to create window"));
        let created = creating.elapsed();
        window.set_cursor_visible(self.pointer_visible);
        // The connection the frames go over is set up while the window is
        // still hidden: its round trips to the display are answered at
        // once then, rather than behind the window manager's work of
        // putting the window up.
        let context = Context::new(window.clone()).expect("failed to connect to display");
        let connected = creating.elapsed();
        self.surface = Some(Surface::new(&context, window.clone()).expect("failed to create surface"));
        // Marked fullscreen now, while the display has nothing else of
        // ours to do: the word with it waits behind the first picture
        // otherwise, and so then does showing the window.
        if self.options.fullscreen && self.snapshot.is_none() {
            x11::mark_fullscreen(&window);
        }
        self.window = Some(window.clone());
        if self.options.debug {
            eprintln!(
                "jog: {:>7.1?} start window made ({created:.1?} to create, {:.1?} to connect and show), {:.1?} since the loop began",
                creating.elapsed(),
                connected - created,
                self.perf.arrived.elapsed()
            );
        }
        // The buffers a frame goes through are given their size now, as
        // pages the system hands out blank, which cost nothing until they
        // are first written, and are then written a row per core. Grown
        // to size one row after another they would take this thread
        // tens of milliseconds, just as the first image arrives.
        let (width, height) = self.window_size();
        for buffer in [&mut self.scene, &mut self.presented] {
            *buffer = ui::blank(width as usize * height as usize);
        }
        self.presented.clear();
        if self.options.debug {
            eprintln!("jog: {:>7.1?} start buffers ready, {:.1?} since the loop began", creating.elapsed(), self.perf.arrived.elapsed());
        }
        self.update_monitor();
        // With --grid the grid is what comes up, and nothing plays until
        // a picture is gone to: not the first video, nor its trailer.
        if std::mem::take(&mut self.options.grid) {
            let layout = self.grid_layout(self.window_size());
            self.grid = Some(Grid { moved: false, ..Grid::at(self.pos, &layout) });
            self.request();
        }
        // The first image goes on the window as its background before the
        // window is shown, and the server paints it the moment the window
        // manager shows it. A small file is nearly decoded by now, having
        // been begun before the display was opened: a moment's wait for it
        // is worth more than a window put up empty and painted a good
        // while later. A large one takes far longer than the wait.
        if self.options.fullscreen && self.snapshot.is_none() {
            let id = self.id();
            let small = std::fs::metadata(&self.files[id]).is_ok_and(|meta| meta.len() <= QUICK_FILE);
            let until = Instant::now() + FIRST_FRAME_WAIT;
            while small && self.loader.fitted(id).is_none() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.refresh();
        if self.options.fullscreen && self.snapshot.is_none() {
            window.set_visible(true);
            if self.options.debug {
                eprintln!("jog: {:>7.1?} start window shown, {:.1?} since the loop began", creating.elapsed(), self.perf.arrived.elapsed());
            }
        }
        // Have the player ready before the first video is reached.
        if self.files.iter().any(|file| files::is_video(file)) {
            self.warm_player();
        }
        // Reading and parsing the font takes a noticeable moment; do it now
        // rather than at the first key press that brings up some text.
        let proxy = self.proxy.clone();
        std::thread::spawn(move || drop(proxy.send_event(Event::Font(Text::load()))));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        // A playing video keeps events coming without a pause, and a
        // scripted run's timetable would otherwise wait for the pause.
        let snapshot_due = self.snapshot.as_ref().and_then(|snapshot| snapshot.at);
        if snapshot_due.is_some_and(|at| at <= Instant::now()) {
            self.finish_snapshot(event_loop);
        }
        match event {
            Event::Loaded(Job::Fitted(id)) if id == self.id() => {
                if self.options.debug && self.perf.shown_in.is_none() {
                    eprintln!("jog: {:>7.1?} start first frame told of", self.perf.arrived.elapsed());
                }
                self.refresh();
                self.start_snapshot(event_loop);
            }
            Event::Loaded(Job::Full(id)) if id == self.id() => self.redraw(),
            // Thumbnails come in a burst, several in a frame's time: the
            // page is drawn once for a few of them rather than for each.
            Event::Loaded(Job::Thumb(_)) if self.strip_visible() || self.grid.is_some() || self.palette.is_some() => {
                let due = Instant::now() + THUMBS_REDRAW;
                self.grid_due = Some(self.grid_due.map_or(due, |at| at.min(due)));
            }
            Event::Loaded(_) => {}
            Event::Converted(done) => self.converted(done),
            Event::ConvertSource(id, source) => self.convert_source(id, source),
            Event::ConvertTrial(asked, format, trial) => self.convert_trial(asked, format, trial),
            Event::ConvertScore(asked, score) => self.convert_score(asked, score),
            // With a Cancel to click, for as long as it goes on.
            Event::VideoProgress(words) => self.toast_with(words, vec![("Cancel".into(), ToastLink::Cancel)]),
            Event::EntryFrame(found) => self.entry_frame(found),
            Event::Video(which, signal) if which == self.active => {
                self.video_signal(signal);
                self.tend_back();
            }
            Event::BackFrames(ticket, frames, took) => self.back_frames(ticket, frames, took),
            Event::CompareLoaded(ticket, images) => self.compare_loaded(ticket, images),
            Event::CompareScore(ticket, score) => self.compare_score(ticket, score),
            Event::Video(_, signal) => self.spare_signal(signal),
            Event::Spare(spare) => {
                self.spare = spare.ok().map(|player| Spare { player, id: None, from: 0.0 });
                self.apply_subtitles();
                // As for the first: it may have queued events unheard.
                self.spare_signal(Signal::Events);
                self.preload_video();
            }
            Event::Player(player) => self.player_ready(player, event_loop),
            Event::Preview(signal) => self.preview_signal(signal),
            // Without it the seek bar still works, just without pictures.
            Event::PreviewPlayer(preview) => {
                self.preview = preview.ok();
                // As for the player: empty the queue so that mpv signals.
                self.preview_signal(Signal::Events);
            }
            Event::Listed(listed) => self.take_listing(listed, true),
            Event::HdrInfo(id, info) => {
                self.hdr_infos.insert(id, info);
                self.redraw();
            }
            Event::SpaceListed(paths, found) => {
                let Some(which) = self.spaces.iter().position(|space| space.paths == paths && space.kept.is_none()) else { return };
                // For the overview: what an unvisited workspace holds.
                let list = self.ids_for(found);
                if which != self.space && self.spaces[which].kept.is_none() && !list.is_empty() {
                    let sort = self.spaces[which].sort;
                    self.spaces[which].kept = Some(Kept { list, pos: 0, cursor: 0, filter: None, filter_from: Vec::new(), sort, reversed: false, shuffled: false });
                }
                self.request();
                self.redraw();
            }
            Event::Duration(id, seconds) => {
                self.durations.insert(id, seconds);
                self.redraw();
            }
            Event::Scenes(scenes::Found(id, index, frame)) => {
                if let Some(place) = self.scenes.get_mut(&id).and_then(|list| list.frames.get_mut(index)) {
                    *place = Some(Arc::new(frame));
                }
                self.redraw();
            }
            Event::Story(Found::Length(id, seconds)) => drop(self.durations.insert(id, seconds)),
            Event::Story(Found::Segment(id, index, segment)) => {
                if let Some(frames) = self.story_frames.get_mut(&id) {
                    frames[index] = Some(segment.into_iter().map(Arc::new).collect());
                }
                if self.story_subject() == Some(id) {
                    self.redraw();
                }
            }
            Event::Trailer(Note::Length(id, seconds)) => drop(self.durations.insert(id, Some(seconds))),
            Event::Trailer(Note::Frame) => {
                // One of a video the cursor has since left is dropped.
                let taken = self.trailers.as_ref().and_then(Trailers::take);
                if let (Some(trailer), Some((id, frame))) = (&mut self.trailer, taken) {
                    if trailer.id == id {
                        trailer.frame = Some(frame);
                        if trailer.since.is_none() {
                            trailer.since = Some(Instant::now());
                            let (id, took) = (trailer.id, trailer.asked.elapsed());
                            self.trailer_note("showing", id, &format!(" after {}", ms(took)));
                        }
                        self.redraw();
                    }
                }
            }
            Event::Story(Found::Done(id)) => {
                if self.story_asked == Some(id) {
                    self.story_asked = None;
                }
            }
            Event::Meta(id, meta) => {
                if self.meta_asked == Some(id) {
                    self.meta = Some((id, meta));
                    self.redraw();
                }
            }
            Event::Font(text) => {
                if self.text.is_none() {
                    self.text = Some(text);
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.leave(event_loop),
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(size) => {
                if self.options.debug {
                    eprintln!("jog: window resized to {}x{}", size.width, size.height);
                }
                self.update_monitor();
                self.menu = None;
                self.request();
                self.apply_video_view();
                // A video is drawn at its old size until this one holds.
                if self.video.is_some() {
                    self.resized_at = Some(Instant::now());
                    self.settle_at = Some(Instant::now() + RESIZE_SETTLE);
                }
                self.redraw();
            }
            // Coming back to the window, all of it is drawn afresh: a part
            // that was covered may have been uncovered while a frame of
            // this program's own asking was already due, and missed.
            WindowEvent::Focused(true) | WindowEvent::CursorEntered { .. } | WindowEvent::Occluded(false) => {
                if self.options.debug {
                    eprintln!("jog: window {}", match event {
                        WindowEvent::Focused(_) => "focused",
                        WindowEvent::CursorEntered { .. } => "entered by the pointer",
                        _ => "uncovered",
                    });
                }
                self.presented.clear();
                // Any of these says the window manager has put the window up.
                self.focused_once = true;
                self.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } if !event.state.is_pressed() => self.key_released(event.logical_key.as_ref()),
            WindowEvent::Focused(false) => {
                // Nothing held is let go where this window can hear of it.
                self.cancel_hold();
                if self.loupe.take().is_some() {
                    self.redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                let numpad = event.location == KeyLocation::Numpad;
                self.key_pressed(event.logical_key.as_ref(), event.repeat, numpad, event_loop);
            }
            WindowEvent::CursorMoved { position, .. } => self.mouse_moved(position.x, position.y),
            WindowEvent::CursorLeft { .. } => {
                self.set_strip_hover(false);
                if self.hover.take().is_some() {
                    self.redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.mouse_button(button, state == ElementState::Pressed, event_loop)
            }
            WindowEvent::MouseWheel { delta, .. } => self.mouse_wheel(delta),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.settle_at.is_some_and(|at| at <= now) {
            self.settle_at = None;
            self.redraw();
        }
        if self.toast.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.toast = None;
            self.redraw();
        }
        if self.slide_at.is_some_and(|at| at <= now) {
            // A video gets to play to its end, however short the delay; the
            // slideshow moves on as soon as it has finished.
            let playing = self.video.is_some()
                && self.player.as_ref().is_some_and(|player| player.flag("eof-reached") != Some(true));
            if playing {
                self.slide_at = Some(now + VIDEO_END_POLL);
            } else {
                self.advance_slideshow(event_loop);
            }
        }
        if self.time_until.is_some_and(|until| until <= now) {
            self.time_until = None;
            self.redraw();
        }
        if self.picture_due.is_some_and(|due| due <= now) {
            self.next_picture(now);
        }
        if self.font_due.is_some_and(|due| due <= now) {
            self.font_due = None;
            self.redraw();
        }
        // A trailer waiting for the cursor to settle may now begin.
        // The overview's slideshow moves on to its next picture.
        if self.overview_due.is_some_and(|due| due <= now) {
            self.overview_due = None;
            if let Some(overview) = &mut self.overview {
                overview.tick += 1;
            }
            self.request();
            self.redraw();
        }
        if self.trailer_due.is_some_and(|(_, at)| at <= now) {
            self.tend_trailer();
        }
        self.tend_hold();
        if self.anim_due.is_some_and(|due| due <= now) {
            self.anim_due = None;
            self.redraw();
        }
        if self.grid_due.is_some_and(|due| due <= now) {
            self.grid_due = None;
            self.redraw();
        }
        // The benchmark's next scene, once the last has had its time.
        if std::mem::take(&mut self.bench_step_due) {
            self.bench_step(event_loop);
        }
        let bench_at = self.bench.as_ref().and_then(|bench| bench.at);
        if bench_at.is_some_and(|at| at <= now) {
            self.bench_step(event_loop);
        }
        let bench_at = self.bench.as_ref().and_then(|bench| bench.at);
        let snapshot_at = self.snapshot.as_ref().and_then(|snapshot| snapshot.at);
        if snapshot_at.is_some_and(|at| at <= now) {
            return self.finish_snapshot(event_loop);
        }

        let hold_due = self.hold.as_ref().filter(|hold| !hold.fast).map(|hold| hold.since + HOLD_AFTER);
        let toast_until = self.toast.as_ref().map(|(_, until)| *until);
        // A little before that it starts on its way out, which wants drawing.
        let toast_leaves = toast_until.map(|until| until - EASE);
        // Once it is on its way, the frames are paced by the display,
        // as any movement is; asked for here as well, with this run after
        // every frame, they would follow one another as fast as they
        // could be drawn.
        if self.options.animations && toast_leaves.is_some_and(|at| at <= now) && self.anim_due.is_none() {
            self.redraw();
        }
        let toast_leaves = toast_leaves.filter(|at| *at > now);
        // A pause held long enough: the frames before it are fetched.
        if self.back_due.is_some_and(|due| due <= now) {
            self.fill_back();
        }
        let deadlines = [self.settle_at, toast_until, self.time_until, self.slide_at, snapshot_at, bench_at, self.picture_due, self.font_due, self.grid_due, self.anim_due, toast_leaves, hold_due, self.trailer_due.map(|(_, at)| at), self.overview_due, self.back_due];
        let next = deadlines.into_iter().flatten().min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_around_a_position_favour_the_way_we_go_and_the_decode_threads() {
        let list: Vec<usize> = (0..100).collect();
        // Few threads: three ahead; many: six, and no further.
        assert_eq!(App::around(&list, 10, true, 2), [10, 11, 9, 12, 13, 8]);
        assert_eq!(App::around(&list, 10, true, 4), [10, 11, 9, 12, 13, 8, 14]);
        assert_eq!(App::around(&list, 10, true, 16), [10, 11, 9, 12, 13, 8, 14, 15, 16]);
        assert_eq!(App::around(&list, 10, false, 16), [10, 9, 11, 8, 7, 12, 6, 5, 4]);
        // Round the ends of the list, and with no file asked for twice.
        assert_eq!(App::around(&list, 99, true, 16), [99, 0, 98, 1, 2, 97, 3, 4, 5]);
        assert_eq!(App::around(&[7, 8], 0, true, 16), [7, 8]);
    }

    #[test]
    fn text_fields_select_and_edit_by_the_word() {
        let (plain, ctrl, shift) = (ModifiersState::empty(), ModifiersState::CONTROL, ModifiersState::SHIFT);
        let mut field = TextField::new("one two three".chars().collect(), 13);
        // Ctrl+A then typing replaces everything.
        assert!(!field.key(Key::Character("a"), ctrl));
        assert_eq!(field.selection(), Some((0, 13)));
        assert!(field.key(Key::Character("x y"), plain));
        assert_eq!(field.string(), "x y");
        // Ctrl+Backspace takes out a word; Shift+Left selects a letter.
        assert!(field.key(Key::Named(NamedKey::Backspace), ctrl));
        assert_eq!(field.string(), "x ");
        assert!(!field.key(Key::Named(NamedKey::ArrowLeft), shift));
        assert_eq!(field.selection(), Some((1, 2)));
        assert!(field.key(Key::Named(NamedKey::Delete), plain));
        assert_eq!((field.string().as_str(), field.cursor), ("x", 1));
        // Moving without Shift lets a selection go.
        field.key(Key::Character("a"), ctrl);
        field.key(Key::Named(NamedKey::Home), plain);
        assert_eq!(field.selection(), None);
    }

    #[test]
    fn titles_drop_the_extension_and_a_release_s_small_print() {
        assert_eq!(title_of(Path::new("/tv/Some.Show.S02E05.1080p.x265-GRP.mkv")), "Some Show S02E05");
        assert_eq!(title_of(Path::new("some_show - s1e12 - The One.mp4")), "some show - s1e12");
        assert_eq!(title_of(Path::new("holiday.video.2024.mp4")), "holiday.video.2024");
        assert_eq!(title_of(Path::new("misses10.jpg")), "misses10");
    }

    #[test]
    fn actions_quote_paths_for_the_shell() {
        let path = Path::new("/pics/it's here.jpg");
        assert_eq!(expand_action("cp %f /tmp/%n", path, 0.0), r"cp '/pics/it'\''s here.jpg' /tmp/'it'\''s here.jpg'");
        assert_eq!(expand_action("echo 100%% %x %", path, 0.0), "echo 100% %x %");
        // The name without its ending, the folder, and the time in a video.
        assert_eq!(expand_action("%s %d %t", path, 12.5), r"'it'\''s here' '/pics' 12.500");
    }
}
