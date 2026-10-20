//! Files a player process has crashed on, kept from run to run, so that
//! the same file does not crash it again: one that crashed the GPU's
//! decoder is decoded on the CPU from then on, and one that crashed the
//! CPU's too is not played at all.
//!
//! Kept in `player-crashes` in the state directory, a line a file: `gpu`
//! or `software`, a tab, the path.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// What a file has crashed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Crash {
    /// The GPU's decoder: it is decoded on the CPU.
    Gpu,
    /// The CPU's decoder as well: it is not played.
    Software,
}

impl Crash {
    fn word(self) -> &'static str {
        match self {
            Crash::Gpu => "gpu",
            Crash::Software => "software",
        }
    }

    fn from_word(word: &str) -> Option<Crash> {
        match word {
            "gpu" => Some(Crash::Gpu),
            "software" => Some(Crash::Software),
            _ => None,
        }
    }
}

fn file() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("player-crashes"))
}

fn known() -> &'static Mutex<HashMap<PathBuf, Crash>> {
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, Crash>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(file().and_then(|file| std::fs::read_to_string(file).ok()).map(|text| parse(&text)).unwrap_or_default()))
}

fn parse(text: &str) -> HashMap<PathBuf, Crash> {
    text.lines()
        .filter_map(|line| {
            let (word, path) = line.split_once('\t')?;
            Some((PathBuf::from(path), Crash::from_word(word)?))
        })
        .collect()
}

fn lines(known: &HashMap<PathBuf, Crash>) -> String {
    let mut lines: Vec<String> = known.iter().map(|(path, crash)| format!("{}\t{}", crash.word(), path.display())).collect();
    lines.sort();
    lines.into_iter().map(|line| line + "\n").collect()
}

/// The same file however it was named.
fn key(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Reads the list, if it has not been: on a thread that may wait on the
/// disk, before the first file is opened on the main one.
pub(super) fn load() {
    let _ = known();
}

/// What `path` has crashed before, if anything.
pub fn crashed_on(path: &Path) -> Option<Crash> {
    known().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key(path)).copied()
}

/// Every file that has crashed a player, and what it crashed, by path.
pub fn all_crashes() -> Vec<(PathBuf, Crash)> {
    let mut all: Vec<(PathBuf, Crash)> = known().lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().map(|(path, crash)| (path.clone(), *crash)).collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

/// Forgets that `path` crashed a player, to try it on the GPU again;
/// the list written down on another thread.
pub fn forget_crash(path: &Path) {
    let text = {
        let mut known = known().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        known.remove(&key(path));
        lines(&known)
    };
    std::thread::spawn(move || write_list(&text));
}

/// Writes the list whole under another name and moves it over: a crash
/// of jog itself halfway through leaves the old list, not half a new one.
fn write_list(text: &str) {
    let Some(file) = file() else { return };
    let partial = file.with_extension("part");
    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::File::create(&partial)).and_then(|mut out| out.write_all(text.as_bytes())).and_then(|()| std::fs::rename(&partial, &file));
    if let Err(error) = written {
        eprintln!("jog: cannot keep the list of files that crashed the player: {error}");
    }
}

/// Notes a crash with `path` open, its GPU decoder in use or not, and
/// writes the list down; what it now counts as. Called from the thread
/// that saw the player go, never the main one.
pub(super) fn note(path: &Path, gpu: bool) -> Crash {
    let crash = if gpu { Crash::Gpu } else { Crash::Software };
    let mut known = known().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = match known.get(&key(path)) {
        Some(Crash::Software) => Crash::Software,
        _ => crash,
    };
    known.insert(key(path), now);
    write_list(&lines(&known));
    now
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_reads_as_written() {
        let mut known = HashMap::new();
        known.insert(PathBuf::from("/v/a b.webm"), Crash::Gpu);
        known.insert(PathBuf::from("/v/c.mkv"), Crash::Software);
        assert_eq!(parse(&lines(&known)), known);
        assert!(parse("nonsense\nfoo\t/x\n").is_empty(), "lines it does not know are passed over");
    }
}
