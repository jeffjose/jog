//! `jog --info`: a line on each file, what it is, and no window. Passed
//! over without ffmpeg to make the video.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn info_tells_what_each_file_is() {
    let dir = std::env::temp_dir().join("jog-info");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mp4");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=2", "-f", "lavfi", "-i", "sine=duration=2"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", "-shortest"])
        .arg(&clip)
        .status();
    if !made.is_ok_and(|status| status.success()) {
        return;
    }
    let picture: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "screens", "grid.png"].iter().collect();
    let info = |paths: &[&PathBuf]| {
        let output = Command::new(env!("CARGO_BIN_EXE_jog")).arg("-i").args(paths).env("JOG_CONFIG", "/dev/null").output().expect("jog runs");
        let said = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "{said}{}", String::from_utf8_lossy(&output.stderr));
        said
    };
    // One file: a line.
    let said = info(&[&clip]);
    assert!(said.starts_with(&format!("{}: 320×240 · 0:02 · H.264 · 25 fps", clip.display())), "{said}");
    assert!(said.contains("AAC stereo"), "{said}");
    // More: a table, a heading and a line apiece in order, the names last.
    let said = info(&[&clip, &picture]);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 3, "{said}");
    assert!(lines[0].trim_start().starts_with("W×H  Length  Format") && lines[0].ends_with("Name"), "{}", lines[0]);
    assert!(lines[1].trim_start().starts_with("320×240    0:02  H.264") && lines[1].contains("AAC stereo") && lines[1].ends_with(&clip.display().to_string()), "{}", lines[1]);
    assert!(lines[2].starts_with("1280×800          PNG") && lines[2].ends_with(&picture.display().to_string()), "{}", lines[2]);
    // The columns line up: every line's name starts where the heading's does.
    let at = lines[0].find("Name").unwrap();
    assert!(lines[1..].iter().all(|line| line.char_indices().nth(lines[0][..at].chars().count()).is_some_and(|(_, c)| c == '/')), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}
