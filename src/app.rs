use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{self, AtomicBool};
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
use crate::options::{LastSlide, Options};
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
const HIGHLIGHT_COLOR: u32 = 0x2D5B9A;
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
    video_lines: Option<(Instant, Vec<String>)>,
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
        let side = self.unit * 5 / 4;
        (self.unit * 3 / 2, self.middle - side / 2, side)
    }

    /// Whether a window position is on the symbol, with some slack around
    /// it to make it easy to hit.
    fn on_symbol(&self, x: f64, y: f64) -> bool {
        let (left, top, side) = self.symbol();
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
}

/// A line of text being typed, and where in it the cursor is.
#[derive(Default)]
struct TextField {
    text: Vec<char>,
    cursor: usize,
}

impl TextField {
    fn string(&self) -> String {
        self.text.iter().collect()
    }

    /// Applies an editing key. Returns whether the text changed.
    fn key(&mut self, key: Key<&str>, modifiers: ModifiersState) -> bool {
        let typing = !modifiers.control_key() && !modifiers.alt_key();
        match key {
            Key::Named(NamedKey::Backspace) if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            Key::Named(NamedKey::Delete) if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            Key::Named(NamedKey::Space) | Key::Character(_) if typing => {
                let typed = if let Key::Character(text) = key { text } else { " " };
                for c in typed.chars() {
                    self.text.insert(self.cursor, c);
                    self.cursor += 1;
                }
            }
            Key::Named(NamedKey::ArrowLeft) => self.cursor = self.cursor.saturating_sub(1),
            Key::Named(NamedKey::ArrowRight) => self.cursor = (self.cursor + 1).min(self.text.len()),
            Key::Named(NamedKey::Home) => self.cursor = 0,
            Key::Named(NamedKey::End) => self.cursor = self.text.len(),
            _ => {}
        }
        matches!(key, Key::Named(NamedKey::Backspace | NamedKey::Delete | NamedKey::Space) | Key::Character(_))
    }
}

/// The trailer playing in the grid, under the cursor.
struct Trailer {
    id: usize,
    /// The frame last taken from the thread playing it.
    frame: Option<Frame>,
}

/// The go-to box: what is typed, the list positions that match it with
/// the closest first, and which of them is picked out.
struct Palette {
    field: TextField,
    matches: Vec<usize>,
    selected: usize,
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

/// The rename box: the file being renamed and the name as typed so far.
struct Prompt {
    id: usize,
    field: TextField,
}

/// How a search is brought to a close.
#[derive(Clone, Copy, PartialEq)]
enum SearchEnd {
    /// Go to the match in view, among all the files.
    Jump,
    /// Carry on with only the matches.
    Keep,
    /// Carry on with every file, dropping a search kept earlier.
    All,
    /// Go back to how things were.
    Cancel,
}

/// The search box, and what to go back to if the search is abandoned.
struct Search {
    field: TextField,
    /// Every file, in order: what is searched, and what the list returns to.
    base: Vec<usize>,
    /// Whether nothing matches what is typed. The list then stays as it was.
    no_match: bool,
    /// Which part has the keyboard: 0 for the text, then the buttons in
    /// the order `search_buttons` gives them.
    focus: usize,
    /// Where the buttons were last drawn, for clicks: left, top, width
    /// and height.
    drawn: Vec<(SearchEnd, [i32; 4])>,
    /// The list, its kept search and the files in view when the box opened.
    prior_list: Vec<usize>,
    prior_filter: Option<(String, Vec<usize>)>,
    prior_viewed: usize,
    prior_focus: usize,
}

pub struct App {
    options: Options,
    /// Path of each file id.
    files: Vec<PathBuf>,
    marks: HashSet<usize>,
    undo: Vec<Undo>,
    prompt: Option<Prompt>,
    search: Option<Search>,
    palette: Option<Palette>,
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
    /// The first row the file list was scrolled to with the wheel, along
    /// with the position and list length it was scrolled at: once either
    /// changes, the list goes back to following the file on view.
    list_scroll: Option<(usize, usize, usize)>,
    /// The list position under the pointer, in the file list.
    list_hover: Option<usize>,
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
        let height = (window_height as i32 / 12).clamp(64, 220).min(window_height as i32);
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
/// the pointer; the page on show is whichever one holds it.
struct Grid {
    cursor: usize,
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
}

impl GridLayout {
    fn for_window(window: (u32, u32)) -> GridLayout {
        let (width, height) = (window.0 as i32, window.1 as i32);
        let usable = (height - Self::footer_height(window.1)).max(1);
        let columns = (width / 240).clamp(2, 6);
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
        }
    }

    fn footer_height(window_height: u32) -> i32 {
        Text::line_height(ui_px(window_height)) * 2
    }

    fn per_page(&self) -> usize {
        (self.columns * self.rows) as usize
    }

    fn pages(&self, images: usize) -> usize {
        images.div_ceil(self.per_page()).max(1)
    }

    fn thumb_size(&self) -> (u32, u32) {
        ((self.cell_w - 2 * self.pad).max(1) as u32, (self.cell_h - 2 * self.pad).max(1) as u32)
    }

