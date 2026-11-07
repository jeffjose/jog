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
        let output = Command::new(env!("CARGO_BIN_EXE_jog")).arg("-i").args(paths).env("JOG_CONFIG", "/dev/null").env("JOG_STATE", dir.join("state")).output().expect("jog runs");
        let said = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "{said}{}", String::from_utf8_lossy(&output.stderr));
        said
    };
    // One file: a line.
    let said = info(&[&clip]);
    assert!(said.starts_with(&format!("{}: 320×240 (4:3) · 0:02 · H.264 · 25 fps", clip.display())), "{said}");
    assert!(said.contains("AAC stereo"), "{said}");
    // More: a table, a heading and a line apiece in order, the names first.
    let said = info(&[&clip, &picture]);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 3, "{said}");
    assert!(lines[0].starts_with("Name ") && lines[0].contains("W×H  Ratio  Length  Format"), "{}", lines[0]);
    assert!(lines[1].starts_with(&clip.display().to_string()) && ["320×240  4:3 ", " 0:02  H.264"].iter().all(|piece| lines[1].contains(piece)) && lines[1].contains("AAC stereo"), "{}", lines[1]);
    assert!(lines[2].starts_with(&picture.display().to_string()) && lines[2].contains("1280×800  16:10 ") && lines[2].contains(" PNG "), "{}", lines[2]);
    // The columns line up: every line's pixels end where the heading's W×H does.
    let column = |line: &str, text: &str| line.find(text).map(|at| line[..at].chars().count() + text.chars().count());
    let end = column(lines[0], "W×H").unwrap();
    assert_eq!(column(lines[1], "320×240"), Some(end), "{said}");
    assert_eq!(column(lines[2], "1280×800"), Some(end), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}
