//! Folders on another machine, reached over ssh: `jog skyfall:.` opens the
//! folder jog is started in, on skyfall.
//!
//! The host is mounted, and the rest of jog is handed a local path, so a
//! remote file is a file like any other: listed, decoded, played, moved and
//! renamed through the mount. One mount per host, of its `/`, under
//! `$XDG_RUNTIME_DIR/jog/mnt/HOST`: a local path and `host:/path` are then a
//! prefix apart, any number of jogs share one connection, and the local path
//! is the same after a remount, so what is kept by path (positions, seen,
//! sessions) still means the same file.
//!
//! Each jog holds a shared lock on the host's lock file while it runs. The
//! last to quit gets it to itself and unmounts; one that crashed lets go of
//! its lock all the same, and the mount is left for the next to use.
//!
//! The path after the host is read as it would be without the host, then
//! looked up over there, as magicfs does: `skyfall:videos` is the `videos`
//! of this folder, not of the home directory there, as scp would have it.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// `http://...` has the shape of `host:path` and is nothing of the sort.
const SCHEMES: &[&str] = &[
    "http", "https", "ftp", "ftps", "file", "smb", "nfs", "rtsp", "rtmp", "mms", "dav", "davs", "sftp", "ssh",
];

/// Handed to ssh so that a host that went away is noticed (a dead mount
/// otherwise hangs whatever touches it) and one that isn't there fails fast.
const KEEPALIVE: [&str; 3] = ["ServerAliveInterval=15", "ServerAliveCountMax=3", "ConnectTimeout=10"];

/// The hosts this jog holds, each by its lock file, kept open for as long
/// as jog runs.
static HELD: Mutex<Vec<(String, File)>> = Mutex::new(Vec::new());

/// A folder or file on another machine: `[user@]host:path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// Whatever `ssh` would be given, so `~/.ssh/config` aliases work.
    pub host: String,
    /// From `/`, from `~` (the home there), or from the folder this was
    /// typed in. Empty is that folder.
    pub path: String,
}

impl Remote {
    /// A word read as `host:path`, unless it is something here: a path that
    /// is there wins, so a file with a colon in its name is that file.
    pub fn parse(word: &str) -> Option<Remote> {
        let remote = Self::shape(word)?;
        if Path::new(word).exists() {
            return None;
        }
        Some(remote)
    }

    /// Whether the word is spelled as `host:path`, whatever is there.
    fn shape(word: &str) -> Option<Remote> {
        let (host, path) = word.split_once(':')?;
        if host.is_empty() || host.starts_with('-') || host.starts_with('@') || host == "." || host == ".." || host.contains(|c: char| c == '/' || c.is_whitespace()) {
            return None;
        }
        if path.starts_with("//") && SCHEMES.contains(&host.to_ascii_lowercase().as_str()) {
            return None;
        }
        Some(Remote { host: host.to_string(), path: path.to_string() })
    }

    /// The path from `/` over there. One from `~` costs a connection to ask
    /// the host where its home is, the first time.
    fn absolute(&self, cwd: &Path) -> Result<String, String> {
        if self.path.starts_with('/') {
            return Ok(tidy(Path::new("/"), &self.path));
        }
        let under_home = match self.path.as_str() {
            "~" => Some(""),
            path => path.strip_prefix("~/"),
        };
        if let Some(rest) = under_home {
            let home = home(&self.host)?;
            return Ok(tidy(Path::new(&home), rest));
        }
        Ok(tidy(&here(cwd), &self.path))
    }
}

/// Where the mounts are. `JOG_MOUNTS` puts them elsewhere, for the tests.
pub fn root() -> PathBuf {
    if let Some(dir) = std::env::var_os("JOG_MOUNTS").filter(|dir| !dir.is_empty()) {
        return dir.into();
    }
    let base = match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("jog"),
        // SAFETY: getuid cannot fail.
        None => std::env::temp_dir().join(format!("jog-{}", unsafe { libc::getuid() })),
    };
    base.join("mnt")
}

/// The host and the path over there of a local path in a mount.
pub fn split(path: &Path) -> Option<(String, PathBuf)> {
    let rest = path.strip_prefix(root()).ok()?;
    let mut parts = rest.components();
    let host = parts.next()?.as_os_str().to_str()?.to_string();
    if host.starts_with('.') {
        return None;
    }
    Some((host, Path::new("/").join(parts.as_path())))
}

