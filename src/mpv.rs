//! Video playback through libmpv, loaded at run time so that jog still
//! builds and runs, for images, on machines without it.
//!
//! mpv decodes, keeps time and plays the audio; its software renderer draws
//! each frame into the same kind of buffer the image path fills, so the
//! overlays work on top of video unchanged.

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use libloading::Library;

const LIBRARY_NAMES: &[&str] = &["libmpv.so.2", "libmpv.so.1", "libmpv.so"];

// Values from mpv's client.h and render.h.
const FORMAT_STRING: c_int = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_COMMAND_REPLY: c_int = 5;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_PLAYBACK_RESTART: c_int = 21;
const EVENT_PROPERTY_CHANGE: c_int = 22;
const RENDER_PARAM_END: c_int = 0;
const RENDER_PARAM_API_TYPE: c_int = 1;
const RENDER_PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;
const RENDER_PARAM_SW_SIZE: c_int = 17;
const RENDER_PARAM_SW_FORMAT: c_int = 18;
const RENDER_PARAM_SW_STRIDE: c_int = 19;
const RENDER_PARAM_SW_POINTER: c_int = 20;

type Handle = *mut c_void;
type RenderContext = *mut c_void;
type Callback = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

/// What a property-change event carries.
#[repr(C)]
struct RawProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

/// The libmpv functions jog calls.
struct Api {
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property_async: unsafe extern "C" fn(Handle, u64, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
    observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    command_async: unsafe extern "C" fn(Handle, u64, *mut *const c_char) -> c_int,
    wait_event: unsafe extern "C" fn(Handle, c_double) -> *mut RawEvent,
    set_wakeup_callback: unsafe extern "C" fn(Handle, Option<Callback>, *mut c_void),
    render_context_create: unsafe extern "C" fn(*mut RenderContext, Handle, *mut RenderParam) -> c_int,
    render_context_set_update_callback: unsafe extern "C" fn(RenderContext, Option<Callback>, *mut c_void),
    render_context_update: unsafe extern "C" fn(RenderContext) -> u64,
    render_context_render: unsafe extern "C" fn(RenderContext, *mut RenderParam) -> c_int,
    render_context_free: unsafe extern "C" fn(RenderContext),
}

impl Api {
    fn load(library: &Library) -> Result<Api, libloading::Error> {
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: each symbol is given the signature its C header declares.
                *unsafe { library.get(concat!("mpv_", $name, "\0").as_bytes())? }
            };
        }
        Ok(Api {
            create: symbol!("create"),
            initialize: symbol!("initialize"),
            terminate_destroy: symbol!("terminate_destroy"),
            set_option_string: symbol!("set_option_string"),
            set_property_async: symbol!("set_property_async"),
            get_property: symbol!("get_property"),
            get_property_string: symbol!("get_property_string"),
            free: symbol!("free"),
            observe_property: symbol!("observe_property"),
            command: symbol!("command"),
            command_async: symbol!("command_async"),
            wait_event: symbol!("wait_event"),
            set_wakeup_callback: symbol!("set_wakeup_callback"),
            render_context_create: symbol!("render_context_create"),
            render_context_set_update_callback: symbol!("render_context_set_update_callback"),
            render_context_update: symbol!("render_context_update"),
            render_context_render: symbol!("render_context_render"),
            render_context_free: symbol!("render_context_free"),
        })
    }
}

/// What a player is for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    /// Playing a video. `silent` discards the audio; `gpu` has it decoded
    /// by the GPU, unless `--no-hwdec`.
    Playback { silent: bool, gpu: bool },
    /// Fetching frames to show while the pointer is over the seek bar.
    Preview,
}

/// Why mpv wants attention. Sent from mpv's own threads.
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
}

type Wake = Box<dyn Fn(Signal) + Send + Sync>;

