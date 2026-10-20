//! The player process: jog started again as `jog --player <role> <hwdec>
//! <debug>`, with the socket to jog's process as descriptor 3. It starts
//! mpv (`Engine`) and does what it is asked, in the order asked, telling of
//! what mpv says as it says it; frames are drawn into memory jog's process
//! shares with it.
//!
//! It goes the moment jog's process does: the socket's end gone, or a
//! `Quit`. It does not wait for mpv to wind down, which is what a hung
//! driver would hold it on; everything it has is the system's to take
//! back as it exits.

use std::ffi::OsStr;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use super::engine::Engine;
use super::wire::{self, Mapped, Notice, Reply, Request};
use super::{Kind, Known, Role, Signal, DEBUG, WATCHED};

/// The descriptor the socket to jog's process is handed on.
pub(super) const SOCKET_FD: i32 = 3;

/// For trying out how jog's process takes a crash: a player that opens a
/// file whose path has this in it crashes as a bad decoder would, when
/// the GPU decodes it; with `CRASH_ALWAYS_ON`, however it is decoded.
const CRASH_ON: &str = "JOG_TEST_PLAYER_CRASH";
const CRASH_ALWAYS_ON: &str = "JOG_TEST_PLAYER_CRASH_ALWAYS";

enum Input {
    Wake(Signal),
    Request(Request, Option<OwnedFd>),
}

/// Exits at once: see the module's notes.
fn leave(code: i32) -> ! {
    // SAFETY: _exit ends the process without running anything further.
    unsafe { libc::_exit(code) }
}

