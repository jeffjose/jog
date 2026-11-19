//! Thumbnails made on the machine the files are on.
//!
//! Over a mount, a thumbnail of a 13 MB photo costs the 13 MB crossing the
//! network. So for a host jog has mounted, it also starts `ssh host jog
//! --serve`, a jog over there that reads the file where it is and sends back
//! only the thumbnail, a few kilobytes. Measured on skyfall over a 32 MB/s
//! link, the first screen of a grid of 24-megapixel photos: 22.6 s through
//! the mount, 1.5 s for the same files here.
//!
//! It is only ever a shortcut. Started alongside the mount, it is waited on
//! by nothing: until it says hello, and after anything goes wrong with it
//! (no jog over there, an older one, a dropped connection, a reply too
//! slow), thumbnails are read through the mount as they would be without it.
//!
//! The protocol, little-endian, over ssh's stdin and stdout:
//! - hello, from the server: `JOGSERVE` and the protocol's version, `u32`.
//! - request: tag `u64`, width and height wanted `u32`s, quarter turns
//!   `u8`, mirrored `u8`, background `u32`, path length `u32`, path.
//! - reply: tag `u64`, ok `u8`. Ok: native width and height, width and
//!   height, `u32`s, then the count of pictures `u32` and whether they are
//!   an animation `u8`, and each picture as its delay in ms `u32`, length
//!   `u32` and a JPEG. Not ok: length `u32` and the error.

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::decode::{self, Spec};
use crate::frame::{Animation, Frame, Transform};
use crate::ultrahdr::Hdr;

/// The word that starts a server.
pub const SERVE_ARG: &str = "--serve";

const MAGIC: &[u8; 8] = b"JOGSERVE";
/// Raised whenever a request or reply changes shape: a jog over there that
/// speaks another is not used.
const PROTOCOL: u32 = 1;
/// How long the server has to say hello before it is given up on. It is
/// waited on by nothing meanwhile.
const HELLO_WITHIN: Duration = Duration::from_secs(6);
/// How long one thumbnail may take over there before it is read here
/// instead: a video whose still is slow to find, a host that has stalled.
const REPLY_WITHIN: Duration = Duration::from_secs(15);
/// JPEG quality of the thumbnails sent: near enough to the decoded pixels
/// not to tell at thumbnail size, a tenth of their bytes.
const QUALITY: u8 = 90;

const PROBING: u8 = 0;
const READY: u8 = 1;
const DEAD: u8 = 2;

static DEBUG: AtomicBool = AtomicBool::new(false);
static HELPERS: Mutex<Vec<(String, Arc<Helper>)>> = Mutex::new(Vec::new());

/// Says on the terminal how each helper fares, with `--debug`.
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}

type Reply = Result<Frame, String>;

/// A server on one host, as this side knows it.
struct Helper {
    host: String,
    state: AtomicU8,
    input: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    waiting: Mutex<HashMap<u64, mpsc::Sender<Reply>>>,
    next: AtomicU64,
}

impl Helper {
    /// Gives up on it: what is waiting is told so, by its sender going,
    /// and goes to the mount.
    fn die(&self, why: &str) {
        if self.state.swap(DEAD, Ordering::AcqRel) != DEAD && DEBUG.load(Ordering::Relaxed) {
            eprintln!("jog: no thumbnails made on {}: {why}; read through the mount", self.host);
        }
        self.input.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(mut child) = self.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
    }
}

/// Starts a server on the host, unless one is started already, and comes
/// back at once: nothing waits on it.
pub fn start(host: &str, ssh: Vec<String>) {
    let mut helpers = HELPERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if helpers.iter().any(|(known, _)| known == host) || std::env::var_os("JOG_NO_SERVE").is_some() {
        return;
    }
    let helper = Arc::new(Helper {
        host: host.to_string(),
        state: AtomicU8::new(PROBING),
        input: Mutex::new(None),
        child: Mutex::new(None),
        waiting: Mutex::new(HashMap::new()),
        next: AtomicU64::new(0),
    });
    helpers.push((host.to_string(), helper.clone()));
    drop(helpers);
    std::thread::spawn(move || connect(helper, ssh));
}

