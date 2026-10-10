mod app;
mod commands;
mod decode;
mod files;
mod frame;
mod loader;
mod meta;
mod options;
mod render;
mod rng;
mod ui;
mod view;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use winit::event_loop::EventLoop;

use app::App;
use loader::{Job, Loader};
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

    let playlist = files::collect(&options.paths, &options.listing);
    if playlist.files.is_empty() {
        eprintln!("jog: no images to show\nTry 'jog --help' for the options.");
        return ExitCode::FAILURE;
    }
    let files: Arc<[PathBuf]> = playlist.files.into();

    let event_loop = match EventLoop::<Job>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("jog: cannot open display: {error}");
            return ExitCode::FAILURE;
        }
    };
    let proxy = event_loop.create_proxy();
    let loader = Loader::new(files.clone(), options.background, move |job| {
        let _ = proxy.send_event(job);
    });

    let mut app = App::new(options, files, playlist.start, loader);
    if let Err(error) = event_loop.run_app(&mut app) {
        eprintln!("jog: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
