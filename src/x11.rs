//! A word with the X server directly, where the windowing library has
//! no way of saying it.
//!
//! One is the `_NET_WM_STATE` of a window that is not yet shown. Set on
//! the window before it is mapped, the window manager puts it up
//! fullscreen in one go; asked for afterwards, as the library does, the
//! window is put up at a size of the manager's own, resized to the work
//! area and only then to the screen, which takes it tens of milliseconds
//! and holds up the first frame.
//!
//! The other is the window's first picture as its background: a pixmap
//! the server itself paints the window with the moment the window
//! manager shows it. A frame handed over the ordinary way before then is
//! thrown away, and the next one waits on the manager being done, which
//! on some managers is a good part of a tenth of a second.

use std::ffi::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};

use libloading::Library;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

type Display = *mut c_void;

/// The X11 display and window behind a window, when it is one.
fn xlib(window: &Window) -> Option<(Display, c_ulong)> {
    let (Ok(display), Ok(handle)) = (window.display_handle(), window.window_handle()) else { return None };
    let (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(handle)) = (display.as_raw(), handle.as_raw()) else { return None };
    Some((display.display?.as_ptr(), handle.window))
}

/// Marks a still-hidden X11 window as fullscreen. Does nothing on another
/// windowing system, or where libX11 cannot be loaded.
pub fn mark_fullscreen(window: &Window) {
    let Some((display, window)) = xlib(window) else { return };
    // SAFETY: loading libX11 runs only its ordinary library initializers,
    // and each symbol is given the signature its C header declares.
    unsafe {
        let Ok(library) = Library::new("libX11.so.6") else { return };
        let Ok(intern) = library.get::<unsafe extern "C" fn(Display, *const c_char, c_int) -> c_ulong>(b"XInternAtom\0") else { return };
        let Ok(change) = library.get::<unsafe extern "C" fn(Display, c_ulong, c_ulong, c_ulong, c_int, c_int, *const c_uchar, c_int) -> c_int>(b"XChangeProperty\0") else { return };
        let Ok(flush) = library.get::<unsafe extern "C" fn(Display) -> c_int>(b"XFlush\0") else { return };
        let state = intern(display, c"_NET_WM_STATE".as_ptr(), 0);
        let fullscreen: c_ulong = intern(display, c"_NET_WM_STATE_FULLSCREEN".as_ptr(), 0);
        // XA_ATOM is 4; 32-bit items are passed as C longs.
        let value: [c_long; 1] = [fullscreen as c_long];
        change(display, window, state, 4, 32, 0, value.as_ptr().cast(), 1);
        flush(display);
    }
}

/// The attributes of a window, as libX11 lays them out.
#[repr(C)]
struct WindowAttributes {
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    border_width: c_int,
    depth: c_int,
    visual: *mut c_void,
    root: c_ulong,
    class: c_int,
    bit_gravity: c_int,
    win_gravity: c_int,
    backing_store: c_int,
    backing_planes: c_ulong,
    backing_pixel: c_ulong,
    save_under: c_int,
    colormap: c_ulong,
    map_installed: c_int,
    map_state: c_int,
    all_event_masks: c_long,
    your_event_mask: c_long,
    do_not_propagate_mask: c_long,
    override_redirect: c_int,
    screen: *mut c_void,
}

/// What the shared memory extension is told of a segment.
#[repr(C)]
struct ShmSegmentInfo {
    shmseg: c_ulong,
    shmid: c_int,
    shmaddr: *mut c_char,
    read_only: c_int,
}

