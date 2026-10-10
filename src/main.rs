mod decode;
mod files;
mod loader;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use decode::Frame;
use loader::Loader;

/// Colour behind and around images, as `0RGB`.
pub const BACKGROUND: u32 = 0x000000;

const USAGE: &str = "\
Usage: jog [FILE|DIR]...

With one file, opens its directory starting at that file.
With no arguments, opens the current directory.

Keys:
  >  .  Right  Space       next image
  <  ,  Left   Backspace   previous image
  Home / End               first / last image
  f                        toggle fullscreen
  q  Esc                   quit";

enum UserEvent {
    Loaded(usize),
}

struct App {
    files: Arc<[PathBuf]>,
    current: usize,
    forward: bool,
    loader: Loader,
    /// Size images are fitted to: the monitor when fullscreen, else the window.
    target: (u32, u32),
    /// What is on screen. Kept while the next image decodes, so navigation
    /// never flashes to black.
    shown: Option<Arc<Frame>>,
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
}

impl App {
    fn request(&self) {
        if self.target.0 > 0 && self.target.1 > 0 {
            self.loader.request(self.current, self.forward, self.target);
        }
    }

    fn go_to(&mut self, index: usize, forward: bool) {
        if index == self.current {
            return;
        }
        self.current = index;
        self.forward = forward;
        self.request();
        self.refresh();
    }

    fn step(&mut self, forward: bool) {
        let len = self.files.len();
        let index = if forward { (self.current + 1) % len } else { (self.current + len - 1) % len };
        self.go_to(index, forward);
    }

    /// Syncs the title and displayed frame with the current image.
    fn refresh(&mut self) {
        let Some(window) = &self.window else { return };
        let path = &self.files[self.current];
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
        let mut title = format!("{name} [{}/{}] - jog", self.current + 1, self.files.len());

        match self.loader.get(self.current) {
            Some(Ok(frame)) => self.shown = Some(frame),
            Some(Err(reason)) => {
                self.shown = None;
                title = format!("{name}: {reason} - jog");
            }
            None => {}
        }
        window.set_title(&title);
        window.request_redraw();
    }

    fn update_target(&mut self) {
        let Some(window) = &self.window else { return };
        let monitor = window.fullscreen().and_then(|_| window.current_monitor());
        let size = monitor.map_or_else(|| window.inner_size(), |m| m.size());
        let target = (size.width, size.height);
        if target != self.target {
            self.target = target;
            self.request();
        }
    }

    fn toggle_fullscreen(&mut self) {
        let Some(window) = &self.window else { return };
        let fullscreen = window.fullscreen().is_none().then_some(Fullscreen::Borderless(None));
        window.set_fullscreen(fullscreen);
        self.update_target();
    }

    fn draw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else { return };
        let size = window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        match &self.shown {
            Some(frame) => blit(&mut buffer, size.width as usize, frame),
            None => buffer.fill(BACKGROUND),
        }
        window.pre_present_notify();
        let _ = buffer.present();
    }
}

/// Draws `frame` centred in a buffer `width` pixels wide, clearing the rest.
fn blit(buffer: &mut [u32], width: usize, frame: &Frame) {
    let height = buffer.len() / width;
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    // A frame fitted to an older, larger window is shrunk crudely until the
    // loader delivers a properly fitted one.
    let (dw, dh) = decode::fit_size((frame.width, frame.height), (width as u32, height as u32));
    let (dw, dh) = (dw as usize, dh as usize);
    let (x0, y0) = ((width - dw) / 2, (height - dh) / 2);

    for (y, row) in buffer.chunks_exact_mut(width).enumerate() {
        if y < y0 || y >= y0 + dh {
            row.fill(BACKGROUND);
            continue;
        }
        let source = &frame.pixels[(y - y0) * fh / dh * fw..][..fw];
        row[..x0].fill(BACKGROUND);
        row[x0 + dw..].fill(BACKGROUND);
        let dest = &mut row[x0..x0 + dw];
        if dw == fw {
            dest.copy_from_slice(source);
        } else {
            for (x, pixel) in dest.iter_mut().enumerate() {
                *pixel = source[x * fw / dw];
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Start decoding before the window exists, so the first image is
        // ready by the time the window is mapped.
        let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next());
        if let Some(size) = monitor.map(|m| m.size()) {
            self.target = (size.width, size.height);
            self.request();
        }

        let attributes = Window::default_attributes()
            .with_title("jog")
            .with_fullscreen(Some(Fullscreen::Borderless(None)));
        let window = Arc::new(event_loop.create_window(attributes).expect("failed to create window"));
        let context = Context::new(window.clone()).expect("failed to connect to display");
        self.surface = Some(Surface::new(&context, window.clone()).expect("failed to create surface"));
        self.window = Some(window);
        self.update_target();
        self.refresh();
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Loaded(index) = event;
        if index == self.current {
            self.refresh();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) => {
                self.update_target();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                match event.logical_key.as_ref() {
                    Key::Character(">" | ".") | Key::Named(NamedKey::ArrowRight | NamedKey::Space) => {
                        self.step(true)
                    }
                    Key::Character("<" | ",") | Key::Named(NamedKey::ArrowLeft | NamedKey::Backspace) => {
                        self.step(false)
                    }
                    Key::Named(NamedKey::Home) => self.go_to(0, true),
                    Key::Named(NamedKey::End) => self.go_to(self.files.len() - 1, false),
                    Key::Character("f") if !event.repeat => self.toggle_fullscreen(),
                    Key::Character("q") | Key::Named(NamedKey::Escape) => event_loop.exit(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn main() -> ExitCode {
    let mut paths = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some("-V" | "--version") => {
                println!("jog {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    let playlist = files::collect(&paths);
    if playlist.files.is_empty() {
        eprintln!("jog: no images to show\n\n{USAGE}");
        return ExitCode::FAILURE;
    }
    let files: Arc<[PathBuf]> = playlist.files.into();

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("jog: cannot open display: {error}");
            return ExitCode::FAILURE;
        }
    };
    let proxy = event_loop.create_proxy();
    let loader = Loader::new(files.clone(), move |index| {
        let _ = proxy.send_event(UserEvent::Loaded(index));
    });

    let mut app = App {
        files,
        current: playlist.start,
        forward: true,
        loader,
        target: (0, 0),
        shown: None,
        window: None,
        surface: None,
    };
    if let Err(error) = event_loop.run_app(&mut app) {
        eprintln!("jog: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blit_centres_small_frames() {
        let frame = Frame { width: 2, height: 1, pixels: vec![1, 2] };
        let mut buffer = [9; 12];
        blit(&mut buffer, 4, &frame);
        assert_eq!(buffer, [0, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn blit_shrinks_oversized_frames() {
        let frame = Frame { width: 4, height: 4, pixels: (0..16).collect() };
        let mut buffer = [9; 6];
        blit(&mut buffer, 3, &frame);
        assert_eq!(buffer, [0, 2, 0, 8, 10, 0]);
    }
}
