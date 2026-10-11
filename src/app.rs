use std::cell::RefCell;
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
use winit::window::{Fullscreen, Window, WindowId};

use crate::commands::{self, Applies, Command, MenuItem};
use crate::convert::{self, Format};
use crate::files::{self, Sort};
use crate::frame::{fit_size, Frame, Transform};
use crate::loader::{Job, Loader, Request};
use crate::meta::{self, Meta};
use crate::mpv::{Player, PlayerEvent, Role, Signal};
use crate::ops;
use crate::options::{LastSlide, Options, Start};
use crate::positions::Positions;
use crate::render;
use crate::rng::Rng;
use crate::search::{Facts, Query};
use crate::story::{self, Found, Stories};
use crate::trailer::{Note, Trailers};
use crate::ui::{Canvas, Text};
use crate::view::{Layout, View, Zoom};

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
const DEFAULT_SLIDE_DELAY: f64 = 3.0;
/// How often a slideshow checks whether the video it is waiting on is over.
const VIDEO_END_POLL: Duration = Duration::from_millis(200);
/// Seconds that `<` and `>` move through a video.
const SEEK_STEP: f64 = 5.0;
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
/// Seconds that Page Up and Page Down move through a video.
const SEEK_JUMP: f64 = 60.0;
/// How long after a seek the player's reported position cannot be trusted.
const SEEK_SETTLE: Duration = Duration::from_millis(500);

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
const GRID_COLOR: u32 = 0x101010;

/// What background threads send to the event loop.
pub enum Event {
    Loaded(Job),
    Converted { created: Vec<PathBuf>, failed: usize, format: Format },
    Video(Signal),
    Player(Result<Player, String>),
    /// The same pair for the player that fetches seek-bar previews.
    Preview(Signal),
    PreviewPlayer(Result<Player, String>),
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
    /// Word from the thread playing the grid's trailer.
    Trailer(Note),
    /// The files of a workspace not yet gone to, listed for the overview.
    SpaceListed(usize, Vec<PathBuf>),
}

/// A file operation that `u` can take back.
enum Undo {
    /// Files written by a copy or a conversion.
    Created(Vec<PathBuf>),
    Moved(Vec<Moved>),
    Renamed { id: usize, from: PathBuf, to: PathBuf },
}

/// A file moved out of the list: where it sat and where it went.
struct Moved {
    id: usize,
    pos: usize,
    from: PathBuf,
    to: PathBuf,
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
    /// When the user last seeked, stepped or paused, or a seek landed.
    touched: Option<Instant>,
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
        (self.unit * 3 / 2 + which * (side + self.unit), self.middle - side / 2, side)
    }

    /// Whether a window position is on the symbol, with some slack around
    /// it to make it easy to hit.
    fn on_symbol(&self, x: f64, y: f64) -> bool {
        self.on_control(1, x, y)
    }

    fn on_control(&self, which: i32, x: f64, y: f64) -> bool {
        let (left, top, side) = self.control(which);
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
    /// Carry on with every file, dropping a search kept earlier.
    All,
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
    /// Whether its files have been asked for, to show in the overview.
    asked: bool,
}

/// What a workspace was showing when it was left, to come back to.
struct Kept {
    list: Vec<usize>,
    pos: usize,
    cursor: usize,
    filter: Option<(String, Vec<usize>)>,
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
#[derive(Clone, Copy)]
enum PromptKind {
    /// A new name for file `id`.
    Rename,
    /// A name to save the folder on view under, as a workspace.
    SaveWorkspace,
    /// A folder to move the target files into, or to copy them into.
    SendTo { moving: bool },
}

pub struct App {
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