/// A path the way it is written to be read or pasted: `skyfall:/srv/a.jpg`
/// for one in a mount, which outlasts the mount; itself otherwise.
pub fn shown(path: &Path) -> String {
    match split(path) {
        Some((host, there)) => format!("{host}:{}", there.display()),
        None => path.display().to_string(),
    }
}

/// Whether a word names a place on another machine.
pub fn is_remote(word: &str) -> bool {
    Remote::parse(word).is_some()
}

/// The local path of `host:path` typed in a field, for a host that is
/// mounted already: nothing waits on a connection being made.
pub fn mounted_path(typed: &str, cwd: &Path) -> Option<PathBuf> {
    let remote = Remote::shape(typed)?;
    let point = root().join(&remote.host);
    if !is_mounted(&point) || Path::new(typed).exists() {
        return None;
    }
    let path = remote.absolute(cwd).ok()?;
    Some(point.join(path.trim_start_matches('/')))
}

/// The paths jog was asked to open, `host:path` ones mounted and made the
/// local path that is them. A path in a mount that is not up, as a session
/// resumed may name, has its host mounted again.
pub fn open_all(paths: &mut [PathBuf], cwd: &Path) -> Result<(), String> {
    for path in paths.iter_mut() {
        let Some(text) = path.to_str() else { continue };
        if let Some(remote) = Remote::parse(text) {
            let point = hold(&remote.host)?;
            let there = remote.absolute(cwd)?;
            let local = point.join(there.trim_start_matches('/'));
            if std::fs::metadata(&local).is_err() {
                return Err(format!("{}:{there} is not there", remote.host));
            }
            *path = local;
        } else if let Some((host, _)) = split(path) {
            hold(&host)?;
        }
    }
    Ok(())
}