/// The libX11 and libXext calls a backdrop takes.
struct Api {
    _x11: Library,
    _xext: Library,
    get_attributes: unsafe extern "C" fn(Display, c_ulong, *mut WindowAttributes) -> c_int,
    create_pixmap: unsafe extern "C" fn(Display, c_ulong, c_uint, c_uint, c_uint) -> c_ulong,
    free_pixmap: unsafe extern "C" fn(Display, c_ulong) -> c_int,
    create_gc: unsafe extern "C" fn(Display, c_ulong, c_ulong, *mut c_void) -> *mut c_void,
    free_gc: unsafe extern "C" fn(Display, *mut c_void) -> c_int,
    set_background_pixmap: unsafe extern "C" fn(Display, c_ulong, c_ulong) -> c_int,
    clear_area: unsafe extern "C" fn(Display, c_ulong, c_int, c_int, c_uint, c_uint, c_int) -> c_int,
    flush: unsafe extern "C" fn(Display) -> c_int,
    shm_create_image: unsafe extern "C" fn(Display, *mut c_void, c_uint, c_int, *mut c_char, *mut ShmSegmentInfo, c_uint, c_uint) -> *mut c_void,
    shm_attach: unsafe extern "C" fn(Display, *mut ShmSegmentInfo) -> c_int,
    shm_detach: unsafe extern "C" fn(Display, *mut ShmSegmentInfo) -> c_int,
    shm_put_image: unsafe extern "C" fn(Display, c_ulong, *mut c_void, *mut c_void, c_int, c_int, c_int, c_int, c_uint, c_uint, c_int) -> c_int,
    destroy_image: unsafe extern "C" fn(*mut c_void) -> c_int,
}

impl Api {
    fn load() -> Option<Api> {
        // SAFETY: as in `mark_fullscreen`.
        unsafe {
            let x11 = Library::new("libX11.so.6").ok()?;
            let xext = Library::new("libXext.so.6").ok()?;
            macro_rules! symbol {
                ($library:expr, $name:literal) => {
                    *$library.get(concat!($name, "\0").as_bytes()).ok()?
                };
            }
            Some(Api {
                get_attributes: symbol!(x11, "XGetWindowAttributes"),
                create_pixmap: symbol!(x11, "XCreatePixmap"),
                free_pixmap: symbol!(x11, "XFreePixmap"),
                create_gc: symbol!(x11, "XCreateGC"),
                free_gc: symbol!(x11, "XFreeGC"),
                set_background_pixmap: symbol!(x11, "XSetWindowBackgroundPixmap"),
                clear_area: symbol!(x11, "XClearArea"),
                flush: symbol!(x11, "XFlush"),
                shm_create_image: symbol!(xext, "XShmCreateImage"),
                shm_attach: symbol!(xext, "XShmAttach"),
                shm_detach: symbol!(xext, "XShmDetach"),
                shm_put_image: symbol!(xext, "XShmPutImage"),
                destroy_image: symbol!(x11, "XDestroyImage"),
                _x11: x11,
                _xext: xext,
            })
        }
    }
}

/// A window's first picture, as its background: painted by the server
/// the moment the window is shown. Made, painted into, shown; and
/// dropped once a frame has reached the window the ordinary way, which
/// takes the picture back off the background, so that it is not painted
/// again on a resize.
pub struct Backdrop {
    api: Api,
    display: Display,
    window: c_ulong,
    width: u32,
    height: u32,
    pixmap: c_ulong,
    image: *mut c_void,
    /// Boxed: the image keeps a pointer to it, which `XShmPutImage` reads
    /// the segment through, so it must not move. Kept on the stack and
    /// moved in here, the image pointed at a stale copy, and the server
    /// was handed whatever was there as the segment: BadShmSeg, which
    /// winit then panics on at its next call that checks.
    info: Box<ShmSegmentInfo>,
    shown: bool,
}

