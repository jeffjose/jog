use std::cmp::Ordering;
use std::path::{Path, PathBuf};

const RASTER_EXTS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff",
];

const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef",
    "srw", "3fr", "erf", "kdc", "mrw", "iiq", "rwl", "x3f",
];

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

pub fn is_raw(path: &Path) -> bool {
    has_ext(path, RAW_EXTS)
}

fn is_supported(path: &Path) -> bool {
    has_ext(path, RASTER_EXTS) || is_raw(path)
}

pub struct Playlist {
    pub files: Vec<PathBuf>,
    pub start: usize,
}

/// Turns command-line paths into the list of images to show.
///
/// A single file opens its whole directory, starting at that file. Otherwise
/// directories expand to the images inside them and files are taken as given.
pub fn collect(args: &[PathBuf]) -> Playlist {
    if let [only] = args {
        if only.is_file() {
            return siblings(only);
        }
    }

    let mut files = Vec::new();
    if args.is_empty() {
        files = list_dir(Path::new("."));
    }
    for arg in args {
        if arg.is_dir() {
            files.extend(list_dir(arg));
        } else {
            files.push(arg.clone());
        }
    }
    Playlist { files, start: 0 }
}

fn siblings(file: &Path) -> Playlist {
    let dir = match file.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut files = list_dir(dir);
    let position = files.iter().position(|f| f.file_name() == file.file_name());
    let start = position.unwrap_or_else(|| {
        // Explicitly named, so show it even though the extension is unknown.
        files.insert(0, file.to_path_buf());
        0
    });
    Playlist { files, start }
}

fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("jog: cannot read directory {}", dir.display());
        return Vec::new();
    };
    let mut named: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_supported(p))
        .map(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            (name, p)
        })
        .collect();
    named.sort_by(|a, b| natural_cmp(&a.0, &b.0));
    named.into_iter().map(|(_, p)| p).collect()
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
