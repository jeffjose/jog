use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::rng::Rng;

/// Formats the built-in decoders read.
const RASTER_EXTS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff", "tga", "ico", "qoi",
    "hdr", "exr", "dds", "ff", "pnm", "pbm", "pgm", "ppm", "pam",
];

/// Formats left to ffmpeg, which decodes them when it was built with the
/// right libraries.
const FFMPEG_EXTS: &[&str] = &["svg", "svgz", "psd"];

/// JPEG XL, decoded by jxl-oxide, in Rust and with no library to install.
const JXL_EXTS: &[&str] = &["jxl"];

/// HEIC and AVIF: libheif's where it is installed, and ffmpeg's otherwise.
const HEIF_EXTS: &[&str] = &["heic", "heif", "hif", "avif"];

const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef",
    "srw", "3fr", "erf", "kdc", "mrw", "iiq", "rwl", "x3f",
];

const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "mpg", "mpeg", "wmv", "flv", "m2ts", "mts", "m2t",
    "ts", "3gp", "3g2", "ogv", "ogm", "vob", "asf", "f4v", "rm", "rmvb", "divx", "mxf", "y4m",
];

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

pub fn is_raw(path: &Path) -> bool {
    has_ext(path, RAW_EXTS)
}

pub fn is_video(path: &Path) -> bool {
    has_ext(path, VIDEO_EXTS)
}

pub fn needs_ffmpeg(path: &Path) -> bool {
    has_ext(path, FFMPEG_EXTS)
}

pub fn is_jxl(path: &Path) -> bool {
    has_ext(path, JXL_EXTS)
}

/// A JPEG, which may be an Ultra HDR one with a gain map after it.
pub fn is_jpeg(path: &Path) -> bool {
    has_ext(path, &["jpg", "jpeg", "jpe", "jfif"])
}

pub fn is_heif(path: &Path) -> bool {
    has_ext(path, HEIF_EXTS)
}

/// Only pictures, or only videos: what `--images` and `--videos` ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Only {
    Pictures,
    Videos,
}

/// Whether a file is of the kind asked for, if one was.
fn of_kind(path: &Path, only: Option<Only>) -> bool {
    match only {
        None => true,
        Some(Only::Videos) => is_video(path),
        Some(Only::Pictures) => !is_video(path),
    }
}

fn is_supported(path: &Path) -> bool {
    has_ext(path, RASTER_EXTS) || needs_ffmpeg(path) || is_heif(path) || is_jxl(path) || is_raw(path) || is_video(path)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sort {
    /// Natural order of the whole path.
    Name,
    /// Natural order of the file name, ignoring directories.
    Filename,
    /// Grouped by directory, then by file name.
    Dirname,
    /// Most recently changed first, or last.
    Newest,
    Oldest,
    /// By how many bytes it is.
    Smallest,
    Largest,
}

/// How the command line asked for the playlist to be built.
#[derive(Default)]
pub struct Listing {
    pub recursive: bool,
    pub sort: Option<Sort>,
    pub reverse: bool,
    pub randomize: bool,
    pub start_at: Option<PathBuf>,
    pub filelist: Option<PathBuf>,
    /// Only pictures, or only videos, listed at all.
    pub only: Option<Only>,
}

pub struct Playlist {
    pub files: Vec<PathBuf>,
    pub start: usize,
}

/// Turns command-line paths into the list of images to show.
///
/// Directories expand to the images inside them; files are taken as given,
/// so naming one file shows just that file.
pub fn collect(args: &[PathBuf], listing: &Listing) -> Playlist {
    gather(args, listing, true)
}

/// As `collect`; `report` says whether to complain of what cannot be read,
/// which is for the first time only, not each time the list is refreshed.
fn gather(args: &[PathBuf], listing: &Listing, report: bool) -> Playlist {
    let mut files = Vec::new();
    let start_at = listing.start_at.clone();

    if let Some(list) = &listing.filelist {
        match std::fs::read_to_string(list) {
            Ok(text) => files.extend(text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).filter(|path| of_kind(path, listing.only))),
            Err(error) => eprintln!("jog: cannot read file list {}: {error}", list.display()),
        }
    }

    match args {
        [] if listing.filelist.is_none() => list_dir(Path::new("."), listing.recursive, listing.only, &mut files),
        _ => {
            for arg in args {
                if arg.is_dir() {
                    list_dir(arg, listing.recursive, listing.only, &mut files);
                } else if arg.exists() && !of_kind(arg, listing.only) {
                    // Named, but not of the kind asked for.
                } else if arg.exists() {
                    files.push(arg.clone());
                } else if report {
                    eprintln!("jog: {}: no such file or folder", arg.display());
                }
            }
        }
    }

    if let Some(sort) = listing.sort {
        sort_files(&mut files, sort);
    }
    if listing.randomize {
        Rng::from_clock().shuffle(&mut files);
    }
    if listing.reverse {
        files.reverse();
    }

    let start = start_at
        .and_then(|wanted| {
            let by_path = files.iter().position(|f| *f == wanted);
            by_path.or_else(|| files.iter().position(|f| f.file_name() == wanted.file_name()))
        })
        .unwrap_or(0);
    Playlist { files, start }
}

