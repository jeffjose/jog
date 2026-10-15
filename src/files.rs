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
            Ok(text) => files.extend(text.lines().filter(|l| !l.is_empty()).map(PathBuf::from)),
            Err(error) => eprintln!("jog: cannot read file list {}: {error}", list.display()),
        }
    }

    match args {
        [] if listing.filelist.is_none() => list_dir(Path::new("."), listing.recursive, &mut files),
        _ => {
            for arg in args {
                if arg.is_dir() {
                    list_dir(arg, listing.recursive, &mut files);
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
    let plain = Listing { recursive: listing.recursive, filelist: listing.filelist.clone(), ..Listing::default() };
    gather(args, &plain, false).files
}

/// Appends the images in `dir` in natural order, subdirectories last.
fn list_dir(dir: &Path, recursive: bool, files: &mut Vec<PathBuf>) {
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
        } else if is_supported(&path) {
            images.push(path);
        }
    }
    images.sort_by(|a, b| natural_cmp(&name_of(a), &name_of(b)));
    subdirs.sort_by(|a, b| natural_cmp(&name_of(a), &name_of(b)));
    files.extend(images);
    for subdir in subdirs {
        list_dir(&subdir, recursive, files);
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