    /// Index within the page of the cell at a window position.
    fn cell_at(&self, x: f64, y: f64) -> Option<usize> {
        let column = ((x - f64::from(self.left)) / f64::from(self.cell_w)).floor() as i32;
        let row = ((y - f64::from(self.top)) / f64::from(self.cell_h)).floor() as i32;
        let inside = (0..self.columns).contains(&column) && (0..self.rows).contains(&row);
        inside.then(|| (row * self.columns + column) as usize)
    }
}

fn ui_px(window_height: u32) -> u32 {
    (window_height / 90).clamp(13, 30)
}

impl App {
    pub fn new(options: Options, files: Vec<PathBuf>, start: usize, loader: Loader, proxy: EventLoopProxy<Event>) -> Self {
        let slide_delay = Duration::from_secs_f64(options.slideshow_delay.unwrap_or(DEFAULT_SLIDE_DELAY));
        App {
            marks: HashSet::new(),
            undo: Vec::new(),
            prompt: None,
            search: None,
            palette: None,
            filter: None,
            sort: options.listing.sort.filter(|_| !options.listing.randomize),
            reversed: options.listing.reverse,
            shuffled: options.listing.randomize,
            show_list: false,
            list_scroll: None,
            list_hover: None,
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
        // While searching it shows the matches as they narrow.
        self.strip_pinned || self.strip_hover || self.search.is_some()
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
        let thumb_size = GridLayout::for_window(self.monitor).thumb_size();
        if let Some(grid) = &self.grid {
            // The page on show, then the pages either side of it.
            let per_page = GridLayout::for_window(self.window_size()).per_page() as i64;
            let page = grid.cursor as i64 / per_page;
            for page in [page, page + 1, page - 1] {
                let positions = (page * per_page).max(0)..((page + 1) * per_page).min(len);
                thumbs.extend(positions.map(|at| self.list[at as usize]));
            }
        } else if self.strip_used {
            for distance in 0..=THUMB_RADIUS {
                for at in [pos + distance, pos - distance] {
                    if (0..len).contains(&at) && !thumbs.contains(&self.list[at as usize]) {
                        thumbs.push(self.list[at as usize]);
                    }
                }
            }
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
        // While searching, the one already playing carries on, but none is
        // started: each match in turn would start playing.
        let startable = self.search.is_none() || playing == Some(id);
        let wanted = self.grid.is_none() && startable && files::is_video(&self.files[id]);
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
        SeekBar { x: unit * 8, width: (width as i32 - unit * 14).max(1), middle: base - unit * 2, unit }
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
        let open = self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some() || self.search.is_some();
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
                self.toast(if self.keep_view { "Keeping zoom between images" } else { "Zoom resets between images" });
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
            ToggleInfo => self.show_info = !self.show_info,
            ToggleDebug => self.show_debug = !self.show_debug,
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
            Palette => {
                self.menu = None;
                self.palette = Some(self::Palette { field: TextField::default(), matches: Vec::new(), selected: 0 });
                self.palette_changed();
            }
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
            SortMenu => self.open_menu(commands::SORT, Some("Sort".to_string())),
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
                self.start_search();
                if let Some(search) = &mut self.search {
                    search.field.text = "is:marked".chars().collect();
                    search.field.cursor = search.field.text.len();
                }
                self.search_changed();
                self.finish_search(SearchEnd::Keep);
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
            Undo => self.undo(),
            Rename => self.start_rename(),
            Delete => self.ask_delete(),
            Convert(format) => self.convert(format),
            SpeedUp => self.change_speed(Some(true)),
            SpeedDown => self.change_speed(Some(false)),
            SpeedReset => self.change_speed(None),
            PlayPause => self.video_action(VideoAction::Pause),
            SeekForward => self.video_action(VideoAction::Seek(SEEK_STEP)),
            SeekBack => self.video_action(VideoAction::Seek(-SEEK_STEP)),
            FrameForward => self.video_action(VideoAction::Frame(true)),
            FrameBack => self.video_action(VideoAction::Frame(false)),
            VolumeUp => self.video_action(VideoAction::Volume(5)),
            VolumeDown => self.video_action(VideoAction::Volume(-5)),
            Restart => self.seek_to(0.0, true),
            Search => self.start_search(),
            ClearFilter => self.clear_filter(),
            Quit => event_loop.exit(),
        }
        self.redraw();
    }

    fn run_action(&mut self, slot: usize) {
        let Some(template) = &self.options.actions[slot] else {
            self.toast(format!("No action {slot} defined"));
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
            ToggleList => self.show_list,
            SortBy(sort) => self.sort == Some(sort),
            ReverseOrder => self.reversed,
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
        let Some(dir) = self.options.output_dir.clone() else {
            self.toast("No destination folder: start jog with -o DIR");
            return;
        };
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

    /// Asks before deleting the target files for good.
    fn ask_delete(&mut self) {
        let ids = self.targets();
        let question = match ids.as_slice() {
            [only] => format!("Delete {}?", self.name_of(*only)),
            many => format!("Delete {} marked images?", many.len()),
        };
        self.menu = None;
        self.confirm = Some(Confirm { question, yes: false, ids });
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
        self.prompt = Some(Prompt { id, field: TextField { text, cursor } });
        self.menu = None;
        self.redraw();
    }

    fn prompt_key(&mut self, key: Key<&str>) {
        let Some(prompt) = &mut self.prompt else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.prompt = None,
            Key::Named(NamedKey::Enter) => self.finish_rename(),
            key => drop(prompt.field.key(key, self.modifiers)),
        }
        self.redraw();
    }

    // ---- Search --------------------------------------------------------

    /// How many files there are in all, whatever the list is narrowed to.
    fn total(&self) -> usize {
        match (&self.search, &self.filter) {
            (Some(search), _) => search.base.len(),
            (None, Some((_, all))) => all.len(),
            (None, None) => self.list.len(),
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

    fn start_search(&mut self) {
        let base = self.filter.as_ref().map_or_else(|| self.list.clone(), |(_, all)| all.clone());
        // A kept search comes back as it was typed, ready to be changed.
        let text: Vec<char> = self.filter.as_ref().map(|(typed, _)| typed.chars().collect()).unwrap_or_default();
        self.search = Some(Search {
            field: TextField { cursor: text.len(), text },
            base,
            no_match: false,
            focus: 0,
            drawn: Vec::new(),
            prior_list: self.list.clone(),
            prior_filter: self.filter.take(),
            prior_viewed: self.id(),
            prior_focus: self.list[self.focus()],
        });
        self.menu = None;
        // The filmstrip shows the matches while typing outside the grid.
        self.strip_used = true;
        self.request();
        self.redraw();
    }

    /// Narrows the list to what the search box now says, with the closest
    /// match in view.
    fn search_changed(&mut self) {
        let Some(search) = &mut self.search else { return };
        let query = Query::parse(&search.field.string());
        let wants_size = query.wants_size();
        let mut pass = |loose: bool| -> Vec<(usize, i32)> {
            (search.base.iter())
                .filter_map(|&id| {
                    let path = &self.files[id];
                    let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                    let size = match wants_size {
                        true => *self.sizes.entry(id).or_insert_with(|| std::fs::metadata(path).ok().map(|meta| meta.len())),
                        false => None,
                    };
                    let facts = Facts { name: &name, video: files::is_video(path), marked: self.marks.contains(&id), size };
                    Some((id, query.score(&facts, loose)?))
                })
                .collect()
        };
        // Names with the letters together, as typed. Only when there are
        // none do names with them scattered count: in a folder of long
        // names nearly any word is in most of them that way.
        let mut scored = pass(false);
        if scored.is_empty() {
            scored = pass(true);
        }
        search.no_match = scored.is_empty();
        if search.no_match {
            return self.redraw();
        }
        // The list keeps its order; only the starting point follows the score.
        let (prior_viewed, prior_focus) = (search.prior_viewed, search.prior_focus);
        let list: Vec<usize> = scored.iter().map(|(id, _)| *id).collect();
        let at = |id: usize| list.iter().position(|&other| other == id);
        let best = if query.is_empty() {
            at(prior_focus).unwrap_or(0)
        } else {
            let top = scored.iter().map(|(_, score)| *score).max().unwrap_or(0);
            scored.iter().position(|(_, score)| *score == top).unwrap_or(0)
        };
        let viewed = if self.grid.is_some() { at(prior_viewed).unwrap_or(best) } else { best };
        self.set_list(list, viewed, best);
    }

    /// The buttons beside the search text, in order: carry on with only the
    /// matches, when the text narrows the list, and go back to every file,
    /// when a search was kept before.
    fn search_buttons(&self) -> Vec<(SearchEnd, String)> {
        let Some(search) = &self.search else { return Vec::new() };
        let mut buttons = Vec::new();
        if !search.no_match && !Query::parse(&search.field.string()).is_empty() {
            buttons.push((SearchEnd::Keep, format!("Show only {} \u{2192}", self.list.len())));
        }
        if search.prior_filter.is_some() {
            buttons.push((SearchEnd::All, format!("Show all {}", search.base.len())));
        }
        buttons
    }

    fn search_key(&mut self, key: Key<&str>) {
        let buttons = self.search_buttons();
        let Some(search) = &mut self.search else { return };
        search.focus = search.focus.min(buttons.len());
        let at_end = search.field.cursor == search.field.text.len();
        match key {
            Key::Named(NamedKey::Escape) => self.finish_search(SearchEnd::Cancel),
            Key::Named(NamedKey::Enter) => {
                let end = search.focus.checked_sub(1).map_or(SearchEnd::Jump, |button| buttons[button].0);
                self.finish_search(end);
            }
            Key::Named(NamedKey::Tab) => self.finish_search(SearchEnd::Keep),
            // Right from the end of the text goes on to the buttons, and
            // left comes back.
            Key::Named(NamedKey::ArrowRight) if at_end || search.focus > 0 => {
                search.focus = (search.focus + 1).min(buttons.len());
            }
            Key::Named(NamedKey::ArrowLeft) if search.focus > 0 => search.focus -= 1,
            // Up and down walk the matches while the box stays open.
            Key::Named(NamedKey::ArrowDown | NamedKey::ArrowUp) => {
                let down = key == Key::Named(NamedKey::ArrowDown);
                if self.grid.is_some() {
                    self.move_cursor(if down { 1 } else { -1 });
                } else if down && self.pos + 1 < self.list.len() {
                    self.go_to(self.pos + 1, true);
                } else if !down && self.pos > 0 {
                    self.go_to(self.pos - 1, false);
                }
            }
            key => {
                // Typing goes to the text wherever the keyboard was.
                search.focus = 0;
                if search.field.key(key, self.modifiers) {
                    self.search_changed();
                }
            }
        }
        self.redraw();
    }

    fn finish_search(&mut self, end: SearchEnd) {
        let Some(search) = self.search.take() else { return };
        let (viewed, focus) = (self.id(), self.list[self.focus()]);
        let typed = search.field.string().trim().to_string();
        let narrowed = !Query::parse(&typed).is_empty() && !search.no_match;
        let at = |list: &[usize], id: usize| list.iter().position(|&other| other == id).unwrap_or(0);
        match end {
            // Keep only the matches. With nothing typed there is nothing to
            // keep, which makes this the way to drop a kept search too.
            SearchEnd::Keep if narrowed => {
                if self.options.verbose {
                    eprintln!("jog: search \"{typed}\" kept: {} of {} files", self.list.len(), search.base.len());
                    for (pos, id) in self.list.iter().enumerate() {
                        eprintln!("jog:     [{}] {}", pos + 1, self.files[*id].display());
                    }
                }
                self.filter = Some((typed, search.base));
                self.toast(format!("Showing {} of {} (Esc for all)", self.list.len(), self.total()));
            }
            SearchEnd::Jump | SearchEnd::Keep | SearchEnd::All => {
                // Go to the match, among all the files again.
                let (viewed, focus) = (at(&search.base, viewed), at(&search.base, focus));
                self.set_list(search.base, viewed, focus);
                if end == SearchEnd::All {
                    self.toast("Showing all files");
                }
            }
            SearchEnd::Cancel => {
                let (viewed, focus) = (at(&search.prior_list, search.prior_viewed), at(&search.prior_list, search.prior_focus));
                self.filter = search.prior_filter;
                self.set_list(search.prior_list, viewed, focus);
            }
        }
        self.sync_video();
        self.redraw();
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

        let text = self.text.get_or_insert_with(Text::load);
        let (row_height, separator_height, pad) = (px as i32 * 9 / 5, px as i32 * 3 / 5, px as i32 / 2);

        let items = entries.iter().flatten();
        let label_width = items.clone().map(|i| text.width(i.label, px)).max().unwrap_or(0);
        let keys_width = items.map(|i| text.width(i.keys, px)).max().unwrap_or(0);
        let title_width = title.as_ref().map_or(0, |title| text.width(title, px));
        // Padding, a gutter for check marks, the label, a gap and the keys.
        let content_width = (px as i32 + label_width + px as i32 * 2 + keys_width).max(title_width);
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

    fn toggle_grid(&mut self) {
        self.grid = match self.grid {
            Some(_) => None,
            None => Some(Grid { cursor: self.pos }),
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
        let Some(grid) = &mut self.grid else { return };
        grid.cursor = (grid.cursor as i64).saturating_add(by).clamp(0, last) as usize;
        self.request();
        self.redraw();
    }

    /// Puts the grid cursor on the thumbnail under the pointer, if any.
    fn point_cursor(&mut self) -> bool {
        let layout = GridLayout::for_window(self.window_size());
        let (len, (x, y)) = (self.list.len(), self.mouse);
        let Some(grid) = &mut self.grid else { return false };
        let first = grid.cursor / layout.per_page() * layout.per_page();
        let Some(pos) = layout.cell_at(x, y).map(|cell| first + cell).filter(|pos| *pos < len) else {
            return false;
        };
        if pos != grid.cursor {
            grid.cursor = pos;
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

    /// Moves the grid cursor one cell. Stepping off the left or right edge
    /// lands on the same row of the neighbouring page; the top and bottom
    /// edges stop it.
    fn step_cursor(&mut self, dx: i64, dy: i64) {
        let layout = GridLayout::for_window(self.window_size());
        let Some(grid) = &self.grid else { return };
        let (columns, rows) = (i64::from(layout.columns), i64::from(layout.rows));
        let (per_page, len) = (layout.per_page() as i64, self.list.len() as i64);
        let cursor = grid.cursor as i64;
        let (column, row) = (cursor % per_page % columns, cursor % per_page / columns);

        let (to_column, to_row) = (column + dx, row + dy);
        let mut target = cursor + dx + dy * columns;
        if to_column >= columns {
            target = cursor + per_page - (columns - 1);
            // A short last page may not reach this row; take its last image.
            let next_page = (cursor / per_page + 1) * per_page;
            if next_page >= len {
                return;
            }
            target = target.min(len - 1);
        } else if to_column < 0 {
            target = cursor - per_page + (columns - 1);
        }
        if !(0..rows).contains(&to_row) || !(0..len).contains(&target) {
            return;
        }
        self.move_cursor(target - cursor);
    }

    /// Shuffles the viewing order. The image being viewed stays current,
    /// and comes first, so that going on from it passes every other file.
    fn shuffle(&mut self) {
        (self.sort, self.reversed, self.shuffled) = (None, false, true);
        let viewed = self.id();
        let mut rng = std::mem::replace(&mut self.rng, Rng::from_clock());
        self.reorder(|ids, _| {
            rng.shuffle(ids);
            if let Some(at) = ids.iter().position(|id| *id == viewed) {
                ids.swap(0, at);
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
        if self.search.is_some() || self.palette.is_some() {
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
            } else if was_first && is_new(0) && sort == Some(Sort::Mtime) && !reversed {
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

    /// Finds the matches for what the go-to box now says: every file with
    /// nothing typed, starting from the one on view, otherwise the closest
    /// match first.
    fn palette_changed(&mut self) {
        let Some(palette) = &self.palette else { return };
        let query = Query::parse(&palette.field.string());
        let (matches, selected) = if query.is_empty() {
            ((0..self.list.len()).collect(), self.pos)
        } else {
            let mut pass = |loose: bool| -> Vec<(usize, i32)> {
                let positions = 0..self.list.len();
                positions.filter_map(|pos| Some((pos, self.score(&query, self.list[pos], loose)?))).collect()
            };
            // As in the search box: scattered letters only as a last resort.
            let mut scored = pass(false);
            if scored.is_empty() {
                scored = pass(true);
            }
            scored.sort_by_key(|(_, score)| -score);
            (scored.into_iter().map(|(pos, _)| pos).collect(), 0)
        };
        if let Some(palette) = &mut self.palette {
            (palette.matches, palette.selected) = (matches, selected);
        }
        self.request();
        self.redraw();
    }

    fn palette_key(&mut self, key: Key<&str>) {
        let rows = self.palette_layout().map_or(1, |layout| layout.rows.max(1));
        let Some(palette) = &mut self.palette else { return };
        let last = palette.matches.len().saturating_sub(1);
        match key {
            Key::Named(NamedKey::Escape) => self.palette = None,
            Key::Named(NamedKey::Enter) => self.finish_palette(),
            Key::Named(NamedKey::ArrowDown) => palette.selected = (palette.selected + 1).min(last),
            Key::Named(NamedKey::ArrowUp) => palette.selected = palette.selected.saturating_sub(1),
            Key::Named(NamedKey::PageDown) => palette.selected = (palette.selected + rows).min(last),
            Key::Named(NamedKey::PageUp) => palette.selected = palette.selected.saturating_sub(rows),
            key => {
                if palette.field.key(key, self.modifiers) {
                    return self.palette_changed();
                }
            }
        }
        self.request();
        self.redraw();
    }

    /// Closes the go-to box and goes to the match picked out in it.
    fn finish_palette(&mut self) {
        let Some(palette) = self.palette.take() else { return };
        if let Some(&pos) = palette.matches.get(palette.selected) {
            // From the grid, the file is opened as Enter on its cell would.
            if let Some(grid) = &mut self.grid {
                grid.cursor = pos;
                return self.open_from_grid();
            }
            self.go_to(pos, pos > self.pos);
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
        let top = height as i32 / 16 + frame.1 + px as i32;
        let fit = ((height as i32 * 5 / 6 - top - head) / row_height).clamp(1, 12) as usize;
        let rows = fit.min(palette.matches.len());
        let first = (palette.selected + 1).saturating_sub(rows);
        let box_width = (px as i32 * 48).min(width as i32);
        Some(PaletteLayout { left: (width as i32 - box_width) / 2, top, width: box_width, head, row_height, first, rows, px, frame })
    }

    /// The files whose matches the go-to box is showing.
    fn palette_shown(&self) -> Vec<usize> {
        let (Some(palette), Some(layout)) = (&self.palette, self.palette_layout()) else { return Vec::new() };
        let shown = palette.matches.iter().skip(layout.first).take(layout.rows);
        shown.map(|pos| self.list[*pos]).collect()
    }

    /// The row of the go-to box at a window position, counted through all
    /// of its matches.
    fn palette_row_at(&self, x: f64, y: f64) -> Option<usize> {
        let layout = self.palette_layout()?;
        let (x, y) = (x as i32 - layout.left, y as i32 - layout.top - layout.head);
        let row = y.div_euclid(layout.row_height);
        ((0..layout.width).contains(&x) && (0..layout.rows as i32).contains(&row)).then(|| layout.first + row as usize)
    }

    fn draw_palette(&self, canvas: &mut Canvas, text: &mut Text, layout: &PaletteLayout) {
        let Some(palette) = &self.palette else { return };
        let (px, pad) = (layout.px, layout.px as i32);
        let line = Text::line_height(px);
        let height = layout.head + layout.rows as i32 * layout.row_height + if layout.rows > 0 { pad / 2 } else { 0 };
        canvas.fill(layout.left, layout.top, layout.width, height, MENU_COLOR);
        canvas.outline(layout.left, layout.top, layout.width, height, 1, MENU_EDGE_COLOR);

        // Above the box, for a video: frames from along it, side by side.
        if let Some(frames) = self.story_subject().and_then(|id| self.story_frames.get(&id)) {
            let gap = (pad / 4).max(2);
            let (cell_w, cell_h) = layout.frame;
            let strip_width = (cell_w + gap) * frames.len() as i32 + gap;
            let (strip_left, strip_top) = ((canvas.width - strip_width) / 2, layout.top - cell_h - 2 * gap - pad / 2);
            canvas.fill(strip_left, strip_top, strip_width, cell_h + 2 * gap, MENU_COLOR);
            canvas.outline(strip_left, strip_top, strip_width, cell_h + 2 * gap, 1, MENU_EDGE_COLOR);
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
            }
        }

        let label = "Go to:   ";
        let typed = palette.field.string();
        let before_cursor: String = palette.field.text[..palette.field.cursor].iter().collect();
        let note = match palette.matches.len() {
            0 => "no match".to_string(),
            count => format!("{} of {count}", palette.selected + 1),
        };
        let (x, y) = (layout.left + pad, layout.top + pad);
        let typed_left = x + text.width(label, px);
        text.draw(canvas, x, y, px, DIM_TEXT_COLOR, label);
        text.draw(canvas, typed_left, y, px, TEXT_COLOR, &typed);
        canvas.fill(typed_left + text.width(&before_cursor, px), y, (pad / 10).max(2), line, TEXT_COLOR);
        let note_left = layout.left + layout.width - pad - text.width(&note, px);
        text.draw(canvas, note_left, y, px, DIM_TEXT_COLOR, &note);

        // Each match: a small picture, the name, and a video's length.
        let thumb = ((layout.row_height - pad / 4) * 16 / 9, layout.row_height - pad / 4);
        for row in 0..layout.rows {
            let at = layout.first + row;
            let (pos, top) = (palette.matches[at], layout.top + layout.head + row as i32 * layout.row_height);
            let id = self.list[pos];
            if at == palette.selected {
                canvas.fill(layout.left + 1, top, layout.width - 2, layout.row_height, HIGHLIGHT_COLOR);
            }
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
            text.draw(canvas, right - length_width, text_top, px, DIM_TEXT_COLOR, &length);
            let name_left = cell_left + thumb.0 + pad;
            let name = clipped(text, &self.name_of(id), right - length_width - pad - name_left, px);
            let color = if self.marks.contains(&id) { MARK_COLOR } else { TEXT_COLOR };
            text.draw(canvas, name_left, text_top, px, color, &name);
        }
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
        let layout = GridLayout::for_window(self.window_size());
        match key {
            Key::Named(NamedKey::Escape) => return self.toggle_grid(),
            Key::Named(NamedKey::ArrowUp) => return self.step_cursor(0, -1),
            Key::Named(NamedKey::ArrowDown) => return self.step_cursor(0, 1),
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
            return self.prompt_key(key);
        }
        if self.search.is_some() {
            return self.search_key(key);
        }
        if self.palette.is_some() {
            return self.palette_key(key);
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
            self.point_cursor();
            return;
        }
        let hover = self.list_row_at(x, y);
        if hover != self.list_hover {
            self.list_hover = hover;
            self.redraw();
        }
        match &mut self.drag {
            Some(Drag::Pan) => self.pan(dx, dy),
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
                let zone = if self.strip_visible() { Strip::for_height(height).height } else { (height as i32 / 50).max(6) };
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
        if self.palette.is_some() {
            // A click on a match goes to it; one anywhere else closes the box.
            if button == MouseButton::Left && pressed {
                let row = self.palette_row_at(x, y);
                match (&mut self.palette, row) {
                    (Some(palette), Some(row)) => {
                        palette.selected = row;
                        self.finish_palette();
                    }
                    _ => self.palette = None,
                }
                self.redraw();
            }
            return;
        }
        if self.search.is_some() {
            // A click picks a match the way Enter does: in the grid, the
            // one clicked on.
            if button == MouseButton::Left && pressed {
                let on = |[left, top, width, height]: [i32; 4]| {
                    (left..left + width).contains(&(x as i32)) && (top..top + height).contains(&(y as i32))
                };
                let drawn = self.search.as_ref().map(|search| search.drawn.clone()).unwrap_or_default();
                if let Some((end, _)) = drawn.into_iter().find(|(_, place)| on(*place)) {
                    return self.finish_search(end);
                }
                if self.grid.is_some() {
                    self.point_cursor();
                }
                self.finish_search(SearchEnd::Jump);
            }
            return;
        }
        if self.grid.is_some() && self.menu.is_none() {
            if button == MouseButton::Left && pressed {
                let hit = |b: &&Button| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32));
                if let Some(command) = self.grid_buttons.iter().find(hit).map(|button| button.command) {
                    self.execute(command, event_loop);
                } else if self.point_cursor() {
                    self.open_from_grid();
                }
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
                if bar.on_symbol(x, y) {
                    self.video_action(VideoAction::Pause);
                } else if bar.on_track(x) {
                    // Click to jump there; keep the button down to scrub.
                    self.seek_to(self.hover.unwrap_or(0.0), true);
                    self.drag = Some(Drag::Scrub);
                }
            }
            (MouseButton::Left, true) => {
                let (width, height) = self.window_size();
                let strip = Strip::for_height(height);
                let panel = self.list_panel().filter(|panel| (x as i32) < panel.width && (y as i32) < panel.bottom);
                if panel.is_some() {
                    // A click on a name goes to that file.
                    if let Some(pos) = self.list_row_at(x, y) {
                        self.go_to(pos, pos > self.pos);
                    }
                } else if self.strip_visible() && y >= f64::from(strip.top) {
                    let first_cell = f64::from(width as i32 / 2 - strip.cell_w / 2 - strip.pad / 2);
                    let offset = ((x - first_cell) / f64::from(strip.pitch())).floor() as i64;
                    let pos = self.pos as i64 + offset;
                    if (0..self.list.len() as i64).contains(&pos) {
                        self.go_to(pos as usize, offset > 0);
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
        // In the go-to box it moves through the matches.
        if self.palette.is_some() {
            let key = if lines < 0.0 { NamedKey::ArrowDown } else { NamedKey::ArrowUp };
            return self.palette_key(Key::Named(key));
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
                let per_page = GridLayout::for_window(self.window_size()).per_page() as i64;
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
        let viewing = self.grid.is_none();
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
        let mut top_left = Vec::new();
        if let Some((typed, all)) = self.filter.as_ref().filter(|_| self.show_info) {
            top_left.push(format!("Search \"{typed}\": {} of {}   (Esc for all)", self.list.len(), all.len()));
        }
        if viewing && !self.marks.is_empty() {
            let count = format!("{} marked", self.marks.len());
            top_left.push(if marked { format!("MARKED   ({count})") } else { count });
        }
        if viewing && self.show_info {
            top_left.push(format!("{}   [{}/{}]", self.name(), self.pos + 1, self.list.len()));
            top_left.push(self.info_line());
            if let Some((position, length)) = self.video_time() {
                let speed = if self.speed == 1.0 { String::new() } else { format!("   {}x", self.speed) };
                top_left.push(format!("{} / {}{speed}", clock(position), clock(length)));
            }
        }
        if viewing && self.show_debug {
            top_left.extend(self.debug_lines());
        }
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
            // Darken the picture behind it, fading in from above.
            for band in 0..6 {
                canvas.shade(0, bar.middle - unit * 4 + band * unit, width, unit, (230 - 22 * band) as u32);
            }
            let done = (f64::from(bar.width) * position / length) as i32;
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 72, bar.width);
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 256, done.max(thickness / 2));
            let handle = unit * 5 / 4;
            canvas.pill(bar.x + done - handle / 2, bar.middle - handle / 2, handle, handle, TEXT_COLOR, 256, handle);
            // The symbol shows the state: a triangle while playing, two bars
            // while paused.
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
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
                    canvas.fill(left - 2, picture_top - 2, w + 4, h + 4, MENU_EDGE_COLOR);
                    canvas.image(frame, left, picture_top, w, h);
                }
            } else if self.time_until.is_some() {
                // To the hundredth while paused, where single frames matter.
                let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
                let now = if paused { clock_precise(position) } else { clock(position) };
                time.push(format!("{now} / {}", clock(length)));
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

        let blocks = [&top_left, &exif, &actions, &toast, &error, &footer, &time];
        let nothing_to_write = blocks.iter().all(|lines| lines.is_empty()) && labels.is_empty();
        let panel = if viewing { self.list_panel() } else { None };
        let boxes = self.menu.is_some() || self.prompt.is_some() || self.search.is_some() || self.confirm.is_some() || panel.is_some();
        let boxes = boxes || self.palette.is_some();
        if nothing_to_write && !boxes && !self.help {
            return;
        }
        let mut text = self.text.take().unwrap_or_else(Text::load);
        self.draw_grid_labels(canvas, &mut text, px);
        self.draw_grid_footer(canvas, &mut text, &footer, px);
        // What sits on the left moves over to make room for the file list.
        let left = margin + panel.as_ref().map_or(0, |panel| panel.width);
        if let Some(panel) = &panel {
            for pos in panel.first..panel.first + panel.rows {
                self.want_duration(self.list[pos]);
            }
            self.draw_list(canvas, &mut text, panel);
        }
        text_block(canvas, &mut text, &top_left, px, |_, _| (left, margin));
        text_block(canvas, &mut text, &exif, px, |_, h| (left, bottom - h));
        text_block(canvas, &mut text, &actions, px, |w, h| (width - margin - w, bottom - h));
        text_block(canvas, &mut text, &toast, px, |w, _| (width - margin - w, margin));
        // Twice the usual size, centred just above the seek bar.
        text_block(canvas, &mut text, &time, px * 2, |w, h| ((width - w) / 2, time_bottom - h));
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
        // Above the filmstrip or, in the grid, above the line under it.
        let box_bottom = if viewing { bottom } else { height - GridLayout::footer_height(height as u32) };
        if let Some(prompt) = &self.prompt {
            draw_prompt(canvas, &mut text, "Rename:   ", &prompt.field, "", &[], 0, px, box_bottom);
        }
        if let Some(layout) = self.palette_layout() {
            for id in self.palette_shown() {
                self.want_duration(id);
            }
            self.draw_palette(canvas, &mut text, &layout);
        }
        let buttons = self.search_buttons();
        if let Some(search) = &mut self.search {
            let note = match search.no_match {
                true => "no match".to_string(),
                false => format!("{} of {}", self.list.len(), search.base.len()),
            };
            let labels: Vec<String> = buttons.iter().map(|(_, label)| label.clone()).collect();
            let focus = search.focus.min(buttons.len());
            let drawn = draw_prompt(canvas, &mut text, "Search:   ", &search.field, &note, &labels, focus, px, box_bottom);
            search.drawn = buttons.iter().map(|(end, _)| *end).zip(drawn).collect();
        }
        if let Some(confirm) = &self.confirm {
            draw_confirm(canvas, &mut text, confirm, px);
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

    /// The space a grid cell gives its picture: left, top, width and
    /// height. With titles on, a line under it is kept for the name.
    fn grid_area(&self, layout: &GridLayout, cell: usize, window_height: u32) -> [i32; 4] {
        let (column, row) = (cell as i32 % layout.columns, cell as i32 / layout.columns);
        let title = if self.grid_titles { Text::line_height(ui_px(window_height)) + layout.pad / 2 } else { 0 };
        let left = layout.left + column * layout.cell_w + layout.pad;
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
        let layout = GridLayout::for_window((canvas.width as u32, canvas.height as u32));
        let first = grid.cursor / layout.per_page() * layout.per_page();
        let edge = (layout.pad / 4).max(2);
        let radius = (layout.cell_w / 36).max(4);

        for (cell, pos) in (first..self.list.len()).take(layout.per_page()).enumerate() {
            let id = self.list[pos];
            let area = self.grid_area(&layout, cell, canvas.height as u32);
            // A video under the cursor plays as a trailer.
            let trailer = self.trailer.as_ref().filter(|trailer| trailer.id == id && pos == grid.cursor);
            let trailer = trailer.and_then(|trailer| trailer.frame.as_ref()).filter(|frame| !frame.pixels.is_empty());
            let thumb = self.loader.thumb(id);
            let [x, y, w, h] = match (trailer, &thumb) {
                (Some(frame), _) => {
                    let place = Self::grid_picture(area, (frame.width, frame.height), true);
                    canvas.image(frame, place[0], place[1], place[2], place[3]);
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
                let color = if pos == grid.cursor { TEXT_COLOR } else { HIGHLIGHT_COLOR };
                let gap = edge * 2;
                canvas.round_outline(x - gap, y - gap, w + 2 * gap, h + 2 * gap, radius + gap, edge, color);
            }
        }
    }

    /// The words on the grid: under each picture its title, when those are
    /// on, and in the corner of each video how long it is.
    fn draw_grid_labels(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let Some(cursor) = self.grid.as_ref().map(|grid| grid.cursor) else { return };
        let layout = GridLayout::for_window((canvas.width as u32, canvas.height as u32));
        let first = cursor / layout.per_page() * layout.per_page();
        let line = Text::line_height(px);
        for (cell, pos) in (first..self.list.len()).take(layout.per_page()).enumerate() {
            let id = self.list[pos];
            let area = self.grid_area(&layout, cell, canvas.height as u32);
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
    }

    /// Draws the line under the grid. While images are marked it also has
    /// the count and a row of buttons for acting on them.
    fn draw_grid_footer(&mut self, canvas: &mut Canvas, text: &mut Text, footer: &[String], px: u32) {
        self.grid_buttons.clear();
        let Some(label) = footer.first() else { return };
        let (pad, gap) = (px as i32 / 2, px as i32 * 2);
        let height = Text::line_height(px) + pad;
        let actions = if self.marks.is_empty() { &[][..] } else { commands::GRID_ACTIONS };
        let count = format!("{} marked", self.marks.len());

        let button_width = |text: &mut Text, label: &str| text.width(label, px) + 2 * pad;
        let mut width = text.width(label, px);
        if !actions.is_empty() {
            width += gap + text.width(&count, px) + gap - pad;
            width += actions.iter().map(|(label, _)| button_width(text, label) + pad).sum::<i32>();
        }
        let footer_height = GridLayout::footer_height(canvas.height as u32);
        let mut x = (canvas.width - width) / 2;
        let y = canvas.height - footer_height + (footer_height - height) / 2;

        canvas.shade(x - pad, y, width + 2 * pad, height, 80);
        text.draw(canvas, x, y + pad / 2, px, TEXT_COLOR, label);
        if actions.is_empty() {
            return;
        }
        x += text.width(label, px) + gap;
        text.draw(canvas, x, y + pad / 2, px, MARK_COLOR, &count);
        x += text.width(&count, px) + gap;
        for &(label, command) in actions {
            let width = button_width(text, label);
            canvas.fill(x, y, width, height, MENU_COLOR);
            canvas.outline(x, y, width, height, 1, MENU_EDGE_COLOR);
            text.draw(canvas, x + pad, y + pad / 2, px, TEXT_COLOR, label);
            self.grid_buttons.push(Button { x, y, width, height, command });
            x += width + pad;
        }
    }

    /// The line under the grid: the image under the cursor and the page.
    fn grid_footer(&self) -> Option<String> {
        let grid = self.grid.as_ref()?;
        let layout = GridLayout::for_window(self.window_size());
        let pages = layout.pages(self.list.len());
        let path = &self.files[self.list[grid.cursor]];
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
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

    /// The debug overlay: what the player reports while a video is showing,
    /// otherwise what decoding and drawing the image cost.
    fn debug_lines(&mut self) -> Vec<String> {
        let perf = &self.perf;
        let redraw = format!("paint {}   present {}", ms(perf.paint), ms(perf.present));
        if self.video.as_ref().is_some_and(|video| video.showing) {
            // Asking the player on every frame would get in its way, and
            // the figures would change too fast to read.
            let fresh = perf.video_lines.as_ref().is_some_and(|(at, _)| at.elapsed() < VIDEO_STATS_EVERY);
            if !fresh {
                let lines = self.video_debug_lines();
                self.perf.video_lines = Some((Instant::now(), lines));
            }
            let perf = &self.perf;
            let mut lines = perf.video_lines.as_ref().map(|(_, lines)| lines.clone()).unwrap_or_default();
            let rate = perf.rate().map(|rate| format!("   {rate:.1} fps drawn")).unwrap_or_default();
            // The player holds a frame back until it is due, so only a
            // paused video shows what drawing one costs by itself.
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
            let render = if paused { "render" } else { "render+wait" };
            lines.insert(1.min(lines.len()), format!("{render} {}   {redraw}{rate}", ms(perf.render)));
            return lines;
        }

        let stats = self.loader.stats(self.id());
        let mut lines = Vec::new();
        if let (Some(made), Some(native)) = (stats.fitted, self.native()) {
            let megapixels = f64::from(native.0) * f64::from(native.1) / 1e6;
            let mut line = if made.how.is_empty() {
                let total = ms(made.read + made.fit);
                let rate = megapixels / made.read.as_secs_f64().max(1e-6);
                format!("decode {total} (read {} + fit {})   {megapixels:.1} MP   {rate:.0} MP/s", ms(made.read), ms(made.fit))
            } else {
                format!("decode {} ({})   {megapixels:.1} MP", ms(made.fit), made.how)
            };
            // Decoded ahead as a neighbour, or only once it was gone to.
            line.push_str(if made.at <= perf.arrived { "   prefetched" } else { "   on demand" });
            if let Some(full) = stats.full {
                let how = if full.how.is_empty() { String::new() } else { format!(" ({})", full.how) };
                line.push_str(&format!("   full-res {}{how}", ms(full.read + full.fit)));
            }
            lines.push(line);
        }
        if let Some(scaled) = &perf.scaled {
            let how = if scaled.smooth { "smooth" } else { "nearest" };
            let (from, to) = (scaled.from, scaled.to);
            lines.push(format!("draw {} {how} {}x{} -> {}x{}", ms(scaled.took), from.0, from.1, to.0, to.1));
        }
        let shown = perf.shown_in.map(|took| format!("   shown in {}", ms(took))).unwrap_or_default();
        lines.push(format!("{redraw}{shown}"));
        let kept = |(count, bytes): (usize, usize)| format!("{count} ({})", meta::human_size(bytes as u64));
        let (fitted, thumbs) = (kept(stats.fitted_kept), kept(stats.thumbs_kept));
        lines.push(format!("in memory: {fitted} frames   {thumbs} thumbnails   {} decoding", stats.busy));
        lines
    }

    /// What the player says about the video: the file, then how playback
    /// is keeping up.
    fn video_debug_lines(&self) -> Vec<String> {
        let (Some(player), Some(video)) = (&self.player, &self.video) else { return Vec::new() };
        let number = |property| player.number(property);
        let mut file = Vec::new();
        if let Some(took) = video.start_took {
            file.push(format!("start {}", ms(took)));
        }
        let size = number("width").zip(number("height")).map(|(w, h)| format!(" {w:.0}x{h:.0}"));
        let codec = player.text("video-format").unwrap_or_default();
        let pixels = player.text("video-params/pixelformat").map(|format| format!(" {format}"));
        file.push(format!("{codec}{}{}", size.unwrap_or_default(), pixels.unwrap_or_default()));
        if let Some(rate) = number("container-fps") {
            file.push(format!("{rate:.2} fps"));
        }
        if let Some(bits) = number("video-bitrate").filter(|bits| *bits > 0.0) {
            file.push(format!("{:.1} Mbps", bits / 1e6));
        }
        match player.text("hwdec-current").as_deref() {
            None | Some("no" | "") => file.push("CPU decode".to_string()),
            Some(decoder) => file.push(format!("{decoder} decode")),
        }

        let mut playback = Vec::new();
        let count = |property| number(property).unwrap_or(0.0);
        playback.push(format!("late {:.0}", count("frame-drop-count")));
        playback.push(format!("dropped {:.0}", count("vo-drop-frame-count")));
        if let Some(sync) = number("avsync") {
            playback.push(format!("A-V {:+.0}ms", sync * 1000.0));
        }
        if let Some(ahead) = number("demuxer-cache-duration") {
            playback.push(format!("read ahead {ahead:.1}s"));
        }
        if let Some(took) = video.seek_took {
            playback.push(format!("seek {}", ms(took)));
        }
        vec![file.join("   "), playback.join("   ")]
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

    fn draw_strip(&self, canvas: &mut Canvas, strip: Strip) {
        canvas.shade(0, strip.top, canvas.width, strip.height, 64);
        let middle = canvas.width / 2 - strip.cell_w / 2;
        let reach = i64::from(canvas.width / 2 / strip.pitch() + 1);
        let top = strip.top + strip.pad;

        for offset in -reach..=reach {
            let pos = self.pos as i64 + offset;
            if !(0..self.list.len() as i64).contains(&pos) {
                continue;
            }
            let left = middle + offset as i32 * strip.pitch();
            match self.loader.thumb(self.list[pos as usize]) {
                Some(thumb) => {
                    let bounds = (strip.cell_w as u32, strip.cell_h as u32);
                    let (w, h) = fit_size((thumb.width, thumb.height), bounds, false);
                    let (w, h) = (w as i32, h as i32);
                    canvas.image(&thumb, left + (strip.cell_w - w) / 2, top + (strip.cell_h - h) / 2, w, h);
                }
                None => canvas.fill(left, top, strip.cell_w, strip.cell_h, PLACEHOLDER_COLOR),
            }
            if self.marks.contains(&self.list[pos as usize]) {
                canvas.fill(left, top, strip.pad, strip.pad, MARK_COLOR);
            }
            if offset == 0 {
                let edge = (strip.pad / 3).max(2);
                canvas.outline(left - edge, top - edge, strip.cell_w + 2 * edge, strip.cell_h + 2 * edge, edge, TEXT_COLOR);
            }
        }
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
        let row_height = Text::line_height(px) + px as i32 / 4;
        // The first row's worth of space holds the heading.
        let top = row_height + px as i32;
        let rows = (((bottom - top) / row_height).max(1) as usize).min(self.list.len());
        let scrolled = self.list_scroll.filter(|(pos, len, _)| (*pos, *len) == (self.pos, self.list.len()));
        let first = scrolled.map_or(self.pos.saturating_sub(rows / 2), |(_, _, first)| first).min(self.list.len() - rows);
        Some(ListPanel { width: (px as i32 * 26).min(width as i32 / 2), bottom, top, row_height, first, rows, px })
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
        let id = self.list[*palette.matches.get(palette.selected)?];
        files::is_video(&self.files[id]).then_some(id)
    }

    /// The video the grid plays a trailer of: the one under its cursor.
    fn trailer_subject(&self) -> Option<usize> {
        let id = self.list[self.grid.as_ref().filter(|_| self.palette.is_none())?.cursor];
        files::is_video(&self.files[id]).then_some(id)
    }

    /// Has the trailer thread play the video under the grid's cursor, and
    /// stop when the cursor leaves it. Called on each redraw. Nothing here
    /// waits on the player: the thread does all of that.
    fn tend_trailer(&mut self) {
        let subject = self.trailer_subject();
        if self.trailer.as_ref().map(|trailer| trailer.id) == subject {
            return;
        }
        if self.trailer.take().is_some() {
            if let Some(trailers) = &self.trailers {
                trailers.stop();
            }
        }
        let Some(id) = subject else { return };
        let proxy = self.proxy.clone();
        let trailers = self.trailers.get_or_insert_with(|| Trailers::new(move |note| proxy.send_event(Event::Trailer(note)).is_ok()));
        let window = self.window.as_ref().map_or((0, 0), |w| w.inner_size().into());
        trailers.play(id, self.files[id].clone(), GridLayout::for_window(window).thumb_size());
        self.trailer = Some(Trailer { id, frame: None });
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

    fn draw_list(&self, canvas: &mut Canvas, text: &mut Text, panel: &ListPanel) {
        let px = panel.px;
        let pad = px as i32 / 2;
        canvas.shade(0, 0, panel.width, panel.bottom, 40);
        let heading = match &self.filter {
            Some((typed, all)) => format!("{} of {}: {typed}", self.list.len(), all.len()),
            None => format!("{} files", self.list.len()),
        };
        let heading = clipped(text, &heading, panel.width - 3 * pad, px);
        text.draw(canvas, pad * 2, pad, px, DIM_TEXT_COLOR, &heading);
        for row in 0..panel.rows {
            let pos = panel.first + row;
            let id = self.list[pos];
            let top = panel.top + row as i32 * panel.row_height;
            if pos == self.pos {
                canvas.fill(0, top, panel.width, panel.row_height, HIGHLIGHT_COLOR);
            } else if self.list_hover == Some(pos) {
                canvas.fill(0, top, panel.width, panel.row_height, MENU_EDGE_COLOR);
            }
            if self.marks.contains(&id) {
                canvas.fill(pad / 2, top + panel.row_height / 4, pad / 2, panel.row_height / 2, MARK_COLOR);
            }
            let line = top + (panel.row_height - Text::line_height(px)) / 2;
            // A video's length sits at the right, once it is known.
            let length = self.durations.get(&id).copied().flatten().map(clock).unwrap_or_default();
            let length_width = if length.is_empty() { 0 } else { text.width(&length, px) + pad };
            text.draw(canvas, panel.width - length_width, line, px, DIM_TEXT_COLOR, &length);
            let name = clipped(text, &self.name_of(id), panel.width - 3 * pad - length_width, px);
            text.draw(canvas, pad * 2, line, px, TEXT_COLOR, &name);
        }
    }

    fn draw_menu(&self, canvas: &mut Canvas, text: &mut Text, menu: &Menu) {
        let px = menu.px;
        let pad = px as i32 / 2;
        canvas.fill(menu.x, menu.y, menu.width, menu.height, MENU_COLOR);
        canvas.outline(menu.x, menu.y, menu.width, menu.height, 1, MENU_EDGE_COLOR);
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
                canvas.fill(menu.x + 1, top, menu.width - 2, row.height, HIGHLIGHT_COLOR);
            }
            if self.is_checked(item.command) {
                let mark = px as i32 / 2;
                canvas.fill(menu.x + pad + mark / 4, top + (row.height - mark) / 2, mark, mark, TEXT_COLOR);
            }
            let line = top + (row.height - Text::line_height(px)) / 2;
            text.draw(canvas, menu.x + pad + px as i32, line, px, TEXT_COLOR, item.label);
            let keys_left = menu.x + menu.width - pad - text.width(item.keys, px);
            text.draw(canvas, keys_left, line, px, DIM_TEXT_COLOR, item.keys);
        }
        if let Some(sub) = &menu.sub {
            self.draw_menu(canvas, text, sub);
        }
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
    canvas.shade(x, y, width, height, 80);
    for (index, line) in lines.iter().enumerate() {
        text.draw(canvas, x + pad, y + pad + index as i32 * line_height, px, TEXT_COLOR, line);
    }
}

/// Draws the delete confirmation in the middle of the window: the question
/// above a No and a Yes button, one of them highlighted.
fn draw_confirm(canvas: &mut Canvas, text: &mut Text, confirm: &Confirm, px: u32) {
    let pad = px as i32;
    let line_height = Text::line_height(px);
    let button_width = px as i32 * 6;
    let button_height = line_height + pad;
    let buttons_width = 2 * button_width + pad;
    let width = (text.width(&confirm.question, px).max(buttons_width) + 2 * pad).min(canvas.width);
    let height = line_height + button_height + 3 * pad;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);

    canvas.fill(left, top, width, height, MENU_COLOR);
    canvas.outline(left, top, width, height, 1, MENU_EDGE_COLOR);
    text.draw(canvas, left + pad, top + pad, px, TEXT_COLOR, &confirm.question);

    let buttons_left = left + (width - buttons_width) / 2;
    let buttons_top = top + 2 * pad + line_height;
    for (index, (label, selected)) in [("No", !confirm.yes), ("Yes", confirm.yes)].into_iter().enumerate() {
        let button_left = buttons_left + index as i32 * (button_width + pad);
        let fill = if selected { HIGHLIGHT_COLOR } else { MENU_COLOR };
        canvas.fill(button_left, buttons_top, button_width, button_height, fill);
        canvas.outline(button_left, buttons_top, button_width, button_height, 1, MENU_EDGE_COLOR);
        let label_left = button_left + (button_width - text.width(label, px)) / 2;
        text.draw(canvas, label_left, buttons_top + pad / 2, px, TEXT_COLOR, label);
    }
}

/// Draws the rename box, centred with its bottom edge at `bottom`.
/// `label` names what is being typed and `note`, at the right, says
/// something about it, such as how many files match.
/// Draws a box to type in, with any buttons in a row to its right, and
/// returns where each button went. `focus` is 0 while the text has the
/// keyboard, otherwise the button that has it, counting from 1.
#[allow(clippy::too_many_arguments)]
fn draw_prompt(
    canvas: &mut Canvas,
    text: &mut Text,
    label: &str,
    field: &TextField,
    note: &str,
    buttons: &[String],
    focus: usize,
    px: u32,
    bottom: i32,
) -> Vec<[i32; 4]> {
    let typed = field.string();
    let before_cursor: String = field.text[..field.cursor].iter().collect();
    let pad = px as i32;
    let gap = pad / 2;
    let line_height = Text::line_height(px);
    let (label_width, note_width) = (text.width(label, px), text.width(note, px));
    let content = label_width + text.width(&typed, px) + note_width;
    let widths: Vec<i32> = buttons.iter().map(|button| text.width(button, px) + 2 * pad).collect();
    let beside: i32 = widths.iter().map(|width| width + gap).sum();
    let width = (content + 5 * pad).max(px as i32 * 30).min(canvas.width - beside);
    let height = line_height + 2 * pad;
    let (left, top) = ((canvas.width - width - beside) / 2, bottom - height);

    canvas.fill(left, top, width, height, MENU_COLOR);
    canvas.outline(left, top, width, height, 1, MENU_EDGE_COLOR);
    text.draw(canvas, left + pad, top + pad, px, DIM_TEXT_COLOR, label);
    text.draw(canvas, left + pad + label_width, top + pad, px, TEXT_COLOR, &typed);
    text.draw(canvas, left + width - pad - note_width, top + pad, px, DIM_TEXT_COLOR, note);
    if focus == 0 {
        let cursor = left + pad + label_width + text.width(&before_cursor, px);
        canvas.fill(cursor, top + pad, (px as i32 / 10).max(2), line_height, TEXT_COLOR);
    }

    let mut x = left + width + gap;
    let mut drawn = Vec::new();
    for (at, (button, button_width)) in buttons.iter().zip(widths).enumerate() {
        let focused = focus == at + 1;
        canvas.fill(x, top, button_width, height, if focused { HIGHLIGHT_COLOR } else { MENU_COLOR });
        canvas.outline(x, top, button_width, height, 1, if focused { TEXT_COLOR } else { MENU_EDGE_COLOR });
        text.draw(canvas, x + pad, top + pad, px, TEXT_COLOR, button);
        drawn.push([x, top, button_width, height]);
        x += button_width + gap;
    }
    drawn
}

/// Draws the list of shortcuts as a panel in the middle of the window.
fn draw_help(canvas: &mut Canvas, text: &mut Text, px: u32) {
    let line_height = Text::line_height(px);
    let (pad, gap) = (px as i32, px as i32 * 2);
    // Each column is as wide as its widest keys plus its widest description.
    let columns: Vec<(i32, i32, i32)> = commands::HELP
        .iter()
        .map(|sections| {
            let entries = sections.iter().flat_map(|(_, entries)| entries.iter());
            let keys = entries.clone().map(|(keys, _)| text.width(keys, px)).max().unwrap_or(0);
            let labels = entries.map(|(_, label)| text.width(label, px)).max().unwrap_or(0);
            let lines: usize = sections.iter().map(|(_, entries)| entries.len() + 2).sum();
            (keys, labels, lines as i32 - 1)
        })
        .collect();
    let width = columns.iter().map(|(keys, labels, _)| keys + gap + labels).sum::<i32>()
        + gap * (columns.len() as i32 - 1)
        + 2 * pad;
    let height = columns.iter().map(|c| c.2).max().unwrap_or(0) * line_height + 2 * pad;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);
    canvas.fill(left, top, width, height, MENU_COLOR);
    canvas.outline(left, top, width, height, 1, MENU_EDGE_COLOR);

    let mut x = left + pad;
    for (sections, (keys_width, labels_width, _)) in commands::HELP.iter().zip(columns) {
        let mut y = top + pad;
        for (title, entries) in sections.iter() {
            text.draw(canvas, x, y, px, DIM_TEXT_COLOR, title);
            y += line_height;
            for (keys, label) in entries.iter() {
                text.draw(canvas, x, y, px, TEXT_COLOR, keys);
                text.draw(canvas, x + keys_width + gap, y, px, TEXT_COLOR, label);
                y += line_height;
            }
            y += line_height;
        }
        x += keys_width + gap + labels_width + gap;
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

/// A playback time to the hundredth of a second, as `m:ss.cc`.
fn clock_precise(seconds: f64) -> String {
    let hundredths = (seconds.max(0.0).fract() * 100.0) as u64;
    format!("{}.{hundredths:02}", clock(seconds))
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
        let snapshot_at = self.snapshot.as_ref().and_then(|snapshot| snapshot.at);
        if snapshot_at.is_some_and(|at| at <= now) {
            return self.finish_snapshot(event_loop);
        }

        let toast_until = self.toast.as_ref().map(|(_, until)| *until);
        let deadlines = [self.settle_at, toast_until, self.time_until, self.slide_at, snapshot_at, self.picture_due, self.font_due];
        let next = deadlines.into_iter().flatten().min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