/// The files the command line names as they are now, in listing order: for
/// finding out what has appeared since `collect` was called.
pub fn relist(args: &[PathBuf], listing: &Listing) -> Vec<PathBuf> {
    let plain = Listing { recursive: listing.recursive, filelist: listing.filelist.clone(), only: listing.only, ..Listing::default() };
    gather(args, &plain, false).files
}

/// A file the scan found, and what it is put in order by: where it falls
/// in the listing (the command line's order, then each folder's files in
/// natural order before its subfolders), and its time or size when the
/// order is by one of those, read while the folder was.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: PathBuf,
    pub placed: Placed,
}

#[derive(Clone, Debug, Default)]
pub struct Placed {
    pub rank: Vec<u32>,
    pub key: u64,
    /// Its name or path made into bytes that compare as the sort by name
    /// has them, when that is the order; empty otherwise.
    pub name: Vec<u8>,
}

/// The order `sort` puts two found files in; ties, and no sort, in the
/// listing's own order.
pub fn found_order(sort: Option<Sort>, (a, at): (&Path, &Placed), (b, bt): (&Path, &Placed)) -> Ordering {
    // Borrowed, not made anew at each of the many comparisons.
    let by_name = |a: &Path, b: &Path| natural_cmp(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy());
    let order = match sort {
        None => Ordering::Equal,
        // By the bytes made for it, which is quicker and the same.
        Some(Sort::Name | Sort::Filename | Sort::Dirname) if !at.name.is_empty() && !bt.name.is_empty() => at.name.cmp(&bt.name),
        Some(Sort::Name) => natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()),
        Some(Sort::Filename) => by_name(a, b),
        Some(Sort::Dirname) => {
            let dirs = (a.parent().unwrap_or(a), b.parent().unwrap_or(b));
            natural_cmp(&dirs.0.to_string_lossy(), &dirs.1.to_string_lossy()).then_with(|| by_name(a, b))
        }
        Some(Sort::Newest | Sort::Largest) => bt.key.cmp(&at.key),
        Some(Sort::Oldest | Sort::Smallest) => at.key.cmp(&bt.key),
    };
    order.then_with(|| at.rank.cmp(&bt.rank))
}

/// Where a file not found by the scan goes among those that were: by its
/// time or size, read now, and after them in the listing.
pub fn placed_now(path: &Path, sort: Option<Sort>) -> Placed {
    Placed { rank: vec![u32::MAX], key: sort_key(sort, || std::fs::metadata(path).ok()), name: name_key(sort, path) }
}

/// What a file is put in order by when that is its time or size: its
/// time in nanoseconds, or its size in bytes.
fn sort_key(sort: Option<Sort>, metadata: impl FnOnce() -> Option<std::fs::Metadata>) -> u64 {
    match sort {
        Some(Sort::Newest | Sort::Oldest) => metadata()
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos() as u64),
        Some(Sort::Largest | Sort::Smallest) => metadata().map_or(0, |m| m.len()),
        _ => 0,
    }
}

