//! Panels popped out into windows of their own, as a browser's developer
//! tools can be: moved to another screen, sized as wanted, while the
//! picture has all of the main window. Closing one's window puts the panel
//! back where it was. The inspector and the file list go out so.
//!
//! While a popped-out window is drawn or answered, `drawing_popout` says
//! which: each panel's own code then lays itself out across that window,
//! and reads the pointer where it always does, set to that window's.

use softbuffer::{Context, Surface};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::{Window, WindowId};

use super::*;

/// What can be popped out.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Popped {
    Inspector,
    List,
}

impl Popped {
    pub const ALL: [Popped; 2] = [Popped::Inspector, Popped::List];

    /// Its name in the panels file.
    pub fn name(self) -> &'static str {
        match self {
            Popped::Inspector => "inspector",
            Popped::List => "list",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Popped::Inspector => "Inspect · jog",
            Popped::List => "Files · jog",
        }
    }
}

/// A panel in a window of its own.
pub struct Popout {
    pub what: Popped,
    pub window: Arc<Window>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    /// The pointer, in this window.
    pub mouse: (f64, f64),
}

impl App {
    /// Whether a panel is out in a window of its own.
    pub(super) fn popped(&self, what: Popped) -> bool {
        self.popouts.iter().any(|popout| popout.what == what)
    }

    /// The size of the popped-out window being drawn or answered.
    pub(super) fn popout_size(&self) -> Option<(u32, u32)> {
        let what = self.drawing_popout?;
        let size = self.popouts.iter().find(|popout| popout.what == what)?.window.inner_size();
        Some((size.width.max(1), size.height.max(1)))
    }

    /// Pops a panel out, or puts it back; remembered for next time.
    pub(super) fn toggle_popout(&mut self, what: Popped) {
        if self.popped(what) {
            self.out.remove(&what);
            self.close_popout(what);
        } else {
            self.out.insert(what);
            // A window is made where the event loop is to hand, on its
            // next turn.
            self.popout_wanted = Some(what);
            self.redraw();
        }
        self.save_panels();
    }

    /// Opens a panel out if that is where it was last: when it is opened.
    pub(super) fn reopen_out(&mut self, what: Popped) {
        if self.out.contains(&what) && !self.popped(what) {
            self.popout_wanted = Some(what);
        }
    }

    /// Opens the window asked for, if any, and closes those whose panel
    /// has been closed. Each turn of the event loop.
    pub(super) fn tend_popouts(&mut self, event_loop: &ActiveEventLoop) {
        if self.inspector.is_none() && self.popped(Popped::Inspector) {
            self.close_popout(Popped::Inspector);
        }
        if !self.show_list && self.popped(Popped::List) {
            self.close_popout(Popped::List);
        }
        let Some(what) = self.popout_wanted.take() else { return };
        if self.popped(what) {
            return;
        }
        let (width, height) = self.window_size();
        let px = ui_px(height);
        let units = if what == Popped::List { self.list_units } else { self.inspector_units };
        let size = PhysicalSize::new((panel_width(width as i32 * 2, px, units).max(420)) as u32, (height * 6 / 7).max(480));
        // A scripted run shows no window: its snapshot draws this one too.
        let mut attributes = Window::default_attributes().with_title(what.title()).with_inner_size(size).with_visible(self.snapshot.is_none());
        // Where it was last, on whichever screen that was.
        if let Some([x, y, w, h]) = self.popout_at.get(&what).copied() {
            attributes = attributes.with_position(PhysicalPosition::new(x, y)).with_inner_size(PhysicalSize::new(w as u32, h as u32));
        }
        let Ok(window) = event_loop.create_window(attributes) else { return self.toast("Not popped out · no window") };
        let window = Arc::new(window);
        let surface = Context::new(window.clone()).and_then(|context| Surface::new(&context, window.clone()));
        let Ok(surface) = surface else { return self.toast("Not popped out · no surface") };
        self.popouts.push(Popout { what, window, surface, mouse: (-1.0, -1.0) });
        self.apply_video_view();
        self.redraw();
    }

    /// Puts a panel back in the main window.
    pub(super) fn close_popout(&mut self, what: Popped) {
        self.popouts.retain(|popout| popout.what != what);
        self.apply_video_view();
        self.redraw();
    }

    /// Asks each popped-out window to be drawn again, with the main one.
    pub(super) fn redraw_popouts(&self) {
        for popout in &self.popouts {
            popout.window.request_redraw();
        }
    }

    /// Runs `then` as for a popped-out window: the panel laid out across
    /// it, and the pointer as it is there. Its pointer is kept as `then`
    /// leaves it.
    fn with_popout<T>(&mut self, what: Popped, then: impl FnOnce(&mut App) -> T) -> T {
        let Some(at) = self.popouts.iter().position(|popout| popout.what == what) else { return then(self) };
        let main = std::mem::replace(&mut self.mouse, self.popouts[at].mouse);
        self.drawing_popout = Some(what);
        let result = then(self);
        self.drawing_popout = None;
        let mouse = std::mem::replace(&mut self.mouse, main);
        if let Some(popout) = self.popouts.iter_mut().find(|popout| popout.what == what) {
            popout.mouse = mouse;
        }
        result
    }