fn connect(helper: Arc<Helper>, mut ssh: Vec<String>) {
    let started = Instant::now();
    // `JOG_REMOTE` names the jog over there, when it is not `jog` on the
    // PATH there.
    let program = std::env::var("JOG_REMOTE").ok().filter(|text| !text.is_empty()).unwrap_or_else(|| "jog".into());
    let ssh_program = ssh.remove(0);
    let spawned = Command::new(&ssh_program)
        .args(ssh)
        // Never a password prompt over the window: a host that would ask
        // has no helper.
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3"])
        .arg(&helper.host)
        .arg(format!("{program} {SERVE_ARG}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return helper.die(&format!("{ssh_program}: {error}")),
    };
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else { return helper.die("no pipes") };
    *helper.input.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(input);
    *helper.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(child);

    // Given up on if hello is slow in coming; the reader is left to find
    // the pipe closed.
    let watched = helper.clone();
    std::thread::spawn(move || {
        std::thread::sleep(HELLO_WITHIN);
        if watched.state.load(Ordering::Acquire) == PROBING {
            watched.die("no answer");
        }
    });

    let mut output = BufReader::new(output);
    match hello(&mut output) {
        Ok(()) => {}
        Err(why) => return helper.die(&why),
    }
    if helper.state.compare_exchange(PROBING, READY, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    if DEBUG.load(Ordering::Relaxed) {
        eprintln!("jog: thumbnails made on {}, ready in {:.1?}", helper.host, started.elapsed());
    }
    loop {
        match read_reply(&mut output) {
            Ok((tag, reply)) => {
                let sender = helper.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&tag);
                if let Some(sender) = sender {
                    let _ = sender.send(reply);
                }
            }
            Err(error) => return helper.die(&format!("connection: {error}")),
        }
    }
}

/// Reads up to the hello. Whatever comes first is passed over: an rc file
/// over there that prints a greeting.
fn hello(output: &mut impl Read) -> Result<(), String> {
    let mut seen: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match output.read(&mut byte) {
            Ok(1) => seen.push(byte[0]),
            Ok(_) => return Err(format!("no jog --serve there{}", greeting(&seen))),
            Err(error) => return Err(error.to_string()),
        }
        if seen.ends_with(MAGIC) {
            break;
        }
        if seen.len() > 64 * 1024 {
            return Err("not jog that answered".into());
        }
    }
    let protocol = read_u32(output).map_err(|error| error.to_string())?;
    if protocol != PROTOCOL {
        return Err(format!("jog there speaks {protocol}, here {PROTOCOL}"));
    }
    Ok(())
}

/// What the other side said before going, if anything, for `--debug`.
fn greeting(seen: &[u8]) -> String {
    let text = String::from_utf8_lossy(seen);
    let text = text.trim();
    if text.is_empty() { String::new() } else { format!(" ({text})") }
}

/// Whether a thumbnail of this file would be made over there just now.
pub fn ready_for(path: &Path) -> bool {
    helper_for(path).is_some()
}

fn helper_for(path: &Path) -> Option<(Arc<Helper>, PathBuf)> {
    let (host, there) = crate::remote::split(path)?;
    let helpers = HELPERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let helper = helpers.iter().find(|(known, _)| *known == host).map(|(_, helper)| helper.clone())?;
    (helper.state.load(Ordering::Acquire) == READY).then_some((helper, there))
}

/// The thumbnail of a file in a mount, made over there. `None` when it
/// can't be, for now or for good: then it is read through the mount.
pub fn thumbnail(path: &Path, spec: Spec) -> Option<Reply> {
    let (helper, there) = helper_for(path)?;
    let (width, height) = spec.target?;
    let tag = helper.next.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = mpsc::channel();
    helper.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(tag, sender);

    let path_bytes = there.as_os_str().as_encoded_bytes();
    let mut request = Vec::with_capacity(34 + path_bytes.len());
    request.extend_from_slice(&tag.to_le_bytes());
    request.extend_from_slice(&width.to_le_bytes());
    request.extend_from_slice(&height.to_le_bytes());
    request.push(spec.transform.turns());
    request.push(u8::from(spec.transform.mirrored()));
    request.extend_from_slice(&spec.background.to_le_bytes());
    request.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
    request.extend_from_slice(path_bytes);
    let sent = {
        let mut input = helper.input.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        input.as_mut().is_some_and(|input| input.write_all(&request).and_then(|()| input.flush()).is_ok())
    };
    if !sent {
        helper.die("connection closed");
        return None;
    }
    match receiver.recv_timeout(REPLY_WITHIN) {
        // A file it could not read is read here: what this side has, the
        // other may lack (a decoder, a tool), and the error said here is
        // the one meant.
        Ok(Ok(frame)) => Some(Ok(frame)),
        Ok(Err(_)) => None,
        Err(_) => {
            helper.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&tag);
            None
        }
    }
}

