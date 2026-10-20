//! Between jog's process and a player process: a socket of whole messages
//! (`SOCK_SEQPACKET`, which keeps each message whole and in order, and says
//! when the other end has gone), what the messages are, and the shared
//! memory a frame is drawn into, handed over the socket as a descriptor.
//!
//! Both ends are the same jog binary, so the messages are laid out by hand
//! and need no versioning: the player process is always started from the
//! running executable (`/proc/self/exe`), even if a newer jog has been
//! installed over it since.

use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use super::{Kind, Known, PlayerEvent};

/// The most a message may be: a path, a command's arguments, a batch of
/// events. Frames go through shared memory, not this.
pub const MAX: usize = 1 << 17;

/// A connected pair of sockets: one for each end.
pub fn pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds: [RawFd; 2] = [0; 2];
    // SAFETY: socketpair fills the two descriptors it is given room for.
    let made = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) };
    if made < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just made, and are owned by nothing else.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Room for the control message that carries one descriptor, aligned as
/// the kernel lays control messages out.
#[repr(C, align(8))]
struct Control([u8; 64]);

/// Sends one message, and with it `pass`, a descriptor for the other end
/// to have too. Never raises SIGPIPE: a gone end is an error.
pub fn send(socket: BorrowedFd, bytes: &[u8], pass: Option<BorrowedFd>) -> io::Result<()> {
    debug_assert!(!bytes.is_empty() && bytes.len() <= MAX);
    let mut iov = libc::iovec { iov_base: bytes.as_ptr() as *mut libc::c_void, iov_len: bytes.len() };
    let mut control = Control([0; 64]);
    // SAFETY: a zeroed msghdr is a valid empty one; every pointer put in it
    // points at storage that outlives the call; the control message is
    // written within `control`, whose size CMSG_SPACE stays under.
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if let Some(pass) = pass {
            let space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
            message.msg_control = control.0.as_mut_ptr().cast();
            message.msg_controllen = space;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
            std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), pass.as_raw_fd());
        }
        loop {
            if libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) >= 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// Waits for one message, into `buffer`; its length, and the descriptor
/// that came with it if one did. `None` when the other end has gone.
pub fn recv(socket: BorrowedFd, buffer: &mut [u8]) -> io::Result<Option<(usize, Option<OwnedFd>)>> {
    let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
    let mut control = Control([0; 64]);
    // SAFETY: as in `send`; what the kernel writes stays within `buffer`
    // and `control`, and a descriptor it passes is ours to own.
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.0.as_mut_ptr().cast();
        message.msg_controllen = control.0.len();
        let got = loop {
            let got = libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC);
            if got >= 0 {
                break got as usize;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        };
        let mut passed = None;
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let fd = std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<RawFd>());
                passed = Some(OwnedFd::from_raw_fd(fd));
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
        // Every message has something in it: nothing is the end gone.
        if got == 0 {
            return Ok(None);
        }
        if message.msg_flags & libc::MSG_TRUNC != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message larger than the buffer"));
        }
        Ok(Some((got, passed)))
    }
}

/// Memory shared between the two processes: a frame's pixels, drawn into
/// by the player process and read by jog's.
pub struct Mapped {
    fd: OwnedFd,
    pointer: *mut u32,
    pixels: usize,
}

// SAFETY: the mapping is plain memory, owned by this value alone in its
// process, and freed once, on drop.
unsafe impl Send for Mapped {}

impl Mapped {
    /// New shared memory for `pixels` pixels.
    pub fn create(pixels: usize) -> io::Result<Mapped> {
        // SAFETY: memfd_create takes a NUL-terminated name and flags.
        let fd = unsafe { libc::memfd_create(c"jog-frame".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: just made, owned by nothing else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: a plain system call on a descriptor we own.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), (pixels * 4) as libc::off_t) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Mapped::map(fd, pixels)
    }

