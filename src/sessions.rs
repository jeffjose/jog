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
    /// A short name of its own, made up when it is first kept, by which
    /// it can be asked for from anywhere.
    pub id: String,
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
        session.id.clone(),
    ];
    parts.join("\t")
}

fn parse(line: &str) -> Option<Session> {
    let parts: Vec<&str> = line.split('\t').collect();
    // One kept before sessions had ids has none; it is given one that
    // comes out the same each time it is read.
    let (parts, id) = match parts.len() {
        13 => (&parts[..12], parts[12].to_string()),
        _ => (&parts[..], String::new()),
    };
    let [cwd, when, recursive, sort, reverse, filelist, filter, file, pos, total, seconds, paths] = parts[..] else { return None };
    let id = if id.is_empty() { id_from(when.parse::<u64>().ok()? ^ cwd.len() as u64 * 0x9E37_79B9) } else { id };
    Some(Session {
        id,
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

/// Keeps a session, and of those of its folder only the latest few; and
/// gives back its id.
pub fn save(mut session: Session) -> String {
    let Some(file) = file() else { return session.id };
    let mut all = load();
    // One of the same things opened takes the place of the one before:
    // the sessions of a folder are told apart by what they were of. It
    // keeps that one's id; a new one gets its own.
    let (cwd, paths, filelist) = (session.cwd.clone(), session.paths.clone(), session.filelist.clone());
    let same = |other: &Session| other.cwd == cwd && other.paths == paths && other.filelist == filelist;
    // One picked up by its id is itself again, whatever has changed.
    let id = session.id.clone();
    all.retain(|other| id.is_empty() || other.id != id);
    if session.id.is_empty() {
        session.id = all.iter().find(|other| same(other)).map_or_else(new_id, |other| other.id.clone());
    }
    all.retain(|other| !same(other));
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
    session.id
}

/// The letters an id is made of: ones that are easy to type and to say.
const LETTERS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
/// How many of them an id has.
const ID_LENGTH: usize = 8;

/// An id spelt out from a number, the same for the same number.
fn id_from(mut number: u64) -> String {
    let mut id = String::new();
    for _ in 0..ID_LENGTH {
        // Stirred between letters, so that numbers close together do
        // not come out as ids that are.
        number = number.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        id.push(LETTERS[(number >> 33) as usize % LETTERS.len()] as char);
    }
    id
}

/// A new id, as good as certain to be no other session's.
fn new_id() -> String {
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos() as u64);
    id_from(clock ^ u64::from(std::process::id()) << 32)
}

/// The session with this id, or the one whose id begins with it when
/// there is only one such, whichever folder it is of.
pub fn by_id(id: &str) -> Option<Session> {
    let all = load();
    let exact = all.iter().find(|session| session.id == id).cloned();
    exact.or_else(|| {
        let mut starting = all.iter().filter(|session| id.len() >= 3 && session.id.starts_with(id));
        starting.next().filter(|_| starting.next().is_none()).cloned()
    })
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
    format!("{}   {} of {}   {name}   {}", ago(session), session.pos + 1, session.total, session.id)
}

/// A session in two lines: its id, when and what was opened; and under
/// them how far through it was, and the file it was on.
pub fn describe(session: &Session) -> String {
    // What is under the home folder is said from `~`, which is shorter.
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let short = |path: &PathBuf| match home.as_ref().and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    };
    let opened: Vec<String> = session.paths.iter().map(short).collect();
    let name = session.file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut second = format!("{} of {}   {name}", session.pos + 1, session.total);
    if session.seconds >= 1.0 {
        let whole = session.seconds as u64;
        second.push_str(&format!(" at {}:{:02}", whole / 60, whole % 60));
    }
    if let Some(filter) = &session.filter {
        second.push_str(&format!("   narrowed to \"{filter}\""));
    }
    // The id, when, and what was opened; and under them, set in as far as
    // the id is wide, where it was left.
    format!("{}  {}   {}\n{:width$}  {second}", session.id, ago(session), opened.join(" "), "", width = session.id.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(cwd: &str, when: u64) -> Session {
        Session {
            id: "k3x9q2ab".to_string(),
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
    fn a_session_is_told_in_two_lines() {
        let told = describe(&session("/home/me", now()));
        let (first, second) = told.split_once('\n').unwrap();
        assert!(first.starts_with("k3x9q2ab  just now   /clips"), "{told}");
        assert!(second.starts_with("          23 of 300"), "{told}");
        assert!(told.contains("23 of 300") && told.contains("a b.mp4 at 12:03") && told.contains("narrowed to \"pix #video\""), "{told}");
    }
}
