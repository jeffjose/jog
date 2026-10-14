mod app;
mod commands;
mod convert;
mod decode;
mod files;
mod frame;
mod heif;
mod hdrmeta;
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
mod scopes;
mod screen;
mod search;
mod sessions;
mod seen;
mod story;
mod trailer;
mod triage;
mod ultrahdr;
mod ui;
mod view;
mod x11;

use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

use winit::event_loop::{EventLoop, EventLoopProxy};

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
    let mut cwd = std::env::current_dir().unwrap_or_default();
    if options.list_sessions || options.session {
        // One asked for by its id is that one, whichever folder it is of:
        // a single word after --resume that is no file or folder.
        let asked: Option<String> = match options.paths.as_slice() {
            [only] if options.session && !only.exists() => Some(only.to_string_lossy().into_owned()),
            _ => None,
        };
        let by_id = asked.as_deref().map(sessions::by_id);
        if let Some(None) = by_id {
            eprintln!("jog: there is no session with the id {}", asked.unwrap_or_default());
            return ExitCode::FAILURE;
        }
        let named = by_id.flatten();
        let found = named.clone().map_or_else(|| sessions::here(&cwd), |session| vec![session]);
        if options.list_sessions {
            match found.is_empty() {
                true => println!("No sessions in {}", cwd.display()),
                // Two lines each, with a line between: easier on the eye than one long one.
                false => println!("{}", found.iter().map(sessions::describe).collect::<Vec<_>>().join("\n\n")),
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
        // It stays the session it was, of the folder it was of, when it
        // is kept again on quitting.
        options.session_id = Some(latest.id.clone());
        cwd = latest.cwd.clone();
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

    // The loader starts before the display is opened, which takes a
    // while, so that the first image is decoding meanwhile: for the
    // screen it was last shown on, which is nearly always the one it is
    // about to be. Until the event loop exists there is no one to tell of
    // a frame that is done; the window picks it up when it opens.
    let proxy_slot: Arc<OnceLock<EventLoopProxy<Event>>> = Arc::new(OnceLock::new());
    let slot = proxy_slot.clone();
    let loader = Loader::new(files.clone(), options.background, options.debug, move |job| {
        if let Some(proxy) = slot.get() {
            let _ = proxy.send_event(Event::Loaded(job));
        }
    });
    if options.debug {
        eprintln!("jog: {:>7.1?} start loader threads up", started.elapsed());
    }
    if let Some(screen) = screen::remembered().filter(|_| options.filter.is_none() && options.sessions.is_empty()) {
        loader.request(App::first_request(files.len(), playlist.start, screen, options.auto_zoom, options.hdr, loader.workers()));
    }

    let event_loop = match EventLoop::<Event>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("jog: cannot open display: {error}");
            return ExitCode::FAILURE;
        }
    };
    if options.debug {
        eprintln!("jog: {:>7.1?} start listed and display opened", started.elapsed());
    }
    let proxy = event_loop.create_proxy();
    let _ = proxy_slot.set(proxy.clone());

    let (watch, filter, choosing) = (options.watch, options.filter.clone(), !options.sessions.is_empty());
    let benchmark = options.benchmark;
    let mut app = App::new(options, files, playlist.start, loader, proxy, cwd);
    if benchmark {
        app.start_benchmark(started);
    }
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
    let leaving = std::time::Instant::now();
    app.save_session();
    app.remember_position();
    app.print_marked();
    if let Err(error) = result {
        eprintln!("jog: {error}");
        return ExitCode::FAILURE;
    }
    if app.debugging() {
        eprintln!("jog: {:>7.1?} quit, state kept", leaving.elapsed());
    }
    // Nothing is taken down on the way out: the players, the decode
    // threads and the window are the system's to reclaim, and it does so
    // at once, where closing a player that is in the middle of opening a
    // file takes it half a second, with the window standing meanwhile.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    std::process::exit(0)
}

#[cfg(test)]
mod hdrmeta_samples {
    /// Reads every sample in a folder named by `JOG_HDR_SAMPLES`, when set,
    /// and prints what was found: a check against real files, kept out of
    /// the ordinary run of the tests.
    #[test]
    fn samples() {
        let Some(folder) = std::env::var_os("JOG_HDR_SAMPLES") else { return };
        let mut paths: Vec<_> = std::fs::read_dir(folder).unwrap().filter_map(|entry| Some(entry.ok()?.path())).collect();
        paths.sort();
        for path in paths {
            let Some(info) = crate::hdrmeta::read(&path) else { continue };
            let first = info.agtm.first().map(|t| (t.value.baseline, t.value.altr.iter().map(|a| a.headroom).collect::<Vec<_>>()));
            println!(
                "{:<46} {:?} agtm={} hdr10+={} {:?} first={:?} hdr10+target={:?}",
                path.file_name().unwrap().to_string_lossy(),
                info.badges(),
                info.agtm.len(),
                info.hdr10plus.len(),
                info.carried,
                first,
                info.hdr10plus.first().map(|t| t.value.target)
            );
        }
    }
}
