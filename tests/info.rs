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
    let output = Command::new(env!("CARGO_BIN_EXE_jog")).arg("-i").arg(&clip).arg(&picture).env("JOG_CONFIG", "/dev/null").output().expect("jog runs");
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{said}{}", String::from_utf8_lossy(&output.stderr));
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 2, "a line apiece, in order:\n{said}");
    assert!(lines[0].starts_with(&format!("{}: 320×240 · 0:02 · H.264 · 25 fps", clip.display())), "{}", lines[0]);
    assert!(lines[0].contains("AAC stereo"), "{}", lines[0]);
    assert!(lines[1].starts_with(&format!("{}: 1280×800 · PNG · ", picture.display())), "{}", lines[1]);
    let _ = std::fs::remove_dir_all(&dir);
}