impl Backdrop {
    /// A picture-to-be of `width` by `height` for an X11 window, in
    /// memory shared with the server. `None` on another windowing system,
    /// without the libraries, or when the server will not have it.
    pub fn new(window: &Window, width: u32, height: u32) -> Option<Backdrop> {
        let (display, window) = xlib(window)?;
        if width == 0 || height == 0 {
            return None;
        }
        let api = Api::load()?;
        let bytes = width as usize * height as usize * 4;
        // SAFETY: the calls are given valid handles and buffers of the
        // sizes they are told; the shared segment outlives its use by the
        // server, which is told of its end before it is unmapped.
        unsafe {
            let mut attributes = std::mem::zeroed::<WindowAttributes>();
            if (api.get_attributes)(display, window, &mut attributes) == 0 || attributes.depth != 24 {
                return None;
            }
            let shmid = libc::shmget(libc::IPC_PRIVATE, bytes, libc::IPC_CREAT | 0o600);
            if shmid < 0 {
                return None;
            }
            let shmaddr = libc::shmat(shmid, std::ptr::null(), 0);
            // Marked for removal now: it goes once both sides let go.
            libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut());
            if shmaddr as isize == -1 {
                return None;
            }
            let mut info = Box::new(ShmSegmentInfo { shmseg: 0, shmid, shmaddr: shmaddr.cast(), read_only: 1 });
            // ZPixmap is 2.
            let image = (api.shm_create_image)(display, attributes.visual, 24, 2, shmaddr.cast(), &mut *info, width, height);
            if image.is_null() {
                libc::shmdt(shmaddr);
                return None;
            }
            if (api.shm_attach)(display, &mut *info) == 0 {
                (api.destroy_image)(image);
                libc::shmdt(shmaddr);
                return None;
            }
            let pixmap = (api.create_pixmap)(display, window, width, height, 24);
            Some(Backdrop { api, display, window, width, height, pixmap, image, info, shown: false })
        }
    }

    /// The picture, `0RGB` a pixel, to be painted before `show`.
    pub fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: the segment is `width * height` pixels, mapped until drop.
        unsafe { std::slice::from_raw_parts_mut(self.info.shmaddr.cast::<u32>(), self.width as usize * self.height as usize) }
    }

    /// Hands the picture to the server as the window's background: painted
    /// at once if the window is already up, at its showing otherwise. No
    /// waiting on the server, which is busy with the picture from here.
    pub fn show(&mut self) {
        // SAFETY: as in `new`; the server lets the segment go once it has
        // taken the picture, which it does in order.
        unsafe {
            let gc = (self.api.create_gc)(self.display, self.pixmap, 0, std::ptr::null_mut());
            (self.api.shm_put_image)(self.display, self.pixmap, gc, self.image, 0, 0, 0, 0, self.width, self.height, 0);
            (self.api.set_background_pixmap)(self.display, self.window, self.pixmap);
            (self.api.clear_area)(self.display, self.window, 0, 0, 0, 0, 0);
            (self.api.shm_detach)(self.display, &mut *self.info);
            (self.api.free_gc)(self.display, gc);
            (self.api.flush)(self.display);
        }
        self.shown = true;
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        // SAFETY: the handles are the ones `new` made, freed once.
        unsafe {
            if !self.shown {
                (self.api.shm_detach)(self.display, &mut *self.info);
            }
            // No background pixmap: `None` is 0.
            (self.api.set_background_pixmap)(self.display, self.window, 0);
            (self.api.free_pixmap)(self.display, self.pixmap);
            (self.api.destroy_image)(self.image);
            (self.api.flush)(self.display);
            libc::shmdt(self.info.shmaddr.cast());
        }
    }
}

/// A window inside jog's that the player draws a video into on the GPU,
/// as mpv draws into a window of its own: placed over where the video
/// goes, shown while it plays. It asks for no input, so the pointer and
/// the keys go on to jog's window around it; nor does the window mpv
/// makes inside it, told not to.
pub struct VideoWindow {
    _x11: Library,
    display: Display,
    id: c_ulong,
    map: unsafe extern "C" fn(Display, c_ulong) -> c_int,
    unmap: unsafe extern "C" fn(Display, c_ulong) -> c_int,
    move_resize: unsafe extern "C" fn(Display, c_ulong, c_int, c_int, c_uint, c_uint) -> c_int,
    raise: unsafe extern "C" fn(Display, c_ulong) -> c_int,
    destroy: unsafe extern "C" fn(Display, c_ulong) -> c_int,
    flush: unsafe extern "C" fn(Display) -> c_int,
    place: [i32; 4],
    shown: bool,
    /// When it was last put up: mpv draws into it a moment after.
    shown_at: Option<std::time::Instant>,
}

