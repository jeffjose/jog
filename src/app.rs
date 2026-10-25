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
mod convert_tab;
mod compare_tab;
mod glossary;
use convert_panel::{ConvertPanel, PanelSource};
mod section;
mod bar_info;
mod windowed;
mod volume;
use section::{Edge, EntryFrames, Section, StripTab};
mod back;
mod hitch;
mod filework;
mod panels;
mod resize;
mod popout;
use popout::Popped;
use resize::PanelEdge;
use panels::StripButton;
use filework::Work;
mod compare;
mod draw;
use draw::*;
mod edit;
mod grid;
mod input;
mod menu;
mod palette;
mod refresh;
mod scanning;
mod video_layer;
mod video_convert;
mod activity;
mod scenes_strip;
mod scripted;
mod inspect;
mod inspect_hdr;
mod devtools;
mod timeline;
mod checks;
use devtools::{Block, DrawerPane, Pane};
mod track_editor;
mod editor;
mod tracks;
mod triaging;
mod video;
mod workspaces;
use crate::view::{Layout, View, Zoom};
use crate::x11;

const ZOOM_STEP: f64 = 1.25;
/// Thumbnails kept ready on each side of the current image.
const THUMB_RADIUS: i64 = 24;
/// How long after the last zoom or pan step the image is redrawn smoothly.
const SETTLE: Duration = Duration::from_millis(150);
/// How often the session is kept while jog runs.
const SESSION_EVERY: Duration = Duration::from_secs(60);
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
/// How long the seek bar may stand where a keyframe seek was aimed, ahead
/// of playback, before it follows playback again: keyframes far apart.
const AIM_GIVEN_UP: Duration = Duration::from_secs(8);
/// How long playback is to have been at or past where a keyframe seek was
/// aimed before the seek bar follows it again.
const AIM_SETTLED: Duration = Duration::from_millis(150);
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
const DIM_TEXT_COLOR: u32 = 0x8E9199;
const MENU_COLOR: u32 = 0x1C1D21;
/// The edge of a panel, a chip or a key cap, and the rules between rows.
const MENU_EDGE_COLOR: u32 = 0x3A3C43;
/// Words meant to recede the most: the dots between details, keys that
/// do not match a search.
const FAINT_TEXT_COLOR: u32 = 0x5C5F67;
/// The edge of what is there to be pressed: a key cap, a chip.
const KEY_EDGE_COLOR: u32 = 0x4A4D55;
/// The color of what is picked out, as `--accent` set it. It is read all
/// over the drawing code, which has no other tie to the options.
static ACCENT: AtomicU32 = AtomicU32::new(0x3E63DD);

fn accent() -> u32 {
    ACCENT.load(atomic::Ordering::Relaxed)
}

/// The color for words on the accent: white, or near black on an accent
/// light enough that white would be lost on it.
fn on_accent() -> u32 {
    let channel = |shift: u32| f64::from(accent() >> shift & 0xFF);
    let brightness = 0.299 * channel(16) + 0.587 * channel(8) + 0.114 * channel(0);
    if brightness > 160.0 { 0x161616 } else { 0xFFFFFF }
}

/// The colors for the main and the lesser words of a row: on one that is
/// picked out, ones that read on the accent, the lesser a little towards
/// it; elsewhere the usual white and gray. Gray on the accent is mud.
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
/// the color of danger.
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
    /// The job it was, in Activity.
    job: u64,
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
    /// How large the picture in the Convert panel comes out in a format, with
    /// that format's settings as described, or why it could not be made.
    ConvertSize(usize, Format, String, Result<u64, String>),
    /// The quality found to fit a size: the picture, the format, the
    /// quality and the size it makes, and the size asked for.
    ConvertFit(usize, Format, Option<(f32, u64)>, u64),
    /// How far along a job is: its number, the share done, and in words.
    JobProgress(u64, f64, String),
    /// What a few seconds of a video encoded say of the whole, for
    /// Inspect's Convert tab.
    VideoTrial(convert_tab::Trial, Result<crate::transcode::Estimate, String>),
    /// What the Convert window for a video needs to know of it.
    VideoFacts(usize, crate::transcode::VideoFacts),
    /// A frame of a video converted, and the same frame as it is.
    VideoCompared(convert_tab::Trial, Result<Arc<crate::transcode::Compared>, String>),
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
    /// Where two pictures of a size differ, as a picture of its own.
    CompareDelta(u64, compare::Delta),
    /// What a file was found to be made of, for the inspector.
    Inspected(usize, Result<inspect::Inspection, String>),
    /// The file Inspect's Compare tab sets beside the one on view, read.
    Compared(usize, Result<inspect::Inspection, String>),
    /// The tracks editor's file written, or why not; and whether it took
    /// the original's place.
    TracksWritten(Result<PathBuf, String>, bool, u64),
    /// Text from the clipboard, for Edit to paste.
    EditPaste(String),
    /// A picture from the clipboard, for Edit to lay over the one edited.
    EditPasteImage(image::RgbaImage),
    /// Edit's picture put on the clipboard, and how large, or why not.
    EditCopied(Result<(u32, u32), String>),
    /// The words read in a box of the picture being edited, or why not.
    EditScanned(Result<String, String>),
    /// Where a picture being edited says what is not for every eye, or why not found.
    EditRedact(usize, Result<Vec<[f32; 4]>, String>),
    /// The straight edges found in a picture being edited, down and across.
    EditEdges(usize, Vec<f32>, Vec<f32>),
    /// A picture saved from Edit, or why not.
    EditSaved(editor::Saved),
    /// Files moved, copied or trashed on another thread.
    FilesDone(filework::FilesDone),
    /// A video's packets, read for Inspect's Timeline.
    Packets(usize, Result<devtools::Packets, String>),
    /// What the Timeline tab looked deeper for: frames near where the video
    /// is, where its scenes cut, its sound.
    TimelineFrames(usize, Result<timeline::Frames, String>),
    TimelineCuts(usize, Result<Vec<f64>, String>),
    TimelineSound(usize, Result<timeline::Sound, String>),
    /// What of the frame paused on is outside BT.709.
    Gamut(Result<checks::GamutMask, String>),
    /// File details read on another thread, for the info and EXIF overlays.
    Meta(usize, Meta),
    /// The font, loaded on another thread while the first image decodes.
    Font(Text),
    /// The files in the directories being watched, after a change.
    Listed(Vec<PathBuf>),
    /// More of the listing, found since the window opened.
    Scanned(Vec<files::Found>),
    /// The listing done.
    ScanDone,
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
    /// A file saved over from Edit, and where its original was kept.
    Replaced { id: usize, path: PathBuf, backup: PathBuf },
}

/// What Undo took back, as Redo does it again. A delete, or files made
/// and then undone, which are deleted, are not among them.
enum Redo {
    /// Files moved, moved back: to be moved again.
    Moved(Vec<Moved>),
    Renamed { id: usize, from: PathBuf, to: PathBuf },
}