/// A folder waiting to be read, and how soon: by where it falls in the
/// listing, or, when the order is by time, by when the folder last had a
/// file come or go, which is where the newest (or oldest) files likely are.
struct Waiting {
    dir: PathBuf,
    rank: Vec<u32>,
    soon: u64,
}

impl PartialEq for Waiting {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Waiting {}
impl PartialOrd for Waiting {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Waiting {
    /// The greatest is read first: the soonest, then the earliest listed.
    fn cmp(&self, other: &Self) -> Ordering {
        self.soon.cmp(&other.soon).then_with(|| other.rank.cmp(&self.rank))
    }
}

/// How many folders are read at once. On a disk in the machine a few do;
/// on a NAS each read waits on the network, and the more there are under
/// way the less of that waiting there is.
const READERS: usize = 16;

/// Lists what the command line names as `collect` does, on threads of its
/// own, each folder's files sent as soon as it has been read. The folders
/// most likely to hold what comes first are read first. The receiver is
/// closed once all has been listed.
pub fn scan(args: &[PathBuf], listing: &Listing) -> std::sync::mpsc::Receiver<Vec<Found>> {
    use std::collections::BinaryHeap;
    use std::sync::{mpsc, Arc, Condvar, Mutex};
    let (sender, receiver) = mpsc::channel();
    let sort = listing.sort.filter(|_| !listing.randomize);
    let (recursive, reverse, only) = (listing.recursive, listing.reverse, listing.only);
    // By time, the folders changed latest go first, or for the oldest, last.
    let soon = move |dir: &Path| -> u64 {
        match sort {
            Some(Sort::Newest | Sort::Oldest) => {
                let time = sort_key(Some(Sort::Newest), || std::fs::metadata(dir).ok());
                if (sort == Some(Sort::Newest)) != reverse { time } else { u64::MAX - time }
            }
            _ => 0,
        }
    };

    // What is named itself, and the file list, at once; the folders to read.
    let mut named = Vec::new();
    let mut heap = BinaryHeap::new();
    if let Some(list) = &listing.filelist {
        match std::fs::read_to_string(list) {
            Ok(text) => named.extend(text.lines().filter(|l| !l.is_empty()).enumerate().map(|(at, line)| (PathBuf::from(line), vec![0, at as u32])).filter(|(path, _)| of_kind(path, listing.only))),
            Err(error) => eprintln!("jog: cannot read file list {}: {error}", list.display()),
        }
    }
    let here = [PathBuf::from(".")];
    let args = if args.is_empty() && listing.filelist.is_none() { &here[..] } else { args };
    for (at, arg) in args.iter().enumerate() {
        if arg.is_dir() {
            heap.push(Waiting { dir: arg.clone(), rank: vec![at as u32 + 1], soon: soon(arg) });
        } else if arg.exists() && !of_kind(arg, listing.only) {
            // Named, but not of the kind asked for.
        } else if arg.exists() {
            named.push((arg.clone(), vec![at as u32 + 1]));
        } else {
            eprintln!("jog: {}: no such file or folder", arg.display());
        }
    }
    let named: Vec<Found> = named.into_iter().map(|(path, rank)| Found { placed: Placed { key: sort_key(sort, || std::fs::metadata(&path).ok()), name: name_key(sort, &path), rank }, path }).collect();
    if !named.is_empty() {
        let _ = sender.send(named);
    }

    // The folders still to read, and how many are being read: when there
    // are none of either, all is listed.
    let work = Arc::new((Mutex::new((heap, 0usize)), Condvar::new()));
    let delay = std::env::var("JOG_SCAN_DELAY_MS").ok().and_then(|ms| ms.parse().ok()).map(std::time::Duration::from_millis);
    for _ in 0..READERS {
        let (work, sender) = (work.clone(), sender.clone());
        std::thread::spawn(move || loop {
            let (lock, wake) = &*work;
            let Ok(mut state) = lock.lock() else { return };
            let job = loop {
                if let Some(job) = state.0.pop() {
                    state.1 += 1;
                    break job;
                }
                if state.1 == 0 {
                    // Nothing waiting and no one reading: done.
                    wake.notify_all();
                    return;
                }
                let Ok(next) = wake.wait(state) else { return };
                state = next;
            };
            drop(state);
            // A stand-in for a slow disk, in tests.
            if let Some(delay) = delay {
                std::thread::sleep(delay);
            }
            let (files, subdirs) = read_folder(&job.dir, recursive, sort, only);
            let found: Vec<Found> = files
                .into_iter()
                .enumerate()
                .map(|(at, (path, key))| Found { placed: Placed { rank: [&job.rank[..], &[0, at as u32]].concat(), key, name: name_key(sort, &path) }, path })
                .collect();
            let sent = found.is_empty() || sender.send(found).is_ok();
            let Ok(mut state) = lock.lock() else { return };
            if sent {
                for (at, dir) in subdirs.into_iter().enumerate() {
                    let soon = soon(&dir);
                    state.0.push(Waiting { dir, rank: [&job.rank[..], &[at as u32 + 1]].concat(), soon });
                }
            } else {
                // No one is listening any more.
                state.0.clear();
            }
            state.1 -= 1;
            wake.notify_all();
        });
    }
    receiver
}

/// A listing still going on: the files to come, and those had so far, in
/// the order the list has them.
pub struct Scan {
    pub receiver: std::sync::mpsc::Receiver<Vec<Found>>,
    pub known: Vec<Found>,
}

/// How long, once the first files are found, the scan has to find any that
/// come before them, before they are shown.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// The first files of the listing, in order, as soon as there are some;
/// and the scan, if it is not done yet, to bring in the rest.
pub fn first_found(args: &[PathBuf], listing: &Listing) -> (Playlist, Option<Scan>) {
    use std::sync::mpsc::RecvTimeoutError;
    let receiver = scan(args, listing);
    let mut known = Vec::new();
    // However long the first take; then a moment more.
    let mut done = match receiver.recv() {
        Ok(batch) => {
            known.extend(batch);
            false
        }
        Err(_) => true,
    };
    let until = std::time::Instant::now() + SETTLE;
    while !done {
        match receiver.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
            Ok(batch) => known.extend(batch),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => done = true,
        }
    }
    if listing.randomize {
        Rng::from_clock().shuffle(&mut known);
    } else {
        let sort = listing.sort;
        // In the listing's order first, as the folders come in out of it: by
        // name it is then nearly in order, which the sort makes quick work of.
        known.sort_by(|a, b| a.placed.rank.cmp(&b.placed.rank));
        known.sort_by(|a, b| found_order(sort, (&a.path, &a.placed), (&b.path, &b.placed)));
        if listing.reverse {
            known.reverse();
        }
    }
    let files = known.iter().map(|found| found.path.clone()).collect();
    (Playlist { files, start: 0 }, (!done).then_some(Scan { receiver, known }))
}

/// The pictures and videos of one folder in natural order, each with its
/// time or size if `sort` is by that, and its subfolders in natural order.
fn read_folder(dir: &Path, recursive: bool, sort: Option<Sort>, only: Option<Only>) -> (Vec<(PathBuf, u64)>, Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("jog: cannot read directory {}", dir.display());
        return (Vec::new(), Vec::new());
    };
    let (mut files, mut subdirs) = (Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = entry.file_type().ok();
        let linked = kind.is_none_or(|kind| kind.is_symlink());
        let is_dir = if linked { path.is_dir() } else { kind.is_some_and(|kind| kind.is_dir()) };
        if recursive && is_dir {
            subdirs.push(path);
        } else if is_supported(&path) && of_kind(&path, only) {
            // The listing's own record where it has one, which on a NAS
            // saves a trip; through a link, the file it leads to.
            let key = sort_key(sort, || if linked { std::fs::metadata(&path).ok() } else { entry.metadata().ok() });
            files.push((path, key));
        }
    }
    files.sort_by(|a, b| natural_cmp(&name_of(&a.0), &name_of(&b.0)));
    subdirs.sort_by(|a, b| natural_cmp(&name_of(a), &name_of(b)));
    (files, subdirs)
}

