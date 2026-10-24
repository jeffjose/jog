//! Panels made wider or narrower, the strip taller or shorter, by dragging
//! the edge they turn to the picture: the file list's and the inspector's
//! inner side, the top of the strip. The pointer turns to a resize arrow
//! over one. The sizes are kept with the pins for the next run.

use std::sync::atomic::AtomicU32;

use winit::window::CursorIcon;

use super::*;

/// How much taller than usual the strip along the bottom is, in
/// thousandths: read wherever the strip is laid out.
pub(super) static STRIP_SCALE: AtomicU32 = AtomicU32::new(1000);

/// The strip's height scale.
pub(super) fn strip_scale() -> f64 {
    f64::from(STRIP_SCALE.load(atomic::Ordering::Relaxed)) / 1000.0
}

/// The usual width of a panel down the side, in lines of text.
pub(super) const PANEL_UNITS: f64 = 30.0;

/// An edge that can be dragged.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PanelEdge {
    List,
    Inspector,
    Strip,
}

impl App {
    /// How near an edge the pointer has to be to take hold of it.
    fn grip(&self) -> i32 {
        (ui_px(self.window_size().1) as i32 / 3).max(4)
    }

    /// The edge under the pointer, if any.
    pub(super) fn edge_at(&self, x: f64, y: f64) -> Option<PanelEdge> {
        if self.drawing_popout.is_some() {
            return None;
        }
        let (x, y, grip) = (x as i32, y as i32, self.grip());
        if let Some(panel) = self.list_panel().filter(|panel| !panel.embedded) {
            let edge = if self.list_right { panel.left } else { panel.left + panel.width };
            if (x - edge).abs() <= grip && y < panel.bottom {
                return Some(PanelEdge::List);
            }
        }
        if let Some([left, top, width, height]) = self.inspector.as_ref().filter(|_| !self.popped(Popped::Inspector)).map(|inspector| inspector.place()) {
            // Along the bottom, its top edge; down the side, the one by the picture.
            if self.inspector_bottom {
                if (y - top).abs() <= grip && (left..left + width).contains(&x) {
                    return Some(PanelEdge::Inspector);
                }
            } else {
                let edge = if self.list_right { left + width } else { left };
                if (x - edge).abs() <= grip && (top..top + height).contains(&y) {
                    return Some(PanelEdge::Inspector);
                }
            }
        }
        if !self.strip_buttons().is_empty() {
            let top = Strip::for_height(self.window_size().1).top;
            if (y - top).abs() <= grip && self.strip_button_at(f64::from(x), f64::from(y)).is_none() {
                return Some(PanelEdge::Strip);
            }
        }
        None
    }

    /// Takes hold of the edge under the pointer, if there is one.
    pub(super) fn grab_edge(&mut self, x: f64, y: f64) -> bool {
        let Some(edge) = self.edge_at(x, y) else { return false };
        self.drag = Some(Drag::Resize(edge));
        true
    }

    /// The edge held follows the pointer.
    pub(super) fn drag_edge_to(&mut self, edge: PanelEdge, x: f64, y: f64) {
        let (width, height) = self.window_size();
        let px = f64::from(ui_px(height));
        // From the window's edge the panel stands at, in lines of text; no
        // narrower than reads well, no wider than half the window.
        let most = f64::from(width) / 2.0 / px;
        let units = |from_left: bool| {
            let across = if from_left { x } else { f64::from(width) - x };
            (across / px).clamp(18.0, most.max(18.0))
        };
        match edge {
            PanelEdge::List => self.list_units = units(!self.list_right),
            // The inspector stands on the side the list is not.
            PanelEdge::Inspector if self.inspector_bottom => self.inspector_rows = ((f64::from(height) - y) / px).clamp(12.0, f64::from(height) * 2.0 / 3.0 / px),
            PanelEdge::Inspector => self.inspector_units = units(self.list_right),
            PanelEdge::Strip => {
                let usual = f64::from(Strip::usual_height(height));
                let scale = ((f64::from(height) - y) / usual).clamp(0.6, 2.5);
                STRIP_SCALE.store((scale * 1000.0) as u32, atomic::Ordering::Relaxed);
            }
        }
        self.apply_video_view();
        self.redraw();
    }

    /// Lets go of an edge, and keeps the size it was left at.
    pub(super) fn let_go_of_edge(&mut self) {
        self.drag = None;
        self.save_panels();
    }

    /// The pointer as a resize arrow over an edge, and as itself elsewhere.
    pub(super) fn edge_cursor(&mut self, x: f64, y: f64) {
        let edge = match self.drag {
            Some(Drag::Resize(edge)) => Some(edge),
            _ => self.edge_at(x, y),
        };
        if edge == self.edge_under {
            return;
        }
        self.edge_under = edge;
        let icon = match edge {
            Some(PanelEdge::Strip) => CursorIcon::NsResize,
            Some(PanelEdge::Inspector) if self.inspector_bottom => CursorIcon::NsResize,
            Some(_) => CursorIcon::EwResize,
            None => CursorIcon::Default,
        };
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }
}

impl App {
    /// How far a dialog has been moved from the middle of the window.
    pub(super) fn dialog_offset(&self, which: &'static str) -> (i32, i32) {
        self.dialog_moved.borrow().get(which).copied().unwrap_or((0, 0))
    }

    /// Where a dialog of `size` stands in a window of `window`: in the
    /// middle, moved as it was dragged, and kept inside. Notes its heading,
    /// `header` high, as the part to take hold of.
    pub(super) fn place_dialog(&self, which: &'static str, window: (i32, i32), size: (i32, i32), header: i32) -> (i32, i32) {
        let (dx, dy) = self.dialog_offset(which);
        let left = ((window.0 - size.0) / 2 + dx).clamp(0, (window.0 - size.0).max(0));
        let top = ((window.1 - size.1) / 2 + dy).clamp(0, (window.1 - size.1).max(0));
        self.dialog_header.set(Some((which, [left, top, size.0, header])));
        (left, top)
    }

    /// Takes hold of a dialog's heading, away from its buttons.
    pub(super) fn grab_dialog(&mut self, x: f64, y: f64) -> bool {
        let Some((which, [left, top, width, height])) = self.dialog_header.get() else { return false };
        let (xi, yi) = (x as i32, y as i32);
        if !((left..left + width).contains(&xi) && (top..top + height).contains(&yi)) || self.on_dialog_button(xi, yi) {
            return false;
        }
        self.drag = Some(Drag::Move { which, from: (x, y), start: self.dialog_offset(which) });
        true
    }

    /// Whether a point is on one of the open dialog's own buttons.
    fn on_dialog_button(&self, x: i32, y: i32) -> bool {
        let on = |[left, top, w, h]: [i32; 4]| (left..left + w).contains(&x) && (top..top + h).contains(&y);
        self.convert_panel.as_ref().is_some_and(|panel| panel.hits().iter().any(|place| on(*place)))
            || self.video_panel.as_ref().is_some_and(|panel| panel.hits().iter().any(|place| on(*place)))
            || self.track_editor.as_ref().is_some_and(|editor| editor.hits().iter().any(|place| on(*place)))
    }

    /// The dialog held follows the pointer.
    pub(super) fn move_dialog(&mut self, which: &'static str, from: (f64, f64), start: (i32, i32), x: f64, y: f64) {
        let offset = (start.0 + (x - from.0) as i32, start.1 + (y - from.1) as i32);
        self.dialog_moved.borrow_mut().insert(which, offset);
        self.redraw();
    }
}