/// Moves a file in a mount to the trash over there, by gio on the host:
/// the trash here is on another disk, and gio here cannot reach the one
/// there through the mount.
pub fn trash(path: &Path) -> Option<Result<(), String>> {
    let (host, there) = split(path)?;
    let mut ssh = ssh_command();
    let program = ssh.remove(0);
    let status = Command::new(&program)
        .args(ssh)
        .args(KEEPALIVE.iter().map(|option| format!("-o{option}")))
        .arg(&host)
        .arg(format!("gio trash -- {}", quote(&there.to_string_lossy())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Some(match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(format!("gio on {host} could not trash it")),
        Err(error) => Err(format!("{program}: {error}")),
    })
}

/// Lets go of the hosts this jog holds, and unmounts those no other jog
/// holds. Once, on the way out.
pub fn release() {
    let held = std::mem::take(&mut *HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    for (host, lock) in held {
        // No other jog mounting it meanwhile, to find it gone from under it.
        let Ok(_setting_up) = lock_file(&host, "mounting", libc::LOCK_EX) else { continue };
        // SAFETY: flock on a file this holds open.
        let alone = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if alone {
            // Lazily: the players, still in the files, go with the process.
            unmount(&root().join(&host), true);
        }
    }
}

/// The host's lock file of that kind, open and locked as asked.
fn lock_file(host: &str, kind: &str, how: libc::c_int) -> Result<File, String> {
    let root = root();
    std::fs::create_dir_all(&root).map_err(|error| format!("cannot make {}: {error}", root.display()))?;
    let path = root.join(format!(".{host}.{kind}"));
    let file = File::options().create(true).truncate(false).write(true).open(&path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    // SAFETY: flock on a file this holds open.
    unsafe { libc::flock(file.as_raw_fd(), how) };
    Ok(file)
}

/// Unmounts on being dropped: the way out of `main` that returns.
pub struct Release;

impl Drop for Release {
    fn drop(&mut self) {
        release();
    }
}

/// The host mounted, if it is not already, and held by this jog: the local
/// folder that is its `/`.
fn hold(host: &str) -> Result<PathBuf, String> {
    let root = root();
    let point = root.join(host);
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if held.iter().any(|(known, _)| known == host) {
        return Ok(point);
    }
    // Two locks: `mounting`, one jog at a time, while it is mounted or
    // unmounted; `in-use`, shared by every jog in it for as long as it
    // runs. One quitting unmounts only with `in-use` to itself.
    let _setting_up = lock_file(host, "mounting", libc::LOCK_EX)?;
    let in_use = lock_file(host, "in-use", libc::LOCK_SH)?;
    mount_if_down(host, &point)?;
    held.push((host.to_string(), in_use));
    Ok(point)
}

fn mount_if_down(host: &str, point: &Path) -> Result<(), String> {
    // Listed but not to be read is a connection that died: made again.
    if is_mounted(point) && std::fs::metadata(point).is_err() {
        unmount(point, true);
    }
    if is_mounted(point) {
        return Ok(());
    }
    std::fs::create_dir_all(point).map_err(|error| format!("cannot make {}: {error}", point.display()))?;
    mount(host, point)
}

fn mount(host: &str, point: &Path) -> Result<(), String> {
    let ssh = ssh_command();
    let custom = std::env::var_os("JOG_SSH").is_some();
    let sshfs = is_program("sshfs");
    let status = if sshfs {
        let mut options = format!("reconnect,follow_symlinks,{}", KEEPALIVE.join(","));
        if custom {
            options.push_str(&format!(",ssh_command={}", ssh.join(" ")));
        }
        Command::new("sshfs").arg(format!("{host}:/")).arg(point).args(["-o", &options]).status()
    } else if is_program("rclone") {
        // rclone's own ssh reads neither ~/.ssh/config nor the agent's idea
        // of which key goes with which host, so it is told to run ssh.
        let mut line = ssh;
        line.extend(KEEPALIVE.iter().map(|option| format!("-o{option}")));
        line.push(host.to_string());
        Command::new("rclone")
            .env("RCLONE_CONFIG_JOGREMOTE_TYPE", "sftp")
            .env("RCLONE_CONFIG_JOGREMOTE_SSH", line.join(" "))
            .env("RCLONE_CONFIG_JOGREMOTE_SHELL_TYPE", "unix")
            .args(["mount", "jogremote:/"])
            .arg(point)
            .args(["--daemon", "--dir-cache-time", "10s", "--log-level", "ERROR"])
            .stderr(Stdio::null())
            .status()
    } else {
        let _ = std::fs::remove_dir(point);
        return Err(format!("opening {host}: needs sshfs (or rclone) → apt install sshfs"));
    };
    let status = status.map_err(|error| format!("cannot mount {host}: {error}"))?;
    // Both go to the background once connected; the kernel lists the mount
    // a moment after.
    let mut up = is_mounted(point);
    for _ in 0..50 {
        if up || !status.success() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        up = is_mounted(point);
    }
    if !up {
        let _ = std::fs::remove_dir(point);
        return Err(format!("cannot mount {host} → does `ssh {host}` work?"));
    }
    Ok(())
}

/// Unmounts, and says whether it went. `lazy` takes it away while files
/// in it are open still.
fn unmount(point: &Path, lazy: bool) -> bool {
    let program = if is_program("fusermount3") { "fusermount3" } else { "fusermount" };
    let gone = Command::new(program).arg(if lazy { "-uz" } else { "-u" }).arg(point).stderr(Stdio::null()).status().is_ok_and(|status| status.success());
    if gone {
        // Only ever the emptied mount point: remove_dir takes nothing with it.
        let _ = std::fs::remove_dir(point);
    }
    gone
}

/// The home directory over there, which `host:~/videos` is from. Asked
/// once and kept by the mounts, since asking is a whole connection.
fn home(host: &str) -> Result<String, String> {
    let cache = root().join(format!(".{host}.home"));
    if let Ok(known) = std::fs::read_to_string(&cache) {
        if known.starts_with('/') {
            return Ok(known.trim_end().to_string());
        }
    }
    let mut ssh = ssh_command();
    let program = ssh.remove(0);
    let out = Command::new(&program)
        .args(ssh)
        .arg(format!("-o{}", KEEPALIVE[2]))
        .arg(host)
        .arg("pwd")
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    // The last line: an rc file that greets must not become the path.
    let text = String::from_utf8_lossy(&out.stdout);
    let home = text.lines().rev().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    if !out.status.success() || !home.starts_with('/') {
        return Err(format!("no home folder found on {host} → name the path in full, {host}:/…"));
    }
    let _ = std::fs::create_dir_all(root());
    let _ = std::fs::write(&cache, home);
    Ok(home.to_string())
}

/// The folder a relative remote path goes on from: this one, by the name
/// the shell knows it by, so a folder reached through a link keeps its
/// name over there. In a mount, the folder over there that it is.
fn here(cwd: &Path) -> PathBuf {
    if let Some((_, there)) = split(cwd) {
        return there;
    }
    // `$PWD` is believed only when it is this folder.
    let logical = std::env::var_os("PWD").map(PathBuf::from).filter(|pwd| pwd.is_absolute() && pwd.canonicalize().ok() == cwd.canonicalize().ok());
    logical.unwrap_or_else(|| cwd.to_path_buf())
}

/// `path` from `base`, `.` and `..` worked out by the names alone: what
/// they lead to here says nothing of the other machine.
fn tidy(base: &Path, path: &str) -> String {
    use std::path::Component;
    let mut names: Vec<String> = Vec::new();
    for part in base.join(path).components() {
        match part {
            Component::Normal(name) => names.push(name.to_string_lossy().into_owned()),
            Component::ParentDir => {
                names.pop();
            }
            _ => {}
        }
    }
    format!("/{}", names.join("/"))
}

/// A word for the shell over there: in single quotes, which sh and csh
/// both take the same way.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The ssh to run. `JOG_SSH` replaces it, as `GIT_SSH_COMMAND` does git's.
fn ssh_command() -> Vec<String> {
    let words: Vec<String> = std::env::var("JOG_SSH").unwrap_or_default().split_whitespace().map(str::to_string).collect();
    if words.is_empty() { vec!["ssh".to_string()] } else { words }
}

fn is_program(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| std::fs::metadata(dir.join(name)).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)))
}

