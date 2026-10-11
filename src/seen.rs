//! Which videos have been watched, so that the ones still new can be told
//! from the rest the next time a folder is gone through.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::positions::{identify, state_dir};

/// How many videos to remember; the ones watched longest ago go first.
const MAX_ENTRIES: usize = 50_000;

/// The videos watched, each with the size it had, and the file they are
/// kept in.
pub struct Seen {
    file: Option<PathBuf>,
    seen: HashMap<PathBuf, u64>,
}

/// One video a line, oldest first: its size and path, separated by a tab.
fn parse(text: &str) -> Vec<(PathBuf, u64)> {
    let entry = |line: &str| {
        let (size, path) = line.split_once('\t')?;
        Some((PathBuf::from(path), size.parse().ok()?))
    };
    text.lines().filter_map(entry).collect()
}

fn render(path: &Path, size: u64) -> String {
    format!("{size}\t{}\n", path.display())
}

impl Seen {
    pub fn load() -> Seen {
        let file = state_dir().map(|dir| dir.join("seen"));
        let text = file.as_ref().and_then(|file| std::fs::read_to_string(file).ok());
        let entries = text.map_or_else(Vec::new, |text| parse(&text));
        // Lines are only ever added, so now and then the oldest are dropped.
        if let (Some(file), true) = (&file, entries.len() > MAX_ENTRIES * 2) {
            let kept: String = entries[entries.len() - MAX_ENTRIES..].iter().map(|(path, size)| render(path, *size)).collect();
            let partial = file.with_extension("new");
            let _ = std::fs::write(&partial, kept).and_then(|()| std::fs::rename(&partial, file));
        }
        Seen { file, seen: entries.into_iter().collect() }
    }

    /// Whether `file` has been watched. A file of another size under the
    /// same name is another video.
    pub fn has(&self, file: &Path) -> bool {
        identify(file).is_some_and(|(path, size)| self.seen.get(&path) == Some(&size))
    }

    /// Records that `file` has been watched.
    pub fn add(&mut self, file: &Path) {
        let Some((path, size)) = identify(file) else { return };
        if self.seen.get(&path) == Some(&size) {
            return;
        }
        if let Some(file) = &self.file {
            let written = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(file))
                .and_then(|mut list| list.write_all(render(&path, size).as_bytes()));
            if let Err(error) = written {
                eprintln!("jog: cannot remember watched videos in {}: {error}", file.display());
            }
        }
        self.seen.insert(path, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_videos_are_remembered_by_path_and_size() {
        let dir = std::env::temp_dir().join(format!("jog-seen-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (video, other) = (dir.join("video.mp4"), dir.join("other.mp4"));
        std::fs::write(&video, "12345").unwrap();
        std::fs::write(&other, "1").unwrap();

        let mut seen = Seen { file: Some(dir.join("state/seen")), seen: HashMap::new() };
        assert!(!seen.has(&video));
        seen.add(&video);
        seen.add(&video);
        assert!(seen.has(&video) && !seen.has(&other));

        // What was written can be read back, once.
        let reread = parse(&std::fs::read_to_string(dir.join("state/seen")).unwrap());
        assert_eq!(reread, [(std::path::absolute(&video).unwrap(), 5)]);

        // A file of another size under the same name is another video.
        std::fs::write(&video, "a longer file").unwrap();
        assert!(!seen.has(&video));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
