//! A word with the X server directly, where the windowing library has
//! no way of saying it: the `_NET_WM_STATE` of a window that is not yet
//! shown. Set on the window before it is mapped, the window manager puts
//! it up fullscreen in one go; asked for afterwards, as the library does,
//! the window is put up at a size of the manager's own, resized to the
//! work area and only then to the screen, which takes it tens of
//! milliseconds and holds up the first frame.

use std::ffi::{c_char, c_int, c_long, c_uchar, c_ulong, c_void};

use libloading::Library;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

type Display = *mut c_void;

/// Marks a still-hidden X11 window as fullscreen. Does nothing on another
/// windowing system, or where libX11 cannot be loaded.
pub fn mark_fullscreen(window: &Window) {
    let (Ok(display), Ok(handle)) = (window.display_handle(), window.window_handle()) else { return };
    let (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(handle)) = (display.as_raw(), handle.as_raw()) else { return };
    let Some(display) = display.display else { return };
    // SAFETY: loading libX11 runs only its ordinary library initialisers,
    // and each symbol is given the signature its C header declares.
    unsafe {
        let Ok(library) = Library::new("libX11.so.6") else { return };
        let Ok(intern) = library.get::<unsafe extern "C" fn(Display, *const c_char, c_int) -> c_ulong>(b"XInternAtom\0") else { return };
        let Ok(change) = library.get::<unsafe extern "C" fn(Display, c_ulong, c_ulong, c_ulong, c_int, c_int, *const c_uchar, c_int) -> c_int>(b"XChangeProperty\0") else { return };
        let Ok(flush) = library.get::<unsafe extern "C" fn(Display) -> c_int>(b"XFlush\0") else { return };
        let display = display.as_ptr();
        let state = intern(display, c"_NET_WM_STATE".as_ptr(), 0);
        let fullscreen: c_ulong = intern(display, c"_NET_WM_STATE_FULLSCREEN".as_ptr(), 0);
        // XA_ATOM is 4; 32-bit items are passed as C longs.
        let value: [c_long; 1] = [fullscreen as c_long];
        change(display, handle.window, state, 4, 32, 0, value.as_ptr().cast(), 1);
        flush(display);
    }
}
