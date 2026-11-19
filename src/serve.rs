//! Pictures made on the machine the files are on.
//!
//! Over a mount, showing a 13 MB photo costs the 13 MB crossing the network,
//! for a thumbnail as much as for the picture on screen. So for a host jog
//! has mounted, it also starts `ssh host jog --serve`, a jog over there that
//! reads the file where it is and sends back only what is shown: the picture
//! at the size of the screen, or of a thumbnail, as a JPEG. Zoomed in to its
//! own pixels, a picture is read through the mount, exactly as it is.
//!
//! Measured on skyfall over a 32 MB/s link, 24-megapixel photos: the first
//! screen of a grid in 22.6 s through the mount, 5.2 s with this; the
//! newest photo of a folder on screen in 8.3 s through the mount.
//!
//! It is only ever a shortcut. Started alongside the mount, it is waited on
//! by nothing: until it says hello, and after anything goes wrong with it
//! (no jog over there, an older one, a dropped connection, a reply too
//! slow), pictures are read through the mount as they would be without it.
//!
//! Other work that reads much of a file to say little of it is done there
//! too: every ffprobe jog runs, and what `--info` says of a file.
//!
//! The protocol, little-endian, over ssh's stdin and stdout. A byte string
//! is its length `u32` and the bytes.
//! - hello, from the server: `JOGSERVE` and the protocol's version, `u32`.
//! - request: tag `u64`, kind `u8`, and what the kind asks:
//!   - 0, a picture: width and height wanted `u32`s, quarter turns `u8`,
//!     mirrored `u8`, flags `u8` (1 a thumbnail, 2 enlarged, 4 on screen),
//!     background `u32`, path.
//!   - 1, ffprobe: the count of its arguments `u32`, each, then the path.
//!   - 2, what `--info` says: the path.
//! - reply: tag `u64`, kind `u8`, ok `u8`; not ok, the error. Ok:
//!   - a picture: the file's format (empty when not known), Ultra HDR `u8`,
//!     native width and height, width and height, `u32`s, then the count of
//!     pictures `u32` and whether they are an animation `u8`, and each
//!     picture as its delay in ms `u32` and a JPEG.
//!   - ffprobe: its exit code `i32`, stdout and stderr.
//!   - `--info`: the facts, as JSON.

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
const PROTOCOL: u32 = 3;

const KIND_PICTURE: u8 = 0;
const KIND_PROBE: u8 = 1;
const KIND_FACTS: u8 = 2;
/// How long the server has to say hello before it is given up on. It is
/// waited on by nothing meanwhile.
const HELLO_WITHIN: Duration = Duration::from_secs(6);
/// How long one picture may take over there before it is read here
/// instead: a video whose still is slow to find, a host that has stalled.
const REPLY_WITHIN: Duration = Duration::from_secs(15);
/// JPEG quality of a thumbnail: near enough not to tell at its size, a
/// tenth of the bytes.
const THUMB_QUALITY: f32 = 88.0;
/// JPEG quality of the picture on screen, with color at full resolution:
/// not to be told from the decoded pixels at the size it is shown.
const SCREEN_QUALITY: f32 = 92.0;

const FLAG_THUMBNAIL: u8 = 1;
const FLAG_ENLARGE: u8 = 2;
/// The picture on screen, which is made before any other waiting.
const FLAG_URGENT: u8 = 4;

/// The formats a file is told to be, by [`crate::files::sniff_file`].
const FORMATS: &[&str] = &[
    "JPEG", "PNG", "GIF", "WebP", "JPEG XL", "AVIF", "HEIC", "HEIF", "TIFF", "BMP", "QOI", "EXR", "PSD", "HDR", "DDS", "ICO", "PNM", "SVG",
];

const PROBING: u8 = 0;
const READY: u8 = 1;
const DEAD: u8 = 2;

static DEBUG: AtomicBool = AtomicBool::new(false);
static HELPERS: Mutex<Vec<(String, Arc<Helper>)>> = Mutex::new(Vec::new());

/// Says on the terminal how each helper fares, with `--debug`.
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}

