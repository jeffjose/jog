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

use libloading::Library;

const LIBRARY_NAMES: &[&str] = &["libmpv.so.2", "libmpv.so.1", "libmpv.so"];

// Values from mpv's client.h and render.h.
const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_PLAYBACK_RESTART: c_int = 21;
const RENDER_PARAM_END: c_int = 0;
const RENDER_PARAM_API_TYPE: c_int = 1;
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

/// The libmpv functions jog calls.
struct Api {
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
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
            set_property_string: symbol!("set_property_string"),
            get_property: symbol!("get_property"),
            command: symbol!("command"),
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
    /// Playing the video on view. `silent` discards the audio.
    Playback { silent: bool },
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
}

type Wake = Box<dyn Fn(Signal) + Send + Sync>;

pub struct Player {
    api: Api,
    handle: Handle,
    render: RenderContext,
    /// Handed to mpv as callback context, so it must not move.
    _wake: Box<Wake>,
    /// Keeps the function pointers in `api` valid. Dropped last.
    _library: Library,
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

fn c_string(text: impl AsRef<[u8]>) -> CString {
    // Interior NULs cannot occur in the names used here or in file paths.
    CString::new(text.as_ref()).unwrap_or_default()
}

fn open_library() -> Option<Library> {
    let chosen = std::env::var("JOG_LIBMPV").ok();
    let names = chosen.iter().map(String::as_str).chain(LIBRARY_NAMES.iter().copied());
    // SAFETY: loading libmpv runs only its ordinary library initialisers.
    names.filter_map(|name| unsafe { Library::new(name) }.ok()).next()
}

/// Whether videos can be played on this machine.
pub fn available() -> bool {
    open_library().is_some()
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
                // main memory anyway.
                ("hwdec", "no"),
                ("input-default-bindings", "no"),
                ("osc", "no"),
                ("terminal", "no"),
                ("audio-client-name", "jog"),
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
            ];
            let extra: &[(&str, &str)] = match role {
                Role::Playback { silent: false } => &[],
                Role::Playback { silent: true } => &[("ao", "null")],
                Role::Preview => preview,
            };
            for (name, value) in options.iter().chain(extra) {
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
            Ok(Player { api, handle, render, _wake: wake, _library: library })
        }
    }

    pub fn command(&self, arguments: &[&[u8]]) {
        let owned: Vec<CString> = arguments.iter().map(c_string).collect();
        let mut pointers: Vec<*const c_char> = owned.iter().map(|argument| argument.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: a NULL-terminated array of strings that outlive the call.
        unsafe { (self.api.command)(self.handle, pointers.as_mut_ptr()) };
    }

    /// Opens a file in place of whatever was open, without changing whether
    /// the player is paused.
    pub fn open(&self, path: &Path) {
        self.command(&[b"loadfile", path.as_os_str().as_bytes()]);
    }

    /// Starts playing a file, replacing whatever was playing.
    pub fn play(&self, path: &Path) {
        self.open(path);
        self.set("pause", "no");
    }

    pub fn stop(&self) {
        self.command(&[b"stop"]);
    }

    pub fn set(&self, property: &str, value: &str) {
        // SAFETY: valid NUL-terminated strings.
        unsafe { (self.api.set_property_string)(self.handle, c_string(property).as_ptr(), c_string(value).as_ptr()) };
    }

    pub fn number(&self, property: &str) -> Option<f64> {
        let mut value: c_double = 0.0;
        let data = (&mut value as *mut c_double).cast();
        // SAFETY: `data` points at storage of the type the format asks for.
        let status = unsafe { (self.api.get_property)(self.handle, c_string(property).as_ptr(), FORMAT_DOUBLE, data) };
        (status >= 0).then_some(value)
    }

    pub fn flag(&self, property: &str) -> Option<bool> {
        let mut value: c_int = 0;
        let data = (&mut value as *mut c_int).cast();
        // SAFETY: as in `number`.
        let status = unsafe { (self.api.get_property)(self.handle, c_string(property).as_ptr(), FORMAT_FLAG, data) };
        (status >= 0).then_some(value != 0)
    }

    /// Reads the events mpv has queued. Call after `Signal::Events`.
    pub fn events(&self) -> Vec<PlayerEvent> {
        let mut events = Vec::new();
        loop {
            // SAFETY: mpv returns a valid event that lives until the next call.
            let id = unsafe { (*(self.api.wait_event)(self.handle, 0.0)).event_id };
            match id {
                EVENT_NONE => return events,
                EVENT_START_FILE => events.push(PlayerEvent::Started),
                EVENT_PLAYBACK_RESTART => events.push(PlayerEvent::Showing),
                EVENT_END_FILE => events.push(PlayerEvent::Ended),
                _ => {}
            }
        }
    }

    /// Draws the current frame, scaled and letterboxed to fill a
    /// `width` x `height` buffer of `0RGB` pixels. Waits for the frame's
    /// display time if it is not due yet.
    pub fn render(&self, pixels: &mut [u32], width: u32, height: u32) -> bool {
        if pixels.len() < width as usize * height as usize {
            return false;
        }
        let mut size = [width as c_int, height as c_int];
        let mut stride = width as usize * 4;
        // In memory a little-endian `0RGB` pixel is blue, green, red, unused.
        let format: &CStr = c"bgr0";
        let mut params = [
            RenderParam { kind: RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr().cast() },
            RenderParam { kind: RENDER_PARAM_SW_FORMAT, data: format.as_ptr() as *mut c_void },
            RenderParam { kind: RENDER_PARAM_SW_STRIDE, data: (&mut stride as *mut usize).cast() },
            RenderParam { kind: RENDER_PARAM_SW_POINTER, data: pixels.as_mut_ptr().cast() },
            RenderParam { kind: RENDER_PARAM_END, data: ptr::null_mut() },
        ];
        // SAFETY: the buffer holds `height` rows of `stride` bytes, checked
        // above, and every parameter points at live storage of its type.
        unsafe {
            (self.api.render_context_update)(self.render);
            (self.api.render_context_render)(self.render, params.as_mut_ptr()) >= 0
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: both were created in `new` and are freed exactly once, the
        // render context first as mpv requires.
        unsafe {
            (self.api.render_context_free)(self.render);
            (self.api.terminate_destroy)(self.handle);
        }
    }
}