    /// Something happening to a popped-out window. Whether it was one.
    pub(super) fn popout_event(&mut self, id: WindowId, event: &WindowEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some(at) = self.popouts.iter().position(|popout| popout.window.id() == id) else { return false };
        let what = self.popouts[at].what;
        match event {
            // Closed from its title bar: back in the main window, and to stay.
            WindowEvent::CloseRequested => {
                self.out.remove(&what);
                self.close_popout(what);
                self.save_panels();
            }
            WindowEvent::RedrawRequested => self.draw_popout(at),
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                let window = &self.popouts[at].window;
                if let Ok(position) = window.outer_position() {
                    let size = window.inner_size();
                    self.popout_at.insert(what, [position.x, position.y, size.width as i32, size.height as i32]);
                }
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x, position.y);
                match what {
                    Popped::Inspector => self.popouts[at].mouse = (x, y),
                    // The list's own handling, rows lighting up and all.
                    Popped::List => self.with_popout(what, |app| app.list_popout_moved(x, y)),
                }
                self.popouts[at].window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.popouts[at].mouse = (-1.0, -1.0);
                if what == Popped::List {
                    self.list_hover = None;
                }
                self.popouts[at].window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == ElementState::Pressed;
                match what {
                    Popped::Inspector if pressed && *button == MouseButton::Left => {
                        let mouse = self.popouts[at].mouse;
                        self.with_popout(what, |app| app.inspector_click(mouse.0, mouse.1, event_loop));
                    }
                    Popped::Inspector => {}
                    Popped::List if pressed && *button == MouseButton::Left => self.with_popout(what, |app| app.list_popout_press(event_loop)),
                    Popped::List => {}
                }
                self.redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                match what {
                    Popped::Inspector => {
                        let lines = match delta {
                            MouseScrollDelta::LineDelta(_, lines) => f64::from(*lines),
                            MouseScrollDelta::PixelDelta(position) => position.y / 40.0,
                        };
                        self.inspector_wheel(lines);
                    }
                    Popped::List => self.with_popout(what, |app| app.mouse_wheel(*delta)),
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            // Its keys first; the rest are the main window's, Space to play
            // and pause and so on, as when it is docked.
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                let taken = what == Popped::Inspector && self.inspector_key(event.logical_key.as_ref());
                if !taken {
                    self.key_pressed(event.logical_key.as_ref(), event.repeat, event.location == KeyLocation::Numpad, event_loop);
                }
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => self.key_released(event.logical_key.as_ref()),
            _ => {}
        }
        true
    }

    /// Draws a panel across a canvas the size of its own window.
    fn draw_popped(&mut self, what: Popped, pixels: &mut [u32], size: (u32, u32)) {
        ui::fill_all(pixels, GRID_COLOR);
        let mut text = self.text.take().unwrap_or_else(Text::load);
        // Its words the size of the main window's.
        let px = ui_px(self.window_size().1.max(1));
        self.with_popout(what, |app| {
            let mut canvas = Canvas::new(pixels, size.0, size.1);
            match what {
                Popped::Inspector => app.draw_inspector(&mut canvas, &mut text, px),
                Popped::List => {
                    if let Some(panel) = app.list_panel() {
                        app.draw_list(&mut canvas, &mut text, &panel);
                    }
                }
            }
        });
        self.text = Some(text);
    }

    /// Draws a popped-out window: the panel into memory, then handed over.
    fn draw_popout(&mut self, at: usize) {
        let what = self.popouts[at].what;
        let size = self.popouts[at].window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        let mut pixels = vec![0u32; size.width as usize * size.height as usize];
        self.draw_popped(what, &mut pixels, (size.width, size.height));
        let popout = &mut self.popouts[at];
        if popout.surface.resize(width, height).is_ok() {
            if let Ok(mut buffer) = popout.surface.buffer_mut() {
                if buffer.len() == pixels.len() {
                    buffer.copy_from_slice(&pixels);
                }
                popout.window.pre_present_notify();
                let _ = buffer.present();
            }
        }
    }

    /// What the first popped-out window shows, drawn off screen: for a
    /// scripted run's snapshot, whose windows are never shown.
    pub(super) fn popout_pixels(&mut self) -> Option<((u32, u32), Vec<u32>)> {
        let popout = self.popouts.first()?;
        let size = popout.window.inner_size();
        let (size, what) = ((size.width.max(1), size.height.max(1)), popout.what);
        let mut pixels = vec![0u32; size.0 as usize * size.1 as usize];
        self.draw_popped(what, &mut pixels, size);
        Some((size, pixels))
    }
}

impl App {
    /// The pointer moving over the file list in its own window: the row
    /// under it lights up, and nothing of the main window sees it.
    fn list_popout_moved(&mut self, x: f64, y: f64) {
        self.mouse = (x, y);
        self.list_hover = self.list_row_at(x, y);
    }

    /// A press in the file list's own window: as on the list docked.
    fn list_popout_press(&mut self, event_loop: &ActiveEventLoop) {
        let (x, y) = self.mouse;
        let Some(panel) = self.list_panel() else { return };
        let within = x as i32 - panel.left;
        let in_field = self.list_field().is_some_and(|[fx, fy, w, h]| (fx..fx + w).contains(&within) && (fy..fy + h).contains(&(y as i32)));
        if !in_field {
            self.list_search = None;
        }
        self.list_click(x, y, in_field, panel.holds(x, y), event_loop);
    }
}