    show_info: bool,
    /// Whether the grid names each file under its picture.
    grid_titles: bool,
    show_debug: bool,
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
    snapshot: Option<Snapshot>,
    /// Created the first time a video comes up.
    player: Option<Player>,
    /// Set once the player has been asked for, so that happens only once.
    player_asked: bool,
    video: Option<Video>,
    /// A second, muted player that fetches the frames shown while the
    /// pointer is over the seek bar, the file it has open, and its picture.
    preview: Option<Player>,
    preview_id: Option<usize>,
    preview_frame: Option<Frame>,
    /// Where along the seek bar the pointer is, from 0 to 1, while over it.
    hover: Option<f64>,
    /// The time the preview should show, the time last asked of its
    /// player, and since when that request has been outstanding.
    preview_want: Option<f64>,
    preview_sent: Option<f64>,
    preview_busy: Option<Instant>,
    /// How many times in a row a seek key has repeated from being held.
    held: u32,
    /// Playback speed, kept from one video to the next.
    speed: f64,
    muted: bool,
    toast: Option<(String, Instant)>,
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
    pub fn new(options: Options, files: Vec<PathBuf>, start: usize, loader: Loader, proxy: EventLoopProxy<Event>) -> Self {
        let slide_delay = Duration::from_secs_f64(options.slideshow_delay.unwrap_or(DEFAULT_SLIDE_DELAY));
        ACCENT.store(options.accent, atomic::Ordering::Relaxed);
        // The workspaces of the config file; and unless one of them was
        // asked for by name, what was opened by its path as one more.
        let mut spaces: Vec<Space> = (options.workspaces.iter())
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
            });
            spaces.len() - 1
        });
        App {
            marks: HashSet::new(),
            undo: Vec::new(),
            last_folder: None,
            prompt: None,
            palette: None,
            run_next: None,
            filter: None,
            sort: options.listing.sort.filter(|_| !options.listing.randomize),
            reversed: options.listing.reverse,
            shuffled: options.listing.randomize,
            show_list: false,
            grid_columns: None,
            grid_due: None,
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
            show_info: options.draw_info,
            grid_titles: options.grid_titles,
            show_debug: false,
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
            snapshot: Snapshot::from_env(),
            player: None,
            player_asked: false,
            video: None,
            preview: None,
            preview_id: None,
            preview_frame: None,
            hover: None,
            preview_want: None,
            preview_sent: None,
            preview_busy: None,
            held: 0,
            speed: 1.0,
            muted: false,
            toast: None,
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
        self.grid.as_ref().map_or(self.pos, |grid| grid.cursor)
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

    fn window_size(&self) -> (u32, u32) {
        self.window.as_ref().map_or((0, 0), |w| w.inner_size().into())
    }

    fn redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), Instant::now() + TOAST));
        self.redraw();
    }

    fn strip_visible(&self) -> bool {
        self.strip_pinned || self.strip_hover
    }

    // ---- Loading -------------------------------------------------------

    /// Tells the loader what is needed around the current position.
    fn request(&self) {
        if self.monitor.0 == 0 || self.monitor.1 == 0 {
            return;
        }
        let len = self.list.len() as i64;
        let pos = self.pos as i64;
        let dir = if self.forward { 1 } else { -1 };

        let mut wanted = Vec::new();
        // The current image, then neighbours, favouring the way we are going.
        for offset in [0, 1, -1, 2, -2, 3] {
            let id = self.list[(pos + offset * dir).rem_euclid(len) as usize];
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
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
            full: self.needs_full().then(|| self.id()),
            thumbs,
            thumb_size,
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
        self.view.layout(frame.native, window).width > frame.width
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
        self.redraw();
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
        let wanted = self.grid.is_none() && self.overview.is_none() && files::is_video(&self.files[id]);
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
    }

    /// Starts the player on another thread, once; `Event::Player` delivers
    /// it. Loading libmpv and starting mpv takes long enough to notice.
    fn warm_player(&mut self) {
        if std::mem::replace(&mut self.player_asked, true) {
            return;
        }
        let proxy = self.proxy.clone();
        // A scripted test run must not make any sound.
        let silent = self.snapshot.is_some();
        std::thread::spawn(move || {
            let wake = proxy.clone();
            let role = Role::Playback { silent };
            let player = Player::new(role, move |signal| drop(wake.send_event(Event::Video(signal))));
            let playable = player.is_ok();
            let _ = proxy.send_event(Event::Player(player));
            if playable {
                let wake = proxy.clone();
                let preview = Player::new(Role::Preview, move |signal| drop(wake.send_event(Event::Preview(signal))));
                let _ = proxy.send_event(Event::PreviewPlayer(preview));
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
        if busy || self.preview_sent == Some(want) {
            return;
        }
        if self.preview_id != Some(video.id) {
            preview.open(&self.files[video.id]);
            self.preview_id = Some(video.id);
            self.preview_frame = None;
        }
        // The nearest keyframe is close enough, and much quicker to reach.
        preview.command_later(&[b"seek", want.to_string().as_bytes(), b"absolute+keyframes"]);
        self.preview_sent = Some(want);
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
            video.seeking = Some((Instant::now(), target));
            video.seek_asked.get_or_insert_with(Instant::now);
            video.touched = Some(Instant::now());
        }
        self.redraw();
    }

    /// Where the seek bar's track is: inset from the sides and lifted off
    /// the bottom edge, or off the filmstrip when that is pinned open.
    fn seek_bar(&self) -> SeekBar {
        let (width, height) = self.window_size();
        let unit = ui_px(height) as i32;
        let base = if self.strip_pinned { Strip::for_height(height).top } else { height as i32 };
        // Room on the left for the play / pause symbol and the elapsed
        // time, and on the right for the duration.
        // Room on the left for the three buttons and the elapsed time.
        SeekBar { x: unit * 13, width: (width as i32 - unit * 19).max(1), middle: base - unit * 2, unit }
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
                player.set("speed", &self.speed.to_string());
                player.set("mute", if self.muted { "yes" } else { "no" });
                self.player = Some(player);
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
        // The video being left, if this replaces one.
        self.remember_position();
        let from = match self.options.resume {
            true => self.positions.get_or_insert_with(Positions::load).get(&self.files[id]),
            false => None,
        };
        if let Some(player) = &self.player {
            let started = Instant::now();
            player.play(&self.files[id], from);
            // Until the player reports for itself, this is where it is.
            let seeking = from.map(|seconds| (started, seconds));
            self.video = Some(Video { id, showing: false, started, seeking, start_took: None, seek_asked: None, seek_took: None, touched: None });
            self.perf.video_lines = None;
            self.apply_video_view();
        }
        if let Some(seconds) = from {
            self.toast(format!("Resumed at {}", clock(seconds)));
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
        let events = if let Signal::Events = signal { player.events() } else { Vec::new() };
        let Some(video) = &mut self.video else { return };
        let mut at_end = false;
        for event in events {
            if event == PlayerEvent::AtEnd {
                at_end = true;
                continue;
            }
            // The still stays up until the player has a frame to show.
            let showing = event == PlayerEvent::Showing;
            if showing && !video.showing {
                let took = video.started.elapsed();
                video.start_took.get_or_insert(took);
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
                    video.touched = Some(Instant::now());
                }
            }
            video.showing = showing;
        }
        // mpv stalls, for over half a second when changing file, if a frame
        // it has announced is not taken. Redrawing takes it, but that only
        // happens once the video is showing, and never for the hidden window
        // of a scripted run; in those cases take it here and discard it.
        if matches!(signal, Signal::Frame) && (!video.showing || self.snapshot.is_some()) {
            let (width, height): (u32, u32) = self.window.as_ref().map_or((0, 0), |w| w.inner_size().into());
            self.scene.resize(width as usize * height as usize, 0);
            self.scene_key = None;
            player.render(&mut self.scene, width, height);
        }
        // A video that played to its end makes way for the next file. One
        // taken to its end by hand, by seeking or stepping there, stays; and
        // the event may be left over from the file before this one.
        let by_hand = video.seek_asked.is_some() || video.touched.is_some_and(|at| at.elapsed() < SEEK_SETTLE);
        if video.showing {
            self.redraw();
        }
        if at_end && !by_hand && player.flag("eof-reached") == Some(true) {
            self.play_next();
        }
    }

    /// Goes on to the next file after a video, unless that is the
    /// slideshow's job, something is open that the user is in the middle
    /// of, or it was the last file.
    fn play_next(&mut self) {
        let open = self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some();
        let occupied = open || self.drag.is_some() || self.palette.is_some();
        if self.options.autoplay && self.slide_at.is_none() && !occupied && self.pos + 1 < self.list.len() {
            self.step(true);
        }
    }

    /// Handles the keys that mean something else while a video plays.
    /// Returns whether the key was one of them.
    fn video_key(&mut self, key: Key<&str>, repeat: bool) -> bool {
        let modifiers = self.modifiers;
        if self.player.is_none() || modifiers.alt_key() {
            return false;
        }
        // Holding a seek key down covers ground faster the longer it is held.
        self.held = if repeat { self.held + 1 } else { 0 };
        let step = match self.held {
            0..=7 => SEEK_STEP,
            8..=23 => SEEK_STEP * 3.0,
            _ => SEEK_STEP * 12.0,
        };
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
            // Holding space down should not flicker between the two.
            Key::Named(NamedKey::Space) if repeat => return true,
            Key::Named(NamedKey::Space) => VideoAction::Pause,
            Key::Named(NamedKey::ArrowRight) => arrow(true),
            Key::Named(NamedKey::ArrowLeft) => arrow(false),
            Key::Named(NamedKey::PageDown) => VideoAction::Seek(SEEK_JUMP),
            Key::Named(NamedKey::PageUp) => VideoAction::Seek(-SEEK_JUMP),
            Key::Named(NamedKey::ArrowUp) => VideoAction::Volume(5),
            Key::Named(NamedKey::ArrowDown) => VideoAction::Volume(-5),
            _ => return false,
        };
        // Forward from the end goes on to the next file. Only on a fresh
        // press: a key held down to reach the end stops there.
        let forward = matches!(action, VideoAction::Seek(seconds) if seconds > 0.0);
        let action = if forward && !repeat && self.at_video_end() { VideoAction::File(true) } else { action };
        self.video_action(action);
        true
    }

    /// Whether the video has played, or been taken, to its last frame.
    fn at_video_end(&self) -> bool {
        let Some(player) = &self.player else { return false };
        let ended = player.flag("eof-reached") == Some(true);
        ended || self.video_time().is_some_and(|(position, length)| position >= length - 0.25)
    }

    fn video_action(&mut self, action: VideoAction) {
        let (Some(player), Some(video)) = (&self.player, &mut self.video) else { return };
        video.touched = Some(Instant::now());
        match action {
            VideoAction::File(forward) => return self.step(forward),
            VideoAction::Pause => player.command(&[b"cycle", b"pause"]),
            VideoAction::Seek(seconds) => {
                player.command(&[b"seek", seconds.to_string().as_bytes(), b"relative+exact"]);
                // mpv has not moved yet when asked straight after a seek, so
                // quick presses build on where the last one was heading.
                let from = match video.seeking {
                    Some((at, target)) if at.elapsed() < SEEK_SETTLE => target,
                    _ => player.number("time-pos").unwrap_or(0.0),
                };
                let length = player.number("duration").unwrap_or(0.0);
                video.seeking = Some((Instant::now(), (from + seconds).clamp(0.0, length)));
                video.seek_asked.get_or_insert_with(Instant::now);
            }
            // Both leave the video paused on the frame they land on.
            VideoAction::Frame(true) => player.command(&[b"frame-step"]),
            VideoAction::Frame(false) => player.command(&[b"frame-back-step"]),
            VideoAction::Volume(change) => {
                player.command(&[b"add", b"volume", change.to_string().as_bytes()]);
                let volume = player.number("volume").unwrap_or(0.0);
                return self.toast(format!("Volume {volume:.0}%"));
            }
        }
        // Pausing says so for a moment, at the top right with the other
        // notes; playing again takes the word away.
        if action == VideoAction::Pause {
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
            match paused {
                true => self.toast(format!("{}  Paused", commands::icons::PAUSE)),
                false => self.toast = None,
            }
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
            player.set("speed", &self.speed.to_string());
        }
        self.toast(format!("Speed {}x", self.speed));
    }

    /// Where playback is and how long the video is, in seconds.
    fn video_time(&self) -> Option<(f64, f64)> {
        let (player, video) = (self.player.as_ref()?, self.video.as_ref()?);
        let length = player.number("duration").filter(|length| *length > 0.0)?;
        // Straight after a seek mpv still reports where it was.
        let position = match video.seeking {
            Some((at, target)) if at.elapsed() < SEEK_SETTLE => target,
            _ => player.number("time-pos")?,
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
            Some(native) if window.0 > 0 && window.1 > 0 => {
                let layout = self.view.layout(native, window);
                // mpv's zoom is a power of two on top of filling the window.
                let filling = fit_size(native, window, true);
                let zoom = (f64::from(layout.width) / f64::from(filling.0)).log2();
                // Its pan is the offset from centre as a share of the scaled size.
                let pan = |origin: i32, size: u32, across: u32| {
                    (f64::from(origin) + f64::from(size) / 2.0 - f64::from(across) / 2.0) / f64::from(size)
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
                player.set(name, &values[index]);
            }
        }
        *sent = Some(values);
    }

    fn set_zoom(&mut self, zoom: Zoom) {
        self.view.zoom = zoom;
        self.view.center = None;
        self.view_changed();
    }

    /// Zooms around `anchor`, a window position, or the middle of the window.
    fn zoom_by(&mut self, factor: f64, anchor: Option<(f64, f64)>) {
        let Some(native) = self.native() else { return };
        let window = self.window_size();
        let anchor = anchor.unwrap_or((f64::from(window.0) / 2.0, f64::from(window.1) / 2.0));
        let scale = self.view.scale(native, window) * factor;
        self.view.zoom_to(scale, anchor, native, window);
        self.view_changed();
    }

    /// Whether the image, as zoomed, has parts off to the side of the window.
    fn wider_than_window(&self) -> bool {
        let window = self.window_size();
        self.native().is_some_and(|native| self.view.layout(native, window).width > window.0)
    }

    fn pan(&mut self, dx: f64, dy: f64) {
        let Some(native) = self.native() else { return };
        self.view.pan(dx, dy, native, self.window_size());
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
            ToggleHelp => self.help = !self.help,
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
            ToggleList => self.show_list = !self.show_list,
            OpenFromGrid => self.open_from_grid(),
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
                    false => self.toast("No clipboard tool found (xclip, xsel or wl-copy); printed to the terminal"),
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
            Save => {
                let copy = format!("jog_{}_{}", std::process::id(), self.name());
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
            CopyPath => self.copy_text(true),
            CopyTo => self.send(false, event_loop),
            MoveTo => self.send(true, event_loop),
            CopyToAsk => self.ask_folder(false),
            MoveToAsk => self.ask_folder(true),
            Undo => self.undo(),
            Rename => self.start_rename(),
            Delete => self.ask_delete(),
            Convert(format) => self.convert(format),
            SpeedUp => self.change_speed(Some(true)),
            SpeedDown => self.change_speed(Some(false)),
            SpeedReset => self.change_speed(None),
            PlayPause => self.video_action(VideoAction::Pause),
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
            Restart => {
                self.seek_to(0.0, true);
                // One that had played to its end is paused there.
                if let (Some(player), Some(_)) = (&self.player, &self.video) {
                    player.set("pause", "no");
                }
            }
            ClearFilter => self.clear_filter(),
            Quit => event_loop.exit(),
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
        let command = expand_action(template, self.path());
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
            ToggleWatch => self.watch.is_some(),
            Workspace(which) => which == self.space,
            ToggleList => self.show_list,
            ToggleGridTitles => self.grid_titles,
            ToggleMute => self.muted,
            SortBy(sort) => self.sort == Some(sort),
            ReverseOrder => self.reversed,
            Shuffle => self.shuffled,
            ToggleVerbose => self.options.verbose,
            ToggleDebugLog => self.options.debug,
            ToggleExif => self.show_exif,
            ToggleSlideshow => self.slide_at.is_some(),
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
            self.toast("No clipboard tool found (xclip, xsel or wl-copy); printed to the terminal");
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
        self.toast(summary);
        if sent.is_empty() {
            return;
        }
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

    fn undo(&mut self) {
        match self.undo.pop() {
            None => self.toast("Nothing to undo"),
            Some(Undo::Created(paths)) => {
                let removed = paths.iter().filter(|path| std::fs::remove_file(path).is_ok()).count();
                for path in &paths {
                    println!("[undo] removed {}", path.display());
                }
                self.toast(format!("Undone: removed {removed} new files"));
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
                self.toast(format!("Undone: moved {restored} of {} back", moved.len()));
                self.request();
                self.refresh();
            }
            Some(Undo::Renamed { id, from, to }) => {
                if ops::move_file(&to, &from).is_ok() {
                    println!("[undo] {} -> {}", to.display(), from.display());
                    self.set_path(id, from);
                    self.toast(format!("Undone: renamed back to {}", self.name_of(id)));
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
        let paths: Vec<PathBuf> = self.targets().into_iter().map(|id| self.files[id].clone()).collect();
        self.toast(format!("Converting {} to {}...", paths.len(), format.name()));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let (mut created, mut failed) = (Vec::new(), 0);
            for path in paths {
                match convert::convert(&path, format) {
                    Ok(to) => {
                        println!("[convert] {} -> {}", path.display(), to.display());
                        created.push(to);
                    }
                    Err(error) => {
                        eprintln!("jog: cannot convert {}: {error}", path.display());
                        failed += 1;
                    }
                }
            }
            let _ = proxy.send_event(Event::Converted { created, failed, format });
        });
    }

    fn converted(&mut self, created: Vec<PathBuf>, failed: usize, format: Format) {
        let mut summary = format!("Converted {} to {}", created.len(), format.name());
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed (see the terminal)"));
        }
        self.toast(summary);
        if !created.is_empty() {
            self.undo.push(Undo::Created(created));
        }
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
            Key::Named(NamedKey::Enter) => match prompt.kind {
                PromptKind::Rename => self.finish_rename(),
                PromptKind::SaveWorkspace => {
                    let typed = prompt.field.string().trim().to_string();
                    self.prompt = None;
                    self.save_space(&typed);
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
        self.toast(format!("Showing {} of {} (Esc for all)", list.len(), everything.len()));
        self.filter = Some((typed, everything));
        self.set_list(list, at, at);
        self.sync_video();
        self.redraw();
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

    /// Shows every file again after a search was kept.
    fn clear_filter(&mut self) {
        let Some((_, all)) = self.filter.take() else { return };
        let (viewed, focus) = (self.id(), self.list[self.focus()]);
        let at = |id: usize| all.iter().position(|&other| other == id).unwrap_or(0);
        let (viewed, focus) = (at(viewed), at(focus));
        self.set_list(all, viewed, focus);
        if self.options.verbose {
            eprintln!("jog: search cleared: all {} files", self.list.len());
        }
        self.toast("Showing all files");
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
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            self.toast("Not a usable file name");
            return;
        }
        let to = from.with_file_name(name);
        match ops::move_file(&from, &to) {
            Ok(()) => {
                println!("[rename] {old_name} -> {name}");
                self.set_path(prompt.id, to.clone());
                self.undo.push(Undo::Renamed { id: prompt.id, from, to });
                self.toast(format!("Renamed to {name}"));
                self.refresh();
            }
            Err(error) => self.toast(format!("Cannot rename to {name}: {error}")),
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
        let layout = self.grid_layout(self.window_size());
        self.grid = match self.grid {
            Some(_) => None,
            None => Some(Grid::at(self.pos, &layout)),
        };
        self.menu = None;
        self.drag = None;
        self.request();
        self.sync_video();
        self.redraw();
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
        let Some(grid) = self.grid.take() else { return };
        self.go_to(grid.cursor, grid.cursor >= self.pos);
        self.request();
        self.sync_video();
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

    /// Shuffles the viewing order around the file in hand: the one on
    /// view, or in the grid the one under the cursor. It stays where it
    /// is in the list, and everything before and after it changes.
    fn shuffle(&mut self) {
        (self.sort, self.reversed, self.shuffled) = (None, false, true);
        let held = self.list[self.focus()];
        let mut rng = std::mem::replace(&mut self.rng, Rng::from_clock());
        self.reorder(|ids, _| {
            let place = ids.iter().position(|id| *id == held);
            rng.shuffle(ids);
            // Back to the place it had, whatever landed there going where
            // the shuffle had put it.
            if let (Some(place), Some(at)) = (place, ids.iter().position(|id| *id == held)) {
                ids.swap(place, at);
            }
        });
        self.rng = rng;
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
                Kept { list, pos: 0, cursor: 0, filter: None, sort: sort.filter(|_| start != Start::Random), reversed, shuffled: start == Start::Random }
            }
        };
        let leaving = Kept {
            cursor: self.focus(),
            list: std::mem::take(&mut self.list),
            pos: self.pos,
            filter: self.filter.take(),
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
        (self.list, self.pos) = (incoming.list, incoming.pos.min(last));
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
        let wanted = |name: &str, loose: bool| query.is_empty() || query.score(&Facts { name, video: false, marked: false, size: None }, loose).is_some();
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
            std::thread::spawn(move || drop(proxy.send_event(Event::SpaceListed(which, files::collect(&paths, &listing).files))));
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

        let heading = format!("{}  Workspaces", commands::icons::FOLDERS);
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
            let about = format!("{key}{files}{}", if space.saved { "" } else { "   not saved" });
            let about = clipped(text, &about, width, px);
            let about_left = left + (width - text.width(&about, px)) / 2;
            text.draw(canvas, about_left, name_top + line * 5 / 4, px, DIM_TEXT_COLOR, &about);
        }
        let hint = "Arrows or the pointer to look, Enter or a click to open, Esc to close";
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
        // Not under an open search box, whose own lists would go stale.
        if self.palette.is_some() {
            return;
        }
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
        if watching && self.grid.is_none() {
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
        self.toast(format!("{}: {}", if watching { "Watch" } else { "Refreshed" }, parts.join(", ")));
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
        let (matches, narrowed, commands) = match typed.chars().next() {
            // `>` and then a number is a size to find files by.
            Some('>') if !typed[1..].starts_with(|c: char| c.is_ascii_digit()) => (Vec::new(), false, self.commands_matching(&typed[1..])),
            Some(':') => self.place_named(&typed[1..]),
            Some('@') => (Vec::new(), false, self.spaces_matching(&typed[1..])),
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
        let query = Query::parse(typed);
        if query.is_empty() {
            return all;
        }
        let score = |name: &str, loose: bool| query.score(&Facts { name, video: false, marked: false, size: None }, loose);
        let pass = |loose: bool| -> Vec<(i32, (String, &'static str, Command))> {
            all.iter().filter_map(|entry| Some((score(&entry.0, loose)?, entry.clone()))).collect()
        };
        let mut scored = pass(false);
        if scored.is_empty() {
            scored = pass(true);
        }
        scored.sort_by_key(|(score, _)| -score);
        scored.into_iter().map(|(_, entry)| entry).collect()
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
        if palette.narrowed && !palette.matches.is_empty() {
            buttons.push((PaletteEnd::Keep, format!("Show only {}", palette.matches.len())));
        }
        if let Some((_, all)) = &self.filter {
            buttons.push((PaletteEnd::All, format!("Show all {}", all.len())));
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
        if let (PaletteEnd::Go, Some((_, _, command))) = (end, palette.commands.get(palette.selected)) {
            self.run_next = Some(*command);
            return self.redraw();
        }
        let picked = palette.matches.get(palette.selected).copied();
        match end {
            PaletteEnd::Keep => {
                let typed = palette.field.string().trim().to_string();
                self.keep_matches(typed, palette.matches, picked);
            }
            PaletteEnd::All => self.clear_filter(),
            PaletteEnd::Go => {
                let Some(id) = picked else { return self.redraw() };
                // One that a kept search has hidden brings every file back.
                if !self.list.contains(&id) {
                    self.clear_filter();
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
            _ if !palette.commands.is_empty() && palette.field.text.first() == Some(&':') => "Go to:   ",
            Some('>') if palette.matches.is_empty() => "Run:   ",
            Some(':') => "Go to:   ",
            Some('@') => "Workspace:   ",
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
        query.score(&Facts { name: &name, video: files::is_video(path), marked: self.marks.contains(&id), size }, loose)
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
                | Command::SortMenu
                | Command::Quit),
            ) => self.execute(command, event_loop),
            _ => {}
        }
    }

    // ---- Input ---------------------------------------------------------

    fn key_pressed(&mut self, key: Key<&str>, repeat: bool, numpad: bool, event_loop: &ActiveEventLoop) {
        if self.help {
            // Any key dismisses the list of shortcuts.
            self.help = false;
            return self.redraw();
        }
        if self.confirm.is_some() {
            return self.confirm_key(key, event_loop);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key, event_loop);
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
        if self.grid.is_some() {
            return self.grid_key(key, event_loop);
        }
        // Escape leaves a kept search before it leaves the program.
        if self.filter.is_some() && key == Key::Named(NamedKey::Escape) {
            return self.clear_filter();
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
                    self.view.zoom_to(scale, anchor, native, self.window_size());
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
                let zone = if self.strip_visible() { Strip::for_height(height).height + reach } else { (height as i32 / 50).max(6) };
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
        if self.help {
            self.help = !pressed;
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
                } else if bar.on_symbol(x, y) {
                    self.video_action(VideoAction::Pause);
                } else if bar.on_track(x) {
                    // Click to jump there; keep the button down to scrub.
                    self.seek_to(self.hover.unwrap_or(0.0), true);
                    self.drag = Some(Drag::Scrub);
                }
            }
            // The buttons on the debug overlay copy its figures.
            (MouseButton::Left, true) if self.debug_buttons.iter().any(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32))) => {
                let hit = self.debug_buttons.iter().find(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32)));
                if let Some(command) = hit.map(|button| button.command) {
                    self.execute(command, event_loop);
                }
            }
            (MouseButton::Left, true) => {
                let height = self.window_size().1;
                let strip = Strip::for_height(height);
                let panel = self.list_panel().filter(|panel| (x as i32) < panel.width && (y as i32) < panel.bottom);
                let inside = |b: &Button| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32));
                if let Some(command) = self.list_buttons().iter().find(|button| inside(button)).map(|button| button.command) {
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
                        self.go_to(pos, pos > self.pos);
                    }
                } else if self.strip_close().is_some_and(|place| (place.x..place.x + place.width).contains(&(x as i32)) && (place.y..place.y + place.height).contains(&(y as i32))) {
                    (self.strip_pinned, self.strip_hover, self.strip_over) = (false, false, None);
                    self.redraw();
                } else if self.strip_visible() && y >= f64::from(strip.top) {
                    if let Some(pos) = self.strip_cell_at(x, y) {
                        self.go_to(pos, pos > self.pos);
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
                    }
                }
            }
            (MouseButton::Middle, true) => {
                if let Some(native) = self.native() {
                    let start_scale = self.view.scale(native, self.window_size());
                    self.drag = Some(Drag::Zoom { start_x: x, start_scale, anchor: (x, y), moved: false });
                }
            }
            (MouseButton::Left | MouseButton::Middle, false) => {
                if let Some(Drag::Zoom { moved: false, .. }) = self.drag.take() {
                    self.execute(Command::ZoomActual, event_loop);
                }
            }
            (MouseButton::Right, true) => self.execute(Command::ToggleMenu, event_loop),
            _ => {}
        }
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            MouseScrollDelta::PixelDelta(position) => position.y / 50.0,
        };
        if lines == 0.0 || self.menu.is_some() {
            return;
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
        if let Some(panel) = self.list_panel().filter(|panel| (x as i32) < panel.width && (y as i32) < panel.bottom) {
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
        // Plain characters are typed; `{Enter}`, `{Tab}`, `{F2}` and so on
        // press the named key.
        let mut rest = keys;
        while let Some(c) = rest.chars().next() {
            let named = rest.strip_prefix('{').and_then(|inner| inner.split_once('}'));
            let key = match named {
                Some((name, after)) => {
                    rest = after;
                    // `{Shift+Right}` and `{Ctrl+Right}` hold a modifier.
                    let (held, name) = match name.split_once('+') {
                        Some(("Shift", name)) => (ModifiersState::SHIFT, name),
                        Some(("Ctrl", name)) => (ModifiersState::CONTROL, name),
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
                    if name == "Click" {
                        self.mouse_button(MouseButton::Left, true, event_loop);
                        self.mouse_button(MouseButton::Left, false, event_loop);
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
            self.key_pressed(key, false, false, event_loop);
            self.modifiers = ModifiersState::empty();
        }
        self.mouse_moved(self.mouse.0, self.mouse.1);
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

        let mut ppm = format!("P6 {} {} 255\n", window.0, window.1).into_bytes();
        ppm.extend(pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
        if let Err(error) = std::fs::write(&snapshot.path, ppm) {
            eprintln!("jog: cannot write {}: {error}", snapshot.path.display());
        }
        event_loop.exit();
    }

    // ---- Drawing -------------------------------------------------------

    fn draw(&mut self) {
        let (Some(window), Some(mut surface)) = (self.window.clone(), self.surface.take()) else { return };
        let size = window.inner_size();
        if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            if surface.resize(width, height).is_ok() {
                if let Ok(mut buffer) = surface.buffer_mut() {
                    let started = Instant::now();
                    self.paint(&mut buffer, (size.width, size.height));
                    let painted = started.elapsed();
                    window.pre_present_notify();
                    let _ = buffer.present();
                    self.painted(started, painted);
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
            pixels.fill(GRID_COLOR);
            return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
        }
        let playing = self.grid.is_none() && self.video.as_ref().is_some_and(|video| video.showing);
        let player = self.player.as_ref().filter(|_| playing);
        let started = Instant::now();
        if player.is_some_and(|player| player.render(pixels, window.0, window.1)) {
            self.perf.render = started.elapsed();
            return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
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
                pixels.fill(GRID_COLOR);
                self.draw_grid(&mut Canvas::new(pixels, window.0, window.1));
                if self.options.debug {
                    eprintln!("jog: {:>7.1?} draw  grid", started.elapsed());
                }
            }
            Some(frame) => {
                let layout = self.view.layout(frame.native, window);
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
                if !self.scene_key.as_ref().is_some_and(|current| current.matches(&key)) {
                    let started = Instant::now();
                    self.scene.resize(pixels.len(), 0);
                    let background = self.options.background;
                    render::draw_image(&mut self.scene, window, &key.frame, picture, layout, smooth, background);
                    let took = started.elapsed();
                    if self.options.debug {
                        let how = if smooth { "smooth" } else { "nearest" };
                        eprintln!("jog: {took:>7.1?} draw  {how} {}x{} -> {}x{}", key.frame.width, key.frame.height, layout.width, layout.height);
                    }
                    let (from, to) = ((key.frame.width, key.frame.height), (layout.width, layout.height));
                    self.perf.scaled = Some(Scaled { took, smooth, from, to });
                    self.scene_key = Some(key);
                }
                pixels.copy_from_slice(&self.scene);
            }
            None => pixels.fill(self.options.background),
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
        let strip = (viewing && self.strip_visible()).then(|| Strip::for_height(height as u32));
        if let Some(strip) = strip {
            self.draw_strip(canvas, strip);
        }
        let bottom = strip.map_or(height, |s| s.top) - margin;

        let marked = viewing && self.marks.contains(&self.id());
        if marked {
            // A bar along the top edge flags a marked image.
            canvas.fill(0, 0, width, (px as i32 / 3).max(4), MARK_COLOR);
        }
        // The lines at the top left, each a run of words in their colours:
        // what matters bright, what describes it muted.
        let (soft, dim) = (accent_soft(), DIM_TEXT_COLOR);
        let mut top_left: Vec<Vec<(String, u32)>> = Vec::new();
        if let Some((typed, all)) = self.filter.as_ref().filter(|_| self.show_info) {
            let found = format!("{} {typed}", commands::icons::SEARCH);
            top_left.push(vec![(found, soft), (format!("   {} of {}", self.list.len(), all.len()), TEXT_COLOR), ("   Esc for all".to_string(), dim)]);
        }
        if viewing && !self.marks.is_empty() {
            let count = format!("{} {} marked", commands::icons::STAR, self.marks.len());
            let line = if marked { vec![(count, MARK_COLOR), ("   this one too".to_string(), dim)] } else { vec![(count, dim)] };
            top_left.push(line);
        }
        if viewing && self.show_info {
            let mut line = vec![(self.name(), TEXT_COLOR), (format!("   {} / {}", self.pos + 1, self.list.len()), soft)];
            // Which workspace this is, when there are several to be in.
            if self.spaces.len() > 1 {
                line.insert(0, (format!("@{}   ", self.spaces[self.space].name), soft));
            }
            top_left.push(line);
            top_left.push(vec![(self.info_line(), dim)]);
            if let Some((position, length)) = self.video_time() {
                let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
                let sign = if paused { commands::icons::PAUSE } else { commands::icons::PLAY };
                let mut line = vec![(format!("{sign} {}", clock(position)), TEXT_COLOR), (format!(" / {}", clock(length)), dim)];
                if self.speed != 1.0 {
                    line.push((format!("   {}x", self.speed), soft));
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
        let bar_wanted = viewing && (self.hover.is_some() || self.time_until.is_some());
        if let Some((position, length)) = self.video_time().filter(|_| bar_wanted) {
            let bar = self.seek_bar();
            let unit = bar.unit;
            let line = Text::line_height(px);
            let thickness = unit * 2 / 3;
            let top = bar.middle - thickness / 2;
            // Darken the picture behind it, more and more towards the
            // bottom edge.
            let shade_top = bar.middle - unit * 6;
            canvas.fade_down(0, shade_top, width, height - shade_top, 70);
            let done = (f64::from(bar.width) * position / length) as i32;
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 72, bar.width);
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 256, done.max(thickness / 2));
            let handle = unit * 5 / 4;
            canvas.pill(bar.x + done - handle / 2, bar.middle - handle / 2, handle, handle, TEXT_COLOR, 256, handle);
            // The symbol shows the state: a triangle while playing, two bars
            // while paused.
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
            // Either side of it: the file before and the file after.
            for (control, icon) in [(0, commands::icons::SKIP_BACK), (2, commands::icons::SKIP_FORWARD)] {
                let (left, top, side) = bar.control(control);
                labels.push((left + side / 2, top + (side - line) / 2, Ordering::Equal, icon.to_string()));
            }
            let (symbol_left, symbol_top, side) = bar.symbol();
            if paused {
                canvas.pause_symbol(symbol_left, symbol_top, side, TEXT_COLOR);
            } else {
                canvas.play_symbol(symbol_left, symbol_top, side, TEXT_COLOR);
            }
            labels.push((bar.x - unit, bar.middle - line / 2, Ordering::Less, clock(position)));
            labels.push((bar.x + bar.width + unit, bar.middle - line / 2, Ordering::Greater, clock(length)));
            time_bottom = bar.middle - unit * 4;

            if let Some(fraction) = self.hover.filter(|_| bar.on_track(self.mouse.0)) {
                // A tick where the pointer is, the time there, and above
                // that the frame from that part of the video.
                let at = bar.x + (f64::from(bar.width) * fraction) as i32;
                canvas.fill(at - 1, top - unit / 3, 2, thickness + unit * 2 / 3, MARK_COLOR);
                let label_top = bar.middle - unit * 2 - line / 2;
                labels.push((at.clamp(unit * 4, width - unit * 4), label_top, Ordering::Equal, clock(fraction * length)));
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
        let boxes = self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some() || panel.is_some();
        // The filmstrip has words of its own: the lengths of its videos.
        let boxes = boxes || self.palette.is_some() || strip.is_some() || self.overview.is_some();
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
        let left = margin + panel.as_ref().map_or(0, |panel| panel.width);
        if let Some(strip) = &strip {
            self.draw_strip_lengths(canvas, &mut text, strip, px);
            // A cross above its right end closes it.
            if let Some(place) = self.strip_close() {
                let over = (place.x..place.x + place.width).contains(&(self.mouse.0 as i32)) && (place.y..place.y + place.height).contains(&(self.mouse.1 as i32));
                canvas.round_fill(place.x, place.y, place.width, place.height, place.width / 2, if over { accent() } else { 0x000000 }, if over { 256 } else { 170 });
                let (cross, small) = (commands::icons::X, (px * 5 / 6).max(11));
                let inset = ((place.width - text.width(cross, small)) / 2, (place.height - Text::line_height(small)) / 2);
                text.draw(canvas, place.x + inset.0, place.y + inset.1, small, if over { on_accent() } else { TEXT_COLOR }, cross);
            }
            self.draw_strip_tip(canvas, &mut text, strip, px);
        }
        if let Some(panel) = &panel {
            for pos in panel.first..panel.first + panel.rows {
                self.want_duration(self.list[pos]);
            }
            self.draw_list(canvas, &mut text, panel);
        }
        let below = rich_block(canvas, &mut text, &top_left, px, (left, margin));
        let pointer = (self.mouse.0 as i32, self.mouse.1 as i32);
        self.debug_buttons = draw_debug(canvas, &mut text, &debug, px, (left, below), &mut self.debug_keep, pointer);
        self.debug_last = debug;
        text_block(canvas, &mut text, &exif, px, |_, h| (left, bottom - h));
        text_block(canvas, &mut text, &actions, px, |w, h| (width - margin - w, bottom - h));
        text_block(canvas, &mut text, &toast, px, |w, _| (width - margin - w, margin));
        // Twice the usual size, centred just above the seek bar.
        text_block(canvas, &mut text, &time, px, |w, h| ((width - w) / 2, time_bottom - h));
        for (anchor, top, side, label) in &labels {
            let label_width = text.width(label, px);
            let left = match side {
                Ordering::Less => anchor - label_width,
                Ordering::Equal => anchor - label_width / 2,
                Ordering::Greater => *anchor,
            };
            text.draw(canvas, left, *top, px, TEXT_COLOR, label);
        }
        text_block(canvas, &mut text, &error, px, |w, h| ((width - w) / 2, (height - h) / 2));
        if let Some(menu) = &self.menu {
            self.draw_menu(canvas, &mut text, menu);
        }
        if let Some(prompt) = &self.prompt {
            let count = self.targets().len();
            let files = if count == 1 { self.name_of(prompt.id) } else { format!("{count} files") };
            let (icon, title, hint) = match prompt.kind {
                PromptKind::Rename => (commands::icons::PENCIL, "Rename".to_string(), "Enter to rename"),
                PromptKind::SaveWorkspace => (commands::icons::FOLDER_PLUS, "Save this folder as a workspace named".to_string(), "Enter to add it to the config file"),
                PromptKind::SendTo { moving: true } => (commands::icons::FOLDER_INPUT, format!("Move {files} to"), "Enter to move, the folder is made if need be"),
                PromptKind::SendTo { moving: false } => (commands::icons::FOLDER_PLUS, format!("Copy {files} to"), "Enter to copy, the folder is made if need be"),
            };
            draw_prompt(canvas, &mut text, icon, &title, &prompt.field, hint, px);
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
            draw_help(canvas, &mut text, px);
        }
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

        for pos in self.grid_shown(&layout) {
            let id = self.list[pos];
            let area = self.grid_area(&layout, pos, canvas.height as u32);
            // A video under the cursor plays as a trailer.
            let trailer = self.trailer.as_ref().filter(|trailer| trailer.id == id && pos == grid.cursor);
            let since = trailer.and_then(|trailer| trailer.since);
            let trailer = trailer.and_then(|trailer| trailer.frame.as_ref()).filter(|frame| !frame.pixels.is_empty());
            let thumb = self.loader.thumb(id);
            let [x, y, w, h] = match (trailer, &thumb) {
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
                (None, Some(thumb)) => {
                    let place = Self::grid_picture(area, (thumb.width, thumb.height), false);
                    canvas.image(thumb, place[0], place[1], place[2], place[3]);
                    place
                }
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
            if pos == grid.cursor || pos == self.pos {
                let color = if pos == grid.cursor { TEXT_COLOR } else { accent() };
                let gap = edge * 2;
                canvas.round_outline(x - gap, y - gap, w + 2 * gap, h + 2 * gap, radius + gap, edge, color);
            }
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
        let mut x = (canvas.width - width) / 2;
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

    fn info_line(&mut self) -> String {
        let window = self.window_size();
        let size = self.current_meta().and_then(|meta| meta.size).map(meta::human_size);
        let Some(native) = self.native() else { return size.unwrap_or_default() };
        let percent = self.view.scale(native, window) * 100.0;
        let mut line = format!("{} x {}   {percent:.0}%", native.0, native.1);
        if let Some(size) = size {
            line.push_str(&format!("   {size}"));
        }
        line
    }

    /// The debug overlay, as groups of figures each with a name: what the
    /// player reports while a video is showing, otherwise what decoding
    /// and drawing the image cost.
    fn debug_groups(&mut self) -> Vec<Group> {
        let perf = &self.perf;
        let mut drawing = vec![("paint", ms(perf.paint)), ("present", ms(perf.present))];
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
        file.push(("decoder", decoder));
        if let Some(took) = video.start_took {
            file.push(("start", ms(took)));
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
        vec![("Video", file), ("Playback", playback)]
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
        let shown = self.grid.is_none() && self.strip_visible();
        shown.then_some(Button { x: width as i32 - size - size / 2, y: strip.top - size - size / 3, width: size, height: size, command: Command::ToggleFilmstrip })
    }

    fn strip_left(&self, strip: &Strip, pos: usize, window_width: i32) -> i32 {
        let middle = window_width / 2 - strip.cell_w / 2;
        middle + (pos as i64 - self.strip_centre()) as i32 * strip.pitch()
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
                    let (w, h) = (w as i32, h as i32);
                    let place = [left + (strip.cell_w - w) / 2, top + (strip.cell_h - h) / 2, w, h];
                    canvas.image_rounded(&thumb, place[0], place[1], w, h, round);
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
        let bottom = if self.strip_visible() { Strip::for_height(height).top } else { height as i32 };
        let row_height = Text::line_height(px) + px as i32 / 2;
        // Above the rows: a heading, and under it the buttons for the order.
        let top = Text::line_height(px) * 2 + px as i32 * 3;
        let rows = (((bottom - top) / row_height).max(1) as usize).min(self.list.len());
        let scrolled = self.list_scroll.filter(|(pos, len, _)| (*pos, *len) == (self.pos, self.list.len()));
        let first = scrolled.map_or(self.pos.saturating_sub(rows / 2), |(_, _, first)| first).min(self.list.len() - rows);
        Some(ListPanel { width: (px as i32 * 30).min(width as i32 / 2), bottom, top, row_height, first, rows, px })
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

    /// The video the grid plays a trailer of: the one under its cursor.
    fn trailer_subject(&self) -> Option<usize> {
        let grid = self.grid.as_ref().filter(|grid| grid.moved && self.palette.is_none() && self.overview.is_none())?;
        let id = self.list[grid.cursor];
        files::is_video(&self.files[id]).then_some(id)
    }

    /// Has the trailer thread play the video under the grid's cursor, and
    /// stop when the cursor leaves it. Called on each redraw. Nothing here
    /// waits on the player: the thread does all of that.
    /// With `--verbose` or `--debug`, says on the terminal what has become
    /// of a trailer: waited for, skipped, started, showing or stopped.
    fn trailer_note(&self, what: &str, id: usize, more: &str) {
        if self.options.verbose || self.options.debug {
            eprintln!("jog: [trailer] {what:<8} {}{more}", self.name_of(id));
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
        let bounds = self.grid_layout(window).thumb_size();
        let trailers = self.trailers.get_or_insert_with(|| Trailers::new(move |note| proxy.send_event(Event::Trailer(note)).is_ok()));
        trailers.play(id, self.files[id].clone(), bounds);
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
        let panel = self.list_panel().filter(|panel| (x as i32) < panel.width && (y as i32) < panel.bottom)?;
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
        let star = (panel.width - panel.px as i32 * 2..panel.width).contains(&(x as i32));
        self.list_row_at(x, y).filter(|_| star)
    }

    fn draw_list(&self, canvas: &mut Canvas, text: &mut Text, panel: &ListPanel) {
        let px = panel.px;
        let pad = px as i32 / 2;
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        canvas.shade(0, 0, panel.width, panel.bottom, 36);
        canvas.fill(panel.width - 1, 0, 1, panel.bottom, MENU_EDGE_COLOR);

        // The heading: how many files, and what they are narrowed to, with
        // a cross at its right to close the list. Under it a row of
        // buttons for the order: the one in force filled in, like the
        // buttons under the grid in look, and lighting up under the pointer.
        let buttons = self.list_buttons();
        let heading = match &self.filter {
            Some((typed, all)) => format!("{} of {}   {typed}", self.list.len(), all.len()),
            None => format!("{} files", self.list.len()),
        };
        let heading = clipped(text, &heading, panel.width - px as i32 * 4, px);
        text.draw(canvas, pad * 2, px as i32 * 3 / 4, px, TEXT_COLOR, &heading);
        canvas.fill(pad, panel.top - pad / 2, panel.width - 2 * pad, 1, MENU_EDGE_COLOR);
        let small = (px * 5 / 6).max(11);
        for button in &buttons {
            let over = (button.x..button.x + button.width).contains(&mouse.0) && (button.y..button.y + button.height).contains(&mouse.1);
            let on = self.is_checked(button.command);
            if button.command == Command::ToggleList {
                // The cross is a round button, as the grid's is.
                canvas.round_fill(button.x, button.y, button.width, button.height, button.width / 2, if over { accent() } else { PLACEHOLDER_COLOR }, 256);
                let cross = commands::icons::X;
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
            if current {
                canvas.round_fill(pad / 2, top, panel.width - pad, panel.row_height, pad, accent(), 256);
            } else if hovered {
                canvas.round_fill(pad / 2, top, panel.width - pad, panel.row_height, pad, TEXT_COLOR, 28);
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
    width: i32,
    bottom: i32,
    top: i32,
    row_height: i32,
    first: usize,
    rows: usize,
    px: u32,
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

/// Draws lines made of runs of words, each run in its own colour, on a
/// darkened panel with its top-left corner at `at`. Returns where the
/// next thing down should start.
fn rich_block(canvas: &mut Canvas, text: &mut Text, lines: &[Vec<(String, u32)>], px: u32, at: (i32, i32)) -> i32 {
    if lines.is_empty() {
        return at.1;
    }
    let pad = px as i32 / 2;
    let line_height = Text::line_height(px);
    let widths: Vec<i32> = lines.iter().map(|line| line.iter().map(|(run, _)| text.width(run, px)).sum()).collect();
    let width = widths.into_iter().max().unwrap_or(0) + 3 * pad;
    let height = line_height * lines.len() as i32 + 2 * pad;
    canvas.round_fill(at.0, at.1, width, height, pad, 0x000000, 176);
    for (index, line) in lines.iter().enumerate() {
        let mut x = at.0 + 3 * pad / 2;
        for (run, color) in line {
            text.draw(canvas, x, at.1 + pad + index as i32 * line_height, px, *color, run);
            x += text.width(run, px);
        }
    }
    at.1 + height + pad
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
fn draw_debug(canvas: &mut Canvas, text: &mut Text, groups: &[Group], px: u32, at: (i32, i32), keep: &mut DebugKeep, pointer: (i32, i32)) -> Vec<Button> {
    if groups.is_empty() {
        return Vec::new();
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
    drawn
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
    let widest = text.width(&typed, px).max(text.width(&heading, px)).max(text.width(hint, small));
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

    let hint = format!("{hint}   Esc to cancel");
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
fn draw_help(canvas: &mut Canvas, text: &mut Text, px: u32) {
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
    let (title, hint) = ("Keyboard shortcuts", "Any key closes this.  > in the find box lists every command.");
    let head = row * 2;
    let height = head + columns.iter().map(|c| c.2).max().unwrap_or(0) * row + row + 2 * pad;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);
    panel(canvas, left, top, width, height, px as i32);
    text.draw(canvas, left + pad, top + pad, px * 5 / 4, TEXT_COLOR, title);
    let hint_width = text.width(hint, small);
    text.draw(canvas, left + width - pad - hint_width, top + height - pad - row / 2, small, DIM_TEXT_COLOR, hint);

    let mut x = left + pad;
    for (sections, (keys_width, labels_width, _)) in commands::HELP.iter().zip(columns) {
        let mut y = top + pad + head;
        for (title, entries) in sections.iter() {
            text.draw(canvas, x, y, px, accent_soft(), title);
            canvas.fill(x, y + row - row / 5, keys_width + gap + labels_width, 1, MENU_EDGE_COLOR);
            y += row;
            for (keys, label) in entries.iter() {
                let mut key_x = x;
                for (key, width, cap) in help_keys(text, keys, small) {
                    let key_top = y + (row - Text::line_height(small)) / 2;
                    if cap {
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
                text.draw(canvas, x + keys_width + gap, label_top, px, TEXT_COLOR, label);
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

/// Fills in an action template: `%f` is the path and `%n` the file name,
/// both quoted for the shell, and `%%` a literal percent sign.
fn expand_action(template: &str, path: &Path) -> String {
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
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        } else {
            let fallback = (self.monitor.0 * 3 / 5, self.monitor.1 * 3 / 5);
            let (width, height) = self.options.geometry.unwrap_or(fallback);
            attributes = attributes.with_inner_size(PhysicalSize::new(width.max(1), height.max(1)));
        }
        let window = Arc::new(event_loop.create_window(attributes).expect("failed to create window"));
        window.set_cursor_visible(self.pointer_visible);
        let context = Context::new(window.clone()).expect("failed to connect to display");
        self.surface = Some(Surface::new(&context, window.clone()).expect("failed to create surface"));
        self.window = Some(window);
        self.update_monitor();
        // With --grid the grid is what comes up, and nothing plays until
        // a picture is gone to: not the first video, nor its trailer.
        if std::mem::take(&mut self.options.grid) {
            let layout = self.grid_layout(self.window_size());
            self.grid = Some(Grid { moved: false, ..Grid::at(self.pos, &layout) });
            self.request();
        }
        self.refresh();
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
                self.refresh();
                self.start_snapshot(event_loop);
            }
            Event::Loaded(Job::Full(id)) if id == self.id() => self.redraw(),
            Event::Loaded(Job::Thumb(_)) if self.strip_visible() || self.grid.is_some() || self.palette.is_some() => {
                self.redraw()
            }
            Event::Loaded(_) => {}
            Event::Converted { created, failed, format } => self.converted(created, failed, format),
            Event::Video(signal) => self.video_signal(signal),
            Event::Player(player) => self.player_ready(player, event_loop),
            Event::Preview(signal) => self.preview_signal(signal),
            // Without it the seek bar still works, just without pictures.
            Event::PreviewPlayer(preview) => {
                self.preview = preview.ok();
                // As for the player: empty the queue so that mpv signals.
                self.preview_signal(Signal::Events);
            }
            Event::Listed(listed) => self.take_listing(listed, true),
            Event::SpaceListed(which, found) => {
                // For the overview: what an unvisited workspace holds.
                let list = self.ids_for(found);
                if which != self.space && self.spaces[which].kept.is_none() && !list.is_empty() {
                    let sort = self.spaces[which].sort;
                    self.spaces[which].kept = Some(Kept { list, pos: 0, cursor: 0, filter: None, sort, reversed: false, shuffled: false });
                }
                self.request();
                self.redraw();
            }
            Event::Duration(id, seconds) => {
                self.durations.insert(id, seconds);
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) => {
                self.update_monitor();
                self.menu = None;
                self.request();
                self.apply_video_view();
                self.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
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
        if self.grid_due.is_some_and(|due| due <= now) {
            self.grid_due = None;
            self.redraw();
        }
        let snapshot_at = self.snapshot.as_ref().and_then(|snapshot| snapshot.at);
        if snapshot_at.is_some_and(|at| at <= now) {
            return self.finish_snapshot(event_loop);
        }

        let toast_until = self.toast.as_ref().map(|(_, until)| *until);
        let deadlines = [self.settle_at, toast_until, self.time_until, self.slide_at, snapshot_at, self.picture_due, self.font_due, self.grid_due, self.trailer_due.map(|(_, at)| at), self.overview_due];
        let next = deadlines.into_iter().flatten().min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(expand_action("cp %f /tmp/%n", path), r"cp '/pics/it'\''s here.jpg' /tmp/'it'\''s here.jpg'");
        assert_eq!(expand_action("echo 100%% %x %", path), "echo 100% %x %");
    }
}
