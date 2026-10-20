//! The video player runs in a process of its own, so that a crash there,
//! in ffmpeg or a GPU driver, is not jog's. These run jog off-screen on a
//! short video whose player is made to crash as a bad decoder does, and
//! check that jog goes on: the file noted, and the video played on the
//! CPU instead, or, when that crashes too, left as not playable.
//!
//! Passed over without a display to open a window on, ffmpeg to make the
//! video, or libmpv to play it.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::RgbImage;

fn display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn libmpv() -> bool {
    ["libmpv.so.2", "libmpv.so.1"].iter().any(|name| {
        Command::new("sh").arg("-c").arg(format!("ldconfig -p | grep -q {name}")).status().is_ok_and(|status| status.success())
    })
}

/// A folder holding a three-second video, its name with `crashy` in it.
fn folder(name: &str) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("jog-player").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("videos")).ok()?;
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=3", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(dir.join("videos/clip-crashy.mp4"))
        .status()
        .ok()?;
    made.success().then_some(dir)
}

struct Run {
    succeeded: bool,
    said: String,
    crashes: String,
    drawn: RgbImage,
}

/// Runs jog on the video with a crash asked for by `env`, and gives back
/// how it went.
fn run(dir: &Path, env: (&str, &str)) -> Run {
    let out = dir.join("out.ppm");
    let output = Command::new(env!("CARGO_BIN_EXE_jog"))
        .args(["--no-animations", "--no-positions", "-g", "640x400"])
        .arg(dir.join("videos"))
        .env("JOG_CONFIG", "/dev/null")
        .env("JOG_STATE", dir.join("state"))
        .env("JOG_SNAPSHOT", &out)
        .env("JOG_SNAPSHOT_WAIT", "4000")
        .env(env.0, env.1)
        .output()
        .expect("jog runs");
    Run {
        succeeded: output.status.success(),
        said: String::from_utf8_lossy(&output.stderr).into_owned(),
        crashes: std::fs::read_to_string(dir.join("state/player-crashes")).unwrap_or_default(),
        drawn: image::open(&out).map(|image| image.to_rgb8()).unwrap_or_else(|_| RgbImage::new(1, 1)),
    }
}

/// Whether the middle of the window shows the test pattern, which is
/// nowhere near black, rather than nothing.
fn video_shown(drawn: &RgbImage) -> bool {
    let (w, h) = drawn.dimensions();
    let bright = (h / 3..h * 2 / 3).flat_map(|y| (w / 3..w * 2 / 3).map(move |x| (x, y))).filter(|(x, y)| drawn.get_pixel(*x, *y).0.iter().any(|channel| *channel > 60)).count();
    bright > (w as usize / 3) * (h as usize / 3) / 2
}

#[test]
fn a_crash_in_the_gpu_decoder_is_survived_and_the_video_played_on_the_cpu() {
    if !display() || !libmpv() {
        return;
    }
    let Some(dir) = folder("gpu") else { return };
    let run = run(&dir, ("JOG_TEST_PLAYER_CRASH", "crashy"));
    assert!(run.succeeded, "jog went down with the player:\n{}", run.said);
    assert!(run.said.contains("the player crashed (signal 6"), "the crash is told of:\n{}", run.said);
    assert!(run.crashes.lines().any(|line| line.starts_with("gpu\t") && line.ends_with("clip-crashy.mp4")), "the file is noted: {:?}", run.crashes);
    assert!(video_shown(&run.drawn), "the video plays on, decoded on the CPU");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_video_that_crashes_every_decoder_is_left_as_not_playable() {
    if !display() || !libmpv() {
        return;
    }
    let Some(dir) = folder("always") else { return };
    let run = run(&dir, ("JOG_TEST_PLAYER_CRASH_ALWAYS", "crashy"));
    assert!(run.succeeded, "jog went down with the player:\n{}", run.said);
    assert!(run.crashes.lines().any(|line| line.starts_with("software\t") && line.ends_with("clip-crashy.mp4")), "the file is noted as crashing the CPU's decoder too: {:?}", run.crashes);
    assert_eq!(run.said.matches("the player crashed").count(), 2, "tried once on the GPU and once on the CPU, then left be:\n{}", run.said);
    let _ = std::fs::remove_dir_all(&dir);
}
