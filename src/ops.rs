use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Moves a file, copying and deleting when the destination is on another
/// filesystem. Refuses to replace an existing file.
pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(ErrorKind::AlreadyExists, "destination already exists"));
    }
    match std::fs::rename(from, to) {
        Err(error) if error.kind() == ErrorKind::CrossesDevices => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
        result => result,
    }
}

/// Where a rename of `from` to what was typed puts the file. A bare name
/// keeps it in its folder. With a slash in it, it is a path: from `/` or
/// `~`, as it is, otherwise from the file's folder. A path ending in a
/// slash, or naming a folder that is there, takes the file in under the
/// name it has. `None` when what was typed names no file.
pub fn rename_target(from: &Path, typed: &str) -> Option<PathBuf> {
    let typed = typed.trim();
    let into = typed.ends_with('/');
    let typed = typed.trim_end_matches('/');
    if typed.is_empty() && !into {
        return None;
    }
    // Joined on, a path from `/` replaces the folder rather than going on
    // from it.
    let path = match typed {
        "" => PathBuf::from("/"),
        _ if typed == "~" || typed.starts_with("~/") => crate::options::expand_home(typed),
        _ => from.parent().unwrap_or(Path::new("")).join(typed),
    };
    let to = match into || path.is_dir() {
        true => path.join(from.file_name()?),
        false => path,
    };
    // One ending in `..` names a folder, not a file. A folder that is there
    // is said as itself, without `.` and `..` or links on the way; one yet
    // to be made, as near to that as its spelling allows.
    let name = to.file_name()?;
    let dir = to.parent()?;
    Some(dir.canonicalize().unwrap_or_else(|_| tidy(dir)).join(name))
}

/// A path with its `.` parts left out and each `..` taking away the part
/// before it, going by the spelling alone.
fn tidy(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Copies a file. Refuses to replace an existing file.
pub fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(ErrorKind::AlreadyExists, "destination already exists"));
    }
    std::fs::copy(from, to).map(drop)
}

/// Puts text on the clipboard with whichever helper is installed. These
/// keep serving the text after jog exits, which an in-process clipboard
/// could not. Returns whether one of them took it.
pub fn set_clipboard(text: &str) -> bool {
    let helpers: [&[&str]; 3] =
        [&["xclip", "-selection", "clipboard"], &["xsel", "--clipboard", "--input"], &["wl-copy"]];
    helpers.iter().any(|helper| {
        let child = Command::new(helper[0])
            .args(&helper[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else { return false };
        let written = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        written && child.wait().is_ok_and(|status| status.success())
    })
}

/// The text on the clipboard, read with whichever helper is installed.
pub fn clipboard() -> Option<String> {
    let helpers: [&[&str]; 3] =
        [&["xclip", "-selection", "clipboard", "-o"], &["xsel", "--clipboard", "--output"], &["wl-paste", "-n"]];
    helpers.iter().find_map(|helper| {
        let output = Command::new(helper[0]).args(&helper[1..]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_and_copying_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("jog-ops-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();

        copy_file(&a, &dir.join("out/a.txt")).unwrap();
        assert!(a.exists());
        assert!(copy_file(&b, &dir.join("out/a.txt")).is_err());
        assert!(move_file(&b, &dir.join("out/a.txt")).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("out/a.txt")).unwrap(), "a");

        move_file(&b, &dir.join("out/b.txt")).unwrap();
        assert!(!b.exists());
        assert_eq!(std::fs::read_to_string(dir.join("out/b.txt")).unwrap(), "b");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rename_may_be_a_path() {
        let dir = std::env::temp_dir().join(format!("jog-rename-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pics/sub")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let from = dir.join("pics/a.png");
        let to = |typed: &str| rename_target(&from, typed);
        // A bare name stays in the folder.
        assert_eq!(to("b.png"), Some(dir.join("pics/b.png")));
        assert_eq!(to("  b.png "), Some(dir.join("pics/b.png")));
        // A path from the root is taken as it is.
        assert_eq!(to("/tmp/foo.png"), Some(Path::new("/tmp").canonicalize().unwrap().join("foo.png")));
        // However it is spelled, the same folder is the same folder.
        assert_eq!(to("./a.png"), Some(from.clone()));
        assert_eq!(to("../pics/a.png"), Some(from.clone()));
        // A path otherwise goes on from the file's folder.
        assert_eq!(to("sub/c.png"), Some(dir.join("pics/sub/c.png")));
        assert_eq!(to("new/c.png"), Some(dir.join("pics/new/c.png")));
        // A folder, said so with a slash or there already, keeps the name.
        assert_eq!(to("sub"), Some(dir.join("pics/sub/a.png")));
        assert_eq!(to("sub/"), Some(dir.join("pics/sub/a.png")));
        assert_eq!(to("new/"), Some(dir.join("pics/new/a.png")));
        assert_eq!(to("../moved/./"), Some(dir.join("moved/a.png")));
        assert_eq!(to("/tmp/"), Some(Path::new("/tmp").canonicalize().unwrap().join("a.png")));
        assert_eq!(to("../"), Some(dir.join("a.png")));
        let home = PathBuf::from(std::env::var_os("HOME").unwrap()).canonicalize().unwrap();
        assert_eq!(to("~/x.png"), Some(home.join("x.png")));
        // Nothing, or no file at the end, is no name.
        assert_eq!(to(""), None);
        assert_eq!(to("   "), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