/// Appends the images in `dir` in natural order, subdirectories last.
fn list_dir(dir: &Path, recursive: bool, only: Option<Only>, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("jog: cannot read directory {}", dir.display());
        return;
    };
    let mut images = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // The directory listing already says what each entry is, which
        // saves asking the file system again for every file.
        let is_dir = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => path.is_dir(),
            Ok(kind) => kind.is_dir(),
            Err(_) => path.is_dir(),
        };
        if recursive && is_dir {
            subdirs.push(path);
        } else if is_supported(&path) && of_kind(&path, only) {
            images.push(path);
        }
    }
    images.sort_by(|a, b| natural_cmp(&name_of(a), &name_of(b)));
    subdirs.sort_by(|a, b| natural_cmp(&name_of(a), &name_of(b)));
    files.extend(images);
    for subdir in subdirs {
        list_dir(&subdir, recursive, only, files);
    }
}

fn name_of(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn sort_files(files: &mut [PathBuf], sort: Sort) {
    let mut order: Vec<usize> = (0..files.len()).collect();
    sort_ids(&mut order, files, sort);
    let sorted: Vec<PathBuf> = order.into_iter().map(|at| std::mem::take(&mut files[at])).collect();
    files.iter_mut().zip(sorted).for_each(|(slot, file)| *slot = file);
}

/// Puts file ids in order, `files` being the path of each id.
pub fn sort_ids(ids: &mut [usize], files: &[PathBuf], sort: Sort) {
    let path = |id: &usize| files[*id].as_path();
    let by_filename = |a: &usize, b: &usize| natural_cmp(&name_of(path(a)), &name_of(path(b)));
    match sort {
        Sort::Name => ids.sort_by(|a, b| natural_cmp(&path(a).to_string_lossy(), &path(b).to_string_lossy())),
        Sort::Filename => ids.sort_by(by_filename),
        Sort::Dirname => ids.sort_by(|a, b| {
            let (a_path, b_path) = (path(a), path(b));
            let dirs = (a_path.parent().unwrap_or(a_path), b_path.parent().unwrap_or(b_path));
            natural_cmp(&dirs.0.to_string_lossy(), &dirs.1.to_string_lossy()).then_with(|| by_filename(a, b))
        }),
        // By when it was last changed, or by how large it is, either way.
        Sort::Newest => ids.sort_by_cached_key(|item| std::cmp::Reverse(std::fs::metadata(path(item)).and_then(|m| m.modified()).ok())),
        Sort::Oldest => ids.sort_by_cached_key(|item| std::fs::metadata(path(item)).and_then(|m| m.modified()).ok()),
        Sort::Smallest => ids.sort_by_cached_key(|item| std::fs::metadata(path(item)).map_or(0, |m| m.len())),
        Sort::Largest => ids.sort_by_cached_key(|item| std::cmp::Reverse(std::fs::metadata(path(item)).map_or(0, |m| m.len()))),
    }
}

/// Case-insensitive ordering that compares digit runs numerically, so
/// `img2` sorts before `img10`.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (ei, ej) = (digits_end(a, i), digits_end(b, j));
            let (da, db) = (trim_zeros(&a[i..ei]), trim_zeros(&b[j..ej]));
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
            (i, j) = (ei, ej);
        } else {
            let ord = a[i].to_ascii_lowercase().cmp(&b[j].to_ascii_lowercase());
            if ord != Ordering::Equal {
                return ord;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

/// Bytes that, compared as they are, put strings in the order `natural_cmp`
/// does: lower case, and each run of digits its length then its digits, so
/// that a longer number comes later. Made once, they spare the many
/// comparisons of a sort working their way through long paths each time.
fn natural_key(s: &str, out: &mut Vec<u8>) {
    let s = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if s[i].is_ascii_digit() {
            let end = digits_end(s, i);
            let digits = trim_zeros(&s[i..end]);
            // A digit's own range, so that against what is not a digit it
            // falls where a digit would; up to nine digits said in it, and
            // more in a byte after.
            if digits.len() < 9 {
                out.push(b'0' + digits.len() as u8);
            } else {
                out.extend([b'9', digits.len().min(255) as u8]);
            }
            out.extend_from_slice(digits);
            i = end;
        } else {
            out.push(s[i].to_ascii_lowercase());
            i += 1;
        }
    }
    // Alike that far, the shorter first, then as they are spelled.
    out.push(0);
    out.extend_from_slice(s);
    out.push(0);
}

/// What `sort` puts a file in order by when that is its name or path.
fn name_key(sort: Option<Sort>, path: &Path) -> Vec<u8> {
    let mut key = Vec::new();
    match sort {
        Some(Sort::Name) => natural_key(&path.to_string_lossy(), &mut key),
        Some(Sort::Filename) => natural_key(&path.file_name().unwrap_or_default().to_string_lossy(), &mut key),
        Some(Sort::Dirname) => {
            natural_key(&path.parent().unwrap_or(path).to_string_lossy(), &mut key);
            natural_key(&path.file_name().unwrap_or_default().to_string_lossy(), &mut key);
        }
        _ => {}
    }
    key
}

fn digits_end(s: &[u8], from: usize) -> usize {
    s[from..].iter().position(|c| !c.is_ascii_digit()).map_or(s.len(), |n| from + n)
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|&&c| c == b'0').count();
    &digits[zeros.min(digits.len() - 1)..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_lists_what_the_walk_does_in_its_order() {
        let dir = std::env::temp_dir().join(format!("jog-scan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (at, file) in ["b/2.jpg", "b/10.png", "a.jpg", "b/c/z.mp4", "b/c/a.gif", "d/x.jpg", "d/notes.txt", "e/f/g/deep.webp", "b1.png"].iter().enumerate() {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![0u8; at * 10 + 1]).unwrap();
            // Each a second older than the one before it.
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(at as u64 * 60);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
        }
        let args = [dir.clone()];
        for sort in [None, Some(Sort::Name), Some(Sort::Filename), Some(Sort::Dirname), Some(Sort::Newest), Some(Sort::Oldest), Some(Sort::Largest)] {
            for (recursive, reverse) in [(true, false), (true, true), (false, false)] {
                let listing = Listing { recursive, sort, reverse, ..Listing::default() };
                let walked = collect(&args, &listing).files;
                let (playlist, rest) = first_found(&args, &listing);
                assert!(rest.is_none(), "a small folder is listed whole before it is shown");
                assert_eq!(playlist.files, walked, "{sort:?} recursive {recursive} reverse {reverse}");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn natural_keys_compare_as_natural_order_does() {
        // Random names from letters of both cases, digits with leading
        // zeros and long runs, and what comes between them.
        let mut rng = Rng::from_clock();
        let letters: Vec<char> = "aAbZz0012789._-/ é".chars().collect();
        let names: Vec<String> = (0..3000)
            .map(|_| {
                let length = rng.below(14);
                let mut name: String = (0..length).map(|_| letters[rng.below(letters.len())]).collect();
                if rng.below(5) == 0 {
                    name.push_str(&"7".repeat(rng.below(14)));
                }
                name
            })
            .collect();
        let key = |s: &str| {
            let mut key = Vec::new();
            natural_key(s, &mut key);
            key
        };
        let keys: Vec<Vec<u8>> = names.iter().map(|name| key(name)).collect();
        for _ in 0..200_000 {
            let (a, b) = (rng.below(names.len()), rng.below(names.len()));
            assert_eq!(keys[a].cmp(&keys[b]), natural_cmp(&names[a], &names[b]), "{:?} {:?}", names[a], names[b]);
        }
    }

    #[test]
    fn natural_order() {
        let mut names = vec!["img10.jpg", "IMG2.jpg", "img1.jpg", "a.png", "img002b.jpg", "img02.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["a.png", "img1.jpg", "IMG2.jpg", "img02.jpg", "img002b.jpg", "img10.jpg"]);
    }
}

/// What format an image file is, by its first bytes and, for camera RAW
/// files, which share TIFF's, by its ending: a short name for a badge.
pub fn format_name(path: &Path) -> Option<String> {
    if is_raw(path) {
        let ending = path.extension()?.to_str()?.to_uppercase();
        return Some(format!("RAW {ending}"));
    }
    Some(sniff_file(path)?.to_string())
}

/// The format named by a file's first bytes, which RAW files share with
/// TIFF; `format_name` tells those apart.
pub fn sniff_file(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 48];
    let read = std::fs::File::open(path).and_then(|mut file| file.read(&mut head)).ok()?;
    sniff(&head[..read])
}

fn sniff(head: &[u8]) -> Option<&'static str> {
    let brand_is = |brands: &[&[u8; 4]]| {
        // The major brand, then the compatible ones, in the ftyp box.
        let size = u32::from_be_bytes(head.get(..4)?.try_into().ok()?) as usize;
        let listed: Vec<&[u8]> = std::iter::once(head.get(8..12)?).chain(head.get(16..size.min(head.len()))?.chunks(4)).collect();
        Some(listed.iter().any(|brand| brands.iter().any(|wanted| brand == wanted)))
    };
    Some(match head {
        [0xFF, 0xD8, 0xFF, ..] => "JPEG",
        [0x89, b'P', b'N', b'G', ..] => "PNG",
        [b'G', b'I', b'F', b'8', ..] => "GIF",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "WebP",
        [0xFF, 0x0A, ..] | [0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', ..] => "JPEG XL",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => {
            if brand_is(&[b"avif", b"avis"])? {
                "AVIF"
            } else if brand_is(&[b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx"])? {
                "HEIC"
            } else if brand_is(&[b"mif1", b"msf1"])? {
                "HEIF"
            } else {
                return None;
            }
        }
        [b'I', b'I', 0x2A, 0, ..] | [b'M', b'M', 0, 0x2A, ..] => "TIFF",
        [b'B', b'M', ..] => "BMP",
        [b'q', b'o', b'i', b'f', ..] => "QOI",
        [0x76, 0x2F, 0x31, 0x01, ..] => "EXR",
        [b'8', b'B', b'P', b'S', ..] => "PSD",
        [b'#', b'?', b'R', b'A', ..] | [b'#', b'?', b'R', b'G', ..] => "HDR",
        [b'D', b'D', b'S', b' ', ..] => "DDS",
        [0, 0, 1, 0, ..] => "ICO",
        [b'P', b'1'..=b'7', ..] => "PNM",
        _ if head.starts_with(b"<?xml") || head.starts_with(b"<svg") => "SVG",
        _ => return None,
    })
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn formats_are_told_by_their_first_bytes() {
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE1rest"), Some("JPEG"));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n"), Some("PNG"));
        assert_eq!(sniff(b"\x00\x00\x00\x1cftypavif\x00\x00\x00\x00avifmif1miaf"), Some("AVIF"));
        assert_eq!(sniff(b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00mif1heic"), Some("HEIC"));
        // A HEIF whose major brand is the general one, with AVIF among the rest.
        assert_eq!(sniff(b"\x00\x00\x00\x1cftypmif1\x00\x00\x00\x00mif1avifmiaf"), Some("AVIF"));
        assert_eq!(sniff(b"\xFF\x0A\xFA"), Some("JPEG XL"));
        assert_eq!(sniff(b"\x00\x00\x00\x0cJXL \r\n\x87\n"), Some("JPEG XL"));
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("WebP"));
        assert_eq!(sniff(b"nothing known"), None);
    }
}
