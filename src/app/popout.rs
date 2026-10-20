//! Panels popped out into windows of their own, as a browser's developer
//! tools can be: moved to another screen, sized as wanted, while the
//! picture has all of the main window. Closing one's window puts the panel
//! back where it was. The inspector is the first to go out so.

use softbuffer::{Context, Surface};
use winit::dpi::PhysicalSize;
use winit::window::{Window, WindowId};

use super::*;

/// What can be popped out.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Popped {
    Inspector,
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

    /// Pops a panel out, or puts it back.
    pub(super) fn toggle_popout(&mut self, what: Popped) {
        // Remembered: the inspector opens out, or in, next time too.
        self.inspector_out = !self.popped(what);
        self.save_panels();
        if self.popped(what) {
            self.close_popout(what);
        } else {
            // A window is made where the event loop is to hand, on its
            // next turn.
            self.popout_wanted = Some(what);
            self.redraw();
        }
    }

    /// Opens the window asked for, if any, and closes those whose panel
    /// has been closed. Each turn of the event loop.
    pub(super) fn tend_popouts(&mut self, event_loop: &ActiveEventLoop) {
        if self.inspector.is_none() && self.popped(Popped::Inspector) {
            self.close_popout(Popped::Inspector);
        }
        let Some(what) = self.popout_wanted.take() else { return };
        if self.popped(what) {
            return;
        }
        let (width, height) = self.window_size();
        let px = ui_px(height);
        let size = PhysicalSize::new((panel_width(width as i32 * 2, px, self.inspector_units).max(480)) as u32, (height * 6 / 7).max(480));
        // A scripted run shows no window: its snapshot draws this one too.
        let mut attributes = Window::default_attributes().with_title("Inspect · jog").with_inner_size(size).with_visible(self.snapshot.is_none());
        // Where it was last, on whichever screen that was.
        if let Some([x, y, w, h]) = self.popout_at {
            attributes = attributes.with_position(winit::dpi::PhysicalPosition::new(x, y)).with_inner_size(PhysicalSize::new(w as u32, h as u32));
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

    /// Runs `then` with the pointer as it is in a popped-out window, the
    /// panel's own code reading it from where it always does.
    fn with_popout_mouse<T>(&mut self, mouse: (f64, f64), then: impl FnOnce(&mut App) -> T) -> T {
        let main = std::mem::replace(&mut self.mouse, mouse);
        self.drawing_popout = true;
        let result = then(self);
        self.drawing_popout = false;
        self.mouse = main;
        result
    }

    /// Something happening to a popped-out window. Whether it was one.
    pub(super) fn popout_event(&mut self, id: WindowId, event: &WindowEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some(at) = self.popouts.iter().position(|popout| popout.window.id() == id) else { return false };
        match event {
            // Closed from its title bar: back in the main window, and to stay.
            WindowEvent::CloseRequested => {
                let what = self.popouts[at].what;
                self.inspector_out = false;
                self.close_popout(what);
                self.save_panels();
            }
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                let window = &self.popouts[at].window;
                if let Ok(position) = window.outer_position() {
                    let size = window.inner_size();
                    self.popout_at = Some([position.x, position.y, size.width as i32, size.height as i32]);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.draw_popout(at),

            WindowEvent::CursorMoved { position, .. } => {
                self.popouts[at].mouse = (position.x, position.y);
                self.popouts[at].window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.popouts[at].mouse = (-1.0, -1.0);
                self.popouts[at].window.request_redraw();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let mouse = self.popouts[at].mouse;
                self.with_popout_mouse(mouse, |app| app.inspector_click(mouse.0, mouse.1));
                self.redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, lines) => f64::from(*lines),
                    MouseScrollDelta::PixelDelta(position) => position.y / 40.0,
                };
                self.inspector_wheel(lines);
                self.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            // Its keys first; the rest are the main window's, Space to play
            // and pause and so on, as when it is docked.
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                if !self.inspector_key(event.logical_key.as_ref()) {
                    self.key_pressed(event.logical_key.as_ref(), event.repeat, event.location == KeyLocation::Numpad, event_loop);
                }
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => self.key_released(event.logical_key.as_ref()),
            _ => {}
        }
        true
    }

    /// Draws a popped-out window: its panel across the whole of it.
    fn draw_popout(&mut self, at: usize) {
        let mut popout = self.popouts.remove(at);
        let size = popout.window.inner_size();
        if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            if popout.surface.resize(width, height).is_ok() {
                if let Ok(mut buffer) = popout.surface.buffer_mut() {
                    ui::fill_all(&mut buffer, GRID_COLOR);
                    let mut text = self.text.take().unwrap_or_else(Text::load);
                    // Its words the size of the main window's.
                    let px = ui_px(self.window_size().1.max(1));
                    self.with_popout_mouse(popout.mouse, |app| {
                        let mut canvas = Canvas::new(&mut buffer, size.width, size.height);
                        app.draw_inspector(&mut canvas, &mut text, px);
                    });
                    self.text = Some(text);
                    popout.window.pre_present_notify();
                    let _ = buffer.present();
                }
            }
        }
        self.popouts.insert(at, popout);
    }
}

impl App {
    /// What the first popped-out window shows, drawn off screen: for a
    /// scripted run's snapshot, whose windows are never shown.
    pub(super) fn popout_pixels(&mut self) -> Option<((u32, u32), Vec<u32>)> {
        let popout = self.popouts.first()?;
        let size = popout.window.inner_size();
        let size = (size.width.max(1), size.height.max(1));
        let mouse = popout.mouse;
        let mut pixels = vec![GRID_COLOR; size.0 as usize * size.1 as usize];
        let mut text = self.text.take().unwrap_or_else(Text::load);
        let px = ui_px(self.window_size().1.max(1));
        self.with_popout_mouse(mouse, |app| {
            let mut canvas = Canvas::new(&mut pixels, size.0, size.1);
            app.draw_inspector(&mut canvas, &mut text, px);
        });
        self.text = Some(text);
        Some((size, pixels))
    }
}
