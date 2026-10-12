mod app;
mod commands;
mod convert;
mod decode;
mod files;
mod frame;
mod heif;
mod history;
mod loader;
mod meta;
mod mpv;
mod ops;
mod options;
mod positions;
mod render;
mod rng;
mod scenes;
mod search;
mod sessions;
mod seen;
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
    let mut options = match options::parse(std::env::args_os().skip(1)) {
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

    // The sessions of the folder jog is started in: to be listed, or one
    // of them picked up in place of what was named to open.
    let cwd = std::env::current_dir().unwrap_or_default();
    if options.list_sessions || options.session {
        let found = sessions::here(&cwd);
        if options.list_sessions {
            match found.is_empty() {
                true => println!("No sessions in {}", cwd.display()),
                false => found.iter().enumerate().for_each(|(at, session)| println!("{:>2}  {}", at + 1, sessions::describe(session))),
            }
            return ExitCode::SUCCESS;
        }
        let Some(latest) = found.first() else {
            eprintln!("jog: there is no session of this folder to resume");
            return ExitCode::FAILURE;
        };
        // The latest is opened either way; with others to choose from,
        // and no word that it is the one, they are shown over it.
        options.paths = latest.paths.clone();
        options.listing.recursive = latest.recursive;
        options.listing.sort = latest.sort;
        options.listing.reverse = latest.reverse;
        options.listing.filelist = latest.filelist.clone();
        options.listing.start_at = Some(latest.file.clone());
        options.filter = latest.filter.clone();
        options.begin_at = Some(latest.seconds).filter(|seconds| *seconds >= 1.0);
        if !options.latest && found.len() > 1 {
            options.sessions = found;
        }
    }

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

    let (watch, filter, choosing) = (options.watch, options.filter.clone(), !options.sessions.is_empty());
    let mut app = App::new(options, files, playlist.start, loader, proxy, cwd);
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
    // With sessions to choose from, they are what is shown first.
    if choosing {
        app.choose_session();
    }
    let result = event_loop.run_app(&mut app);
    app.save_session();
    app.remember_position();
    app.print_marked();
    if let Err(error) = result {
        eprintln!("jog: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
