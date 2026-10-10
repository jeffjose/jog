use std::io::{self, ErrorKind, Write};
use std::path::Path;
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
}