    /// Maps memory the other process made, sent over the socket.
    pub fn map(fd: OwnedFd, pixels: usize) -> io::Result<Mapped> {
        if pixels == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no pixels"));
        }
        // SAFETY: maps `pixels * 4` bytes of a descriptor at least that
        // large, shared, and checked for failure.
        let pointer = unsafe { libc::mmap(std::ptr::null_mut(), pixels * 4, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if pointer == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapped { fd, pointer: pointer.cast(), pixels })
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub fn pixels(&self) -> usize {
        self.pixels
    }

    pub fn as_slice(&self) -> &[u32] {
        // SAFETY: mapped for `pixels` pixels until drop. The other process
        // writes into it only while this one waits for it to be done.
        unsafe { std::slice::from_raw_parts(self.pointer, self.pixels) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u32] {
        // SAFETY: as in `as_slice`.
        unsafe { std::slice::from_raw_parts_mut(self.pointer, self.pixels) }
    }
}

/// The frames drawn ahead: the player process draws each frame mpv has
/// ready into one of `SLOTS` shared buffers as soon as it is ready, and
/// jog's main thread only copies out the newest, rather than waiting on
/// the drawing. Which buffer holds what, and who has it, is kept in a
/// small block of shared memory both processes change atomically: for
/// each slot its state, the width and height drawn at, and a count that
/// says which frame is newest.
pub const SLOTS: usize = 3;
/// Nobody has it: the player process may draw into it.
pub const FREE: u32 = 0;
/// The player process is drawing into it.
pub const WRITING: u32 = 1;
/// It holds a whole frame; drawn over only when no slot is free.
pub const READY: u32 = 2;
/// jog's process is copying it out.
pub const READING: u32 = 3;

/// What each slot's four numbers are, in order.
pub const STATE: usize = 0;
pub const WIDTH: usize = 1;
pub const HEIGHT: usize = 2;
pub const COUNT: usize = 3;

impl Mapped {
    /// One of the numbers kept for `slot`, in shared memory made for the
    /// slots' states, to be changed atomically by either process.
    pub fn slot(&self, slot: usize, which: usize) -> &std::sync::atomic::AtomicU32 {
        let at = slot * 4 + which;
        assert!(at < self.pixels);
        // SAFETY: within the mapping; an AtomicU32 has a u32's layout, and
        // the memory is only ever changed through atomics, by both ends.
        unsafe { &*self.pointer.add(at).cast::<std::sync::atomic::AtomicU32>() }
    }
}

/// The player process's side: a slot to draw the next frame into. A free
/// one, or failing that the oldest whole frame, which is the least
/// likely to be wanted. None only while jog's process copies one out and
/// the others are being drawn, which with three slots does not happen.
pub fn claim_to_draw(states: &Mapped) -> Option<usize> {
    use std::sync::atomic::Ordering::AcqRel;
    let take = |slot: usize, from: u32| states.slot(slot, STATE).compare_exchange(from, WRITING, AcqRel, std::sync::atomic::Ordering::Acquire).is_ok();
    if let Some(slot) = (0..SLOTS).find(|slot| take(*slot, FREE)) {
        return Some(slot);
    }
    let mut ready: Vec<usize> = (0..SLOTS).filter(|slot| states.slot(*slot, STATE).load(std::sync::atomic::Ordering::Acquire) == READY).collect();
    ready.sort_by_key(|slot| states.slot(*slot, COUNT).load(std::sync::atomic::Ordering::Acquire));
    ready.into_iter().find(|slot| take(*slot, READY))
}

/// The player process's side: the slot drawn into, as a whole frame of
/// this size, the `count`th; or let go, undrawn.
pub fn drawn(states: &Mapped, slot: usize, size: Option<(u32, u32)>, count: u32) {
    use std::sync::atomic::Ordering::{Relaxed, Release};
    match size {
        Some((width, height)) => {
            states.slot(slot, WIDTH).store(width, Relaxed);
            states.slot(slot, HEIGHT).store(height, Relaxed);
            states.slot(slot, COUNT).store(count, Relaxed);
            // Last, and released: whoever sees it ready sees the rest.
            states.slot(slot, STATE).store(READY, Release);
        }
        None => states.slot(slot, STATE).store(FREE, Release),
    }
}

/// jog's side: the count of the newest whole frame of this size, if any.
pub fn newest(states: &Mapped, width: u32, height: u32) -> Option<(usize, u32)> {
    use std::sync::atomic::Ordering::{Acquire, Relaxed};
    (0..SLOTS)
        .filter(|slot| states.slot(*slot, STATE).load(Acquire) == READY)
        .filter(|slot| states.slot(*slot, WIDTH).load(Relaxed) == width && states.slot(*slot, HEIGHT).load(Relaxed) == height)
        .map(|slot| (slot, states.slot(slot, COUNT).load(Relaxed)))
        .max_by_key(|(_, count)| *count)
}

/// jog's side: holds the newest whole frame of this size while it is
/// copied out, so that it is not drawn over meanwhile; its slot and count.
/// `give_back` it after.
pub fn claim_newest(states: &Mapped, width: u32, height: u32) -> Option<(usize, u32)> {
    use std::sync::atomic::Ordering::{AcqRel, Acquire};
    // Drawn over between being found and being taken: the next newest.
    for _ in 0..SLOTS {
        let (slot, count) = newest(states, width, height)?;
        if states.slot(slot, STATE).compare_exchange(READY, READING, AcqRel, Acquire).is_ok() {
            return Some((slot, count));
        }
    }
    None
}

/// jog's side: done copying the slot out. It stays a whole frame, to be
/// drawn over when it is the oldest.
pub fn give_back(states: &Mapped, slot: usize) {
    states.slot(slot, STATE).store(READY, std::sync::atomic::Ordering::Release);
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what `map` mapped, once.
        unsafe { libc::munmap(self.pointer.cast(), self.pixels * 4) };
    }
}

/// What jog's process asks of a player process.
#[derive(Clone, PartialEq, Debug)]
pub enum Request {
    /// A command for mpv; `tag` is told of in `PlayerEvent::Replied` when
    /// not 0. With `reply`, answered with `Reply::Done` once mpv has done it.
    Command { tag: u64, reply: Option<u64>, args: Vec<Vec<u8>> },
    Set { name: String, value: String },
    SetFilter(String),
    ChooseNext { aid: String, sid: String, sub_delay: f64, audio_delay: f64 },
    Play { path: Vec<u8>, from: Option<f64>, paused: bool },
    Hold { path: Vec<u8>, from: Option<f64> },
    Open { path: Vec<u8> },
    Stop,
    /// A property asked for now; answered with `Reply::Value`.
    Get { id: u64, name: String, kind: Kind },
    /// Answered with `Reply::Yes`.
    FrameWaiting { id: u64 },
    /// The shared memory to draw into from now on, of `pixels` pixels; its
    /// descriptor comes with the message.
    Buffer { pixels: u64 },
    /// The frame drawn into the shared memory; answered with `Reply::Yes`.
    Render { id: u64, width: u32, height: u32 },
    /// Answered with `Reply::Done` once the draw ahead has begun.
    Warm { id: u64, width: u32, height: u32 },
    /// Answered with `Reply::Yes`.
    Warming { id: u64 },
    /// Answered with `Reply::Done` once a draw ahead is over.
    Settle { id: u64 },
    /// The shared block of the slots' states; its descriptor comes with it.
    Slots,
    /// Slot `index`'s buffer from now on, of `pixels` pixels; its
    /// descriptor comes with it.
    Slot { index: u8, pixels: u64 },
    /// Each frame drawn into a slot as soon as it is ready, at this size;
    /// 0 by 0 for none, each drawn only when asked for.
    Ahead { width: u32, height: u32 },
    Quit,
}

/// What a player process tells jog's.
#[derive(Clone, PartialEq, Debug)]
pub enum Notice {
    /// It has started mpv, and is ready.
    Ready,
    /// It could not start mpv, and why: it goes after saying so.
    Failed(String),
    /// What happened, and the watched properties that changed with it, by
    /// their place in `WATCHED`.
    Events { events: Vec<PlayerEvent>, changed: Vec<(u8, Option<Known>)> },
    /// A new frame is due.
    Frame,
    Reply { id: u64, reply: Reply },
}

/// The answer to a request that waits for one.
#[derive(Clone, PartialEq, Debug)]
pub enum Reply {
    Done,
    Yes(bool),
    Value(Option<Known>),
}

/// A message being laid out.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, value: u8) -> &mut Self {
        self.0.push(value);
        self
    }
    fn u32(&mut self, value: u32) -> &mut Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }
    fn u64(&mut self, value: u64) -> &mut Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }
    fn f64(&mut self, value: f64) -> &mut Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }
    fn bool(&mut self, value: bool) -> &mut Self {
        self.u8(u8::from(value))
    }
    fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.u32(value.len() as u32);
        self.0.extend_from_slice(value);
        self
    }
    fn text(&mut self, value: &str) -> &mut Self {
        self.bytes(value.as_bytes())
    }
    fn maybe_f64(&mut self, value: Option<f64>) -> &mut Self {
        match value {
            Some(value) => self.u8(1).f64(value),
            None => self.u8(0),
        }
    }
    fn kind(&mut self, kind: Kind) -> &mut Self {
        self.u8(match kind {
            Kind::Flag => 0,
            Kind::Number => 1,
            Kind::Text => 2,
        })
    }
    fn known(&mut self, value: &Option<Known>) -> &mut Self {
        match value {
            None => self.u8(0),
            Some(Known::Flag(flag)) => self.u8(1).bool(*flag),
            Some(Known::Number(number)) => self.u8(2).f64(*number),
            Some(Known::Text(text)) => self.u8(3).text(text),
        }
    }
    fn event(&mut self, event: &PlayerEvent) -> &mut Self {
        match event {
            PlayerEvent::Started => self.u8(0),
            PlayerEvent::Showing => self.u8(1),
            PlayerEvent::Ended => self.u8(2),
            PlayerEvent::AtEnd => self.u8(3),
            PlayerEvent::Changed => self.u8(4),
            PlayerEvent::Moved => self.u8(5),
            PlayerEvent::Replied(tag, ok) => self.u8(6).u64(*tag).bool(*ok),
            PlayerEvent::Crashed => self.u8(7),
        }
    }
}