impl VideoWindow {
    /// One inside `window`, hidden, with no background of its own: put up,
    /// it leaves what was under it until mpv draws, rather than flash
    /// black first. `None` on another windowing
    /// system, or without libX11.
    pub fn new(window: &Window) -> Option<VideoWindow> {
        let (display, parent) = xlib(window)?;
        // SAFETY: as in `mark_fullscreen`; the window made is a child of a
        // live window, on the display it belongs to.
        unsafe {
            let x11 = Library::new("libX11.so.6").ok()?;
            macro_rules! symbol {
                ($name:literal) => {
                    *x11.get(concat!($name, "\0").as_bytes()).ok()?
                };
            }
            let create: unsafe extern "C" fn(Display, c_ulong, c_int, c_int, c_uint, c_uint, c_uint, c_ulong, c_ulong) -> c_ulong = symbol!("XCreateSimpleWindow");
            let flush: unsafe extern "C" fn(Display) -> c_int = symbol!("XFlush");
            let background: unsafe extern "C" fn(Display, c_ulong, c_ulong) -> c_int = symbol!("XSetWindowBackgroundPixmap");
            let id = create(display, parent, 0, 0, 1, 1, 0, 0, 0);
            if id == 0 {
                return None;
            }
            // No background: `None` is 0.
            background(display, id, 0);
            flush(display);
            Some(VideoWindow {
                map: symbol!("XMapWindow"),
                unmap: symbol!("XUnmapWindow"),
                move_resize: symbol!("XMoveResizeWindow"),
                raise: symbol!("XRaiseWindow"),
                destroy: symbol!("XDestroyWindow"),
                flush,
                display,
                id,
                place: [0, 0, 1, 1],
                shown: false,
                shown_at: None,
                _x11: x11,
            })
        }
    }

    /// Its X11 id, for the player's `wid`.
    pub fn id(&self) -> u64 {
        u64::from(self.id)
    }

    /// Shown over `place` (left, top, width, height in the window), or
    /// hidden; the server told only of what changed.
    pub fn show(&mut self, place: Option<[i32; 4]>) {
        // SAFETY: the window is this one's own, alive until drop.
        unsafe {
            match place.filter(|[_, _, w, h]| *w > 0 && *h > 0) {
                Some(place) => {
                    if place != self.place {
                        (self.move_resize)(self.display, self.id, place[0], place[1], place[2] as c_uint, place[3] as c_uint);
                        self.place = place;
                    }
                    if !self.shown {
                        (self.map)(self.display, self.id);
                        (self.raise)(self.display, self.id);
                        self.shown = true;
                        self.shown_at = Some(std::time::Instant::now());
                    }
                }
                None if self.shown => {
                    (self.unmap)(self.display, self.id);
                    self.shown = false;
                }
                None => return,
            }
            (self.flush)(self.display);
        }
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// How long it has been up, if it is.
    pub fn up_for(&self) -> Option<std::time::Duration> {
        self.shown_at.filter(|_| self.shown).map(|at| at.elapsed())
    }
}

impl Drop for VideoWindow {
    fn drop(&mut self) {
        // SAFETY: the window made in `new`, destroyed once.
        unsafe {
            (self.destroy)(self.display, self.id);
            (self.flush)(self.display);
        }
    }
}

/// Whether the pointer is over `window`, as the X server has it now: a
/// window inside it, as a video's is, is still over it. `None` where that
/// cannot be asked.
pub fn pointer_over(window: &Window) -> Option<bool> {
    let (display, id) = xlib(window)?;
    // SAFETY: as in `mark_fullscreen`; every pointer the call writes to
    // is to a local of the type it writes.
    unsafe {
        let x11 = Library::new("libX11.so.6").ok()?;
        type Query = unsafe extern "C" fn(Display, c_ulong, *mut c_ulong, *mut c_ulong, *mut c_int, *mut c_int, *mut c_int, *mut c_int, *mut c_uint) -> c_int;
        let query: Query = *x11.get(b"XQueryPointer\0").ok()?;
        let (mut root, mut child, mut root_x, mut root_y, mut x, mut y, mut mask) = (0, 0, 0, 0, 0, 0, 0);
        if query(display, id, &mut root, &mut child, &mut root_x, &mut root_y, &mut x, &mut y, &mut mask) == 0 {
            return Some(false);
        }
        let size = window.inner_size();
        Some(x >= 0 && y >= 0 && (x as u32) < size.width && (y as u32) < size.height)
    }
}
