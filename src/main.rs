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
mod positions;
mod render;
mod rng;
mod search;
mod story;
mod trailer;
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

    let started = std::time::Instant::now();
    let playlist = files::collect(&options.paths, &options.listing);
    if playlist.files.is_empty() {
        // A path that is not there has been said to be missing already;
        // that is the whole of the matter.
        if options.paths.iter().all(|path| path.exists()) {
            eprintln!("jog: no images or videos to show");
        }
        return ExitCode::FAILURE;
    }
    if options.verbose {
        let videos = playlist.files.iter().filter(|path| files::is_video(path)).count();
        eprintln!("jog: {} files, {videos} of them videos, listed in {:.1?}", playlist.files.len(), started.elapsed());
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

    let (watch, filter) = (options.watch, options.filter.clone());
    let mut app = App::new(options, files, playlist.start, loader, proxy);
    if watch {
        app.set_watch(true);
    }
    // --filter narrows the list before anything is shown; with nothing
    // left there is nothing to open a window for.
    if let Some(typed) = filter {
        if !app.start_filter(&typed) {
            eprintln!("jog: nothing matches --filter '{typed}'");
            return ExitCode::FAILURE;
        }
    }
    let result = event_loop.run_app(&mut app);
    app.remember_position();
    app.print_marked();
    if let Err(error) = result {
        eprintln!("jog: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
