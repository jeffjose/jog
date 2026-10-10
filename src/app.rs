use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
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
use crate::files;
use crate::frame::{fit_size, Frame, Transform};
use crate::loader::{Job, Loader, Request};
use crate::meta::{self, Meta};
use crate::mpv::{Player, PlayerEvent, Role, Signal};
use crate::ops;
use crate::options::{LastSlide, Options};
use crate::render;
use crate::rng::Rng;
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
/// How long to wait on the preview player before asking it again.
const PREVIEW_TIMEOUT: Duration = Duration::from_millis(400);
/// Seconds that Page Up and Page Down move through a video.
const SEEK_JUMP: f64 = 60.0;
/// How long after a seek the player's reported position cannot be trusted.
const SEEK_SETTLE: Duration = Duration::from_millis(500);

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

/// The rename box: the file being renamed and the name as typed so far.
struct Prompt {
    id: usize,
    text: Vec<char>,
    cursor: usize,
}

pub struct App {
    options: Options,
    /// Path of each file id.
    files: Vec<PathBuf>,
    marks: HashSet<usize>,
    undo: Vec<Undo>,
    prompt: Option<Prompt>,
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
    /// The image layer as last rendered, reused when only overlays change.
    scene: Vec<u32>,
    scene_key: Option<SceneKey>,

    show_info: bool,
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
        let cell_h = (cell_w * 3 / 4).min(usable);
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
            scene: Vec::new(),
            scene_key: None,
            show_info: options.draw_info,
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
            toast: None,
            time_until: None,
            meta: None,
            meta_asked: None,
            text: None,
            video_view_sent: RefCell::new(None),
            scrolled: 0.0,
            modifiers: ModifiersState::empty(),
            mouse: (0.0, 0.0),
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
        let Some(size) = self.window.as_ref().and_then(|w| w.current_monitor()).map(|m| m.size()) else {
            return;
        };
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
        let wanted = self.grid.is_none() && files::is_video(&self.files[id]);
        let playing = self.video.as_ref().map(|video| video.id);
        if wanted && playing != Some(id) {
            self.start_video(id);
        } else if !wanted && playing.is_some() {
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
                self.player = Some(player);
                *self.video_view_sent.borrow_mut() = None;
                // A video may already be waiting for it.
                self.sync_video();
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
        if let Some(player) = &self.player {
            let started = Instant::now();
            player.play(&self.files[id]);
            self.video = Some(Video { id, showing: false, started, seeking: None });
            self.apply_video_view();
        }
    }