/// A picture made over there, and what was learned of its file.
pub struct Made {
    pub frame: Frame,
    pub format: Option<&'static str>,
    pub ultra_hdr: bool,
}

enum Reply {
    Picture(Result<Made, String>),
    /// What any other kind of request says, as it was sent.
    Bytes(Result<Vec<u8>, String>),
}

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
            eprintln!("jog: no pictures made on {}: {why}; read through the mount", self.host);
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
/// back at once: nothing waits on it. `ssh` is the command and options
/// that reach the host, without it.
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
        .args(["-o", "BatchMode=yes"])
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
    if let Err(why) = hello(&mut output) {
        return helper.die(&why);
    }
    if helper.state.compare_exchange(PROBING, READY, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    if DEBUG.load(Ordering::Relaxed) {
        eprintln!("jog: pictures made on {}, ready in {:.1?}", helper.host, started.elapsed());
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

/// Whether this picture of the file would be made over there just now.
pub fn ready_for(path: &Path, spec: Spec) -> bool {
    wanted(spec) && helper_for(path).is_some()
}

/// What is made over there: a picture at a size, as any viewer shows it.
/// One at its own size is read here, for its exact pixels, and so is an
/// HDR rendition, which wants the gain map.
fn wanted(spec: Spec) -> bool {
    spec.target.is_some() && spec.hdr == Hdr::Sdr
}

fn helper_for(path: &Path) -> Option<(Arc<Helper>, PathBuf)> {
    let (host, there) = crate::remote::split(path)?;
    let helpers = HELPERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let helper = helpers.iter().find(|(known, _)| *known == host).map(|(_, helper)| helper.clone())?;
    (helper.state.load(Ordering::Acquire) == READY).then_some((helper, there))
}

/// How long a picture waits for a helper still connecting, rather than be
/// read through the mount: a photo read whole over the link takes longer
/// than the helper takes to answer, and a host without jog fails as fast.
const CONNECTING_WAIT: Duration = Duration::from_millis(1500);

/// Waits, a little, for the host's helper to have connected or failed.
fn settled(path: &Path) {
    let Some((host, _)) = crate::remote::split(path) else { return };
    let helper = {
        let helpers = HELPERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        helpers.iter().find(|(known, _)| *known == host).map(|(_, helper)| helper.clone())
    };
    let Some(helper) = helper else { return };
    let until = Instant::now() + CONNECTING_WAIT;
    while helper.state.load(Ordering::Acquire) == PROBING && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The picture of a file in a mount, made over there. `None` when it can't
/// be, for now or for good: then it is read through the mount.
pub fn make_there(path: &Path, spec: Spec, urgent: bool) -> Option<Made> {
    if !wanted(spec) {
        return None;
    }
    let (width, height) = spec.target?;
    let flags = if spec.thumbnail { FLAG_THUMBNAIL } else { 0 } | if spec.enlarge { FLAG_ENLARGE } else { 0 } | if urgent { FLAG_URGENT } else { 0 };
    let mut body = Vec::new();
    body.extend_from_slice(&width.to_le_bytes());
    body.extend_from_slice(&height.to_le_bytes());
    body.push(spec.transform.turns());
    body.push(u8::from(spec.transform.mirrored()));
    body.push(flags);
    body.extend_from_slice(&spec.background.to_le_bytes());
    let asked = Instant::now();
    let (host, made) = match ask(path, KIND_PICTURE, body)? {
        (host, Reply::Picture(Ok(made))) => (host, made),
        // A file it could not read is read here: what this side has, the
        // other may lack (a decoder, a tool), and the error said here is
        // the one meant.
        _ => return None,
    };
    if DEBUG.load(Ordering::Relaxed) {
        eprintln!("jog: {:>7.1?} made on {host} at {}×{}{}", asked.elapsed(), made.frame.width, made.frame.height, if urgent { ", on screen" } else { "" });
    }
    Some(made)
}

/// Asks the helper of the host the file is on: `body` is the request after
/// its kind, without the path, which is put after it. `None` when there is
/// no helper to ask, or no answer in time.
fn ask(path: &Path, kind: u8, mut body: Vec<u8>) -> Option<(String, Reply)> {
    settled(path);
    let (helper, there) = helper_for(path)?;
    let tag = helper.next.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = mpsc::channel();
    helper.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(tag, sender);

    let mut request = tag.to_le_bytes().to_vec();
    request.push(kind);
    request.append(&mut body);
    put_bytes(&mut request, there.as_os_str().as_encoded_bytes());
    let sent = {
        let mut input = helper.input.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        input.as_mut().is_some_and(|input| input.write_all(&request).and_then(|()| input.flush()).is_ok())
    };
    if !sent {
        helper.die("connection closed");
        return None;
    }
    match receiver.recv_timeout(REPLY_WITHIN) {
        Ok(reply) => Some((helper.host.clone(), reply)),
        Err(_) => {
            helper.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&tag);
            None
        }
    }
}

/// Runs ffprobe with `arguments` on the file, over there when the file is
/// in a mount and jog is there: ffprobe seeks about a file for what it
/// says, each seek a round trip through the mount.
pub fn ffprobe(arguments: &[&str], path: &Path) -> std::io::Result<std::process::Output> {
    let mut body = (arguments.len() as u32).to_le_bytes().to_vec();
    for argument in arguments {
        put_bytes(&mut body, argument.as_bytes());
    }
    if let Some((_, Reply::Bytes(Ok(said)))) = ask(path, KIND_PROBE, body) {
        if let Some(output) = unpack_output(&said) {
            return Ok(output);
        }
    }
    Command::new("ffprobe").args(arguments).arg(path).stdin(Stdio::null()).output()
}

/// What `--info` says of a file, as JSON, worked out over there.
pub fn facts(path: &Path) -> Option<String> {
    match ask(path, KIND_FACTS, Vec::new())? {
        (_, Reply::Bytes(Ok(json))) => String::from_utf8(json).ok(),
        _ => None,
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn pack_output(output: &std::process::Output) -> Vec<u8> {
    let mut out = output.status.code().unwrap_or(-1).to_le_bytes().to_vec();
    put_bytes(&mut out, &output.stdout);
    put_bytes(&mut out, &output.stderr);
    out
}

fn unpack_output(mut bytes: &[u8]) -> Option<std::process::Output> {
    use std::os::unix::process::ExitStatusExt;
    let code = read_u32(&mut bytes).ok()? as i32;
    let stdout = read_bytes(&mut bytes).ok()?;
    let stderr = read_bytes(&mut bytes).ok()?;
    // A wait status: the exit code in the second byte.
    Some(std::process::Output { status: std::process::ExitStatus::from_raw((code & 0xff) << 8), stdout, stderr })
}

fn read_reply(output: &mut impl Read) -> std::io::Result<(u64, Reply)> {
    let tag = read_u64(output)?;
    let kind = read_u8(output)?;
    let ok = read_u8(output)? == 1;
    if !ok {
        let error = String::from_utf8_lossy(&read_bytes(output)?).into_owned();
        return Ok((tag, if kind == KIND_PICTURE { Reply::Picture(Err(error)) } else { Reply::Bytes(Err(error)) }));
    }
    if kind != KIND_PICTURE {
        return Ok((tag, Reply::Bytes(Ok(read_bytes(output)?))));
    }
    let format = read_bytes(output)?;
    let format = FORMATS.iter().find(|name| name.as_bytes() == format.as_slice()).copied();
    let ultra_hdr = read_u8(output)? == 1;
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
        return Ok((tag, Reply::Picture(Err("a picture that would not decode".into()))));
    }
    let mut pictures = pictures.into_iter().flatten();
    let first = pictures.next().unwrap_or_default();
    let animation = animated.then(|| Animation { frames: pictures.collect(), delays });
    let frame = Frame { width, height, native, pixels: first, animation };
    Ok((tag, Reply::Picture(Ok(Made { frame, format, ultra_hdr }))))
}

fn unjpeg(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u32>> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?.into_rgb8();
    if image.dimensions() != (width, height) {
        return None;
    }
    Some(image.pixels().map(|p| (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2])).collect())
}

/// A picture as a JPEG: a thumbnail's color at half resolution, as most
/// JPEGs have it; the picture on screen's at full.
fn jpeg(pixels: &[u32], width: u32, height: u32, thumbnail: bool) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    let encode = || -> std::io::Result<Vec<u8>> {
        let mut compress = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        // libjpeg-turbo's way, not mozjpeg's: its trellis search makes a
        // file a tenth smaller in five times as long, and the link is
        // quicker than that.
        compress.set_fastest_defaults();
        compress.set_size(width as usize, height as usize);
        compress.set_quality(if thumbnail { THUMB_QUALITY } else { SCREEN_QUALITY });
        let chroma = if thumbnail { (2, 2) } else { (1, 1) };
        compress.set_chroma_sampling_pixel_sizes(chroma, chroma);
        let mut started = compress.start_compress(Vec::new())?;
        started.write_scanlines(&rgb)?;
        started.finish()
    };
    // mozjpeg reports a broken encode by unwinding.
    match std::panic::catch_unwind(encode) {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("the JPEG encoder failed".into()),
    }
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

/// `jog --serve`: makes the pictures asked for on stdin and writes them to
/// stdout, several at once, until stdin closes.
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
    // The other side asks for as many at once as it has threads, which may
    // be more than this has cores. The picture on screen has the machine
    // and the link to itself: one worker is kept for it, and the rest
    // neither start nor send while it is being made.
    let queue = Arc::new(Queue::default());
    let cores = std::thread::available_parallelism().map_or(4, usize::from);
    queue.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).others_at_most = cores.saturating_sub(1).max(1);
    for _ in 0..cores {
        let (queue, output) = (queue.clone(), output.clone());
        std::thread::spawn(move || {
            while let Some((tag, path, spec, urgent)) = queue.take() {
                let made = std::panic::catch_unwind(|| make(&path, spec)).unwrap_or_else(|_| Err("the decoder crashed on it".into()));
                let reply = encode_reply(tag, made, spec.thumbnail);
                if !urgent {
                    queue.wait_for_urgent();
                }
                let mut out = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if out.write_all(&reply).and_then(|()| out.flush()).is_err() {
                    std::process::exit(0);
                }
                drop(out);
                queue.done(urgent);
            }
        });
    }
    let mut input = BufReader::new(std::io::stdin().lock());
    while let Ok(tag) = read_u64(&mut input) {
        let Ok(kind) = read_u8(&mut input) else { break };
        if kind == KIND_PICTURE {
            let Ok((path, spec, urgent)) = read_request(&mut input) else { break };
            queue.put((tag, path, spec), urgent);
            continue;
        }
        // The rest are quick, each little more than a process run, and
        // wait behind no picture.
        let asked = match kind {
            KIND_PROBE => read_probe(&mut input).map(|(arguments, path)| Asked::Probe(arguments, path)),
            KIND_FACTS => read_path(&mut input).map(Asked::Facts),
            _ => break,
        };
        let Ok(asked) = asked else { break };
        let output = output.clone();
        std::thread::spawn(move || {
            let answer = match asked {
                Asked::Probe(arguments, path) => {
                    Command::new("ffprobe").args(&arguments).arg(&path).stdin(Stdio::null()).output().map(|output| pack_output(&output)).map_err(|error| format!("cannot run ffprobe: {error}"))
                }
                Asked::Facts(path) => crate::info::facts_json(&path),
            };
            let mut reply = tag.to_le_bytes().to_vec();
            reply.push(kind);
            match answer {
                Ok(bytes) => {
                    reply.push(1);
                    put_bytes(&mut reply, &bytes);
                }
                Err(error) => {
                    reply.push(0);
                    put_bytes(&mut reply, error.as_bytes());
                }
            }
            let mut out = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if out.write_all(&reply).and_then(|()| out.flush()).is_err() {
                std::process::exit(0);
            }
        });
    }
    // The other side went: what is still being made is for no one.
    std::process::ExitCode::SUCCESS
}

