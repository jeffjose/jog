//! What jog was showing each time it was quit, by the folder it was
//! started in, so that it can be picked up there again.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::files::Sort;
use crate::positions::state_dir;

/// How many sessions are kept for each folder jog was started in; the
/// oldest go first.
const KEPT: usize = 10;

/// One run of jog, as it stood when it was quit.
#[derive(Clone, PartialEq, Debug)]
pub struct Session {
    /// The folder jog was started in.
    pub cwd: PathBuf,
    /// When it was quit, in seconds since 1970.
    pub when: u64,
    /// What was opened, and how it was listed.
    pub paths: Vec<PathBuf>,
    pub recursive: bool,
    pub sort: Option<Sort>,
    pub reverse: bool,
    pub filelist: Option<PathBuf>,
    /// The search the list was narrowed to, if it was.
    pub filter: Option<String>,
    /// The file on view, where it was in the list and how long that was,
    /// and how far into it, for a video.
    pub file: PathBuf,
    pub pos: usize,
    pub total: usize,
    pub seconds: f64,
}

const SORTS: [(&str, Sort); 7] = [
    ("name", Sort::Name),
    ("filename", Sort::Filename),
    ("dirname", Sort::Dirname),
    ("newest", Sort::Newest),
    ("oldest", Sort::Oldest),
    ("largest", Sort::Largest),
    ("smallest", Sort::Smallest),
];

/// A tab or a line break in a name would break the line a session is
/// kept on; neither is worth more than a space there.
fn plain(text: &str) -> String {
    text.replace(['\t', '\n', '\u{1f}'], " ")
}

/// One session a line, its parts separated by tabs; the paths opened are
/// the last part, with a unit separator between them.
fn render(session: &Session) -> String {
    let sort = session.sort.and_then(|sort| SORTS.iter().find(|(_, other)| *other == sort)).map_or("", |(name, _)| name);
    let paths: Vec<String> = session.paths.iter().map(|path| plain(&path.to_string_lossy())).collect();
    let parts = [
        plain(&session.cwd.to_string_lossy()),
        session.when.to_string(),
        (session.recursive as u8).to_string(),
        sort.to_string(),
        (session.reverse as u8).to_string(),
        session.filelist.as_ref().map(|file| plain(&file.to_string_lossy())).unwrap_or_default(),
        session.filter.as_deref().map(plain).unwrap_or_default(),
        plain(&session.file.to_string_lossy()),
        session.pos.to_string(),
        session.total.to_string(),
        format!("{:.1}", session.seconds),
        paths.join("\u{1f}"),
    ];
    parts.join("\t")
}

fn parse(line: &str) -> Option<Session> {
    let parts: Vec<&str> = line.split('\t').collect();
    let [cwd, when, recursive, sort, reverse, filelist, filter, file, pos, total, seconds, paths] = parts[..] else { return None };
    Some(Session {
        cwd: cwd.into(),
        when: when.parse().ok()?,
        paths: paths.split('\u{1f}').filter(|path| !path.is_empty()).map(PathBuf::from).collect(),
        recursive: recursive == "1",
        sort: SORTS.iter().find(|(name, _)| *name == sort).map(|(_, sort)| *sort),
        reverse: reverse == "1",
        filelist: Some(filelist).filter(|file| !file.is_empty()).map(PathBuf::from),
        filter: Some(filter).filter(|typed| !typed.is_empty()).map(str::to_string),
        file: file.into(),
        pos: pos.parse().ok()?,
        total: total.parse().ok()?,
        seconds: seconds.parse().ok()?,
    })
}

fn file() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("sessions"))
}

fn load() -> Vec<Session> {
    let text = file().and_then(|file| std::fs::read_to_string(file).ok()).unwrap_or_default();
    text.lines().filter_map(parse).collect()
}