    fn video_signal(&mut self, signal: Signal) {
        let Some(player) = &self.player else { return };
        // Read the events even when no video is wanted: mpv only signals
        // again once its queue has been emptied.
        let events = if let Signal::Events = signal { player.events() } else { Vec::new() };
        let Some(video) = &mut self.video else { return };
        for event in events {
            // The still stays up until the player has a frame to show.
            let showing = event == PlayerEvent::Showing;
            if showing && !video.showing && self.options.debug {
                eprintln!("jog: {:>7.1?} video start {}", video.started.elapsed(), self.files[video.id].display());
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
        if video.showing {
            self.redraw();
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
        self.video_action(action);
        true
    }

    fn video_action(&mut self, action: VideoAction) {
        let (Some(player), Some(video)) = (&self.player, &mut self.video) else { return };
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
        if self.list.is_empty() {
            event_loop.exit();
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
            Random => {
                let len = self.list.len();
                if len > 1 {
                    let pick = self.rng.below(len - 1);
                    self.go_to(if pick >= self.pos { pick + 1 } else { pick }, true);
                }
            }
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
                self.request();
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
            PlayPause => self.video_action(VideoAction::Pause),
            SeekForward => self.video_action(VideoAction::Seek(SEEK_STEP)),
            SeekBack => self.video_action(VideoAction::Seek(-SEEK_STEP)),
            FrameForward => self.video_action(VideoAction::Frame(true)),
            FrameBack => self.video_action(VideoAction::Frame(false)),
            VolumeUp => self.video_action(VideoAction::Volume(5)),
            VolumeDown => self.video_action(VideoAction::Volume(-5)),
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
        if self.list.is_empty() {
            event_loop.exit();
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
                        self.list.insert(item.pos.min(self.list.len()), item.id);
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
        self.prompt = Some(Prompt { id, text, cursor });
        self.menu = None;
        self.redraw();
    }

    fn prompt_key(&mut self, key: Key<&str>) {
        let Some(prompt) = &mut self.prompt else { return };
        let typing = !self.modifiers.control_key() && !self.modifiers.alt_key();
        match key {
            Key::Named(NamedKey::Escape) => self.prompt = None,
            Key::Named(NamedKey::Enter) => self.finish_rename(),
            Key::Named(NamedKey::Backspace) if prompt.cursor > 0 => {
                prompt.cursor -= 1;
                prompt.text.remove(prompt.cursor);
            }
            Key::Named(NamedKey::Delete) if prompt.cursor < prompt.text.len() => {
                prompt.text.remove(prompt.cursor);
            }
            Key::Named(NamedKey::ArrowLeft) => prompt.cursor = prompt.cursor.saturating_sub(1),
            Key::Named(NamedKey::ArrowRight) => prompt.cursor = (prompt.cursor + 1).min(prompt.text.len()),
            Key::Named(NamedKey::Home) => prompt.cursor = 0,
            Key::Named(NamedKey::End) => prompt.cursor = prompt.text.len(),
            Key::Named(NamedKey::Space) | Key::Character(_) if typing => {
                let typed = if let Key::Character(text) = key { text } else { " " };
                for c in typed.chars() {
                    prompt.text.insert(prompt.cursor, c);
                    prompt.cursor += 1;
                }
            }
            _ => {}
        }
        self.redraw();
    }

    fn finish_rename(&mut self) {
        let Some(prompt) = self.prompt.take() else { return };
        let typed: String = prompt.text.iter().collect();
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
    fn open_menu(&mut self, all_entries: &'static [Option<MenuItem>], title: Option<String>) {
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
        let (x, y) = if title.is_some() { centred } else { (self.mouse.0 as i32, self.mouse.1 as i32) };
        self.menu = Some(Menu {
            x: x.clamp(0, (width as i32 - menu_width).max(0)),
            y: y.clamp(0, (height as i32 - menu_height).max(0)),
            width: menu_width,
            height: menu_height,
            px,
            selected: title.as_ref().and(rows.iter().position(|row| row.item.is_some())),
            rows,
            title,
        });
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
        match key {
            Key::Named(NamedKey::ArrowDown) => step(menu, true),
            Key::Named(NamedKey::ArrowUp) => step(menu, false),
            Key::Named(NamedKey::Enter) => self.activate_menu(event_loop),
            Key::Named(NamedKey::Escape | NamedKey::Tab) => self.menu = None,
            _ => {}
        }
        self.redraw();
    }

    /// Closes the menu, running the selected item if there is one.
    fn activate_menu(&mut self, event_loop: &ActiveEventLoop) {
        let Some(menu) = self.menu.take() else { return };
        if let Some(item) = menu.selected.and_then(|row| menu.rows[row].item) {
            self.execute(item.command, event_loop);
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

    /// Shuffles the viewing order. The image being viewed stays current.
    fn shuffle(&mut self) {
        let id = self.id();
        self.rng.shuffle(&mut self.list);
        self.pos = self.list.iter().position(|&other| other == id).unwrap_or(0);
        self.request();
        self.refresh();
    }

    fn grid_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let layout = GridLayout::for_window(self.window_size());
        match key {
            Key::Named(NamedKey::Escape) => return self.toggle_grid(),
            Key::Named(NamedKey::ArrowUp) => return self.step_cursor(0, -1),
            Key::Named(NamedKey::ArrowDown) => return self.step_cursor(0, 1),
            _ => {}
        }
        match commands::for_key(key, ModifiersState::empty(), false) {
            Some(Command::Next) => self.step_cursor(1, 0),
            Some(Command::Previous) => self.step_cursor(-1, 0),
            Some(Command::Random) => self.shuffle(),
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
        if self.menu.is_some() {
            return self.menu_key(key, event_loop);
        }
        if self.grid.is_some() {
            return self.grid_key(key, event_loop);
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
            let hovered = menu.row_at(x, y);
            if hovered.is_some() && hovered != menu.selected {
                menu.selected = hovered;
                self.redraw();
            }
            return;
        }
        if self.grid.is_some() {
            self.point_cursor();
            return;
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
                    menu.selected = menu.row_at(x, y);
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
                if self.strip_visible() && y >= f64::from(strip.top) {
                    let first_cell = f64::from(width as i32 / 2 - strip.cell_w / 2 - strip.pad / 2);
                    let offset = ((x - first_cell) / f64::from(strip.pitch())).floor() as i64;
                    let pos = self.pos as i64 + offset;
                    if (0..self.list.len() as i64).contains(&pos) {
                        self.go_to(pos as usize, offset > 0);
                    }
                } else {
                    self.drag = Some(Drag::Pan);
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
                        "F2" => Key::Named(NamedKey::F2),
                        "F5" => Key::Named(NamedKey::F5),
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
                    self.paint(&mut buffer, (size.width, size.height));
                    window.pre_present_notify();
                    let _ = buffer.present();
                }
            }
        }
        self.surface = Some(surface);
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
        if player.is_some_and(|player| player.render(pixels, window.0, window.1)) {
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
                    if self.options.debug {
                        let how = if smooth { "smooth" } else { "nearest" };
                        eprintln!("jog: {:>7.1?} draw  {how} {}x{} -> {}x{}", started.elapsed(), key.frame.width, key.frame.height, layout.width, layout.height);
                    }
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
        if viewing && !self.marks.is_empty() {
            let count = format!("{} marked", self.marks.len());
            top_left.push(if marked { format!("MARKED   ({count})") } else { count });
        }
        if viewing && self.show_info {
            top_left.push(format!("{}   [{}/{}]", self.name(), self.pos + 1, self.list.len()));
            top_left.push(self.info_line());
            if let Some((position, length)) = self.video_time() {
                top_left.push(format!("{} / {}", clock(position), clock(length)));
            }
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
        if nothing_to_write && self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() && !self.help {
            return;
        }
        let mut text = self.text.take().unwrap_or_else(Text::load);
        self.draw_grid_footer(canvas, &mut text, &footer, px);
        text_block(canvas, &mut text, &top_left, px, |_, _| (margin, margin));
        text_block(canvas, &mut text, &exif, px, |_, h| (margin, bottom - h));
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
        if let Some(prompt) = &self.prompt {
            draw_prompt(canvas, &mut text, prompt, px, bottom);
        }
        if let Some(confirm) = &self.confirm {
            draw_confirm(canvas, &mut text, confirm, px);
        }
        if self.help {
            draw_help(canvas, &mut text, px);
        }
        self.text = Some(text);
    }

    fn draw_grid(&self, canvas: &mut Canvas) {
        let Some(grid) = &self.grid else { return };
        let layout = GridLayout::for_window((canvas.width as u32, canvas.height as u32));
        let first = grid.cursor / layout.per_page() * layout.per_page();
        let edge = (layout.pad / 3).max(2);

        for (cell, pos) in (first..self.list.len()).take(layout.per_page()).enumerate() {
            let (column, row) = (cell as i32 % layout.columns, cell as i32 / layout.columns);
            let left = layout.left + column * layout.cell_w + layout.pad;
            let top = layout.top + row * layout.cell_h + layout.pad;
            let (cell_w, cell_h) = (layout.cell_w - 2 * layout.pad, layout.cell_h - 2 * layout.pad);
            match self.loader.thumb(self.list[pos]) {
                Some(thumb) => {
                    let (w, h) = fit_size((thumb.width, thumb.height), (cell_w as u32, cell_h as u32), false);
                    let (w, h) = (w as i32, h as i32);
                    canvas.image(&thumb, left + (cell_w - w) / 2, top + (cell_h - h) / 2, w, h);
                }
                None => canvas.fill(left, top, cell_w, cell_h, PLACEHOLDER_COLOR),
            }
            if self.marks.contains(&self.list[pos]) {
                canvas.fill(left, top, layout.pad * 2, layout.pad * 2, MARK_COLOR);
            }
            // The cursor is boxed brightly, the image being viewed quietly.
            if pos == grid.cursor || pos == self.pos {
                let color = if pos == grid.cursor { TEXT_COLOR } else { HIGHLIGHT_COLOR };
                canvas.outline(left - edge, top - edge, cell_w + 2 * edge, cell_h + 2 * edge, edge, color);
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
fn draw_prompt(canvas: &mut Canvas, text: &mut Text, prompt: &Prompt, px: u32, bottom: i32) {
    const LABEL: &str = "Rename:   ";
    let typed: String = prompt.text.iter().collect();
    let before_cursor: String = prompt.text[..prompt.cursor].iter().collect();
    let pad = px as i32;
    let line_height = Text::line_height(px);
    let label_width = text.width(LABEL, px);
    let width = (label_width + text.width(&typed, px) + 3 * pad).max(px as i32 * 30).min(canvas.width);
    let height = line_height + 2 * pad;
    let (left, top) = ((canvas.width - width) / 2, bottom - height);

    canvas.fill(left, top, width, height, MENU_COLOR);
    canvas.outline(left, top, width, height, 1, MENU_EDGE_COLOR);
    text.draw(canvas, left + pad, top + pad, px, DIM_TEXT_COLOR, LABEL);
    text.draw(canvas, left + pad + label_width, top + pad, px, TEXT_COLOR, &typed);
    let cursor = left + pad + label_width + text.width(&before_cursor, px);
    canvas.fill(cursor, top + pad, (px as i32 / 10).max(2), line_height, TEXT_COLOR);
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
            Event::Loaded(Job::Thumb(_)) if self.strip_visible() || self.grid.is_some() => self.redraw(),
            Event::Loaded(_) => {}
            Event::Converted { created, failed, format } => self.converted(created, failed, format),
            Event::Video(signal) => self.video_signal(signal),
            Event::Player(player) => self.player_ready(player, event_loop),
            Event::Preview(signal) => self.preview_signal(signal),
            // Without it the seek bar still works, just without pictures.
            Event::PreviewPlayer(preview) => self.preview = preview.ok(),
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
        let snapshot_at = self.snapshot.as_ref().and_then(|snapshot| snapshot.at);
        if snapshot_at.is_some_and(|at| at <= now) {
            return self.finish_snapshot(event_loop);
        }

        let toast_until = self.toast.as_ref().map(|(_, until)| *until);
        let deadlines = [self.settle_at, toast_until, self.time_until, self.slide_at, snapshot_at, self.picture_due];
        let next = deadlines.into_iter().flatten().min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_quote_paths_for_the_shell() {
        let path = Path::new("/pics/it's here.jpg");
        assert_eq!(expand_action("cp %f /tmp/%n", path), r"cp '/pics/it'\''s here.jpg' /tmp/'it'\''s here.jpg'");
        assert_eq!(expand_action("echo 100%% %x %", path), "echo 100% %x %");
    }
}
