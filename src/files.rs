use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::rng::Rng;

const RASTER_EXTS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff",
];

const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef",
    "srw", "3fr", "erf", "kdc", "mrw", "iiq", "rwl", "x3f",
];

const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "mpg", "mpeg", "wmv", "flv", "ts", "m2ts",
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

fn is_supported(path: &Path) -> bool {
    has_ext(path, RASTER_EXTS) || is_raw(path) || is_video(path)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sort {
    /// Natural order of the whole path.
    Name,
    /// Natural order of the file name, ignoring directories.
    Filename,
    /// Grouped by directory, then by file name.
    Dirname,
    /// Most recently modified first.
    Mtime,
    /// Smallest first.
    Size,
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
/// A single file opens its whole directory, starting at that file. Otherwise
/// directories expand to the images inside them and files are taken as given.
pub fn collect(args: &[PathBuf], listing: &Listing) -> Playlist {
    let mut files = Vec::new();
    let mut start_at = listing.start_at.clone();

    if let Some(list) = &listing.filelist {
        match std::fs::read_to_string(list) {
            Ok(text) => files.extend(text.lines().filter(|l| !l.is_empty()).map(PathBuf::from)),
            Err(error) => eprintln!("jog: cannot read file list {}: {error}", list.display()),
        }
    }

    match args {
        [only] if only.is_file() && listing.filelist.is_none() => {
            let dir = match only.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            };
            list_dir(dir, listing.recursive, &mut files);
            if !files.iter().any(|f| f.file_name() == only.file_name()) {
                // Explicitly named, so show it even though the extension is unknown.
                files.insert(0, only.clone());
            }
            start_at.get_or_insert_with(|| only.clone());
        }
        [] if listing.filelist.is_none() => list_dir(Path::new("."), listing.recursive, &mut files),
        _ => {
            for arg in args {
                if arg.is_dir() {
                    list_dir(arg, listing.recursive, &mut files);
                } else {
                    files.push(arg.clone());
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

/// Appends the images in `dir` in natural order, subdirectories last.
fn list_dir(dir: &Path, recursive: bool, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("jog: cannot read directory {}", dir.display());
        return;
    };
    let mut images = Vec::new();
    let mut subdirs = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if recursive && path.is_dir() {
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
    let by_filename = |a: &PathBuf, b: &PathBuf| natural_cmp(&name_of(a), &name_of(b));
    match sort {
        Sort::Name => files.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy())),
        Sort::Filename => files.sort_by(by_filename),
        Sort::Dirname => files.sort_by(|a, b| {
            let dirs = (a.parent().unwrap_or(a), b.parent().unwrap_or(b));
            natural_cmp(&dirs.0.to_string_lossy(), &dirs.1.to_string_lossy()).then_with(|| by_filename(a, b))
        }),
        Sort::Mtime => files.sort_by_cached_key(|f| {
            std::cmp::Reverse(std::fs::metadata(f).and_then(|m| m.modified()).ok())
        }),
        Sort::Size => files.sort_by_cached_key(|f| std::fs::metadata(f).map_or(0, |m| m.len())),
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