pub struct Player {
    api: Api,
    handle: Handle,
    render: RenderContext,
    /// The last known values of the properties the viewer asks about
    /// while a video plays. Each is told of as it changes, so that
    /// asking never waits on the player's core, which is busy for tens
    /// of milliseconds whenever it opens a file or seeks.
    known: Mutex<Vec<Option<Known>>>,
    /// Whether this player watches them: a preview player does not.
    watching: bool,
    /// Whether the file open is HLG, and the filters the viewer has asked
    /// for, to turn or mirror it: see `set_filter`.
    hlg: Cell<bool>,
    view_filter: RefCell<String>,
    /// The tracks and delays the next file opened starts with, when one
    /// was chosen for it: audio, subtitles ("no" for none), then the
    /// subtitle and audio delays in seconds.
    next_tracks: RefCell<Option<(String, String, f64, f64)>>,
    /// Held while a frame is drawn: by `warm` on its own thread, and
    /// by `render`, so that one draws at a time.
    drawing: std::sync::Arc<Mutex<()>>,
    /// Handed to mpv as callback context, so it must not move.
    _wake: Box<Wake>,
    /// Keeps the function pointers in `api` valid. Dropped last.
    _library: Library,
}

/// The properties watched, with the format each is wanted in. Their
/// places here, from 1, are what mpv tags the change events with.
const WATCHED: &[(&str, c_int)] = &[
    ("eof-reached", FORMAT_FLAG),
    ("time-pos", FORMAT_DOUBLE),
    ("duration", FORMAT_DOUBLE),
    ("pause", FORMAT_FLAG),
    ("seeking", FORMAT_FLAG),
    ("vo-configured", FORMAT_FLAG),
    ("path", FORMAT_STRING),
    ("volume", FORMAT_DOUBLE),
    ("track-list/count", FORMAT_DOUBLE),
    ("video-format", FORMAT_STRING),
    ("video-params/gamma", FORMAT_STRING),
    ("video-params/primaries", FORMAT_STRING),
    ("container-fps", FORMAT_DOUBLE),
    ("frame-drop-count", FORMAT_DOUBLE),
    ("vo-drop-frame-count", FORMAT_DOUBLE),
];

/// A property's value as last told of.
#[derive(Clone, PartialEq, Debug)]
enum Known {
    Flag(bool),
    Number(f64),
    Text(String),
}

fn watched(property: &str) -> Option<usize> {
    WATCHED.iter().position(|(name, _)| *name == property)
}

// SAFETY: mpv's client and render APIs may be called from any thread, as
// long as it is one at a time, which `&self` on a non-`Sync` type ensures.
unsafe impl Send for Player {}

unsafe extern "C" fn on_events(context: *mut c_void) {
    // SAFETY: the context is the `Wake` that `Player` keeps boxed for as
    // long as mpv can call back.
    unsafe { (*(context as *const Wake))(Signal::Events) }
}

unsafe extern "C" fn on_frame(context: *mut c_void) {
    // SAFETY: as in `on_events`.
    unsafe { (*(context as *const Wake))(Signal::Frame) }
}

/// Whether reading a property that takes a while is reported on the
/// terminal: every read waits for the player's core, which is busy for a
/// good while when it opens a file.
pub static DEBUG: AtomicBool = AtomicBool::new(false);

/// Whether videos played are decoded by the GPU, where it can: see
/// `Player::new`.
pub static HWDEC: AtomicBool = AtomicBool::new(true);

fn report_slow(property: &str, asked: Instant) {
    let took = asked.elapsed();
    if took > Duration::from_millis(2) && DEBUG.load(Ordering::Relaxed) {
        eprintln!("jog: {took:>7.1?} mpv   waited for {property}");
    }
}

fn c_string(text: impl AsRef<[u8]>) -> CString {
    // Interior NULs cannot occur in the names used here or in file paths.
    CString::new(text.as_ref()).unwrap_or_default()
}

fn open_library() -> Option<Library> {
    let chosen = std::env::var("JOG_LIBMPV").ok();
    let names = chosen.iter().map(String::as_str).chain(LIBRARY_NAMES.iter().copied());
    // SAFETY: loading libmpv runs only its ordinary library initializers.
    names.filter_map(|name| unsafe { Library::new(name) }.ok()).next()
}