fn read_reply(output: &mut impl Read) -> std::io::Result<(u64, Reply)> {
    let tag = read_u64(output)?;
    let ok = read_u8(output)? == 1;
    if !ok {
        let error = read_bytes(output)?;
        return Ok((tag, Err(String::from_utf8_lossy(&error).into_owned())));
    }
    let native = (read_u32(output)?, read_u32(output)?);
    let (width, height) = (read_u32(output)?, read_u32(output)?);
    let count = read_u32(output)?;
    let animated = read_u8(output)? == 1;
    let mut pictures = Vec::new();
    let mut delays = Vec::new();
    for _ in 0..count {
        delays.push(Duration::from_millis(u64::from(read_u32(output)?)));
        let jpeg = read_bytes(output)?;
        pictures.push(unjpeg(&jpeg, width, height));
    }
    if pictures.is_empty() || pictures.iter().any(Option::is_none) {
        return Ok((tag, Err("a picture that would not decode".into())));
    }
    let mut pictures = pictures.into_iter().flatten();
    let first = pictures.next().unwrap_or_default();
    let animation = animated.then(|| Animation { frames: pictures.collect(), delays });
    Ok((tag, Ok(Frame { width, height, native, pixels: first, animation })))
}

fn unjpeg(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u32>> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?.into_rgb8();
    if image.dimensions() != (width, height) {
        return None;
    }
    Some(image.pixels().map(|p| (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2])).collect())
}

fn jpeg(pixels: &[u32], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY)
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|error| error.to_string())?;
    Ok(out)
}