/// What a toast offers to be clicked: to undo what it tells of, or to go
/// to the file it made.
#[derive(Clone, PartialEq, Debug)]
enum ToastLink {
    Undo,
    Show(PathBuf),
    /// The Activity panel, open.
    Activity,
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
    /// A seek to a keyframe and the time it was aimed at: the seek bar
    /// stands there, not at the keyframe before, until playback gets there.
    aimed: Option<(Instant, f64)>,
    /// Since when playback has been at or past it, the seek landed: a
    /// moment of that and the bar follows playback again. Straight after
    /// a seek the player may still tell of where it was.
    aim_reached: Option<Instant>,
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
    /// The audio and subtitles playing, in words, for the info overlay.
    track_words: Option<String>,
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

/// A video frame being saved as a picture: where to, what the toast said,
/// the kept frame's time when it is one, and when it was asked for.
struct FrameSave {
    tag: u64,
    to: PathBuf,
    said: String,
    at: Option<f64>,
    asked: Instant,
}

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

/// The seek bar's track: its left end, length and vertical center. `unit`
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
    /// How wide the title at its left may be: none in a window too narrow
    /// for it, where the time alone stands there.
    title_w: i32,
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
    /// A size for the Convert panel to fit the picture under.
    FitSize,
    /// The tracks editor's: a track's language, its title, a file to add.
    TrackLanguage(usize),
    TrackTitle(usize),
    TrackFile,
    /// The name to save what Edit drew as, beside the picture.
    EditCopy,
}

