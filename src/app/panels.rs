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

/// What was kept of the panels from the last run, each where it was said.
#[derive(Default)]
pub(super) struct Saved {
    pub list: Option<bool>,
    pub inspector: Option<bool>,
    pub strip: Option<bool>,
    /// Widths, in lines of text.
    pub list_width: Option<f64>,
    pub inspector_width: Option<f64>,
    /// The panels out in windows of their own, and where each one was.
    pub out: std::collections::HashSet<super::Popped>,
    pub at: std::collections::HashMap<super::Popped, [i32; 4]>,
    /// The tab Inspect was last on.
    pub pane: Option<super::Pane>,
    /// Inspect along the bottom, and how tall, in lines of text.
    pub bottom: bool,
    pub rows: Option<f64>,
}

/// The panels as last left. The strip's height scale is set as it is read.
pub(super) fn load() -> Saved {
    let text = crate::positions::state_dir().and_then(|dir| std::fs::read_to_string(dir.join(FILE)).ok()).unwrap_or_default();
    let value = |name: &str| text.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('=')).map(str::trim);
    let get = |name: &str| value(name).map(|value| value == "pinned");
    let number = |name: &str| value(name).and_then(|value| value.parse::<f64>().ok()).filter(|n| n.is_finite() && *n > 0.0);
    if let Some(scale) = number("strip-height") {
        super::resize::STRIP_SCALE.store((scale.clamp(0.6, 2.5) * 1000.0) as u32, atomic::Ordering::Relaxed);
    }
    let place = |name: &str| {
        value(name).and_then(|value| {
            let parts: Vec<i32> = value.split(',').filter_map(|part| part.trim().parse().ok()).collect();
            (parts.len() == 4 && parts[2] > 100 && parts[3] > 100).then(|| [parts[0], parts[1], parts[2], parts[3]])
        })
    };
    let out = super::Popped::ALL.into_iter().filter(|what| value(&format!("{}-window", what.name())) == Some("out")).collect();
    let at = super::Popped::ALL.into_iter().filter_map(|what| Some((what, place(&format!("{}-window-at", what.name()))?))).collect();
    Saved {
        list: get("list"),
        inspector: get("inspector"),
        strip: get("strip"),
        list_width: number("list-width"),
        inspector_width: number("inspector-width"),
        out,
        at,
        pane: value("inspect-tab").and_then(super::Pane::from_name),
        bottom: value("inspect-dock") == Some("bottom"),
        rows: number("inspect-height"),
    }
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
        if let Some(text) = self.panels_text() {
            std::thread::spawn(move || write_panels(&text));
        }
    }

    /// As `save_panels`, before quitting, which would not wait for a thread.
    pub(super) fn save_panels_now(&self) {
        if let Some(text) = self.panels_text() {
            write_panels(&text);
        }
    }

    fn panels_text(&self) -> Option<String> {
        if self.snapshot.is_some() || self.bench.is_some() {
            return None;
        }
        let word = |pinned: bool| if pinned { "pinned" } else { "over" };
        let text = format!(
            "list={}\ninspector={}\nstrip={}\nlist-width={:.1}\ninspector-width={:.1}\nstrip-height={:.2}\ninspect-tab={}\ninspect-dock={}\ninspect-height={:.1}\n{}",
            word(self.list_pinned),
            word(self.inspector_pinned),
            word(self.strip_docked),
            self.list_units,
            self.inspector_units,
            super::resize::strip_scale(),
            self.inspect_pane.name(),
            if self.inspector_bottom { "bottom" } else { "side" },
            self.inspector_rows,
            super::Popped::ALL
                .into_iter()
                .map(|what| {
                    let at = self.popout_at.get(&what).map(|[x, y, w, h]| format!("{}-window-at={x},{y},{w},{h}\n", what.name())).unwrap_or_default();
                    format!("{}-window={}\n{at}", what.name(), if self.out.contains(&what) { "out" } else { "in" })
                })
                .collect::<String>()
        );
        Some(text)
    }
}

fn write_panels(text: &str) {
    if let Some(dir) = crate::positions::state_dir() {
        let _ = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(FILE), text));
    }
}