impl Player {
    /// Starts an idle player. `wake` is called from mpv's threads and must
    /// only pass the signal on.
    pub fn new(role: Role, wake: impl Fn(Signal) + Send + Sync + 'static) -> Result<Player, String> {
        let library = open_library().ok_or("libmpv is not installed")?;
        let api = Api::load(&library).map_err(|error| format!("unusable libmpv: {error}"))?;

        // SAFETY: plain calls into libmpv with valid, NUL-terminated strings;
        // the handle is checked before use.
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                return Err("could not create an mpv player".into());
            }
            let options = [
                ("vo", "libmpv"),
                ("idle", "yes"),
                // Hold the last frame at the end instead of going blank.
                ("keep-open", "yes"),
                // Decoding on the CPU starts a file about twice as fast as
                // setting up a GPU decoder for it, and frames are wanted in
                // main memory anyway: for previews and steps back. Playback
                // is decoded by the GPU and copied back, set below.
                ("hwdec", "no"),
                ("input-default-bindings", "no"),
                ("osc", "no"),
                ("terminal", "no"),
                ("audio-client-name", "jog"),
                // After a seek the player otherwise waits for its first
                // frame to have been drawn before it does anything else,
                // answering questions included. Whoever draws it here is
                // as likely as not asking one just then, and each waits on
                // the other until the player gives up a fifth of a second
                // later: a pause after every seek.
                ("video-latency-hacks", "yes"),
            ];
            // A preview only ever shows single frames near where it is asked
            // to look, so it trades accuracy and picture quality for speed.
            let preview: &[(&str, &str)] = &[
                ("pause", "yes"),
                ("aid", "no"),
                ("sid", "no"),
                ("hr-seek", "no"),
                ("vd-lavc-fast", "yes"),
                ("vd-lavc-skiploopfilter", "all"),
                // Leave the cores to the video that is actually playing.
                ("vd-lavc-threads", "2"),
            ];
            // On four cores a 4K AV1 video at 60 fps, decoded on the CPU,
            // dropped 40 frames in 9 s, 90 with panels open; decoded by the
            // GPU, none, its frames drawn faster for the cores left free.
            // Only the first video waits longer to start, the GPU decoder
            // being set up (88 -> 340 ms here): those after it are made
            // ready ahead. Without a decoder for it, the CPU decodes.
            let mut extra: Vec<(&str, &str)> = Vec::new();
            match role {
                Role::Playback { silent, gpu } => {
                    if gpu && HWDEC.load(Ordering::Relaxed) {
                        extra.push(("hwdec", "auto-copy"));
                    }
                    if silent {
                        extra.push(("ao", "null"));
                    }
                }
                Role::Preview => extra.extend_from_slice(preview),
            }
            // `JOG_MPV` sets further options of the player, as `name=value`
            // with commas between: for trying things out.
            let tried = std::env::var("JOG_MPV").unwrap_or_default();
            let tried = tried.split(',').filter_map(|pair| pair.split_once('='));
            for (name, value) in options.iter().chain(&extra).copied().chain(tried) {
                (api.set_option_string)(handle, c_string(name).as_ptr(), c_string(value).as_ptr());
            }
            if (api.initialize)(handle) < 0 {
                (api.terminate_destroy)(handle);
                return Err("could not start the mpv player".into());
            }

            let mut params = [
                RenderParam { kind: RENDER_PARAM_API_TYPE, data: c"sw".as_ptr() as *mut c_void },
                RenderParam { kind: RENDER_PARAM_END, data: ptr::null_mut() },
            ];
            let mut render = ptr::null_mut();
            if (api.render_context_create)(&mut render, handle, params.as_mut_ptr()) < 0 {
                (api.terminate_destroy)(handle);
                return Err("this libmpv has no software renderer (mpv 0.33 or newer is needed)".into());
            }