/// A message being read. Every read says `None` past its end or on a
/// value it does not know, which is a broken message.
struct In<'a> {
    bytes: &'a [u8],
}

impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.bytes.len() < n {
            return None;
        }
        let (taken, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Some(taken)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        Some(self.take(n)?.to_vec())
    }
    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }
    fn maybe_f64(&mut self) -> Option<Option<f64>> {
        match self.u8()? {
            0 => Some(None),
            1 => Some(Some(self.f64()?)),
            _ => None,
        }
    }
    fn kind(&mut self) -> Option<Kind> {
        match self.u8()? {
            0 => Some(Kind::Flag),
            1 => Some(Kind::Number),
            2 => Some(Kind::Text),
            _ => None,
        }
    }
    fn known(&mut self) -> Option<Option<Known>> {
        match self.u8()? {
            0 => Some(None),
            1 => Some(Some(Known::Flag(self.bool()?))),
            2 => Some(Some(Known::Number(self.f64()?))),
            3 => Some(Some(Known::Text(self.text()?))),
            _ => None,
        }
    }
    fn event(&mut self) -> Option<PlayerEvent> {
        match self.u8()? {
            0 => Some(PlayerEvent::Started),
            1 => Some(PlayerEvent::Showing),
            2 => Some(PlayerEvent::Ended),
            3 => Some(PlayerEvent::AtEnd),
            4 => Some(PlayerEvent::Changed),
            5 => Some(PlayerEvent::Moved),
            6 => Some(PlayerEvent::Replied(self.u64()?, self.bool()?)),
            7 => Some(PlayerEvent::Crashed),
            _ => None,
        }
    }
    /// The whole message read, nothing left over.
    fn end<T>(self, value: T) -> Option<T> {
        self.bytes.is_empty().then_some(value)
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Out::default();
        match self {
            Request::Command { tag, reply, args } => {
                out.u8(0).u64(*tag).u64(reply.map_or(0, |id| id + 1)).u32(args.len() as u32);
                for arg in args {
                    out.bytes(arg);
                }
            }
            Request::Set { name, value } => {
                out.u8(1).text(name).text(value);
            }
            Request::SetFilter(filter) => {
                out.u8(2).text(filter);
            }
            Request::ChooseNext { aid, sid, sub_delay, audio_delay } => {
                out.u8(3).text(aid).text(sid).f64(*sub_delay).f64(*audio_delay);
            }
            Request::Play { path, from, paused } => {
                out.u8(4).bytes(path).maybe_f64(*from).bool(*paused);
            }
            Request::Hold { path, from } => {
                out.u8(5).bytes(path).maybe_f64(*from);
            }
            Request::Open { path } => {
                out.u8(6).bytes(path);
            }
            Request::Stop => {
                out.u8(7);
            }
            Request::Get { id, name, kind } => {
                out.u8(8).u64(*id).text(name).kind(*kind);
            }
            Request::FrameWaiting { id } => {
                out.u8(9).u64(*id);
            }
            Request::Buffer { pixels } => {
                out.u8(10).u64(*pixels);
            }
            Request::Render { id, width, height } => {
                out.u8(11).u64(*id).u32(*width).u32(*height);
            }
            Request::Warm { id, width, height } => {
                out.u8(12).u64(*id).u32(*width).u32(*height);
            }
            Request::Warming { id } => {
                out.u8(13).u64(*id);
            }
            Request::Settle { id } => {
                out.u8(14).u64(*id);
            }
            Request::Quit => {
                out.u8(15);
            }
            Request::Slots => {
                out.u8(16);
            }
            Request::Slot { index, pixels } => {
                out.u8(17).u8(*index).u64(*pixels);
            }
            Request::Ahead { width, height } => {
                out.u8(18).u32(*width).u32(*height);
            }
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Option<Request> {
        let mut input = In { bytes };
        let request = match input.u8()? {
            0 => {
                let (tag, reply, count) = (input.u64()?, input.u64()?, input.u32()?);
                let args = (0..count).map(|_| input.bytes()).collect::<Option<Vec<_>>>()?;
                Request::Command { tag, reply: reply.checked_sub(1), args }
            }
            1 => Request::Set { name: input.text()?, value: input.text()? },
            2 => Request::SetFilter(input.text()?),
            3 => Request::ChooseNext { aid: input.text()?, sid: input.text()?, sub_delay: input.f64()?, audio_delay: input.f64()? },
            4 => Request::Play { path: input.bytes()?, from: input.maybe_f64()?, paused: input.bool()? },
            5 => Request::Hold { path: input.bytes()?, from: input.maybe_f64()? },
            6 => Request::Open { path: input.bytes()? },
            7 => Request::Stop,
            8 => Request::Get { id: input.u64()?, name: input.text()?, kind: input.kind()? },
            9 => Request::FrameWaiting { id: input.u64()? },
            10 => Request::Buffer { pixels: input.u64()? },
            11 => Request::Render { id: input.u64()?, width: input.u32()?, height: input.u32()? },
            12 => Request::Warm { id: input.u64()?, width: input.u32()?, height: input.u32()? },
            13 => Request::Warming { id: input.u64()? },
            14 => Request::Settle { id: input.u64()? },
            15 => Request::Quit,
            16 => Request::Slots,
            17 => Request::Slot { index: input.u8()?, pixels: input.u64()? },
            18 => Request::Ahead { width: input.u32()?, height: input.u32()? },
            _ => return None,
        };
        input.end(request)
    }
}

impl Notice {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Out::default();
        match self {
            Notice::Ready => {
                out.u8(0);
            }
            Notice::Failed(why) => {
                out.u8(1).text(why);
            }
            Notice::Events { events, changed } => {
                out.u8(2).u32(events.len() as u32);
                for event in events {
                    out.event(event);
                }
                out.u32(changed.len() as u32);
                for (index, value) in changed {
                    out.u8(*index).known(value);
                }
            }
            Notice::Frame => {
                out.u8(3);
            }
            Notice::Reply { id, reply } => {
                out.u8(4).u64(*id);
                match reply {
                    Reply::Done => out.u8(0),
                    Reply::Yes(yes) => out.u8(1).bool(*yes),
                    Reply::Value(value) => out.u8(2).known(value),
                };
            }
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Option<Notice> {
        let mut input = In { bytes };
        let notice = match input.u8()? {
            0 => Notice::Ready,
            1 => Notice::Failed(input.text()?),
            2 => {
                let count = input.u32()?;
                let events = (0..count).map(|_| input.event()).collect::<Option<Vec<_>>>()?;
                let count = input.u32()?;
                let changed = (0..count).map(|_| Some((input.u8()?, input.known()?))).collect::<Option<Vec<_>>>()?;
                Notice::Events { events, changed }
            }
            3 => Notice::Frame,
            4 => {
                let id = input.u64()?;
                let reply = match input.u8()? {
                    0 => Reply::Done,
                    1 => Reply::Yes(input.bool()?),
                    2 => Reply::Value(input.known()?),
                    _ => return None,
                };
                Notice::Reply { id, reply }
            }
            _ => return None,
        };
        input.end(notice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_as_written() {
        let requests = [
            Request::Command { tag: 7, reply: Some(0), args: vec![b"loadfile".to_vec(), b"/a b/\xffc.mkv".to_vec()] },
            Request::Command { tag: 0, reply: None, args: vec![] },
            Request::Set { name: "pause".into(), value: "yes".into() },
            Request::SetFilter("lavfi=[hflip]".into()),
            Request::ChooseNext { aid: "2".into(), sid: "no".into(), sub_delay: -0.5, audio_delay: 0.25 },
            Request::Play { path: b"/v.mp4".to_vec(), from: Some(12.5), paused: true },
            Request::Hold { path: b"/v.mp4".to_vec(), from: None },
            Request::Open { path: b"/v.mp4".to_vec() },
            Request::Stop,
            Request::Get { id: 3, name: "track-list".into(), kind: Kind::Text },
            Request::FrameWaiting { id: u64::MAX },
            Request::Buffer { pixels: 3840 * 2160 },
            Request::Render { id: 1, width: 1920, height: 1080 },
            Request::Warm { id: 2, width: 8, height: 8 },
            Request::Warming { id: 4 },
            Request::Settle { id: 5 },
            Request::Slots,
            Request::Slot { index: 2, pixels: 1920 * 1080 },
            Request::Ahead { width: 0, height: 0 },
            Request::Quit,
        ];
        for request in requests {
            assert_eq!(Request::decode(&request.encode()), Some(request.clone()), "{request:?}");
        }
    }

    #[test]
    fn notices_read_as_written() {
        let notices = [
            Notice::Ready,
            Notice::Failed("libmpv is not installed".into()),
            Notice::Events { events: vec![PlayerEvent::Started, PlayerEvent::Replied(9, false), PlayerEvent::Moved], changed: vec![(1, Some(Known::Number(3.5))), (6, Some(Known::Text("/v.mp4".into()))), (3, Some(Known::Flag(true))), (2, None)] },
            Notice::Frame,
            Notice::Reply { id: 1, reply: Reply::Done },
            Notice::Reply { id: 2, reply: Reply::Yes(true) },
            Notice::Reply { id: 3, reply: Reply::Value(None) },
        ];
        for notice in notices {
            assert_eq!(Notice::decode(&notice.encode()), Some(notice.clone()), "{notice:?}");
        }
    }

    #[test]
    fn broken_messages_are_refused() {
        let whole = Request::Play { path: b"/v.mp4".to_vec(), from: Some(1.0), paused: false }.encode();
        for cut in 0..whole.len() {
            assert_eq!(Request::decode(&whole[..cut]), None, "cut at {cut}");
        }
        let mut longer = whole.clone();
        longer.push(0);
        assert_eq!(Request::decode(&longer), None);
        assert_eq!(Notice::decode(&[200]), None);
    }

    #[test]
    fn the_newest_frame_is_taken_and_a_frame_being_read_is_not_drawn_over() {
        let states = Mapped::create(SLOTS * 4).unwrap();
        // Three frames drawn: each into a free slot.
        for count in 1..=3 {
            let slot = claim_to_draw(&states).unwrap();
            drawn(&states, slot, Some((1920, 1080)), count);
        }
        assert_eq!(newest(&states, 1920, 1080).map(|(_, count)| count), Some(3));
        assert_eq!(newest(&states, 1280, 720), None, "a frame of another size is not this one");
        // jog takes the newest; the next frame goes over the oldest, not it.
        let (reading, count) = claim_newest(&states, 1920, 1080).unwrap();
        assert_eq!(count, 3);
        let slot = claim_to_draw(&states).unwrap();
        assert_ne!(slot, reading);
        assert_eq!(states.slot(slot, COUNT).load(std::sync::atomic::Ordering::Relaxed), 1, "the oldest is drawn over");
        drawn(&states, slot, Some((1920, 1080)), 4);
        give_back(&states, reading);
        assert_eq!(newest(&states, 1920, 1080).map(|(_, count)| count), Some(4));
        // A draw that came to nothing leaves the slot free.
        let slot = claim_to_draw(&states).unwrap();
        drawn(&states, slot, None, 0);
        assert_eq!(states.slot(slot, STATE).load(std::sync::atomic::Ordering::Relaxed), FREE);
    }

    #[test]
    fn a_message_and_a_descriptor_go_over_the_socket() {
        let (a, b) = pair().unwrap();
        let mut shared = Mapped::create(16).unwrap();
        shared.as_mut_slice()[5] = 0x00AB_CDEF;
        send(a.as_fd(), &Request::Buffer { pixels: 16 }.encode(), Some(shared.fd())).unwrap();
        let mut buffer = vec![0; MAX];
        let (n, fd) = recv(b.as_fd(), &mut buffer).unwrap().unwrap();
        assert_eq!(Request::decode(&buffer[..n]), Some(Request::Buffer { pixels: 16 }));
        let other = Mapped::map(fd.unwrap(), 16).unwrap();
        assert_eq!(other.as_slice()[5], 0x00AB_CDEF, "the same memory, seen from the other end");
        drop(a);
        assert!(recv(b.as_fd(), &mut buffer).unwrap().is_none(), "the end gone reads as none");
    }
}
