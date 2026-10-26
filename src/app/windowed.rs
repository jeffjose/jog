//! The window out of fullscreen: the size it was last given by hand while
//! not filling the screen, and where it was, kept for the next run; or else
//! a quarter of the screen, in its middle. Leaving fullscreen goes to it,
//! and so does the seek bar's small window button, rather than to a window
//! as large as a 4K video.

use winit::dpi::PhysicalPosition;
use winit::monitor::MonitorHandle;

use super::*;

/// Where the size and place are kept between runs, as `1280x720+40+60`.
fn path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("window-size"))
}

/// What is kept: the size given by hand, and where the window's top left
/// was; either may not be known.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct Kept {
    size: Option<(u32, u32)>,
    at: Option<(i32, i32)>,
}

fn read() -> Kept {
    path().and_then(|path| std::fs::read_to_string(path).ok()).map_or(Kept::default(), |text| parse(text.trim()))
}

/// `1280x720+40+60`, `1280x720`, or `+40+60`, as X11 writes a geometry: a
/// place left of or above the first monitor is negative, `-1880+0`.
fn parse(text: &str) -> Kept {
    let (size, at) = text.split_at(text.find(['+', '-']).unwrap_or(text.len()));
    let size = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
    // The second number starts at the sign after the first's.
    let at = at.get(1..).and_then(|rest| rest.find(['+', '-'])).and_then(|second| {
        let (x, y) = at.split_at(second + 1);
        Some((x.parse().ok()?, y.parse().ok()?))
    });
    Kept { size, at }
}

fn written(kept: Kept) -> String {
    let size = kept.size.map(|(w, h)| format!("{w}x{h}")).unwrap_or_default();
    let at = kept.at.map(|(x, y)| format!("{x:+}{y:+}")).unwrap_or_default();
    format!("{size}{at}\n")
}

/// Whether a window of `size` with its top left at `at` is where it can be
/// taken hold of: the middle of its top edge on one of the monitors.
fn on_a_monitor(at: (i32, i32), size: (u32, u32), monitors: impl IntoIterator<Item = MonitorHandle>) -> bool {
    let grip = (at.0 + (size.0 / 2) as i32, at.1 + 16);
    monitors.into_iter().any(|monitor| {
        let (origin, extent) = (monitor.position(), monitor.size());
        (origin.x..origin.x + extent.width as i32).contains(&grip.0) && (origin.y..origin.y + extent.height as i32).contains(&grip.1)
    })
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

    /// Whether the window is an ordinary one: not filling the screen.
    fn ordinary_window(&self) -> bool {
        self.window.as_ref().is_some_and(|window| window.fullscreen().is_none() && !window.is_maximized())
    }

    /// The window was given a size: kept as the one to come back to, when
    /// it is an ordinary window, not filling the screen.
    pub(super) fn note_window_size(&mut self, size: PhysicalSize<u32>) {
        // Not one jog gave it, as it starts or goes small: only the user's.
        let here = self.sized_here == Some((size.width, size.height));
        if here || !self.ordinary_window() || !self.small_beside_screen((size.width, size.height)) {
            return;
        }
        self.windowed = Some((size.width, size.height));
    }

    /// The window was moved: where an ordinary one is, kept to open there.
    pub(super) fn note_window_moved(&mut self, at: PhysicalPosition<i32>) {
        if self.ordinary_window() && self.small_beside_screen(self.window_size()) {
            self.windowed_at = Some((at.x, at.y));
        }
    }

    /// The size of the small window: the one last kept, or a quarter of
    /// the screen, half as wide and half as tall.
    pub(super) fn small_size(&self) -> (u32, u32) {
        let (sw, sh) = self.monitor;
        self.windowed.or_else(|| read().size).filter(|size| self.small_beside_screen(*size)).unwrap_or((sw / 2, sh / 2))
    }

    /// Where the small window goes: where it was last, while that is on one
    /// of `monitors` still.
    pub(super) fn small_at(&self, size: (u32, u32), monitors: impl IntoIterator<Item = MonitorHandle>) -> Option<PhysicalPosition<i32>> {
        let at = self.windowed_at.or_else(|| read().at)?;
        on_a_monitor(at, size, monitors).then(|| PhysicalPosition::new(at.0, at.1))
    }

    /// Out of fullscreen or a maximized window, to the small one: where it
    /// was last, or in the middle of the screen.
    pub(super) fn go_small(&mut self) {
        let size = self.small_size();
        let Some(window) = &self.window else { return };
        if window.fullscreen().is_some() {
            window.set_fullscreen(None);
        }
        window.set_maximized(false);
        let (sw, sh) = self.monitor;
        let middle = window.current_monitor().map(|monitor| {
            let origin = monitor.position();
            PhysicalPosition::new(origin.x + (sw as i32 - size.0 as i32) / 2, origin.y + (sh as i32 - size.1 as i32) / 2)
        });
        if let Some(at) = self.small_at(size, window.available_monitors()).or(middle) {
            window.set_outer_position(at);
        }
        // Jog's own size, not one to keep as the user's: said so before
        // asking, as on Wayland the size is had at once. A window manager
        // putting back the size from before fullscreen does so after this,
        // and it is asked for again once it has: wanted still after the
        // size had at once.
        self.sized_here = Some(size);
        self.ask_size(size);
        self.small_wanted = Some(size);
    }

    /// The window has a new size: told of by the display, or given at once
    /// as asked for, which on Wayland is not told of again.
    pub(super) fn resized(&mut self, size: PhysicalSize<u32>) {
        self.size.set((size.width, size.height));
        if self.options.debug {
            let scale = self.window.as_ref().map_or(1.0, |window| window.scale_factor());
            eprintln!("jog: resize {}x{} x{scale}", size.width, size.height);
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

    /// Asks for the window to be this size: when it is so at once, as on
    /// Wayland, taken as told of.
    pub(super) fn ask_size(&mut self, size: (u32, u32)) {
        let given = self.window.as_ref().and_then(|window| window.request_inner_size(PhysicalSize::new(size.0, size.1)));
        if let Some(size) = given {
            self.resized(size);
        }
    }

    /// Asked for the small size again, once, after the window manager
    /// has had its say about it.
    pub(super) fn settle_small(&mut self, size: PhysicalSize<u32>) {
        let Some(wanted) = self.small_wanted.take() else { return };
        if (size.width, size.height) != wanted {
            self.ask_size(wanted);
        }
    }

    /// Keeps the small size and where the window was, for the next run.
    pub(super) fn save_window_size(&self) {
        let Some(file) = path() else { return };
        let before = read();
        let kept = Kept { size: self.windowed.or(before.size), at: self.windowed_at.or(before.at) };
        if kept != before {
            let _ = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, written(kept)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_and_place_kept_read_back_as_written() {
        let both = Kept { size: Some((1280, 720)), at: Some((40, 60)) };
        assert_eq!(written(both), "1280x720+40+60\n");
        for kept in [both, Kept { size: Some((703, 507)), at: None }, Kept { size: None, at: Some((-1880, 0)) }, Kept { size: Some((800, 450)), at: Some((3000, -20)) }] {
            assert_eq!(parse(written(kept).trim()), kept);
        }
        // As kept before there was a place to keep.
        assert_eq!(parse("703x507"), Kept { size: Some((703, 507)), at: None });
        assert_eq!(parse("nonsense"), Kept::default());
    }
}