pub struct App {
    /// The folder jog was started in, which its sessions are kept by.
    cwd: PathBuf,
    options: Options,
    /// Path of each file id.
    files: Vec<PathBuf>,
    marks: HashSet<usize>,
    undo: Vec<Undo>,
    /// What Undo took back, the latest last, for Redo; gone once anything
    /// new is done.
    redo: Vec<Redo>,
    /// What the toasts have said, the latest last, for the find box.
    messages: Vec<(Instant, String)>,
    /// The files jumps were made from, for Ctrl+O, and those gone back
    /// from, for Ctrl+I; and whether one of those is being made now.
    jumps_back: Vec<usize>,
    /// The file marked last, where a Shift+click run of marks starts.
    mark_anchor: Option<usize>,
    /// When the session is next kept, so that a crash or a kill loses
    /// little: every minute while jog runs.
    session_due: Instant,
    jumps_forward: Vec<usize>,
    traveling: bool,
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
    /// Where the players draw videos on the GPU (`mpv::gpu_video`): one
    /// for each, by the index the player is known by, the playing one's shown.
    video_windows: [Option<x11::VideoWindow>; 2],
    /// What is drawn over the video in that window, handed to the player.
    video_layer: video_layer::Layer,
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
    strip_button_over: Option<StripButton>,
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
    /// The listing still coming in, when the window opened before it was done.
    scan: Option<scanning::Scanning>,
    /// File sizes looked up for searches such as `>10mb`.
    sizes: HashMap<usize, Option<u64>>,
    /// The last folder the find box matched a file's folder for, and how it
    /// read: files come a folder at a time, so each is made once.
    folder_memo: RefCell<Option<(PathBuf, std::rc::Rc<str>)>>,
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
    /// The window's size when last an ordinary one, and the small size
    /// asked for, to be asked again once a window manager has had its say.
    windowed: Option<(u32, u32)>,
    small_wanted: Option<(u32, u32)>,
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
    /// Which graphs of the picture's colors stand under the debug
    /// overlay, by `scopes::KINDS`; whether they count by logarithm; what
    /// they last counted, and how long counting and drawing them took.
    scopes: [bool; 5],
    /// The checks drawn on the video: zebra, false color, out of gamut.
    checks: [bool; 3],
    /// What is outside BT.709 on the frame paused on, and the frame asked for.
    gamut: Option<checks::GamutMask>,
    gamut_asked: Option<(usize, f64)>,
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
    /// Where the seek bar's title, time and details were written, to be
    /// copied by a click; and each video file's size, read once.
    bar_hits: Vec<([i32; 4], Command)>,
    file_bytes: HashMap<usize, u64>,
    /// How many letters the longest title in the list has, for the
    /// seek bar's room for it: by the list's length and first file.
    title_chars: Cell<Option<(usize, usize, usize)>>,
    /// The format of each image looked at, by its first bytes.
    formats: RefCell<HashMap<usize, Option<String>>>,
    scope_counts: scopes::Measured,
    scope_cost: Duration,
    perf: Perf,
    /// Late frames and stalls, told of on the terminal.
    hitches: hitch::Hitches,
    /// Frames the player is writing, a frame waiting for the player to
    /// get to it before it is, and the tag the last was sent with.
    frames_saving: Vec<FrameSave>,
    frame_waiting: Option<FrameSave>,
    frame_tag: u64,
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
    /// Whether the preview and spare players have been asked for, which
    /// waits on the first video showing.
    helpers_asked: bool,
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
    /// The next video started opens paused: one the player crashed on, put
    /// back as it was.
    resume_paused: bool,
    /// When players crashed lately: one that keeps crashing is not started
    /// again and again.
    player_crashes: Vec<Instant>,
    /// The video on view that crashes the player, once that has been said.
    unplayable_said: Option<usize>,
    /// Whether the pointer has moved since it came into the window: see
    /// `mouse_moved`.
    pointer_found: input::Arrival,
    /// Playback speed, kept from one video to the next.
    speed: f64,
    muted: bool,
    /// What a video that has played to its end does, kept from one video
    /// to the next.
    looping: Looping,
    /// When a run measuring playback for a set time quits by itself.
    bench_quit: Option<Instant>,
    /// The player's own counts of frames dropped, while measuring: as the
    /// script measuring mpv itself counts them.
    bench_drops: BenchDrops,
    bench_keys_played: bool,
    /// What is left of them to play, and when.
    bench_keys: Option<(String, Instant)>,
    /// The second and the bar's handle last drawn, for a video on the GPU.
    time_key: Cell<Option<(i64, i32)>>,
    /// Videos looped on their own, from their row in the file list.
    looped: HashSet<usize>,
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
    /// The recent files the find box lists now, which OpenRecent indexes.
    recent_shown: Vec<PathBuf>,
    shell_typed: Option<String>,
    /// Whether the file list stands at the right of the window, and whether
    /// it is pinned there: the picture then has what is left beside it,
    /// where an unpinned list lies over the picture.
    list_right: bool,
    list_pinned: bool,
    /// The file list has the arrow keys: it was clicked on last.
    list_keys: bool,
    /// Whether the inspector stands beside the picture rather than over it,
    /// and likewise the strip along the bottom below it.
    inspector_pinned: bool,
    /// How wide the file list and the inspector are, in lines of text,
    /// and the edge under the pointer, which it shows as an arrow.
    list_units: f64,
    inspector_units: f64,
    edge_under: Option<PanelEdge>,
    /// The end of a section the pointer is over on the seek bar, to drag.
    flag_under: Option<section::Edge>,
    /// How far each dialog has been moved from the middle, and the heading
    /// of the one drawn last frame, to take hold of.
    dialog_moved: RefCell<HashMap<&'static str, (i32, i32)>>,
    dialog_header: Cell<Option<(&'static str, [i32; 4])>>,
    /// Panels out in windows of their own; one asked for, made on the next
    /// turn of the event loop; and whether one of them is being drawn or
    /// answered just now, rather than the main window.
    popouts: Vec<popout::Popout>,
    popout_wanted: Option<Popped>,
    drawing_popout: Option<Popped>,
    /// The panels that go out in windows of their own when opened, and
    /// where each one's window last was: x, y, width, height.
    out: HashSet<Popped>,
    /// The tab Inspect opens on, the one used last; and a command one of
    /// its buttons asked for, run on the next turn of the event loop.
    inspect_pane: Pane,
    /// Inspect docked along the bottom rather than down the side, and how
    /// tall it is there, in lines of text.
    inspector_bottom: bool,
    /// The drawer under Inspect's tab: the file list, in view whatever
    /// tab is up, as a browser's tools keep their console.
    inspect_drawer: bool,
    drawer_pane: DrawerPane,
    inspector_rows: f64,
    /// Inspect's Convert tab's choices for a video.
    video_convert: convert_tab::VideoConvert,
    /// The file Inspect's Compare tab is comparing with, and what it is
    /// once read.
    compare_found: Option<(usize, Option<Result<inspect::Inspection, String>>)>,
    pending_command: Option<Command>,
    /// Where Inspect's Files tab has the list, as left, top, width,
    /// bottom; and whether it is being drawn there just now.
    inspector_list: Cell<Option<[i32; 4]>>,
    inspector_list_drawing: Cell<bool>,
    popout_at: HashMap<Popped, [i32; 4]>,
    strip_docked: bool,
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
    /// The Convert window for a video.
    video_panel: Option<video_convert::VideoPanel>,
    /// Conversions and saves, running and done, and the panel listing them.
    activity: activity::Activity,
    /// The picker of a video's tracks (Ctrl+J), and the tracks and timing
    /// kept for each video, read once.
    track_picker: Option<tracks::TrackPicker>,
    /// The inspector (F3), of what a file is made of.
    inspector: Option<inspect::Inspector>,
    /// The share of the window the video was last told to keep clear on
    /// each side, left and right, for the inspector beside it.
    video_margins: Cell<(f64, f64, f64)>,
    /// The shortcuts panel as drawn for what was typed in it and the
    /// window's size: where, and its inside.
    help_kept: Option<((String, i32, i32), [i32; 4], Vec<u32>)>,
    /// The tracks editor: a video's streams to keep, order and tag.
    track_editor: Option<track_editor::TrackEditor>,
    /// Edit, while it is open; its marks while a save of them is on its
    /// way, to go back to if it fails.
    editor: Option<editor::Editor>,
    edit_stash: Option<editor::Editor>,
    /// The file `--edit` opens in Edit once it is loaded.
    edit_waiting: Option<usize>,
    track_memory: Option<HashMap<PathBuf, tracks::TrackChoice>>,
    /// The section marked in each video, by file: still there when the
    /// video is come back to.
    sections: HashMap<usize, Section>,
    /// The other sections marked in each video, by file, in no order: each
    /// made the one in use by a click on it.
    other_sections: HashMap<usize, Vec<Section>>,
    /// Where on the seek bar it was right-clicked, in seconds, for its menu.
    bar_menu_at: Option<f64>,
    /// The sections marked when jog was last quit, by video, read once:
    /// the one in use first.
    marked_sections: Option<HashMap<PathBuf, Vec<(f64, f64)>>>,
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
    /// Dragging along the volume slider over its button.
    Volume,
    /// Dragging an end of the section along the seek bar.
    Section(Edge),
    Zoom { start_x: f64, start_scale: f64, anchor: (f64, f64), moved: bool },
    /// Dragging along the bar under the grid, through its pages.
    Pages,
    /// Dragging a panel's edge, to make it wider or the strip taller.
    Resize(PanelEdge),
    /// Moving a dialog by its heading: which, where the pointer took hold
    /// and how far it had been moved then.
    Move { which: &'static str, from: (f64, f64), start: (i32, i32) },
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
    /// Opened above what it is for, as the seek bar's cog's: its own
    /// submenus grow up from their row, not down over the bar.
    upward: bool,
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
    /// How tall the strip is before it is made taller or shorter.
    fn usual_height(window_height: u32) -> i32 {
        (window_height as i32 / 12).clamp(110, 220)
    }

    fn for_height(window_height: u32) -> Strip {
        let height = ((f64::from(Self::usual_height(window_height)) * resize::strip_scale()) as i32).min(window_height as i32 * 2 / 5);
        let pad = height / 10;
        let cell_h = height - 2 * pad;
        Strip { top: window_height as i32 - height, height, pad, cell_w: cell_h * 3 / 2, cell_h }
    }

    /// From one cell to the next. Never 0, which is divided by: a window a
    /// pixel or two high, as one is while it is being made, has no room
    /// for cells at all.
    fn pitch(&self) -> i32 {
        (self.cell_w + self.pad).max(1)
    }

    /// Whether the window has room for its cells: less than a few pixels
    /// high, there is nothing to see in them, and it is not drawn.
    fn has_room(&self) -> bool {
        self.cell_h >= 8
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
        let pins = panels::load();
        let slide_delay = Duration::from_secs_f64(options.slideshow_delay.unwrap_or(DEFAULT_SLIDE_DELAY));
        ACCENT.store(options.accent, atomic::Ordering::Relaxed);
        crate::mpv::DEBUG.store(options.debug, atomic::Ordering::Relaxed);
        crate::mpv::HWDEC.store(options.hwdec, atomic::Ordering::Relaxed);
        // Videos drawn on the GPU where they can be, unless asked not to.
        crate::mpv::choose_gpu_video(options.gpu_video, std::env::var_os("JOG_SNAPSHOT").is_some());
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
            redo: Vec::new(),
            messages: Vec::new(),
            jumps_back: Vec::new(),
            mark_anchor: None,
            session_due: Instant::now() + SESSION_EVERY,
            jumps_forward: Vec::new(),
            traveling: false,
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
            video_windows: [None, None],
            video_layer: Default::default(),
            backdrop_done: false,
            focused_once: false,
            list_scroll: None,
            list_hover: None,
            list_button_over: None,
            strip_over: None,
            strip_scroll: 0,
            strip_button_over: None,
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
            scan: None,
            sizes: HashMap::new(),
            folder_memo: RefCell::new(None),
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
            windowed: None,
            small_wanted: None,
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
            checks: [false; 3],
            gamut: None,
            gamut_asked: None,
            scope_log: true,
            hdr: options.hdr,
            ultra_hdr: RefCell::new(HashMap::new()),
            hdr_infos: HashMap::new(),
            hdr_headroom: 0.0,
            info_buttons: Vec::new(),
            bar_hits: Vec::new(),
            file_bytes: HashMap::new(),
            title_chars: Cell::new(None),
            formats: RefCell::new(HashMap::new()),
            scope_counts: scopes::Measured::default(),
            scope_cost: Duration::ZERO,
            perf: Perf::new(),
            hitches: hitch::Hitches::new(options.hitches),
            frames_saving: Vec::new(),
            frame_waiting: None,
            frame_tag: 0,
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
            helpers_asked: false,
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
            resume_paused: false,
            player_crashes: Vec::new(),
            unplayable_said: None,
            pointer_found: input::Arrival::Coming,
            speed: 1.0,
            muted: false,
            // Measuring playback for a set time: the video loops meanwhile.
            looping: if bench_seconds().is_some() { Looping::File } else { Looping::Off },
            bench_quit: bench_seconds().map(|seconds| Instant::now() + Duration::from_secs_f64(seconds + 1.5)),
            bench_drops: BenchDrops { from: Some(Instant::now() + Duration::from_millis(1500)), ..BenchDrops::default() },
            bench_keys_played: false,
            bench_keys: None,
            time_key: Cell::new(None),
            looped: HashSet::new(),
            pip: false,
            pip_live: None,
            pip_in_cell: false,
            hold: None,
            loupe: None,
            precise: options.precise_seeking,
            shell_history: None,
            visited: RefCell::new(Vec::new()),
            recent_shown: Vec::new(),
            shell_typed: None,
            list_right: options.list_right,
            // Pinned or not as last time, unless the config or the command
            // line pins the list.
            list_pinned: options.list_pinned || pins.list.unwrap_or(false),
            list_keys: false,
            // Inspect beside the picture unless left over it: it is read alongside.
            inspector_pinned: pins.inspector.unwrap_or(true),
            list_units: pins.list_width.unwrap_or(resize::PANEL_UNITS),
            inspector_units: pins.inspector_width.unwrap_or(resize::PANEL_UNITS),
            out: pins.out.clone(),
            inspect_pane: pins.pane.unwrap_or_default(),
            inspector_bottom: pins.bottom,
            inspect_drawer: pins.drawer,
            drawer_pane: pins.drawer_pane.unwrap_or_default(),
            video_convert: Default::default(),
            compare_found: None,
            inspector_rows: pins.rows.unwrap_or(24.0),
            pending_command: None,
            inspector_list: Cell::new(None),
            inspector_list_drawing: Cell::new(false),
            popout_at: pins.at.clone(),
            edge_under: None,
            flag_under: None,
            dialog_moved: RefCell::new(HashMap::new()),
            dialog_header: Cell::new(None),
            popouts: Vec::new(),
            popout_wanted: None,
            drawing_popout: None,
            strip_docked: pins.strip.unwrap_or(false),
            triage_on: false,
            triage: RefCell::new(None),
            verdicts: RefCell::new(HashMap::new()),
            triage_buttons: RefCell::new(Vec::new()),
            convert_panel: None,
            video_panel: None,
            activity: Default::default(),
            track_picker: None,
            inspector: None,
            video_margins: Cell::new((0.0, 0.0, 0.0)),
            help_kept: None,
            track_editor: None,
            editor: None,
            edit_stash: None,
            edit_waiting: None,
            track_memory: None,
            sections: HashMap::new(),
            other_sections: HashMap::new(),
            bar_menu_at: None,
            marked_sections: None,
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
        self.hitches.summary();
        filework::finish();
        // Where a popped-out panel's window was left.
        if !self.popouts.is_empty() {
            self.save_panels_now();
        }
        self.finish_frames();
        self.players().for_each(Player::settle);
        crate::transcode::abandon();
        crate::quit_now();
    }

    fn window_size(&self) -> (u32, u32) {
        self.window.as_ref().map_or((0, 0), |w| w.inner_size().into())
    }

    fn redraw(&self) {
        if let Some(window) = &self.window {
            self.asked.set(true);
            window.request_redraw();
        }
        self.redraw_popouts();
    }

    fn toast(&mut self, message: impl Into<String>) {
        // It slides in from the edge, unless one is already up: then the
        // words change where it stands, so that pressing a key again and
        // again does not set it sliding in each time.
        let showing = self.toast.as_ref().is_some_and(|(_, until)| *until > Instant::now());
        if !showing {
            self.toast_slide.set(Glide { from: 0.0, to: 1.0, since: Instant::now() });
        }
        let message = message.into();
        // Kept for the log: an encode's progress only as its latest word.
        let progress = self.messages.last().is_some_and(|(_, last)| last.contains('%') && message.contains('%') && last.split("  ·  ").next() == message.split("  ·  ").next());
        if progress {
            self.messages.pop();
        }
        self.messages.push((Instant::now(), message.clone()));
        if self.messages.len() > 200 {
            self.messages.remove(0);
        }
        self.toast = Some((message, Instant::now() + TOAST));
        self.toast_links.clear();
        self.redraw();
    }

    /// The toasts of late, the latest first, as the find box lists
    /// commands: each with how long ago, Enter copying it.
    fn messages_matching(&self, typed: &str) -> Vec<(String, &'static str, Command)> {
        let typed = typed.trim().to_lowercase();
        let ago = |at: Instant| match at.elapsed().as_secs() {
            0..=59 => "just now".to_string(),
            seconds @ 60..=3599 => format!("{} min ago", seconds / 60),
            seconds => format!("{} h ago", seconds / 3600),
        };
        (self.messages.iter().enumerate().rev())
            .filter(|(_, (_, said))| typed.is_empty() || said.to_lowercase().contains(&typed))
            .map(|(at, (when, said))| (format!("{said}  ·  {}", ago(*when)), "", Command::CopyMessage(at)))
            .collect()
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
            ToastLink::Activity => {
                self.activity.open = true;
                self.redraw();
            }
        }
        true
    }

    /// Goes to a file, by its path: one just made, say.
    fn show_path(&mut self, path: &Path) {
        let Some(id) = self.files.iter().position(|file| file == path) else {
            return self.toast(format!("Not in the list · {}", path.display()));
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
    /// then its neighbors, favoring the way `forward` says we are
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
        let mut wanted = Self::around(&self.list, self.pos, self.forward, self.loader.workers());
        // A video that plays gets no still: black until the player has its
        // first frame, rather than a frame held still and then played on
        // from, which looks stuck. One the player cannot play keeps its. A
        // scripted run's timetable starts from the video's first frame then.
        wanted.retain(|id| !self.playable(*id));
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
                let center = self.strip_center();
                for at in [center + distance, center - distance] {
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
        self.follow_inspector();
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
                // `--edit`, now that there is a picture to edit.
                if self.edit_waiting.take_if(|waiting| *waiting == id).is_some() {
                    self.open_editor();
                }
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
        // A jump, not a step, is one to come back from with Ctrl+O.
        if pos.abs_diff(self.pos) > 1 && !self.traveling {
            if let Some(&id) = self.list.get(self.pos) {
                self.jumps_back.retain(|had| *had != id);
                self.jumps_back.push(id);
                if self.jumps_back.len() > 100 {
                    self.jumps_back.remove(0);
                }
                self.jumps_forward.clear();
            }
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

    /// Ctrl+O and Ctrl+I: back to where the last jump was from, and on
    /// again, as a browser's Back and Forward. Files no longer listed are
    /// passed over.
    fn travel(&mut self, back: bool) {
        let current = self.list.get(self.pos).copied();
        loop {
            let popped = if back { self.jumps_back.pop() } else { self.jumps_forward.pop() };
            let Some(id) = popped else {
                return self.toast(if back { "No jump to go back to" } else { "Nothing ahead" });
            };
            let Some(pos) = self.list.iter().position(|&other| other == id) else { continue };
            if let Some(current) = current {
                if back { self.jumps_forward.push(current) } else { self.jumps_back.push(current) }
            }
            self.traveling = true;
            self.go_to(pos, !back);
            self.traveling = false;
            return;
        }
    }

    // ---- View ----------------------------------------------------------

    fn native(&self) -> Option<(u32, u32)> {
        self.played_size(self.id()).or_else(|| self.shown.as_ref().map(|(_, frame)| frame.native))
    }

    /// The size of video `id` as it is shown, whichever way it is turned,
    /// by its maker and by a turn here: the player's, once it has it open.
    /// A video that plays has no still to tell it.
    pub(super) fn played_size(&self, id: usize) -> Option<(u32, u32)> {
        let player = self.player.as_ref().filter(|_| self.video.as_ref().is_some_and(|video| video.id == id))?;
        let turns = self.transforms.get(&id).copied().unwrap_or_default().turns();
        let rotate = player.number("video-params/rotate").unwrap_or(0.0) + 90.0 * f64::from(turns);
        shown_size(player.number("width"), player.number("height"), Some(rotate))
    }

    /// The size of file `id`: a video's as the player has it, or else its
    /// still's.
    pub(super) fn size_of(&self, id: usize) -> Option<(u32, u32)> {
        self.played_size(id).or_else(|| self.shown.as_ref().filter(|(shown, _)| *shown == id).map(|(_, frame)| frame.native))
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
    /// does its own scaling, laid out at the size the video is shown at.
    fn apply_video_view(&self) {
        let (Some(player), Some(video)) = (&self.player, &self.video) else { return };
        let window = self.window_size();
        let native = self.size_of(video.id);
        let (zoom, pan) = match native {
            // In its corner of the grid it is simply all there.
            Some(native) if window.0 > 0 && window.1 > 0 && self.grid.is_none() => {
                let mut layout = self.layout_of(native);
                // The player draws within margins kept for the pinned list
                // and the inspector: the same part of the window the layout
                // is in, its zoom and pan measured against that part.
                let (left, window) = self.stage();
                layout.x -= left;
                // mpv's zoom is a power of two on top of filling that part.
                let filling = fit_size(native, window, true);
                // Less than a pixel either way is the layout's rounding, and
                // not passed on: each change has the player's renderer set
                // itself up again, which for a large video costs ten frames'
                // time, and resizing the window would change it every time.
                let zoom = (f64::from(layout.width) / f64::from(filling.0)).log2();
                let zoom = if layout.width.abs_diff(filling.0) <= 1 { 0.0 } else { zoom };
                // Its pan is the offset from center as a share of the scaled size.
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

    /// Where the video's picture lies in the window: fitted and centered,
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
        // Out of it, to the small window, not to one the size of what
        // filled the screen.
        if window.fullscreen().is_some() {
            return self.go_small();
        }
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
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
                self.toast(if self.view.enlarge { "Small images: fit" } else { "Small images: actual size" });
                self.view_changed();
            }
            ToggleKeepView => {
                self.keep_view = !self.keep_view;
                self.toast(if self.keep_view { "Zoom kept between files" } else { "Zoom resets per file" });
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
            SectionLoop => self.toggle_section_loop(),
            SectionPlay => self.play_section(),
            SectionConvert => self.section_tool(section::SectionTool::Convert),
            SectionClear => {
                self.clear_section();
            }
            SectionStartAt | SectionEndAt | PlayFromHere => {
                let Some(at) = self.bar_menu_at.take().or_else(|| self.video_time().map(|(at, _)| at)) else { return };
                match command {
                    SectionStartAt => self.mark_edge_at(Edge::A, at, true),
                    SectionEndAt => self.mark_edge_at(Edge::B, at, false),
                    _ => self.play_from(at),
                }
            }
            SectionClearAll => self.clear_all_sections(),
            SaveSection(clip) => self.save_section(clip, crate::transcode::Knobs::default()),
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
                self.toast(if self.antialias { "Smooth scaling" } else { "Pixelated scaling" });
            }
            Render => {
                self.settle_at = None;
                self.redraw();
            }
            ToggleFullscreen => self.toggle_fullscreen(),
            SmallWindow => self.go_small(),
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
                self.toast(if self.options.verbose { "Verbose on" } else { "Verbose off" });
            }
            ToggleDebugLog => {
                self.options.debug = !self.options.debug;
                self.loader.set_debug(self.options.debug);
                crate::mpv::DEBUG.store(self.options.debug, atomic::Ordering::Relaxed);
                self.toast(if self.options.debug { "Debug on" } else { "Debug off" });
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
                    self.toast(format!("Slideshow  ·  {}s", self.slide_delay.as_secs_f64()));
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
                // Out in its own window, if that is where it was last.
                if self.show_list {
                    self.reopen_out(Popped::List);
                }
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
                self.save_panels();
                self.apply_video_view();
            }
            ListFlip => {
                self.list_right = !self.list_right;
                self.apply_video_view();
            }
            ListPopOut => self.toggle_popout(Popped::List),
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
            Check(kind) => self.toggle_check(kind),
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
                    false => format!("Ultra HDR: {}  ·  not this one", self.hdr.name()),
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
            CommandPalette => self.open_palette_with(">"),
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
                self.toast(format!("Saved {copy}"));
                let item = Moved { id: self.id(), pos: self.pos, from: self.path().to_path_buf(), to: PathBuf::from(copy) };
                self.file_work(filework::Work::Save, vec![item]);
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
                    Err(error) => self.toast(format!("Not saved · {error}")),
                }
            }
            Action(slot) => self.run_action(slot),
            Mark => self.toggle_mark(),
            MarkMenu => self.open_mark_menu(),
            SortMenu => self.open_palette_with(">sort "),
            Workspace(to) => self.switch_space(to),
            WorkspaceKey(key) => match self.spaces.iter().position(|space| space.key == Some(key)) {
                Some(to) => self.switch_space(to),
                None => self.toast(format!("Nothing on Ctrl+{key}")),
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
            // As `#image` or `#video` typed and kept, staying on the file
            // on view if it is one of them.
            OnlyImages | OnlyVideos => {
                let typed = if command == OnlyVideos { "#video" } else { "#image" };
                let (matches, _) = self.matching(typed);
                if matches.is_empty() {
                    return self.toast(if command == OnlyVideos { "No videos" } else { "No pictures" });
                }
                let picked = Some(self.id()).filter(|id| matches.contains(id)).or(matches.first().copied());
                self.keep_matches(typed.to_string(), matches, picked);
            }
            MarkAll => {
                self.marks = self.list.iter().copied().collect();
                self.toast(format!("{} marked", self.marks.len()));
            }
            InvertMarks => {
                self.marks = self.list.iter().copied().filter(|id| !self.marks.contains(id)).collect();
                self.toast(format!("{} marked", self.marks.len()));
            }
            ClearMarks => {
                let count = self.marks.len();
                self.marks.clear();
                self.toast(format!("{count} unmarked"));
            }
            CopyName => self.copy_text(false),
            CopyViewedFolder | CopyBarTitle | CopyTime | CopyDetail(_) | CopyDetails => self.copy_bar(command),
            Activity => self.toggle_activity(),
            CopyViewedName | CopyViewedPath | CopyViewedSize | CopyMessage(_) => {
                let name = match command {
                    CopyViewedPath => {
                        let path = &self.files[self.id()];
                        std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()).display().to_string()
                    }
                    CopyViewedSize => self.native().map(|(w, h)| format!("{w}x{h}")).unwrap_or_default(),
                    CopyMessage(at) => self.messages.get(at).map(|(_, said)| said.clone()).unwrap_or_default(),
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
            Redo => self.redo(event_loop),
            TravelBack => self.travel(true),
            TravelForward => self.travel(false),
            Rename => self.start_rename(),
            Delete => self.ask_delete(),
            Convert(format) => self.convert(format),
            ConvertPanel => self.open_convert_panel(),
            ConvertVideo(target) => self.convert_videos(target, crate::transcode::Knobs::default()),
            Compare => self.open_compare(),
            SpeedUp => self.change_speed(Some(true)),
            SpeedDown => self.change_speed(Some(false)),
            SpeedReset => self.change_speed(None),
            SetSpeed(at) => self.set_speed(SPEEDS[usize::from(at).min(SPEEDS.len() - 1)]),
            PlaybackSettings => {
                self.open_playback_menu();
                self.redraw();
            }
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
                self.toast(if self.precise { "Precise seeking" } else { "Keyframe seeking" });
            }
            ToggleScenes => self.cycle_strip(),
            CycleSubtitles => self.cycle_subtitles(),
            CycleAudio => self.cycle_audio(),
            TrackPicker => self.open_track_picker(),
            Inspect => self.open_inspector(),
            EditTracks => self.open_track_editor(),
            Edit => self.open_editor(),
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
                self.toast(if self.triage_on { "Triage  ·  Space keeps  ·  x rejects" } else { "Triage off  ·  decisions kept" });
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
            OpenRecent(at) => {
                let Some(path) = self.recent_shown.get(at).cloned() else { return };
                // One from another folder, or from a run before, joins the
                // list to be gone to.
                let listed = self.files.iter().position(|file| std::path::absolute(file).ok().as_ref() == Some(&path));
                match listed {
                    Some(id) => {
                        let path = self.files[id].clone();
                        self.show_path(&path);
                    }
                    None => {
                        self.adopt_created(std::slice::from_ref(&path));
                        self.show_path(&path);
                    }
                }
            }
            RunShell(at) => {
                if let Some(command) = self.shell_history.get_or_insert_with(history::load).get(at).cloned() {
                    self.run_shell(&command);
                }
            }
            RepeatShell => match self.shell_history.get_or_insert_with(history::load).first().cloned() {
                Some(command) => self.run_shell(&command),
                None => self.toast("No command yet  ·  ! to type one"),
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
                self.toast(format!("Nothing on {slot}"));
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
            Err(error) => self.toast(format!("Action {slot} failed · {error}")),
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
            Check(kind) => self.checks[kind],
            ToggleWatch => self.watch.is_some(),
            Workspace(which) => which == self.space,
            ToggleList => self.show_list,
            ToggleGridTitles => self.grid_titles,
            ToggleMute => self.muted,
            SetSpeed(at) => SPEEDS.get(usize::from(at)).is_some_and(|speed| (speed - self.speed).abs() < 1e-9),
            CycleLoop => self.looping != Looping::Off,
            Triage => self.triage_on,
            TogglePrecise => self.precise,
            ToggleScenes => self.scenes_on,
            CycleSubtitles => self.subtitles,
            ListPin => self.list_pinned,
            ListPopOut => self.popped(Popped::List),
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

    // ---- Sessions --------------------------------------------------------

    /// Keeps what is on view as a session of the folder jog was started
    /// in, to be picked up with `--resume`.
    pub fn save_session(&mut self) {
        self.save_recent();
        self.save_marked_sections();
        self.save_window_size();
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
        let (mut left, mut width) = (0, width as i32);
        if let Some(panel) = self.list_panel().filter(|panel| self.list_pinned && !panel.embedded) {
            width -= panel.width;
            left = if self.list_right { 0 } else { panel.width };
        }
        // The inspector beside it, on the side the list is not.
        if let Some((room, on_left)) = self.inspector_room() {
            width -= room;
            if on_left {
                left += room;
            }
        }
        // The strip along the bottom, pinned below it.
        let height = if self.band_docked() { (height as i32 - Strip::for_height(height).height).max(1) as u32 } else { height };
        // And Inspect, docked along the bottom.
        let height = self.inspector_bottom_room().map_or(height, |room| (height as i32 - room).max(1) as u32);
        (left, (width.max(1) as u32, height))
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
    /// by the width of the file list, when that is on the right, and of the
    /// Activity panel while it is up.
    fn right_inset(&self) -> i32 {
        self.list_right_inset() + self.activity_width()
    }

    /// The file list's width, when it is on the right.
    fn list_right_inset(&self) -> i32 {
        self.list_panel().filter(|panel| self.list_right && !panel.embedded).map_or(0, |panel| panel.width)
    }

    /// The Activity panel's width, while it is up.
    fn activity_width(&self) -> i32 {
        let (width, height) = self.window_size();
        if self.activity.open { draw::panel_width(width as i32, ui_px(height), 30.0) } else { 0 }
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
            (Some(error), _) => format!("Not run · {error}"),
            (None, 1) => format!("{}  {}", commands::icons::TERMINAL, short(template)),
            (None, count) => format!("{}  {count} files  ·  {}", commands::icons::TERMINAL, short(template)),
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
        Some([px / 2, panel.head + px / 2, right - px / 2 - px / 3, side])
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
        let native = self.size_of(video.id).unwrap_or((16, 9));
        let area = self.grid_area(&self.grid_layout(window), self.pos, window.1);
        let place = Self::grid_picture(area, native, true);
        (place[0] >= 0 && place[0] + place[2] <= window.0 as i32).then_some(place)
    }

    /// Where, at the left of the line under the grid, the small picture
    /// of the video playing stands: there on every page, whether or not
    /// its own cell is on the one showing.
    fn pip_small(&self, window: (u32, u32)) -> Option<[i32; 4]> {
        let video = self.video.as_ref().filter(|_| self.pip && self.grid.is_some())?;
        let native = self.size_of(video.id).unwrap_or((16, 9));
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
        if player.render_latest(&mut pixels, w as u32, h as u32) {
            // Its full size is the video's, for when it grows out of the cell.
            let native = self.size_of(self.id()).unwrap_or((w as u32, h as u32));
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
        // In Edit, Space let go: drags draw and pick out again.
        if let (Some(editor), Key::Named(NamedKey::Space)) = (&mut self.editor, &key) {
            return editor.let_go_of_space();
        }
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
    /// Where its heading starts: the top of the window, or below the tabs
    /// of Inspect, whose Files tab it is drawn in, `embedded`.
    head: i32,
    embedded: bool,
}

impl ListPanel {
    /// Whether a window position is on the list.
    fn holds(&self, x: f64, y: f64) -> bool {
        (self.left..self.left + self.width).contains(&(x as i32)) && (self.head..self.bottom).contains(&(y as i32))
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

/// An error about a file, short enough for a toast: the file's own folder
/// left out of the paths in it.
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

/// A folder as it is written in a menu: `~` for the home directory, and
/// only its ends when it is a long one.
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

/// How long picture `index` of an animated frame stays up.
fn picture_delay(frame: &Frame, index: usize) -> Duration {
    let delays = frame.animation.as_ref().map(|animation| &animation.delays[..]).unwrap_or_default();
    delays.get(index).or(delays.first()).copied().unwrap_or(Duration::from_millis(100))
}

/// A frame rate as it is usually written: 60, 29.97, 23.976.
/// A video's size on the screen from the player's width, height and
/// rotation: turned a quarter, as phones record upright video, the other
/// way round. None until the player knows it.
fn shown_size(width: Option<f64>, height: Option<f64>, rotate: Option<f64>) -> Option<(u32, u32)> {
    let (w, h) = width.zip(height).filter(|(w, h)| *w >= 1.0 && *h >= 1.0)?;
    let turned = rotate.is_some_and(|degrees| (degrees as i64).rem_euclid(180) == 90);
    Some(if turned { (h as u32, w as u32) } else { (w as u32, h as u32) })
}

fn fps_words(fps: f64) -> String {
    if (fps - fps.round()).abs() < 0.005 {
        return format!("{:.0}", fps);
    }
    let words = format!("{fps:.3}");
    words.trim_end_matches('0').to_string()
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

/// The tracks of one kind, `audio` or `sub`, in the file a player has open:
/// from the track list the player keeps jog told of.
fn tracks_of(player: &Player, kind: &str) -> Vec<Track> {
    let kind = match kind {
        "audio" => tracks::TrackKind::Audio,
        "sub" => tracks::TrackKind::Sub,
        _ => tracks::TrackKind::Video,
    };
    tracks::track_list(player)
        .into_iter()
        .filter(|track| track.kind == kind)
        .map(|track| {
            let named: Vec<String> = [track.lang, track.title].into_iter().flatten().collect();
            let name = if named.is_empty() { format!("Track {}", track.id) } else { named.join("  ") };
            Track { id: track.id, name, selected: track.selected }
        })
        .collect()
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
        let began = Instant::now();
        let what = hitch::event_name(&event);
        self.handle_event(event_loop, event);
        self.hitch_turn(began, what);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let began = Instant::now();
        // A panel's own window, popped out.
        if self.popout_event(id, &event, event_loop) {
            return self.hitch_turn(began, "a popped-out panel");
        }
        let what = match &event {
            WindowEvent::RedrawRequested => "drawing".to_string(),
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                let name = hitch::key_name(event.logical_key.as_ref(), self.modifiers);
                if !event.repeat {
                    self.hitches.input(name.clone());
                }
                name
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                let name = if *button == MouseButton::Right { "right-click" } else { "a click" }.to_string();
                self.hitches.input(name.clone());
                name
            }
            WindowEvent::MouseWheel { .. } => "the wheel".to_string(),
            WindowEvent::Resized(_) => "a resize".to_string(),
            _ => "the window".to_string(),
        };
        self.handle_window_event(event_loop, event);
        self.hitch_turn(began, &what);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let began = Instant::now();
        // What the listing found while a search box was open, now it is not.
        self.scan_catch_up();
        self.waiting(event_loop);
        self.hitch_turn(began, "timers");
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.hitches.summary();
        filework::finish();
        self.finish_frames();
        // Nor a frame being drawn ahead of the next video.
        self.players().for_each(Player::settle);
    }
}

impl App {
    fn handle_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
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
            Event::ConvertSize(id, format, said, size) => self.convert_size(id, format, said, size),
            Event::ConvertFit(id, format, found, target) => self.convert_fit(id, format, found, target),
            // With a Cancel to click, for as long as it goes on.
            Event::JobProgress(job, share, words) => self.job_progress(job, share, words),
            Event::VideoTrial(trial, result) => self.convert_tried(trial, result),
            Event::VideoFacts(id, facts) => self.video_facts(id, facts),
            Event::VideoCompared(trial, result) => self.video_compared(trial, result),
            Event::EntryFrame(found) => self.entry_frame(found),
            Event::Video(which, signal) if which == self.active => {
                self.video_signal(signal);
                // A video has no still: a scripted run's timetable starts
                // from its first frame instead.
                if self.video.as_ref().is_some_and(|video| video.showing && video.id == self.id()) {
                    self.start_snapshot(event_loop);
                }
                self.tend_back();
                self.tend_gamut();
            }
            Event::BackFrames(ticket, frames, took) => self.back_frames(ticket, frames, took),
            Event::CompareLoaded(ticket, images) => self.compare_loaded(ticket, images),
            Event::CompareScore(ticket, score) => self.compare_score(ticket, score),
            Event::CompareDelta(ticket, delta) => self.compare_delta(ticket, delta),
            Event::Inspected(id, found) => self.inspected(id, found),
            Event::Compared(id, found) => self.compared(id, found),
            Event::TracksWritten(result, replaced, job) => self.tracks_written(result, replaced, job),
            Event::EditSaved(saved) => self.edit_saved(saved),
            Event::EditPaste(words) => self.edit_paste(words),
            Event::EditPasteImage(picture) => self.edit_paste_image(picture),
            Event::EditCopied(copied) => self.edit_copied(copied),
            Event::EditScanned(words) => self.edit_scanned(words),
            Event::EditRedact(id, found) => self.edit_redact(id, found),
            Event::EditEdges(id, down, across) => self.edit_edges(id, down, across),
            Event::FilesDone(done) => self.files_done(done, event_loop),
            Event::Packets(id, packets) => self.packets_read(id, packets),
            Event::TimelineFrames(id, frames) => self.timeline_frames(id, frames),
            Event::TimelineCuts(id, cuts) => self.timeline_cuts(id, cuts),
            Event::TimelineSound(id, sound) => self.timeline_sound(id, sound),
            Event::Gamut(mask) => self.gamut_read(mask),
            Event::Video(_, signal) => self.spare_signal(signal),
            Event::Spare(spare) => {
                // Drawing on the GPU, it has a window of its own too, out of sight
                // until it takes over.
                if let Ok(spare) = &spare {
                    if let Some(id) = self.video_window_id(usize::from(self.active ^ 1)) {
                        spare.set("wid", &id.to_string());
                    }
                }
                self.spare = spare.ok().map(|player| Spare { player, id: None, from: 0.0 });
                self.apply_subtitles();
                self.apply_track_prefs();
                self.apply_checks();
                // As for the first: it may have queued events unheard.
                self.spare_signal(Signal::Events);
                self.preload_video();
            }
            Event::Player(player) => self.player_ready(player, event_loop),
            Event::Preview(signal) => self.preview_signal(signal),
            // Without it the seek bar still works, just without pictures.
            Event::PreviewPlayer(preview) => {
                self.preview = preview.ok();
                // Whatever it had open went with the one before.
                self.preview_id = None;
                self.preview_busy = None;
                // As for the player: empty the queue so that mpv signals.
                self.preview_signal(Signal::Events);
            }
            Event::Listed(listed) => self.take_listing(listed, true),
            Event::Scanned(found) => self.take_scanned(found),
            Event::ScanDone => self.scan_done(),
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

    fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.leave(event_loop),
            WindowEvent::RedrawRequested => {
                let _drawing = crate::mpv::drawing();
                self.draw();
            }
            WindowEvent::Resized(size) => {
                if self.options.debug {
                    eprintln!("jog: window resized to {}x{}", size.width, size.height);
                }
                self.update_monitor();
                self.settle_small(size);
                self.note_window_size(size);
                self.menu = None;
                self.request();
                // A video is drawn at its old size, and with its old zoom and
                // pan, until this one holds: with a panel docked those change
                // at every step of a drag, and each change has the player set
                // its renderer up again. The new ones go once it settles.
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
                // The pointer coming in is told where it is next: no move.
                if let WindowEvent::CursorEntered { .. } = event {
                    self.pointer_found = input::Arrival::Coming;
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                // Shift or Alt pressed in the middle of a drag in Edit
                // changes what is being dragged at once, not at the next
                // move of the pointer.
                if self.editor.as_ref().is_some_and(|editor| editor.dragging()) {
                    self.editor_motion();
                }
            }
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
                // Into the window a video is drawn in on the GPU, which is inside
                // this one, the pointer has not left.
                let within = self.video_window_shown() && self.window.as_ref().and_then(|window| x11::pointer_over(window)) == Some(true);
                if within {
                    return;
                }
                self.pointer_found = input::Arrival::Coming;
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

    fn waiting(&mut self, event_loop: &ActiveEventLoop) {
        self.count_bench_drops();
        // `JOG_BENCH_KEYS`: played when the measuring begins, as a scripted
        // run plays its keys, a `{Wait}` being half a second: the pointer
        // moved, say, for what it brings up over the video.
        if self.bench_quit.is_some() && self.bench_drops.from.is_some_and(|from| Instant::now() >= from) {
            if !std::mem::replace(&mut self.bench_keys_played, true) {
                self.bench_keys = std::env::var("JOG_BENCH_KEYS").ok().map(|keys| (keys, Instant::now()));
            }
            if let Some((script, due)) = self.bench_keys.take() {
                if Instant::now() < due {
                    self.bench_keys = Some((script, due));
                } else {
                    let (keys, later) = script.split_once("{Wait}").unwrap_or((&script, ""));
                    let later = (!later.is_empty()).then(|| (later.to_string(), Instant::now() + Duration::from_millis(500)));
                    let keys = keys.to_string();
                    self.bench_keys = later;
                    self.play_keys(&keys, event_loop);
                }
            }
        }
        if self.bench_quit.is_some_and(|at| Instant::now() >= at) {
            self.say_bench_drops();
            // `JOG_BENCH_SHOT=FILE`: the window as the player shows it, with
            // what is drawn over it, for checking a run that no one watched.
            if let (Ok(file), Some(player)) = (std::env::var("JOG_BENCH_SHOT"), &self.player) {
                player.command(&[b"screenshot-to-file", file.as_bytes(), b"window"]);
            }
            self.bench_quit = None;
            return self.leave(event_loop);
        }
        let now = Instant::now();
        self.tend_popouts(event_loop);
        // What one of Inspect's buttons asked for.
        if let Some(command) = self.pending_command.take() {
            self.execute(command, event_loop);
            self.redraw();
        }
        self.hitches.flush(Some(now));
        if self.settle_at.is_some_and(|at| at <= now) {
            self.settle_at = None;
            // The view for the size the window has settled at.
            self.apply_video_view();
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
        // The session kept now and then, not only on quitting; a
        // remembered video position too.
        if self.session_due <= now {
            self.session_due = now + SESSION_EVERY;
            if self.snapshot.is_none() && self.bench.is_none() {
                self.remember_position();
                self.save_session();
            }
        }
        let flicker_due = self.tend_flicker(now);
        // Inspect's panel for the file before, held up too long.
        let held_until = self.inspector.as_ref().and_then(|inspector| inspector.held_until());
        if held_until.is_some_and(|at| at <= now) {
            self.redraw();
        }
        let held_until = held_until.filter(|at| *at > now);
        let deadlines = [Some(self.session_due), flicker_due, held_until, self.settle_at, toast_until, self.time_until, self.slide_at, snapshot_at, bench_at, self.picture_due, self.font_due, self.grid_due, self.anim_due, toast_leaves, hold_due, self.trailer_due.map(|(_, at)| at), self.overview_due, self.back_due, self.hitches.due(), self.bench_quit, self.bench_keys.as_ref().map(|(_, at)| *at)];
        let next = deadlines.into_iter().flatten().min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_videos_size_is_the_players_turned_as_it_is_shown() {
        // Nothing until the player knows it: zoom has nothing to go by.
        assert_eq!(shown_size(None, None, None), None);
        assert_eq!(shown_size(Some(0.0), Some(0.0), None), None);
        assert_eq!(shown_size(Some(3840.0), Some(2160.0), None), Some((3840, 2160)));
        assert_eq!(shown_size(Some(3840.0), Some(2160.0), Some(180.0)), Some((3840, 2160)));
        // A phone's upright video, turned a quarter either way.
        assert_eq!(shown_size(Some(1920.0), Some(1080.0), Some(90.0)), Some((1080, 1920)));
        assert_eq!(shown_size(Some(1920.0), Some(1080.0), Some(270.0)), Some((1080, 1920)));
    }

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
    fn frame_rates_read_as_usually_written() {
        assert_eq!(fps_words(60.0), "60");
        assert_eq!(fps_words(30000.0 / 1001.0), "29.97");
        assert_eq!(fps_words(24000.0 / 1001.0), "23.976");
        assert_eq!(fps_words(19001.0 / 317.0), "59.94");
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

/// `JOG_FRAMESTATS=SECONDS`: play for that long after settling, looping,
/// then quit, saying how it went; as `JOG_FRAMESTATS=1`, it only says so on
/// quitting.
fn bench_seconds() -> Option<f64> {
    std::env::var("JOG_FRAMESTATS").ok()?.parse::<f64>().ok().filter(|seconds| *seconds >= 2.0)
}

/// Frames the player dropped, counted while measuring.
#[derive(Default)]
struct BenchDrops {
    from: Option<Instant>,
    last: [Option<f64>; 2],
    drops: [f64; 2],
    fps: f64,
}

impl App {
    /// The player's drop counts gone up since last looked at: each goes
    /// back to nothing when the video starts over, which is not counted.
    fn count_bench_drops(&mut self) {
        let Some(from) = self.bench_drops.from.filter(|_| self.bench_quit.is_some()) else { return };
        if Instant::now() < from {
            return;
        }
        let Some(player) = &self.player else { return };
        let now = [player.number("decoder-frame-drop-count"), player.number("frame-drop-count")];
        if let Some(fps) = player.number("container-fps") {
            self.bench_drops.fps = fps;
        }
        for (at, value) in now.into_iter().enumerate() {
            let Some(value) = value else { continue };
            if let Some(last) = self.bench_drops.last[at].filter(|last| value > *last) {
                self.bench_drops.drops[at] += value - last;
            }
            self.bench_drops.last[at] = Some(value);
        }
    }

    /// The line on quitting a run measuring playback: what the player says
    /// it dropped, against the frames the time called for.
    fn say_bench_drops(&self) {
        let Some(from) = self.bench_drops.from else { return };
        let wall = from.elapsed().as_secs_f64();
        let expected = wall * if self.bench_drops.fps > 0.0 { self.bench_drops.fps } else { 60.0 };
        let [dec, vo] = self.bench_drops.drops;
        eprintln!(
            "PLAYERSTATS dec_drop={dec:.0} vo_drop={vo:.0} expected={expected:.0} missed_pct={:.1} fps={:.1} gpu={} hwdec={} drawn_at={}x{} paused={}",
            100.0 * (dec + vo) / expected.max(1.0),
            (expected - dec - vo) / wall.max(1e-9),
            crate::mpv::gpu_video(),
            self.player.as_ref().and_then(|player| player.text("hwdec-current")).unwrap_or_default(),
            self.player.as_ref().and_then(|player| player.number("osd-width")).unwrap_or(0.0),
            self.player.as_ref().and_then(|player| player.number("osd-height")).unwrap_or(0.0),
            self.player.as_ref().and_then(|player| player.flag("pause")).unwrap_or(false)
        );
    }
}