fn read_u8(input: &mut impl Read) -> std::io::Result<u8> {
    let mut bytes = [0u8; 1];
    input.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

fn read_u32(input: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut impl Read) -> std::io::Result<u64> {
    let mut bytes = [0u8; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_bytes(input: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let length = read_u32(input)? as usize;
    if length > 256 << 20 {
        return Err(std::io::Error::other("a reply too long to be one"));
    }
    let mut bytes = vec![0u8; length];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// `jog --serve`: makes the thumbnails asked for on stdin and writes them
/// to stdout, several at once, until stdin closes.
pub fn serve_main() -> std::process::ExitCode {
    use std::os::fd::FromRawFd;
    // The replies get stdout to themselves: anything else that would write
    // there, a decoder's tool or a stray print, writes to stderr instead,
    // and cannot break a reply in two.
    // SAFETY: dup and dup2 on the process's own standard descriptors; the
    // copy is owned by the File made of it alone.
    let replies = unsafe {
        let copy = libc::dup(1);
        if copy < 0 || libc::dup2(2, 1) < 0 {
            return std::process::ExitCode::FAILURE;
        }
        std::fs::File::from_raw_fd(copy)
    };
    let output = Arc::new(Mutex::new(std::io::BufWriter::new(replies)));
    {
        let mut out = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut hello = MAGIC.to_vec();
        hello.extend_from_slice(&PROTOCOL.to_le_bytes());
        if out.write_all(&hello).and_then(|()| out.flush()).is_err() {
            return std::process::ExitCode::FAILURE;
        }
    }
    let mut input = BufReader::new(std::io::stdin().lock());
    while let Ok(tag) = read_u64(&mut input) {
        let Ok(request) = read_request(&mut input) else { break };
        let output = output.clone();
        rayon::spawn(move || {
            let (path, spec) = request;
            let made = std::panic::catch_unwind(|| make(&path, spec)).unwrap_or_else(|_| Err("the decoder crashed on it".into()));
            let reply = encode_reply(tag, made);
            let mut out = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if out.write_all(&reply).and_then(|()| out.flush()).is_err() {
                std::process::exit(0);
            }
        });
    }
    // The other side went: what is still being made is for no one.
    std::process::ExitCode::SUCCESS
}

fn read_request(input: &mut impl Read) -> std::io::Result<(PathBuf, Spec)> {
    let width = read_u32(input)?;
    let height = read_u32(input)?;
    let turns = read_u8(input)?;
    let mirrored = read_u8(input)? == 1;
    let background = read_u32(input)?;
    let path = read_bytes(input)?;
    // SAFETY: the bytes came from `as_encoded_bytes` on the same platform.
    let path = PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(path) });
    let spec = Spec { target: Some((width, height)), enlarge: false, transform: Transform::from_parts(turns, mirrored), background, thumbnail: true, hdr: Hdr::Sdr };
    Ok((path, spec))
}

/// A thumbnail made the way the loader makes one here.
fn make(path: &Path, spec: Spec) -> Result<Frame, String> {
    let source = decode::read(path, spec).map_err(|error| error.to_string())?;
    decode::finish(&source, spec).map_err(|error| error.to_string())
}

fn encode_reply(tag: u64, made: Result<Frame, String>) -> Vec<u8> {
    let mut out = tag.to_le_bytes().to_vec();
    let encoded = made.and_then(|frame| {
        let delays = frame.animation.as_ref().map(|animation| animation.delays.clone()).unwrap_or_default();
        let mut pictures = vec![jpeg(&frame.pixels, frame.width, frame.height)?];
        if let Some(animation) = &frame.animation {
            for picture in &animation.frames {
                pictures.push(jpeg(picture, frame.width, frame.height)?);
            }
        }
        Ok((frame, pictures, delays))
    });
    match encoded {
        Ok((frame, pictures, delays)) => {
            out.push(1);
            for value in [frame.native.0, frame.native.1, frame.width, frame.height, pictures.len() as u32] {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.push(u8::from(frame.animation.is_some()));
            for (at, picture) in pictures.iter().enumerate() {
                let delay = delays.get(at).map_or(0, |delay| delay.as_millis() as u32);
                out.extend_from_slice(&delay.to_le_bytes());
                out.extend_from_slice(&(picture.len() as u32).to_le_bytes());
                out.extend_from_slice(picture);
            }
        }
        Err(error) => {
            out.push(0);
            out.extend_from_slice(&(error.len() as u32).to_le_bytes());
            out.extend_from_slice(error.as_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_comes_back_as_the_frame_it_was() {
        let pixels: Vec<u32> = (0..64 * 48).map(|at| if at % 64 < 32 { 0x00ff_0000 } else { 0x0000_00ff }).collect();
        let frame = Frame::still(64, 48, (6000, 4500), pixels.clone());
        let reply = encode_reply(7, Ok(frame));
        let (tag, back) = read_reply(&mut reply.as_slice()).unwrap();
        let back = back.unwrap();
        assert_eq!((tag, back.width, back.height, back.native), (7, 64, 48, (6000, 4500)));
        assert!(back.animation.is_none());
        // JPEG: near, not the same.
        let red = back.pixels[5 * 64 + 4];
        assert!((red >> 16) > 0xf0 && (red & 0xff) < 0x10, "{red:06x}");
    }

    #[test]
    fn an_error_comes_back_as_one() {
        let reply = encode_reply(3, Err("no such file".into()));
        let (tag, back) = read_reply(&mut reply.as_slice()).unwrap();
        assert_eq!((tag, back.err().as_deref()), (3, Some("no such file")));
    }

    #[test]
    fn hello_is_found_past_a_greeting_and_a_stranger_is_not_used() {
        let mut said = b"Welcome to skyfall\n".to_vec();
        said.extend_from_slice(MAGIC);
        said.extend_from_slice(&PROTOCOL.to_le_bytes());
        assert!(hello(&mut said.as_slice()).is_ok());
        let mut other = MAGIC.to_vec();
        other.extend_from_slice(&(PROTOCOL + 1).to_le_bytes());
        assert!(hello(&mut other.as_slice()).is_err());
        assert!(hello(&mut b"jog: Command not found.\n".as_slice()).unwrap_err().contains("Command not found"));
    }
}