/// The player process's `main`, given the arguments after `--player`.
pub fn main(args: &[String]) -> ExitCode {
    // SAFETY: fcntl only asks whether the descriptor is open.
    if unsafe { libc::fcntl(SOCKET_FD, libc::F_GETFD) } < 0 {
        eprintln!("jog: --player is for jog's own use");
        return ExitCode::FAILURE;
    }
    // SAFETY: descriptor 3 is the socket jog's process handed over, open
    // as just checked, and owned by nothing else in this process.
    let socket = unsafe { OwnedFd::from_raw_fd(SOCKET_FD) };
    let (Some(role), Some(hwdec)) = (args.first().and_then(|word| Role::from_word(word)), args.get(1)) else {
        eprintln!("jog: --player: no such role");
        return ExitCode::FAILURE;
    };
    DEBUG.store(args.get(2).is_some_and(|debug| debug == "1"), Ordering::Relaxed);
    let say = |notice: Notice| {
        if wire::send(socket.as_fd(), &notice.encode(), None).is_err() {
            leave(0);
        }
    };

    let (inputs, inbox) = mpsc::channel::<Input>();
    let wakes = inputs.clone();
    let engine = match Engine::new(role, hwdec == "1", move |signal| drop(wakes.send(Input::Wake(signal)))) {
        Ok(engine) => engine,
        Err(why) => {
            say(Notice::Failed(why));
            return ExitCode::FAILURE;
        }
    };
    say(Notice::Ready);

    // Requests are read on a thread of their own, so that the end of jog's
    // process is seen however long mpv takes over what it is doing.
    let reader = socket.try_clone().unwrap_or_else(|_| leave(1));
    std::thread::spawn(move || {
        let mut buffer = vec![0; wire::MAX];
        loop {
            match wire::recv(reader.as_fd(), &mut buffer) {
                Ok(Some((n, passed))) => match Request::decode(&buffer[..n]) {
                    Some(Request::Quit) => leave(0),
                    Some(request) => {
                        if inputs.send(Input::Request(request, passed)).is_err() {
                            leave(0);
                        }
                    }
                    None => {
                        eprintln!("jog: the player was sent a message it cannot read");
                        leave(1);
                    }
                },
                _ => leave(0),
            }
        }
    });

    let test_crash = TestCrash { gpu: std::env::var(CRASH_ON).ok().filter(|text| !text.is_empty()), always: std::env::var(CRASH_ALWAYS_ON).ok().filter(|text| !text.is_empty()) };
    let mut gpu = role.gpu() && hwdec == "1";
    let mut told: Vec<Option<Known>> = vec![None; WATCHED.len()];
    let mut buffer: Option<Mapped> = None;
    // Drawing ahead: the slots' states, their buffers, the size to draw
    // at, and how many frames have been drawn.
    let mut states: Option<Mapped> = None;
    let mut slots: [Option<Mapped>; wire::SLOTS] = Default::default();
    let mut ahead: Option<(u32, u32)> = None;
    let mut count: u32 = 0;
    // What is drawn over each frame, as a monitor does.
    let mut checks = super::checks::Checks::default();
    for input in inbox {
        match input {
            Input::Wake(Signal::Events) => {
                let events = engine.events();
                // The watched properties that changed, and only those.
                let now = engine.known_values();
                let changed: Vec<(u8, Option<Known>)> = now.iter().enumerate().filter(|(index, value)| told[*index] != **value).map(|(index, value)| (index as u8, value.clone())).collect();
                told = now;
                if !events.is_empty() || !changed.is_empty() {
                    say(Notice::Events { events, changed });
                }
            }
            // Drawing ahead: the frame drawn now, into a slot, and only
            // then told of, so that jog's main thread has only to copy it.
            Input::Wake(Signal::Frame) if ahead.is_some() && states.is_some() => {
                let (Some(size), Some(states)) = (ahead, states.as_ref()) else { continue };
                // Woken again for a frame already drawn: nothing new.
                if !engine.frame_waiting() {
                    continue;
                }
                if draw_ahead(&engine, states, &mut slots, size, checks, &mut count) {
                    say(Notice::Frame);
                }
            }
            Input::Wake(Signal::Frame) => say(Notice::Frame),
            Input::Request(request, passed) => match request {
                Request::Command { tag, reply, args } => {
                    let args: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
                    match reply {
                        Some(id) => {
                            engine.command(&args);
                            say(Notice::Reply { id, reply: Reply::Done });
                        }
                        None => engine.command_tagged(tag, &args),
                    }
                }
                Request::Set { name, value } => {
                    // Which decoder the next file gets, for the test crash.
                    if name == "hwdec" {
                        gpu = value != "no";
                    }
                    engine.set(&name, &value);
                }
                Request::SetFilter(filter) => engine.set_filter(&filter),
                Request::ChooseNext { aid, sid, sub_delay, audio_delay } => engine.choose_next(&aid, &sid, sub_delay, audio_delay),
                Request::Play { path, from, paused } => {
                    test_crash.check(&path, gpu);
                    if let Some(states) = states.as_ref() {
                        wire::opened(states);
                    }
                    engine.play(Path::new(OsStr::from_bytes(&path)), from, paused);
                }
                Request::Hold { path, from } => {
                    test_crash.check(&path, gpu);
                    if let Some(states) = states.as_ref() {
                        wire::opened(states);
                    }
                    engine.hold(Path::new(OsStr::from_bytes(&path)), from);
                }
                Request::Open { path } => {
                    test_crash.check(&path, gpu);
                    if let Some(states) = states.as_ref() {
                        wire::opened(states);
                    }
                    engine.open(Path::new(OsStr::from_bytes(&path)));
                }
                Request::Stop => engine.stop(),
                Request::Get { id, name, kind } => {
                    let value = match kind {
                        Kind::Number => engine.number_now(&name).map(Known::Number),
                        Kind::Flag => engine.flag(&name).map(Known::Flag),
                        Kind::Text => engine.text(&name).map(Known::Text),
                    };
                    say(Notice::Reply { id, reply: Reply::Value(value) });
                }
                Request::FrameWaiting { id } => say(Notice::Reply { id, reply: Reply::Yes(engine.frame_waiting()) }),
                Request::Buffer { pixels } => {
                    // Let go of the last before taking the next: jog's
                    // process has made this one to replace it.
                    drop(buffer.take());
                    buffer = passed.and_then(|fd| Mapped::map(fd, pixels as usize).ok());
                }
                Request::Slots => states = passed.and_then(|fd| Mapped::map(fd, wire::STATE_WORDS).ok()),
                Request::Slot { index, pixels } => {
                    if let Some(slot) = slots.get_mut(index as usize) {
                        drop(slot.take());
                        *slot = passed.and_then(|fd| Mapped::map(fd, pixels as usize).ok());
                    }
                }
                Request::Ahead { width, height } => ahead = (width > 0 && height > 0).then_some((width, height)),
                Request::Render { id, width, height } => {
                    let wanted = width as usize * height as usize;
                    let drawn = buffer.as_mut().filter(|buffer| buffer.pixels() >= wanted).is_some_and(|buffer| {
                        let pixels = &mut buffer.as_mut_slice()[..wanted];
                        let drawn = engine.render(pixels, width, height);
                        if drawn {
                            super::checks::apply(pixels, width, checks);
                        }
                        drawn
                    });
                    say(Notice::Reply { id, reply: Reply::Yes(drawn) });
                }
                // The frame on view drawn again with them, paused as it may
                // be, rather than waiting for the next.
                Request::Checks(now) => {
                    checks = now;
                    if let (Some(size), Some(states)) = (ahead, states.as_ref()) {
                        if draw_ahead(&engine, states, &mut slots, size, checks, &mut count) {
                            say(Notice::Frame);
                        }
                    }
                }
                Request::Warm { id, width, height } => {
                    engine.warm(width, height);
                    say(Notice::Reply { id, reply: Reply::Done });
                }
                Request::Warming { id } => say(Notice::Reply { id, reply: Reply::Yes(engine.warming()) }),
                Request::Settle { id } => {
                    engine.settle();
                    say(Notice::Reply { id, reply: Reply::Done });
                }
                Request::Quit => leave(0),
            },
        }
    }
    leave(0)
}