type Task = (u64, PathBuf, Spec);

#[derive(Default)]
struct Waiting {
    urgent: std::collections::VecDeque<Task>,
    others: std::collections::VecDeque<Task>,
    /// Urgent ones being made or sent.
    urgent_busy: usize,
    /// The others being made, and how many may be at once.
    others_busy: usize,
    others_at_most: usize,
}

/// What is waiting to be made, the urgent first.
#[derive(Default)]
struct Queue {
    waiting: Mutex<Waiting>,
    wake: std::sync::Condvar,
}

impl Queue {
    fn put(&self, task: Task, urgent: bool) {
        let mut waiting = self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if urgent { waiting.urgent.push_back(task) } else { waiting.others.push_back(task) }
        self.wake.notify_all();
    }

    /// The next to make, and whether it is urgent.
    fn take(&self) -> Option<(u64, PathBuf, Spec, bool)> {
        let mut waiting = self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some((tag, path, spec)) = waiting.urgent.pop_front() {
                waiting.urgent_busy += 1;
                return Some((tag, path, spec, true));
            }
            if waiting.urgent_busy == 0 && waiting.others_busy < waiting.others_at_most {
                if let Some((tag, path, spec)) = waiting.others.pop_front() {
                    waiting.others_busy += 1;
                    return Some((tag, path, spec, false));
                }
            }
            waiting = self.wake.wait(waiting).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Holds a reply back while an urgent one is being made or sent.
    fn wait_for_urgent(&self) {
        let mut waiting = self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while waiting.urgent_busy > 0 || !waiting.urgent.is_empty() {
            waiting = self.wake.wait(waiting).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn done(&self, urgent: bool) {
        let mut waiting = self.waiting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if urgent { waiting.urgent_busy -= 1 } else { waiting.others_busy -= 1 }
        self.wake.notify_all();
    }
}

/// A request other than for a picture.
enum Asked {
    Probe(Vec<String>, PathBuf),
    Facts(PathBuf),
}

fn read_path(input: &mut impl Read) -> std::io::Result<PathBuf> {
    let path = read_bytes(input)?;
    // SAFETY: the bytes came from `as_encoded_bytes` on the same platform.
    Ok(PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(path) }))
}

fn read_probe(input: &mut impl Read) -> std::io::Result<(Vec<String>, PathBuf)> {
    let count = read_u32(input)?;
    if count > 256 {
        return Err(std::io::Error::other("too many arguments"));
    }
    let mut arguments = Vec::new();
    for _ in 0..count {
        arguments.push(String::from_utf8_lossy(&read_bytes(input)?).into_owned());
    }
    Ok((arguments, read_path(input)?))
}

fn read_request(input: &mut impl Read) -> std::io::Result<(PathBuf, Spec, bool)> {
    let width = read_u32(input)?;
    let height = read_u32(input)?;
    let turns = read_u8(input)?;
    let mirrored = read_u8(input)? == 1;
    let flags = read_u8(input)?;
    let background = read_u32(input)?;
    let path = read_bytes(input)?;
    // SAFETY: the bytes came from `as_encoded_bytes` on the same platform.
    let path = PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(path) });
    let spec = Spec {
        target: Some((width, height)),
        enlarge: flags & FLAG_ENLARGE != 0,
        transform: Transform::from_parts(turns, mirrored),
        background,
        thumbnail: flags & FLAG_THUMBNAIL != 0,
        hdr: Hdr::Sdr,
    };
    Ok((path, spec, flags & FLAG_URGENT != 0))
}