            let wake: Box<Wake> = Box::new(Box::new(wake));
            let context = &*wake as *const Wake as *mut c_void;
            (api.set_wakeup_callback)(handle, Some(on_events), context);
            (api.render_context_set_update_callback)(render, Some(on_frame), context);
            // Asked for after the callbacks are in place: mpv queues an
            // event for each straight away, and only signals when its
            // queue was empty.
            // A preview watches only the curve its file is coded on.
            for (index, (name, format)) in WATCHED.iter().enumerate() {
                if matches!(role, Role::Playback { .. }) || *name == "video-params/gamma" {
                    (api.observe_property)(handle, index as u64 + 1, c_string(name).as_ptr(), *format);
                }
            }
            let known = Mutex::new(vec![None; WATCHED.len()]);
            let watching = matches!(role, Role::Playback { .. });
            Ok(Player { api, handle, render, known, watching, hlg: Cell::new(false), view_filter: RefCell::new(String::new()), next_tracks: RefCell::new(None), drawing: std::sync::Arc::new(Mutex::new(())), _wake: wake, _library: library })
        }
    }

    /// What is known of a watched property: `None` for one this player
    /// does not watch, which is to be asked for; otherwise its last value
    /// told of, or none when it has none, as while a file is opening. A
    /// watched property is never asked for outright: asking waits on the
    /// player's core, which while it opens a file can be hundreds of
    /// milliseconds; and a frame the player has announced meanwhile is
    /// not taken, on which the player waits in turn.
    fn known(&self, property: &str) -> Option<Option<Known>> {
        let index = watched(property).filter(|_| self.watching)?;
        Some(self.known.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[index].clone())
    }

    pub fn command(&self, arguments: &[&[u8]]) {
        let owned: Vec<CString> = arguments.iter().map(c_string).collect();
        let mut pointers: Vec<*const c_char> = owned.iter().map(|argument| argument.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: a NULL-terminated array of strings that outlive the call.
        unsafe { (self.api.command)(self.handle, pointers.as_mut_ptr()) };
    }

    /// Queues a command without waiting for the player to get to it, which
    /// can take a while when it is busy seeking or decoding.
    pub fn command_later(&self, arguments: &[&[u8]]) {
        self.command_tagged(0, arguments);
    }

    /// As `command_later`, its outcome told of by `PlayerEvent::Replied`
    /// with this tag, which is not 0.
    pub fn command_tagged(&self, tag: u64, arguments: &[&[u8]]) {
        let owned: Vec<CString> = arguments.iter().map(c_string).collect();
        let mut pointers: Vec<*const c_char> = owned.iter().map(|argument| argument.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: as in `command`; mpv copies the strings before returning.
        unsafe { (self.api.command_async)(self.handle, tag, pointers.as_mut_ptr()) };
    }

    /// Opens a file in place of whatever was open, without changing whether
    /// the player is paused or waiting for it to load.
    pub fn open(&self, path: &Path) {
        self.command_later(&[b"loadfile", path.as_os_str().as_bytes()]);
    }

    /// Starts playing a file, replacing whatever was playing, from the
    /// beginning or from a number of seconds in; or, `paused`, opens it on
    /// that frame and stays there.
    pub fn play(&self, path: &Path, from: Option<f64>, paused: bool) {
        self.pick_tracks();
        let start = from.map(|seconds| format!("start={seconds:.3}"));
        let mut arguments: Vec<&[u8]> = vec![b"loadfile", path.as_os_str().as_bytes()];
        if let Some(start) = &start {
            arguments.extend([b"replace".as_slice(), start.as_bytes()]);
        }
        self.command_later(&arguments);
        self.set("pause", if paused { "yes" } else { "no" });
    }

    /// Opens a file and stops on the frame it would start at, so that it
    /// can be played at once when its turn comes. Does not wait for it.
    pub fn hold(&self, path: &Path, from: Option<f64>) {
        self.pick_tracks();
        self.set("pause", "yes");
        // Whatever view the last video was given is not this one's.
        self.set_filter("");
        for (name, value) in [("video-zoom", "0"), ("video-pan-x", "0"), ("video-pan-y", "0")] {
            self.set(name, value);
        }
        let start = from.map(|seconds| format!("start={seconds:.3}"));
        let mut arguments: Vec<&[u8]> = vec![b"loadfile", path.as_os_str().as_bytes()];
        if let Some(start) = &start {
            arguments.extend([b"replace".as_slice(), start.as_bytes()]);
        }
        self.command_later(&arguments);
    }

    /// The tracks and delays the next file opened is to start with, as
    /// kept for it from before; otherwise it gets those the player would
    /// pick by itself.
    pub fn choose_next(&self, aid: &str, sid: &str, sub_delay: f64, audio_delay: f64) {
        *self.next_tracks.borrow_mut() = Some((aid.to_string(), sid.to_string(), sub_delay, audio_delay));
    }

    /// A track chosen for one file is not the choice for the next, which
    /// gets the ones chosen for it, or those the player would pick itself.
    fn pick_tracks(&self) {
        let (aid, sid, sub_delay, audio_delay) = self.next_tracks.take().unwrap_or_else(|| ("auto".into(), "auto".into(), 0.0, 0.0));
        self.set("aid", &aid);
        self.set("sid", &sid);
        self.set("sub-delay", &format!("{sub_delay:.3}"));
        self.set("audio-delay", &format!("{audio_delay:.3}"));
    }

    /// Whether the file has been opened and its frame is there to be shown.
    pub fn holds(&self, path: &Path) -> bool {
        let open = self.text("path").is_some_and(|open| open.as_bytes() == path.as_os_str().as_bytes());
        open && self.flag("seeking") == Some(false) && self.flag("vo-configured") == Some(true)
    }

    pub fn stop(&self) {
        // Queued: waited for, it would take as long as whatever the core
        // has in hand, a seek or the opening of a file.
        self.command_later(&[b"stop"]);
    }

    /// Sets a property, without waiting for the player to get to it:
    /// waiting takes as long as the core is busy, which when it is
    /// opening a file is hundreds of milliseconds. The settings reach
    /// it in the order they are made, along with the commands.
    /// Sets the filters the viewer wants, as mpv's `vf` would take them.
    /// They go after the one this player puts in by itself for HLG.
    pub fn set_filter(&self, filter: &str) {
        if *self.view_filter.borrow() != filter {
            *self.view_filter.borrow_mut() = filter.to_string();
            self.apply_filter();
        }
    }

    /// An HLG video comes out of the software renderer white, with the
    /// brightest parts gone magenta and cyan: mpv 0.34 converts HLG with no
    /// tone mapping, and clips. HLG was made to look right on an SDR screen
    /// as it is, so the player is told it is SDR and converts nothing.
    fn apply_filter(&self) {
        let view = self.view_filter.borrow();
        let hlg = self.hlg.get().then_some("format=gamma=bt.1886");
        let filters: Vec<&str> = hlg.into_iter().chain((!view.is_empty()).then_some(view.as_str())).collect();
        self.set("vf", &filters.join(","));
    }

    pub fn set(&self, property: &str, value: &str) {
        let value = c_string(value);
        let mut text: *const c_char = value.as_ptr();
        // SAFETY: valid NUL-terminated strings, and a pointer to a string
        // pointer, which is what the string format asks for; mpv copies
        // the string before returning.
        unsafe {
            (self.api.set_property_async)(self.handle, 0, c_string(property).as_ptr(), FORMAT_STRING, (&mut text as *mut *const c_char).cast())
        };
    }

    pub fn number(&self, property: &str) -> Option<f64> {
        if let Some(known) = self.known(property) {
            return match known {
                Some(Known::Number(value)) => Some(value),
                _ => None,
            };
        }
        self.number_now(property)
    }

    /// A number asked of the player now, not as it was last told of: for
    /// waiting on it, when the events that would tell are not being read.
    pub fn number_now(&self, property: &str) -> Option<f64> {
        let mut value: c_double = 0.0;
        let data = (&mut value as *mut c_double).cast();
        let asked = Instant::now();
        // SAFETY: `data` points at storage of the type the format asks for.
        let status = unsafe { (self.api.get_property)(self.handle, c_string(property).as_ptr(), FORMAT_DOUBLE, data) };
        report_slow(property, asked);
        (status >= 0).then_some(value)
    }

    pub fn flag(&self, property: &str) -> Option<bool> {
        if let Some(known) = self.known(property) {
            return match known {
                Some(Known::Flag(value)) => Some(value),
                _ => None,
            };
        }
        let mut value: c_int = 0;
        let data = (&mut value as *mut c_int).cast();
        let asked = Instant::now();
        // SAFETY: as in `number`.
        let status = unsafe { (self.api.get_property)(self.handle, c_string(property).as_ptr(), FORMAT_FLAG, data) };
        report_slow(property, asked);
        (status >= 0).then_some(value != 0)
    }

    pub fn text(&self, property: &str) -> Option<String> {
        if let Some(known) = self.known(property) {
            return match known {
                Some(Known::Text(value)) => Some(value),
                _ => None,
            };
        }
        let asked = Instant::now();
        // SAFETY: mpv returns NULL or a NUL-terminated string that is ours
        // to free, which happens once it has been copied.
        unsafe {
            let value = (self.api.get_property_string)(self.handle, c_string(property).as_ptr());
            report_slow(property, asked);
            if value.is_null() {
                return None;
            }
            let text = CStr::from_ptr(value).to_string_lossy().into_owned();
            (self.api.free)(value.cast());
            Some(text)
        }
    }

    /// Reads the events mpv has queued. Call after `Signal::Events`.
    pub fn events(&self) -> Vec<PlayerEvent> {
        let mut events = Vec::new();
        loop {
            // SAFETY: mpv returns a valid event that lives until the next call.
            let event = unsafe { &*(self.api.wait_event)(self.handle, 0.0) };
            match event.event_id {
                EVENT_NONE => return events,
                EVENT_PROPERTY_CHANGE => {
                    let Some(index) = (event.reply_userdata as usize).checked_sub(1).filter(|index| *index < WATCHED.len()) else { continue };
                    // SAFETY: for this event the data is a property record,
                    // whose own data is of the format it says, or none
                    // when the property has no value just now.
                    let value = unsafe {
                        let property = &*(event.data as *const RawProperty);
                        match (property.format, property.data.is_null()) {
                            (_, true) => None,
                            (FORMAT_FLAG, _) => Some(Known::Flag(*(property.data as *const c_int) != 0)),
                            (FORMAT_DOUBLE, _) => Some(Known::Number(*(property.data as *const c_double))),
                            (FORMAT_STRING, _) => {
                                let text = *(property.data as *const *const c_char);
                                (!text.is_null()).then(|| Known::Text(CStr::from_ptr(text).to_string_lossy().into_owned()))
                            }
                            _ => None,
                        }
                    };
                    if WATCHED[index].0 == "eof-reached" && matches!(value, Some(Known::Flag(true))) {
                        events.push(PlayerEvent::AtEnd);
                    }
                    if WATCHED[index].0 == "video-params/gamma" {
                        let hlg = matches!(&value, Some(Known::Text(gamma)) if gamma == "hlg");
                        if hlg != self.hlg.replace(hlg) {
                            self.apply_filter();
                        }
                    }
                    let mut known = self.known.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Told of again with the same value, as it is on a
                    // file being opened, nothing has changed.
                    if known[index] != value {
                        known[index] = value;
                        events.push(if WATCHED[index].0 == "time-pos" { PlayerEvent::Moved } else { PlayerEvent::Changed });
                    }
                }
                EVENT_COMMAND_REPLY if event.reply_userdata != 0 => events.push(PlayerEvent::Replied(event.reply_userdata, event.error >= 0)),
                EVENT_START_FILE => events.push(PlayerEvent::Started),
                EVENT_PLAYBACK_RESTART => events.push(PlayerEvent::Showing),
                EVENT_END_FILE => events.push(PlayerEvent::Ended),
                _ => {}
            }
        }
    }

    /// Whether a frame not yet drawn is waiting: the player also asks to
    /// be drawn again for other reasons, with the frame it had before.
    pub fn frame_waiting(&self) -> bool {
        let _drawing = self.drawing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: the render context lives as long as the player, and the
        // lock keeps it to one thread at a time.
        unsafe { (self.api.render_context_update)(self.render) & 1 != 0 }
    }

    /// Draws the current frame, scaled and letterboxed to fill a
    /// `width` x `height` buffer of `0RGB` pixels. Waits for the frame's
    /// display time if it is not due yet.
    pub fn render(&self, pixels: &mut [u32], width: u32, height: u32) -> bool {
        // Not while it is being drawn ahead on another thread: see `warm`.
        let _drawing = self.drawing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let renderer = Renderer { update: self.api.render_context_update, render: self.api.render_context_render, context: self.render };
        // SAFETY: the context lives as long as the player, and the lock
        // keeps it to one thread at a time.
        unsafe { renderer.draw(pixels, width, height) }
    }

    /// Whether a draw ahead is going on: a frame taken only to keep the
    /// player moving can be left until it is done.
    pub fn warming(&self) -> bool {
        self.drawing.try_lock().is_err()
    }

    /// Draws the frame the player holds at `width` x `height`, into
    /// nothing, on another thread: the first draw at a size sets the
    /// conversion up, 40-100 ms for an HDR video on four cores, which
    /// would otherwise be the first frame's on the main thread when this
    /// player takes over. Drawing on this player waits meanwhile, as does
    /// letting it go.
    pub fn warm(&self, width: u32, height: u32) {
        let renderer = Renderer { update: self.api.render_context_update, render: self.api.render_context_render, context: self.render };
        let drawing = self.drawing.clone();
        let (ready_tx, ready) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _drawing = drawing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = ready_tx.send(());
            let mut pixels = vec![0u32; width as usize * height as usize];
            // SAFETY: the player waits for this lock before it frees the
            // context, and draws on it only under the lock.
            unsafe { renderer.draw(&mut pixels, width, height) };
        });
        // The lock is the thread's before this returns, so that a draw
        // asked for at once waits for it rather than going first.
        let _ = ready.recv();
    }
}