/// Draws the frame the player has into a slot at `size`, with `checks`
/// over it, counted; whether it was drawn, to be told of.
fn draw_ahead(engine: &Engine, states: &Mapped, slots: &mut [Option<Mapped>; wire::SLOTS], (width, height): (u32, u32), checks: super::checks::Checks, count: &mut u32) -> bool {
    let wanted = width as usize * height as usize;
    let Some(slot) = wire::claim_to_draw(states) else {
        // Every slot in use: the frame is still taken, which the player
        // waits for, and not kept.
        engine.render(&mut [0; 64], 8, 8);
        return false;
    };
    let drawn = slots[slot].as_mut().filter(|buffer| buffer.pixels() >= wanted).is_some_and(|buffer| {
        let pixels = &mut buffer.as_mut_slice()[..wanted];
        let drawn = engine.render(pixels, width, height);
        if drawn {
            super::checks::apply(pixels, width, checks);
        }
        drawn
    });
    *count = count.wrapping_add(1);
    wire::drawn(states, slot, drawn.then_some((width, height)), *count);
    drawn
}

/// The crashes a test asks for: on files whose paths have these in them,
/// when the GPU decodes them, or always.
struct TestCrash {
    gpu: Option<String>,
    always: Option<String>,
}

impl TestCrash {
    /// Crashes as a bad decoder would, if asked to for files like this one
    /// decoded as this one is to be.
    fn check(&self, path: &[u8], gpu: bool) {
        let has = |text: &Option<String>| text.as_ref().is_some_and(|text| path.windows(text.len()).any(|window| window == text.as_bytes()));
        if has(&self.always) || gpu && has(&self.gpu) {
            // As ffmpeg goes when one of its assertions fails, which is how
            // a GPU decoder's error took the player down on Intel's driver.
            // (A SIGSEGV raised would not do: Rust's handler for stack
            // overflow takes it, finds no overflow, and returns.)
            std::process::abort();
        }
    }
}
