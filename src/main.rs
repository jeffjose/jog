mod app;
mod commands;
mod convert;
mod decode;
mod files;
mod frame;
mod loader;
mod meta;
mod mpv;
mod ops;
mod options;
mod render;
mod rng;
mod ui;
mod view;

use std::process::ExitCode;

use winit::event_loop::EventLoop;

use app::{App, Event};
use loader::Loader;
use options::Parsed;

fn main() -> ExitCode {
    let options = match options::parse(std::env::args_os().skip(1)) {
        Ok(Parsed::Run(options)) => *options,
        Ok(Parsed::Help) => {
            println!("{}", options::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            println!("jog {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("jog: {error}\nTry 'jog --help' for the options.");
            return ExitCode::from(2);
        }
    };

    let mut playlist = files::collect(&options.paths, &options.listing);
    // Without libmpv, videos would only be entries that cannot be shown.
    let videos = playlist.files.iter().filter(|file| files::is_video(file)).count();
    if videos > 0 && !mpv::available() {
        let start = playlist.files.get(playlist.start).cloned();
        playlist.files.retain(|file| !files::is_video(file));
        playlist.start = start.and_then(|start| playlist.files.iter().position(|file| *file == start)).unwrap_or(0);
        eprintln!("jog: leaving out {videos} videos; playing them needs libmpv (the libmpv1 or libmpv2 package)");
    }
    if playlist.files.is_empty() {
        eprintln!("jog: no images to show\nTry 'jog --help' for the options.");
        return ExitCode::FAILURE;
    }
    let files = playlist.files;

    let event_loop = match EventLoop::<Event>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("jog: cannot open display: {error}");
            return ExitCode::FAILURE;
        }
    };
    let proxy = event_loop.create_proxy();
    let loaded = proxy.clone();
    let loader = Loader::new(files.clone(), options.background, options.debug, move |job| {
        let _ = loaded.send_event(Event::Loaded(job));
    });

    let mut app = App::new(options, files, playlist.start, loader, proxy);
    let result = event_loop.run_app(&mut app);
    app.print_marked();
    if let Err(error) = result {
        eprintln!("jog: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
