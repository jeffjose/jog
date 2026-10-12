//! What has been decided about each file in a triage: kept, or rejected.
//! Only that it was decided is recorded here; nothing is done to a file.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::positions::state_dir;

/// What was decided about a file.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Verdict {
    Kept,
    Rejected,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Verdict::Kept => "kept",
            Verdict::Rejected => "rejected",
        }
    }
}

/// The decisions made so far, by the full path of the file, and the file
/// they are kept in.
pub struct Triage {
    file: Option<PathBuf>,
    verdicts: HashMap<PathBuf, Verdict>,
}

/// One decision a line, in the order they were made: `kept`, `rejected`
/// or `undecided`, a tab, and the path. A later line for a file takes
/// the place of an earlier one.
fn parse(text: &str) -> HashMap<PathBuf, Verdict> {
    let mut verdicts = HashMap::new();
    for line in text.lines() {
        let Some((word, path)) = line.split_once('\t') else { continue };
        match word {
            "kept" => drop(verdicts.insert(PathBuf::from(path), Verdict::Kept)),
            "rejected" => drop(verdicts.insert(PathBuf::from(path), Verdict::Rejected)),
            _ => drop(verdicts.remove(Path::new(path))),
        }
    }
    verdicts
}

fn full(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Triage {
    pub fn load() -> Triage {
        let file = state_dir().map(|dir| dir.join("triage"));
        let text = file.as_ref().and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default();
        Triage { file, verdicts: parse(&text) }
    }

    pub fn of(&self, path: &Path) -> Option<Verdict> {
        self.verdicts.get(&full(path)).copied()
    }

    /// Records what was decided about a file, or with `None` that it is
    /// undecided again, at once: a decision is not to be lost to a crash.
    pub fn decide(&mut self, path: &Path, verdict: Option<Verdict>) {
        let path = full(path);
        match verdict {
            Some(verdict) => drop(self.verdicts.insert(path.clone(), verdict)),
            None => drop(self.verdicts.remove(&path)),
        }
        let Some(file) = &self.file else { return };
        let line = format!("{}\t{}\n", verdict.map_or("undecided", Verdict::word), path.display());
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(file))
            .and_then(|mut list| list.write_all(line.as_bytes()));
        if let Err(error) = written {
            eprintln!("jog: cannot keep what was decided in {}: {error}", file.display());
        }
    }
}

/// Writes the files of a session out in three parts, the kept, the
/// rejected and the undecided, each a path a line, to a file named after
/// the session; and gives back where that is. To be read by a person or
/// fed to a script: nothing in jog reads it.
pub fn write_out(session: &str, files: &[(PathBuf, Option<Verdict>)]) -> Option<PathBuf> {
    let to = state_dir()?.join("triage-lists").join(format!("{session}.txt"));
    let part = |title: &str, wanted: Option<Verdict>| {
        let paths: Vec<String> = files.iter().filter(|(_, verdict)| *verdict == wanted).map(|(path, _)| full(path).display().to_string()).collect();
        format!("# {title} ({})\n{}\n", paths.len(), paths.iter().map(|path| format!("{path}\n")).collect::<String>())
    };
    let text = [part("kept", Some(Verdict::Kept)), part("rejected", Some(Verdict::Rejected)), part("undecided", None)].concat();
    std::fs::create_dir_all(to.parent()?).and_then(|()| std::fs::write(&to, text)).ok()?;
    Some(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_word_on_a_file_is_what_counts() {
        let verdicts = parse("kept\t/a\nrejected\t/b\nrejected\t/a\nkept\t/c\nundecided\t/c\nnonsense\n");
        assert_eq!(verdicts.get(Path::new("/a")), Some(&Verdict::Rejected));
        assert_eq!(verdicts.get(Path::new("/b")), Some(&Verdict::Rejected));
        assert_eq!(verdicts.get(Path::new("/c")), None);
    }
}
