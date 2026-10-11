//! Where each video was left, so that it can be picked up there next time.

use std::path::{Path, PathBuf};

/// How many videos to remember; the least recently watched go first.
const MAX_ENTRIES: usize = 2000;

#[derive(Clone, PartialEq, Debug)]
struct Entry {
    seconds: f64,
    /// Size of the file when the position was taken. A different size means
    /// a different video has taken the name.
    size: u64,
    path: PathBuf,
}

/// The remembered positions, oldest first, and the file they are kept in.
pub struct Positions {
    file: Option<PathBuf>,
    entries: Vec<Entry>,
}

/// `$JOG_STATE`, or `jog` under `$XDG_STATE_HOME` or `~/.local/state`.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("JOG_STATE") {
        return Some(dir.into());
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/state")))?;
    Some(base.join("jog"))
}

/// One entry a line: seconds, size and path, separated by tabs.
fn parse(text: &str) -> Vec<Entry> {
    let entry = |line: &str| {
        let mut parts = line.splitn(3, '\t');
        let (seconds, size, path) = (parts.next()?, parts.next()?, parts.next()?);
        Some(Entry { seconds: seconds.parse().ok()?, size: size.parse().ok()?, path: path.into() })
    };
    text.lines().filter_map(entry).collect()
}

fn render(entries: &[Entry]) -> String {
    entries.iter().map(|e| format!("{:.1}\t{}\t{}\n", e.seconds, e.size, e.path.display())).collect()
}

/// A file's identity for this purpose: its absolute path and its size.
pub fn identify(file: &Path) -> Option<(PathBuf, u64)> {
    Some((std::path::absolute(file).ok()?, std::fs::metadata(file).ok()?.len()))
}

impl Positions {
    pub fn load() -> Positions {
        let file = state_dir().map(|dir| dir.join("positions"));
        let text = file.as_ref().and_then(|file| std::fs::read_to_string(file).ok());
        Positions { file, entries: text.map_or_else(Vec::new, |text| parse(&text)) }
    }

    /// Where `file` was left, in seconds, if that is known.
    pub fn get(&self, file: &Path) -> Option<f64> {
        let (path, size) = identify(file)?;
        self.entries.iter().find(|e| e.path == path && e.size == size).map(|e| e.seconds)
    }

    /// Records where `file` was left, or with `None` forgets it, and writes
    /// the list out.
    pub fn set(&mut self, file: &Path, seconds: Option<f64>) {
        let Some((path, size)) = identify(file) else { return };
        let before = self.entries.len();
        self.entries.retain(|e| e.path != path);
        if seconds.is_none() && self.entries.len() == before {
            return;
        }
        if let Some(seconds) = seconds {
            self.entries.push(Entry { seconds, size, path });
        }
        let excess = self.entries.len().saturating_sub(MAX_ENTRIES);
        self.entries.drain(..excess);
        self.save();
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        // Written beside the real file and moved into place, so that a crash
        // part-way cannot leave half a list.
        let partial = file.with_extension("new");
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&partial, render(&self.entries)))
            .and_then(|()| std::fs::rename(&partial, file));
        if let Err(error) = written {
            eprintln!("jog: cannot remember video positions in {}: {error}", file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_survive_a_round_trip() {
        let entries = vec![
            Entry { seconds: 12.5, size: 1000, path: "/videos/a b.mkv".into() },
            Entry { seconds: 3600.0, size: 5, path: "/videos/tab\tin name.mp4".into() },
        ];
        assert_eq!(parse(&render(&entries)), entries);
        assert_eq!(parse("nonsense\n\n1.0\tnot a size\t/x\n"), []);
    }

    #[test]
    fn positions_are_set_replaced_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("jog-positions-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (video, other) = (dir.join("video.mp4"), dir.join("other.mp4"));
        std::fs::write(&video, "12345").unwrap();
        std::fs::write(&other, "1").unwrap();

        let mut positions = Positions { file: Some(dir.join("state/positions")), entries: Vec::new() };
        positions.set(&video, Some(40.0));
        positions.set(&other, Some(7.0));
        positions.set(&video, Some(90.0));
        assert_eq!(positions.get(&video), Some(90.0));
        assert_eq!(positions.entries.len(), 2);

        // What was written can be read back.
        let reread = parse(&std::fs::read_to_string(dir.join("state/positions")).unwrap());
        assert_eq!(reread, positions.entries);

        // A file of another size under the same name is another video.
        std::fs::write(&video, "a longer file").unwrap();
        assert_eq!(positions.get(&video), None);

        positions.set(&other, None);
        assert_eq!(positions.get(&other), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
