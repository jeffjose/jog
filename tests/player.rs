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
fn drawing_never_waits_on_the_player() {
    if !display() || !libmpv() {
        return;
    }
    let Some(dir) = folder("drawing") else { return };
    // Inspect up on its Tracks tab, and the info overlay: what reads most
    // of the player while drawing.
    std::fs::create_dir_all(dir.join("state")).unwrap();
    std::fs::write(dir.join("state/panels"), "inspector=pinned\ninspect-tab=Tracks\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jog"))
        .args(["--no-animations", "--no-positions", "-g", "640x400"])
        .arg(dir.join("videos"))
        .env("JOG_CONFIG", "/dev/null")
        .env("JOG_STATE", dir.join("state"))
        .env("JOG_SNAPSHOT", "/dev/null")
        .env("JOG_SNAPSHOT_KEYS", "{F12}{Wait}i{Wait}")
        .env("JOG_SNAPSHOT_WAIT", "1500")
        .output()
        .expect("jog runs");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{said}");
    assert!(!said.contains("drawing waited on the player"), "a property read while drawing is not watched:\n{said}");
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

/// How wide the picture is across the middle of the window: from the
/// first pixel to the last that is not black.
fn picture_width(drawn: &RgbImage) -> u32 {
    let y = drawn.height() / 2;
    let lit: Vec<u32> = (0..drawn.width()).filter(|x| drawn.get_pixel(*x, y).0.iter().any(|channel| *channel > 40)).collect();
    lit.last().map_or(0, |last| last + 1 - lit[0])
}

#[test]
fn a_video_zooms_with_plus_and_minus_and_ctrl_with_the_wheel() {
    if !display() || !libmpv() {
        return;
    }
    // A video of one flat color, smaller than the window, and no still of
    // it: its size is the player's to tell.
    let dir = std::env::temp_dir().join("jog-player").join("zoom");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("videos")).unwrap();
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "color=c=0x3080FF:size=320x240:rate=25:duration=3", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(dir.join("videos/flat.mp4"))
        .status();
    if !made.is_ok_and(|status| status.success()) {
        return;
    }
    let width = |name: &str, keys: &str| {
        let out = dir.join(format!("{name}.ppm"));
        let output = Command::new(env!("CARGO_BIN_EXE_jog"))
            .args(["--no-animations", "--no-positions", "-g", "640x400"])
            .arg(dir.join("videos"))
            .env("JOG_CONFIG", "/dev/null")
            .env("JOG_STATE", dir.join(format!("state-{name}")))
            .env("JOG_SNAPSHOT", &out)
            .env("JOG_SNAPSHOT_KEYS", format!("{{Move:320,200}}{keys}{{Wait}}"))
            .env("JOG_SNAPSHOT_WAIT", "800")
            .output()
            .expect("jog runs");
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        picture_width(&image::open(&out).unwrap_or_else(|error| panic!("{name}: no snapshot: {error}")).to_rgb8())
    };
    let fitted = width("fitted", "");
    assert!(fitted > 100, "the video is shown: {fitted} wide");
    for (name, keys) in [("plus", "+"), ("wheel-up", "{Hold:Ctrl}{Wheel:2}")] {
        let zoomed = width(name, keys);
        assert!(zoomed > fitted + 20, "{name} zooms in: {zoomed} wide, from {fitted}");
    }
    for (name, keys) in [("minus", "-"), ("wheel-down", "{Hold:Ctrl}{Wheel:-2}")] {
        let zoomed = width(name, keys);
        assert!(zoomed + 20 < fitted, "{name} zooms out: {zoomed} wide, from {fitted}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