/// A picture made the way the loader makes one here.
fn make(path: &Path, spec: Spec) -> Result<Made, String> {
    let source = decode::read(path, spec).map_err(|error| error.to_string())?;
    let frame = decode::finish(&source, spec).map_err(|error| error.to_string())?;
    Ok(Made { frame, format: source.format, ultra_hdr: source.ultra_hdr })
}

fn encode_reply(tag: u64, made: Result<Made, String>, thumbnail: bool) -> Vec<u8> {
    let mut out = tag.to_le_bytes().to_vec();
    out.push(KIND_PICTURE);
    let encoded = made.and_then(|made| {
        let frame = &made.frame;
        let delays = frame.animation.as_ref().map(|animation| animation.delays.clone()).unwrap_or_default();
        let mut pictures = vec![jpeg(&frame.pixels, frame.width, frame.height, thumbnail)?];
        if let Some(animation) = &frame.animation {
            for picture in &animation.frames {
                pictures.push(jpeg(picture, frame.width, frame.height, thumbnail)?);
            }
        }
        Ok((made, pictures, delays))
    });
    match encoded {
        Ok((made, pictures, delays)) => {
            let frame = made.frame;
            out.push(1);
            let format = made.format.unwrap_or_default();
            out.extend_from_slice(&(format.len() as u32).to_le_bytes());
            out.extend_from_slice(format.as_bytes());
            out.push(u8::from(made.ultra_hdr));
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

    fn halves() -> Frame {
        let pixels: Vec<u32> = (0..64 * 48).map(|at| if at % 64 < 32 { 0x00ff_0000 } else { 0x0000_00ff }).collect();
        Frame::still(64, 48, (6000, 4500), pixels)
    }

    fn picture(reply: Reply) -> Result<Made, String> {
        match reply {
            Reply::Picture(made) => made,
            Reply::Bytes(_) => panic!("not a picture"),
        }
    }

    #[test]
    fn a_reply_comes_back_as_the_frame_it_was() {
        for thumbnail in [true, false] {
            let made = Made { frame: halves(), format: Some("JPEG"), ultra_hdr: true };
            let reply = encode_reply(7, Ok(made), thumbnail);
            let (tag, back) = read_reply(&mut reply.as_slice()).unwrap();
            let back = picture(back).unwrap();
            assert_eq!((tag, back.format, back.ultra_hdr), (7, Some("JPEG"), true));
            let frame = back.frame;
            assert_eq!((frame.width, frame.height, frame.native), (64, 48, (6000, 4500)));
            assert!(frame.animation.is_none());
            // JPEG: near, not the same.
            let red = frame.pixels[5 * 64 + 4];
            assert!((red >> 16) > 0xf0 && (red & 0xff) < 0x10, "{red:06x}");
        }
    }

    #[test]
    fn an_unknown_format_comes_back_as_none() {
        let made = Made { frame: halves(), format: None, ultra_hdr: false };
        let reply = encode_reply(1, Ok(made), true);
        assert_eq!(picture(read_reply(&mut reply.as_slice()).unwrap().1).unwrap().format, None);
    }

    #[test]
    fn an_error_comes_back_as_one() {
        let reply = encode_reply(3, Err("no such file".into()), true);
        let (tag, back) = read_reply(&mut reply.as_slice()).unwrap();
        assert_eq!((tag, picture(back).err().as_deref()), (3, Some("no such file")));
    }

    #[test]
    fn a_probe_comes_back_as_the_output_it_was() {
        use std::os::unix::process::ExitStatusExt;
        let output = std::process::Output { status: std::process::ExitStatus::from_raw(1 << 8), stdout: b"{}".to_vec(), stderr: b"bad".to_vec() };
        let back = unpack_output(&pack_output(&output)).unwrap();
        assert_eq!((back.status.code(), back.stdout, back.stderr), (Some(1), b"{}".to_vec(), b"bad".to_vec()));
    }

    #[test]
    fn a_request_is_read_as_it_was_written() {
        let spec = Spec { target: Some((300, 200)), enlarge: true, transform: Transform::CW, background: 0x123456, thumbnail: false, hdr: Hdr::Sdr };
        let path = b"/srv/a b.jpg";
        let mut request = Vec::new();
        for value in [300u32, 200] {
            request.extend_from_slice(&value.to_le_bytes());
        }
        request.extend_from_slice(&[1, 0, FLAG_ENLARGE]);
        request.extend_from_slice(&0x123456u32.to_le_bytes());
        request.extend_from_slice(&(path.len() as u32).to_le_bytes());
        request.extend_from_slice(path);
        let (read, back, urgent) = read_request(&mut request.as_slice()).unwrap();
        assert!(!urgent);
        assert_eq!(read, PathBuf::from("/srv/a b.jpg"));
        assert_eq!((back.target, back.enlarge, back.transform, back.background, back.thumbnail), (spec.target, spec.enlarge, spec.transform, spec.background, spec.thumbnail));
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