/// What drawing a frame takes: two of mpv's functions and the render
/// context, to be handed to another thread.
#[derive(Clone, Copy)]
struct Renderer {
    update: unsafe extern "C" fn(RenderContext) -> u64,
    render: unsafe extern "C" fn(RenderContext, *mut RenderParam) -> c_int,
    context: RenderContext,
}

// SAFETY: mpv's render functions may be called from any thread, one at a
// time, which the player's drawing lock sees to.
unsafe impl Send for Renderer {}

impl Renderer {
    /// Draws the current frame, scaled and letterboxed to fill a
    /// `width` x `height` buffer of `0RGB` pixels.
    ///
    /// # Safety
    /// The context must be alive, and drawn on by no other thread.
    unsafe fn draw(&self, pixels: &mut [u32], width: u32, height: u32) -> bool {
        if pixels.len() < width as usize * height as usize {
            return false;
        }
        let mut size = [width as c_int, height as c_int];
        let mut stride = width as usize * 4;
        // Left to itself the call waits until the frame is due on screen,
        // which holds up whoever called for as long as a frame lasts, and
        // for a fifth of a second when a seek does away with the frame
        // being waited on. The player says when each frame is to be drawn
        // anyway, so nothing is gained by waiting here as well.
        let mut wait: c_int = 0;
        // In memory a little-endian `0RGB` pixel is blue, green, red, unused.
        let format: &CStr = c"bgr0";
        let mut params = [
            RenderParam { kind: RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr().cast() },
            RenderParam { kind: RENDER_PARAM_SW_FORMAT, data: format.as_ptr() as *mut c_void },
            RenderParam { kind: RENDER_PARAM_SW_STRIDE, data: (&mut stride as *mut usize).cast() },
            RenderParam { kind: RENDER_PARAM_SW_POINTER, data: pixels.as_mut_ptr().cast() },
            RenderParam { kind: RENDER_PARAM_BLOCK_FOR_TARGET_TIME, data: (&mut wait as *mut c_int).cast() },
            RenderParam { kind: RENDER_PARAM_END, data: ptr::null_mut() },
        ];
        // SAFETY: the buffer holds `height` rows of `stride` bytes, checked
        // above, and every parameter points at live storage of its type.
        unsafe {
            (self.update)(self.context);
            (self.render)(self.context, params.as_mut_ptr()) >= 0
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let started = Instant::now();
        // A draw ahead on another thread finishes first.
        let _drawing = self.drawing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: both were created in `new` and are freed exactly once, the
        // render context first as mpv requires.
        unsafe {
            (self.api.render_context_free)(self.render);
            (self.api.terminate_destroy)(self.handle);
        }
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!("jog: {:>7.1?} mpv   player torn down", started.elapsed());
        }
    }
}