/// The sessions of jog started in `cwd`, the latest first.
pub fn here(cwd: &Path) -> Vec<Session> {
    let mut found: Vec<Session> = load().into_iter().filter(|session| session.cwd == cwd).collect();
    found.sort_by_key(|session| std::cmp::Reverse(session.when));
    found
}

/// Keeps a session, and of those of its folder only the latest few.
pub fn save(session: Session) {
    let Some(file) = file() else { return };
    let mut all = load();
    // One of the same things opened takes the place of the one before:
    // the sessions of a folder are told apart by what they were of.
    all.retain(|other| !(other.cwd == session.cwd && other.paths == session.paths && other.filelist == session.filelist));
    all.push(session.clone());
    let mut of_folder = all.iter().filter(|other| other.cwd == session.cwd).count();
    all.retain(|other| {
        let surplus = other.cwd == session.cwd && of_folder > KEPT;
        of_folder -= surplus as usize;
        !surplus
    });
    let text: String = all.iter().map(|session| render(session) + "\n").collect();
    let partial = file.with_extension("new");
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&partial, text))
        .and_then(|()| std::fs::rename(&partial, &file));
    if let Err(error) = written {
        eprintln!("jog: cannot keep this session in {}: {error}", file.display());
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// How long ago a session was quit, in a word or two.
pub fn ago(session: &Session) -> String {
    match now().saturating_sub(session.when) {
        0..=59 => "just now".to_string(),
        seconds @ 60..=3599 => format!("{} min ago", seconds / 60),
        seconds @ 3600..=86_399 => format!("{} h ago", seconds / 3600),
        seconds => format!("{} d ago", seconds / 86_400),
    }
}

/// What a session was of, in a word or two: the folders or files opened,
/// by their own names.
pub fn title(session: &Session) -> String {
    let names: Vec<String> = session.paths.iter().map(|path| path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()).collect();
    names.join(", ")
}

/// Where a session was left, to go under its title: when, how far
/// through, and the file.
pub fn caption(session: &Session) -> String {
    let name = session.file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{}   {} of {}   {name}", ago(session), session.pos + 1, session.total)
}

/// A session in a line: when, what was opened, how far through, and the
/// file it was on.
pub fn describe(session: &Session) -> String {
    let opened: Vec<String> = session.paths.iter().map(|path| path.to_string_lossy().into_owned()).collect();
    let name = session.file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut line = format!("{:<11} {}   {} of {}   {name}", ago(session), opened.join(" "), session.pos + 1, session.total);
    if session.seconds >= 1.0 {
        let whole = session.seconds as u64;
        line.push_str(&format!(" at {}:{:02}", whole / 60, whole % 60));
    }
    if let Some(filter) = &session.filter {
        line.push_str(&format!("   narrowed to \"{filter}\""));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(cwd: &str, when: u64) -> Session {
        Session {
            cwd: cwd.into(),
            when,
            paths: vec!["/clips".into(), "/more clips".into()],
            recursive: true,
            sort: Some(Sort::Newest),
            reverse: false,
            filelist: None,
            filter: Some("pix #video".to_string()),
            file: "/clips/a b.mp4".into(),
            pos: 22,
            total: 300,
            seconds: 723.4,
        }
    }

    #[test]
    fn a_session_survives_being_written_and_read() {
        let kept = session("/home/me", 1_700_000_000);
        assert_eq!(parse(&render(&kept)), Some(kept));
        let bare = Session { sort: None, filter: None, paths: vec![".".into()], ..session("/x", 5) };
        assert_eq!(parse(&render(&bare)), Some(bare));
        assert_eq!(parse("not a session"), None);
    }

    #[test]
    fn a_session_is_told_in_a_line() {
        let told = describe(&session("/home/me", now()));
        assert!(told.starts_with("just now"), "{told}");
        assert!(told.contains("23 of 300") && told.contains("a b.mp4 at 12:03") && told.contains("narrowed to \"pix #video\""), "{told}");
    }
}
