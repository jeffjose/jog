//! jog's side of a player: `Player`, which starts a player process and
//! asks it what jog used to ask libmpv, with the same calls.
//!
//! What can be sent and forgotten is: commands, settings, files to open.
//! What needs an answer waits for one, for `ANSWER_WITHIN` at most: a
//! player that takes longer has hung, in a driver as like as not, and is
//! ended. The watched properties are told of as they change, as mpv told
//! of them, and asking for one never waits.
//!
//! A thread listens to the player process: passes its events and frames
//! on, and sees it go. Gone without being asked to, it notes the file it
//! had open (`crashes`) and tells of `PlayerEvent::Crashed`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use super::crashes::{self, Crash};
use super::helper::SOCKET_FD;
use super::wire::{self, Mapped, Notice, Reply, Request};
use super::{watched, Kind, Known, PlayerEvent, Role, Signal, DEBUG, HWDEC, PLAYER_ARG, WATCHED};

/// How long an answer may take before the player is taken to have hung.
/// Opening a large file keeps mpv's core busy for hundreds of
/// milliseconds, and a first draw at a new size for a hundred more; a
/// player that has not answered in this has stopped.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// How long a player process may take to start mpv.
const START_WITHIN: Duration = Duration::from_secs(10);

/// What jog's process and the thread listening to the player share.
struct Shared {
    /// The watched properties' last values, in `WATCHED`'s order.
    known: Mutex<Vec<Option<Known>>>,
    events: Mutex<Vec<PlayerEvent>>,
    /// Answers waited for, by request.
    waiting: Mutex<HashMap<u64, mpsc::Sender<Reply>>>,
    /// The player process has gone.
    gone: AtomicBool,
    /// It was asked to go: its going is no crash.
    quitting: AtomicBool,
    /// The file it has open, and whether the GPU decodes it: what a crash
    /// is put down to.
    open: Mutex<Option<(PathBuf, bool)>>,
    /// What it crashed on, once it has.
    lost: Mutex<Option<Lost>>,
}

/// A player process that crashed: what it had open, and what that file
/// now counts as. How it went is said on the terminal.
#[derive(Clone, Debug)]
pub struct Lost {
    pub path: Option<PathBuf>,
    pub crash: Option<Crash>,
}

pub struct Player {
    role: Role,
    socket: OwnedFd,
    pid: i32,
    shared: Arc<Shared>,
    next_id: Cell<u64>,
    /// The memory frames are drawn into when asked for, shared with the
    /// player process.
    buffer: RefCell<Option<Mapped>>,
    /// Drawing ahead (see `wire::SLOTS`): the slots' states and buffers,
    /// the size frames are drawn at, and the count of the last copied out.
    states: RefCell<Option<Mapped>>,
    slots: RefCell<Vec<Mapped>>,
    ahead: Cell<Option<(u32, u32)>>,
    taken: Cell<Option<u32>>,
    /// Whether the player process has the GPU decode what it opens next.
    gpu: Cell<bool>,
    /// Whether this player watches properties: a preview player does not.
    watching: bool,
}

