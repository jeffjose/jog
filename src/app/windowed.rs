//! The window out of fullscreen: the size it was last given by hand while
//! not filling the screen, kept for the next run, or else a quarter of the
//! screen. Leaving fullscreen goes to it, and so does the seek bar's small
//! window button, rather than to a window as large as a 4K video.

use winit::dpi::PhysicalPosition;

use super::*;

/// Where the size is kept between runs, as `1280x720`.
fn path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("window-size"))
}

fn read() -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(path()?).ok()?;
    let (w, h) = text.trim().split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Parts of the screen a window may cover and still count as a small one:
/// larger, and it is as good as filling it.
const MOST: f64 = 0.6;

impl App {
    /// Whether a window of `size` is small beside the screen.
    fn small_beside_screen(&self, (w, h): (u32, u32)) -> bool {
        let (sw, sh) = self.monitor;
        sw > 0 && sh > 0 && w > 0 && h > 0 && f64::from(w) * f64::from(h) <= f64::from(sw) * f64::from(sh) * MOST
    }

    /// The window was given a size: kept as the one to come back to, when
    /// it is an ordinary window, not filling the screen.
    pub(super) fn note_window_size(&mut self, size: PhysicalSize<u32>) {
        let Some(window) = &self.window else { return };
        // Not one jog gave it, as it starts or goes small: only the user's.
        let here = self.sized_here == Some((size.width, size.height));
        if here || window.fullscreen().is_some() || window.is_maximized() || !self.small_beside_screen((size.width, size.height)) {
            return;
        }
        self.windowed = Some((size.width, size.height));
    }

    /// The size of the small window: the one last kept, or a quarter of
    /// the screen, half as wide and half as tall.
    pub(super) fn small_size(&self) -> (u32, u32) {
        let (sw, sh) = self.monitor;
        self.windowed.or_else(read).filter(|size| self.small_beside_screen(*size)).unwrap_or((sw / 2, sh / 2))
    }

    /// Out of fullscreen or a maximized window, to the small one, in the
    /// middle of the screen.
    pub(super) fn go_small(&mut self) {
        let size = self.small_size();
        let Some(window) = &self.window else { return };
        if window.fullscreen().is_some() {
            window.set_fullscreen(None);
        }
        window.set_maximized(false);
        let _ = window.request_inner_size(PhysicalSize::new(size.0, size.1));
        let (sw, sh) = self.monitor;
        if let Some(monitor) = window.current_monitor() {
            let origin = monitor.position();
            let at = PhysicalPosition::new(origin.x + (sw as i32 - size.0 as i32) / 2, origin.y + (sh as i32 - size.1 as i32) / 2);
            window.set_outer_position(at);
        }
        // A window manager putting back the size from before fullscreen
        // does so after this; asked again once it has.
        self.small_wanted = Some(size);
        self.sized_here = Some(size);
    }

    /// Asked for the small size again, once, after the window manager
    /// has had its say about it.
    pub(super) fn settle_small(&mut self, size: PhysicalSize<u32>) {
        let Some(wanted) = self.small_wanted.take() else { return };
        if (size.width, size.height) != wanted {
            if let Some(window) = &self.window {
                let _ = window.request_inner_size(PhysicalSize::new(wanted.0, wanted.1));
            }
        }
    }

    /// Keeps the small size for the next run.
    pub(super) fn save_window_size(&self) {
        let (Some((w, h)), Some(file)) = (self.windowed, path()) else { return };
        if read() != Some((w, h)) {
            let _ = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, format!("{w}x{h}\n")));
        }
    }
}