fn is_mounted(point: &Path) -> bool {
    let table = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    table.lines().filter_map(mount_point).any(|known| known == point)
}

/// The mount point out of a line of `/proc/self/mountinfo`: the fifth
/// field, its spaces and backslashes written as octal escapes.
fn mount_point(line: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let field = line.split(' ').nth(4)?.as_bytes();
    let mut out = Vec::with_capacity(field.len());
    let mut at = 0;
    while at < field.len() {
        let octal = field.get(at + 1..at + 4).filter(|_| field[at] == b'\\');
        match octal.and_then(|digits| std::str::from_utf8(digits).ok()).and_then(|digits| u8::from_str_radix(digits, 8).ok()) {
            Some(byte) => {
                out.push(byte);
                at += 4;
            }
            None => {
                out.push(field[at]);
                at += 1;
            }
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(host: &str, path: &str) -> Option<Remote> {
        Some(Remote { host: host.into(), path: path.into() })
    }

    #[test]
    fn a_host_and_a_path_is_a_remote() {
        assert_eq!(Remote::parse("skyfall:."), remote("skyfall", "."));
        assert_eq!(Remote::parse("skyfall:"), remote("skyfall", ""));
        assert_eq!(Remote::parse("me@skyfall:~/videos"), remote("me@skyfall", "~/videos"));
        assert_eq!(Remote::parse("skyfall:/srv/x"), remote("skyfall", "/srv/x"));
    }

    #[test]
    fn local_things_and_urls_are_not_remotes() {
        for word in ["photos", "./a:b.jpg", "-o:x", "@space:x", ".:x", "dir/a:b", "https://example.com/a.mp4", "sftp://host/x", ":x"] {
            assert_eq!(Remote::parse(word), None, "{word}");
        }
    }

    #[test]
    fn a_path_is_read_from_here_and_looked_up_there() {
        let cwd = Path::new("/home/you/scripts");
        let at = |path: &str| Remote { host: "h".into(), path: path.into() }.absolute(cwd).unwrap();
        // $PWD is believed only when it is this folder, which it is not.
        assert_eq!(at("."), "/home/you/scripts");
        assert_eq!(at(""), "/home/you/scripts");
        assert_eq!(at("videos"), "/home/you/scripts/videos");
        assert_eq!(at("./videos/"), "/home/you/scripts/videos");
        assert_eq!(at("../photos"), "/home/you/photos");
        assert_eq!(at("/srv/../x"), "/x");
    }

    #[test]
    fn a_path_in_a_mount_is_shown_by_its_host() {
        let root = root();
        assert_eq!(shown(&root.join("skyfall/home/you/a.jpg")), "skyfall:/home/you/a.jpg");
        assert_eq!(shown(&root.join("skyfall")), "skyfall:/");
        assert_eq!(shown(Path::new("/home/you/a.jpg")), "/home/you/a.jpg");
        assert_eq!(shown(&root.join(".skyfall.lock")), root.join(".skyfall.lock").display().to_string());
        // Relative from a mount is relative over there.
        assert_eq!(here(&root.join("skyfall/srv")), PathBuf::from("/srv"));
    }

    #[test]
    fn quoted_for_the_shell_over_there() {
        assert_eq!(quote("/a b/it's.jpg"), r"'/a b/it'\''s.jpg'");
    }

    #[test]
    fn mount_points_are_unescaped() {
        let line = r"36 35 0:31 / /run/user/1000/jog/mnt/my\040host rw - fuse.rclone jogremote: rw";
        assert_eq!(mount_point(line), Some(PathBuf::from("/run/user/1000/jog/mnt/my host")));
    }
}
