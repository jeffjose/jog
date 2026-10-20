//! What the panels have in common: the file list and the inspector down
//! the side, the filmstrip or the scenes along the bottom. Each lies over
//! the picture, or, pinned, stands beside or below it and the picture
//! makes room. Which are pinned is remembered from one run to the next.

use super::*;

/// The round buttons on the strip along the bottom.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum StripButton {
    Pin,
    Close,
}

/// Where the pins are kept between runs.
const FILE: &str = "panels";

/// The pins kept from the last run, as (file list, inspector, strip),
/// each where it was said.
pub(super) fn load() -> (Option<bool>, Option<bool>, Option<bool>) {
    let text = crate::positions::state_dir().and_then(|dir| std::fs::read_to_string(dir.join(FILE)).ok()).unwrap_or_default();
    let get = |name: &str| text.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('=')).map(|value| value.trim() == "pinned");
    (get("list"), get("inspector"), get("strip"))
}

impl App {
    /// Whether the strip along the bottom stands below the picture, which
    /// makes room for it: pinned, and up for good rather than for the
    /// pointer passing.
    pub(super) fn band_docked(&self) -> bool {
        self.strip_docked && self.grid.is_none() && (self.strip_pinned || self.triage_on || self.scenes_shown())
    }

    /// A click on one of the strip's buttons.
    pub(super) fn strip_button(&mut self, which: StripButton) {
        match which {
            StripButton::Close if self.scenes_shown() => self.scenes_on = false,
            StripButton::Close => (self.strip_pinned, self.strip_hover, self.strip_over) = (false, false, None),
            StripButton::Pin => {
                self.strip_docked = !self.strip_docked;
                // A filmstrip up for the pointer stays up once pinned.
                if self.strip_docked && !self.scenes_shown() {
                    self.strip_pinned = true;
                }
                self.save_panels();
            }
        }
        self.apply_video_view();
        self.redraw();
    }

    /// Keeps which panels are pinned for the next run, on another thread.
    pub(super) fn save_panels(&self) {
        if self.snapshot.is_some() || self.bench.is_some() {
            return;
        }
        let word = |pinned: bool| if pinned { "pinned" } else { "over" };
        let text = format!("list={}\ninspector={}\nstrip={}\n", word(self.list_pinned), word(self.inspector_pinned), word(self.strip_docked));
        std::thread::spawn(move || {
            if let Some(dir) = crate::positions::state_dir() {
                let _ = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(FILE), text));
            }
        });
    }
}