impl Player {
    /// Starts a player process, idle, and waits for it to have started
    /// mpv: not on the main thread. `wake` is called from the thread that
    /// listens to it, and must only pass the signal on.
    pub fn new(role: Role, wake: impl Fn(Signal) + Send + Sync + 'static) -> Result<Player, String> {
        crashes::load();
        let started = Instant::now();
        let (ours, theirs) = wire::pair().map_err(|error| format!("cannot start the player: {error}"))?;
        let gpu = role.gpu() && HWDEC.load(Ordering::Relaxed);
        let debug = DEBUG.load(Ordering::Relaxed);
        let handed = theirs.as_raw_fd();
        let mut command = Command::new("/proc/self/exe");
        command.arg(PLAYER_ARG).arg(role.word()).arg(if gpu { "1" } else { "0" }).arg(if debug { "1" } else { "0" });
        command.stdin(Stdio::null()).stdout(Stdio::null());
        // SAFETY: between fork and exec only async-signal-safe calls: dup2
        // and fcntl, on descriptors this process owns.
        unsafe {
            command.pre_exec(move || {
                if handed == SOCKET_FD {
                    // Already there: only kept open across exec.
                    if libc::fcntl(SOCKET_FD, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                } else if libc::dup2(handed, SOCKET_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|error| format!("cannot start the player: {error}"))?;
        drop(theirs);
        let pid = child.id() as i32;

        // Its first word: mpv started, or why not.
        set_timeout(ours.as_fd(), Some(START_WITHIN));
        let mut buffer = vec![0; wire::MAX];
        let first = wire::recv(ours.as_fd(), &mut buffer).ok().flatten().and_then(|(n, _)| Notice::decode(&buffer[..n]));
        set_timeout(ours.as_fd(), None);
        match first {
            Some(Notice::Ready) => {}
            Some(Notice::Failed(why)) => {
                let _ = child.wait();
                return Err(why);
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the player did not start".into());
            }
        }
        if debug {
            eprintln!("jog: {:>7.1?} mpv   player process {pid} started ({})", started.elapsed(), role.word());
        }

        let shared = Arc::new(Shared {
            known: Mutex::new(vec![None; WATCHED.len()]),
            events: Mutex::new(Vec::new()),
            waiting: Mutex::new(HashMap::new()),
            gone: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            open: Mutex::new(None),
            lost: Mutex::new(None),
        });
        let listener = ours.try_clone().map_err(|error| format!("cannot start the player: {error}"))?;
        let heard = shared.clone();
        std::thread::spawn(move || listen(listener, child, heard, wake));
        Ok(Player {
            role,
            socket: ours,
            pid,
            shared,
            next_id: Cell::new(1),
            buffer: RefCell::new(None),
            states: RefCell::new(None),
            slots: RefCell::new(Vec::new()),
            ahead: Cell::new(None),
            taken: Cell::new(None),
            gpu: Cell::new(gpu),
            watching: matches!(role, Role::Playback { .. }),
        })
    }

    /// Whether the player process is gone: everything asked of it after
    /// is answered with nothing.
    pub fn gone(&self) -> bool {
        self.shared.gone.load(Ordering::SeqCst)
    }

    /// What it crashed on, once it has.
    pub fn lost(&self) -> Option<Lost> {
        self.shared.lost.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Sends a request that needs no answer. One the process is gone for
    /// is dropped: the listening thread has seen to that.
    fn tell(&self, request: &Request) {
        if !self.gone() {
            let _ = wire::send(self.socket.as_fd(), &request.encode(), None);
        }
    }

    /// Sends a request made with a fresh id, and waits for its answer:
    /// none if the process goes meanwhile, or does not answer in time, in
    /// which case it is ended as hung.
    fn ask(&self, request: impl FnOnce(u64) -> Request) -> Option<Reply> {
        if self.gone() {
            return None;
        }
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let (answer, answered) = mpsc::channel();
        self.shared.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id, answer);
        if wire::send(self.socket.as_fd(), &request(id).encode(), None).is_err() {
            self.shared.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id);
            return None;
        }
        match answered.recv_timeout(ANSWER_WITHIN) {
            Ok(reply) => Some(reply),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                eprintln!("jog: the player did not answer in {ANSWER_WITHIN:?}, and is ended");
                // SAFETY: a signal to the process this player started.
                unsafe { libc::kill(self.pid, libc::SIGKILL) };
                None
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => None,
        }
    }

    /// What is known of a watched property: `None` for one this player
    /// does not watch, which is to be asked for; otherwise its last value
    /// told of, or none when it has none, as while a file is opening.
    fn known(&self, property: &str) -> Option<Option<Known>> {
        let index = watched(property).filter(|_| self.watching)?;
        Some(self.shared.known.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[index].clone())
    }

    /// Before a file is opened: decoded by the GPU unless it crashed the
    /// GPU's decoder before; and noted as the file open, for a crash to be
    /// put down to.
    fn opening(&self, path: &Path) {
        let gpu = self.role.gpu() && HWDEC.load(Ordering::Relaxed) && crashes::crashed_on(path).is_none();
        if gpu != self.gpu.get() {
            self.tell(&Request::Set { name: "hwdec".into(), value: if gpu { "auto-copy" } else { "no" }.into() });
            self.gpu.set(gpu);
        }
        *self.shared.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((path.to_path_buf(), gpu));
    }

    pub fn command(&self, arguments: &[&[u8]]) {
        let args = arguments.iter().map(|argument| argument.to_vec()).collect();
        self.ask(|id| Request::Command { tag: 0, reply: Some(id), args });
    }

    /// Queues a command without waiting for the player to get to it, which
    /// can take a while when it is busy seeking or decoding.
    pub fn command_later(&self, arguments: &[&[u8]]) {
        self.command_tagged(0, arguments);
    }

    /// As `command_later`, its outcome told of by `PlayerEvent::Replied`
    /// with this tag, which is not 0.
    pub fn command_tagged(&self, tag: u64, arguments: &[&[u8]]) {
        self.tell(&Request::Command { tag, reply: None, args: arguments.iter().map(|argument| argument.to_vec()).collect() });
    }

    /// Opens a file in place of whatever was open, without changing whether
    /// the player is paused or waiting for it to load.
    pub fn open(&self, path: &Path) {
        self.opening(path);
        self.tell(&Request::Open { path: path.as_os_str().as_bytes().to_vec() });
    }

    /// Starts playing a file, replacing whatever was playing, from the
    /// beginning or from a number of seconds in; or, `paused`, opens it on
    /// that frame and stays there.
    pub fn play(&self, path: &Path, from: Option<f64>, paused: bool) {
        self.opening(path);
        self.tell(&Request::Play { path: path.as_os_str().as_bytes().to_vec(), from, paused });
    }

    /// Opens a file and stops on the frame it would start at, so that it
    /// can be played at once when its turn comes. Does not wait for it.
    pub fn hold(&self, path: &Path, from: Option<f64>) {
        // Held for later, its frames are not drawn ahead until it plays.
        self.draw_ahead(None);
        self.opening(path);
        self.tell(&Request::Hold { path: path.as_os_str().as_bytes().to_vec(), from });
    }

    /// The tracks and delays the next file opened is to start with, as
    /// kept for it from before; otherwise it gets those the player would
    /// pick by itself.
    pub fn choose_next(&self, aid: &str, sid: &str, sub_delay: f64, audio_delay: f64) {
        self.tell(&Request::ChooseNext { aid: aid.into(), sid: sid.into(), sub_delay, audio_delay });
    }

    /// Whether the file has been opened and its frame is there to be shown.
    pub fn holds(&self, path: &Path) -> bool {
        let open = self.text("path").is_some_and(|open| open.as_bytes() == path.as_os_str().as_bytes());
        open && self.flag("seeking") == Some(false) && self.flag("vo-configured") == Some(true)
    }

    pub fn stop(&self) {
        self.draw_ahead(None);
        *self.shared.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.tell(&Request::Stop);
    }

    /// Sets the filters the viewer wants, as mpv's `vf` would take them.
    /// They go after the one the player puts in by itself for HDR.
    pub fn set_filter(&self, filter: &str) {
        self.tell(&Request::SetFilter(filter.into()));
    }

    /// Sets a property, without waiting for the player to get to it. The
    /// settings reach it in the order they are made, along with the
    /// commands.
    pub fn set(&self, property: &str, value: &str) {
        self.tell(&Request::Set { name: property.into(), value: value.into() });
    }

    fn get(&self, property: &str, kind: Kind) -> Option<Known> {
        let started = Instant::now();
        let reply = self.ask(|id| Request::Get { id, name: property.into(), kind });
        if started.elapsed() > Duration::from_millis(2) && DEBUG.load(Ordering::Relaxed) {
            eprintln!("jog: {:>7.1?} mpv   waited for {property}", started.elapsed());
        }
        match reply {
            Some(Reply::Value(value)) => value,
            _ => None,
        }
    }

    pub fn number(&self, property: &str) -> Option<f64> {
        if let Some(known) = self.known(property) {
            return match known {
                Some(Known::Number(value)) => Some(value),
                _ => None,
            };
        }
        self.number_now(property)
    }

    /// A number asked of the player now, not as it was last told of: for
    /// waiting on it, when the events that would tell are not being read.
    pub fn number_now(&self, property: &str) -> Option<f64> {
        match self.get(property, Kind::Number) {
            Some(Known::Number(value)) => Some(value),
            _ => None,
        }
    }

    pub fn flag(&self, property: &str) -> Option<bool> {
        if let Some(known) = self.known(property) {
            return match known {
                Some(Known::Flag(value)) => Some(value),
                _ => None,
            };
        }
        match self.get(property, Kind::Flag) {
            Some(Known::Flag(value)) => Some(value),
            _ => None,
        }
    }

    pub fn text(&self, property: &str) -> Option<String> {
        if let Some(known) = self.known(property) {
            return match known {
                Some(Known::Text(value)) => Some(value),
                _ => None,
            };
        }
        match self.get(property, Kind::Text) {
            Some(Known::Text(value)) => Some(value),
            _ => None,
        }
    }

    /// The events told of since this was last asked. Call after
    /// `Signal::Events`.
    pub fn events(&self) -> Vec<PlayerEvent> {
        std::mem::take(&mut *self.shared.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Whether a frame not yet drawn is waiting: the player also asks to
    /// be drawn again for other reasons, with the frame it had before.
    /// Drawing ahead, a frame drawn and not yet copied out.
    pub fn frame_waiting(&self) -> bool {
        if let Some((width, height)) = self.ahead.get() {
            let states = self.states.borrow();
            let newest = states.as_ref().and_then(|states| wire::newest(states, width, height)).map(|(_, count)| count);
            return newest.is_some_and(|count| Some(count) != self.taken.get());
        }
        matches!(self.ask(|id| Request::FrameWaiting { id }), Some(Reply::Yes(true)))
    }

    /// Has each frame drawn at `size` as soon as it is ready, into slots
    /// shared with this process, or with `None` no longer.
    fn draw_ahead(&self, size: Option<(u32, u32)>) {
        if self.ahead.get() == size || self.gone() {
            return;
        }
        if let Some((width, height)) = size {
            let wanted = width as usize * height as usize;
            // The states' block once; the slots made anew only to grow,
            // with room to spare, as the buffer is.
            if self.states.borrow().is_none() {
                let Ok(states) = Mapped::create(wire::SLOTS * 4) else { return };
                if wire::send(self.socket.as_fd(), &Request::Slots.encode(), Some(states.fd())).is_err() {
                    return;
                }
                *self.states.borrow_mut() = Some(states);
            }
            let mut slots = self.slots.borrow_mut();
            if slots.len() < wire::SLOTS || slots.iter().any(|slot| slot.pixels() < wanted) {
                let room = wanted + wanted / 4;
                let Ok(made) = (0..wire::SLOTS).map(|_| Mapped::create(room)).collect::<std::io::Result<Vec<Mapped>>>() else { return };
                for (index, slot) in made.iter().enumerate() {
                    if wire::send(self.socket.as_fd(), &Request::Slot { index: index as u8, pixels: room as u64 }.encode(), Some(slot.fd())).is_err() {
                        return;
                    }
                }
                // Frames drawn at the old size are of no use.
                if let Some(states) = self.states.borrow().as_ref() {
                    for slot in 0..wire::SLOTS {
                        wire::drawn(states, slot, None, 0);
                    }
                }
                *slots = made;
            }
        }
        let (width, height) = size.unwrap_or((0, 0));
        self.tell(&Request::Ahead { width, height });
        self.ahead.set(size);
        self.taken.set(None);
    }

    /// As `render`, for a player whose every frame is wanted at this size,
    /// as the video on view is: each is drawn in the player process as
    /// soon as it is ready, and only copied in here. The first at a size
    /// is drawn as `render` draws it, while the slots are set up for the
    /// next.
    pub fn render_latest(&self, pixels: &mut [u32], width: u32, height: u32) -> bool {
        let wanted = width as usize * height as usize;
        if wanted == 0 || pixels.len() < wanted || self.gone() {
            return false;
        }
        if self.ahead.get() != Some((width, height)) {
            self.draw_ahead(Some((width, height)));
            return self.render(pixels, width, height);
        }
        let states = self.states.borrow();
        let Some(states) = states.as_ref() else { return false };
        let Some((slot, count)) = wire::claim_newest(states, width, height) else {
            // None drawn yet at this size: drawn now.
            return self.render(pixels, width, height);
        };
        if let Some(buffer) = self.slots.borrow().get(slot) {
            crate::ui::copy_into(&mut pixels[..wanted], &buffer.as_slice()[..wanted]);
        }
        wire::give_back(states, slot);
        self.taken.set(Some(count));
        true
    }

    /// Draws the current frame, scaled and letterboxed to fill a
    /// `width` x `height` buffer of `0RGB` pixels. The player process
    /// draws it into the memory it shares with this one, which is then
    /// copied in.
    pub fn render(&self, pixels: &mut [u32], width: u32, height: u32) -> bool {
        let wanted = width as usize * height as usize;
        if wanted == 0 || pixels.len() < wanted || self.gone() {
            return false;
        }
        let mut buffer = self.buffer.borrow_mut();
        // Shared memory as large as the frame: made anew only to grow, and
        // then with room to spare, so that a window being made larger does
        // not make it anew at every step.
        if buffer.as_ref().is_none_or(|buffer| buffer.pixels() < wanted) {
            let room = wanted + wanted / 4;
            let Ok(made) = Mapped::create(room) else { return false };
            if wire::send(self.socket.as_fd(), &Request::Buffer { pixels: room as u64 }.encode(), Some(made.fd())).is_err() {
                return false;
            }
            *buffer = Some(made);
        }
        let drawn = matches!(self.ask(|id| Request::Render { id, width, height }), Some(Reply::Yes(true)));
        if drawn {
            if let Some(buffer) = buffer.as_ref() {
                crate::ui::copy_into(&mut pixels[..wanted], &buffer.as_slice()[..wanted]);
            }
        }
        drawn
    }

    /// Waits for a draw ahead to finish.
    pub fn settle(&self) {
        self.ask(|id| Request::Settle { id });
    }

    /// Whether a draw ahead is going on: a frame taken only to keep the
    /// player moving can be left until it is done.
    pub fn warming(&self) -> bool {
        matches!(self.ask(|id| Request::Warming { id }), Some(Reply::Yes(true)))
    }

    /// Has the player draw the frame it holds at `width` x `height`, into
    /// nothing, on a thread of its own: the first draw at a size sets the
    /// conversion up, 40-100 ms for an HDR video on four cores, which
    /// would otherwise be the first frame's when this player takes over.
    /// Returns once it has begun; drawing on this player waits for it.
    pub fn warm(&self, width: u32, height: u32) {
        self.ask(|id| Request::Warm { id, width, height });
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // It goes at once when asked; the listening thread sees it go and
        // takes back what is left of it.
        self.shared.quitting.store(true, Ordering::SeqCst);
        self.tell(&Request::Quit);
    }
}

/// Sets how long a read on the socket may wait: `None` for as long as it
/// takes.
fn set_timeout(socket: std::os::fd::BorrowedFd, within: Option<Duration>) {
    let within = within.unwrap_or_default();
    let value = libc::timeval { tv_sec: within.as_secs() as libc::time_t, tv_usec: within.subsec_micros() as libc::suseconds_t };
    // SAFETY: setsockopt with a timeval of the size it is told.
    unsafe {
        libc::setsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&value as *const libc::timeval).cast(), std::mem::size_of::<libc::timeval>() as libc::socklen_t);
    }
}

/// The listening thread: passes on what the player process says, and
/// sees it go.
fn listen(socket: OwnedFd, mut child: std::process::Child, shared: Arc<Shared>, wake: impl Fn(Signal)) {
    let mut buffer = vec![0; wire::MAX];
    loop {
        let message = match wire::recv(socket.as_fd(), &mut buffer) {
            Ok(Some((n, _))) => Notice::decode(&buffer[..n]),
            _ => break,
        };
        match message {
            Some(Notice::Events { events, changed }) => {
                {
                    let mut known = shared.known.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    for (index, value) in changed {
                        if let Some(slot) = known.get_mut(index as usize) {
                            *slot = value;
                        }
                    }
                }
                shared.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(events);
                wake(Signal::Events);
            }
            Some(Notice::Frame) => wake(Signal::Frame),
            Some(Notice::Reply { id, reply }) => {
                if let Some(answer) = shared.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id) {
                    let _ = answer.send(reply);
                }
            }
            Some(Notice::Ready | Notice::Failed(_)) => {}
            None => {
                eprintln!("jog: the player said something that cannot be read, and is ended");
                let _ = child.kill();
                break;
            }
        }
    }
    // Gone: whoever waits for an answer has none.
    shared.gone.store(true, Ordering::SeqCst);
    shared.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    let status = child.wait();
    if shared.quitting.load(Ordering::SeqCst) {
        return;
    }
    let how = match &status {
        Ok(status) => match status.signal() {
            Some(signal) => format!("signal {signal} ({})", signal_name(signal)),
            None => format!("exit {}", status.code().unwrap_or(-1)),
        },
        Err(error) => error.to_string(),
    };
    let open = shared.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let crash = open.as_ref().map(|(path, gpu)| crashes::note(path, *gpu));
    match &open {
        Some((path, gpu)) => eprintln!("jog: the player crashed ({how}) on {}, decoded by the {}", path.display(), if *gpu { "GPU" } else { "CPU" }),
        None => eprintln!("jog: the player crashed ({how}) with nothing open"),
    }
    *shared.lost.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Lost { path: open.map(|(path, _)| path), crash });
    shared.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(PlayerEvent::Crashed);
    wake(Signal::Events);
}

fn signal_name(signal: i32) -> &'static str {
    match signal {
        libc::SIGSEGV => "segmentation fault",
        libc::SIGABRT => "aborted",
        libc::SIGBUS => "bus error",
        libc::SIGILL => "illegal instruction",
        libc::SIGFPE => "arithmetic fault",
        libc::SIGKILL => "killed",
        _ => "signal",
    }
}
