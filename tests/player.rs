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

/// Two short videos, one after the other, for going from one to the next.
fn two_videos(name: &str) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("jog-player").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("videos")).ok()?;
    for file in ["a.mp4", "b.mp4"] {
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=2", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(dir.join("videos").join(file))
            .status()
            .ok()?;
        if !made.success() {
            return None;
        }
    }
    Some(dir)
}

/// What jog says with `--debug` as it plays the keys on the two videos.
fn said_playing(dir: &Path, keys: &str, wait_ms: u32) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_jog"))
        .args(["--debug", "--no-animations", "--no-positions", "-g", "640x400"])
        .arg(dir.join("videos"))
        .env("JOG_CONFIG", "/dev/null")
        .env("JOG_STATE", dir.join(format!("state-{}", keys.len())))
        .env("JOG_SNAPSHOT", "/dev/null")
        .env("JOG_SNAPSHOT_KEYS", keys)
        .env("JOG_SNAPSHOT_WAIT", wait_ms.to_string())
        .output()
        .expect("jog runs");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_next_video_is_paused_only_when_the_user_paused() {
    if !display() || !libmpv() {
        return;
    }
    let Some(dir) = two_videos("paused") else { return };
    let starts = |said: &str| said.lines().filter(|line| line.contains("video start")).count();
    // Playing, on to the next: it plays.
    let said = said_playing(&dir, "{Wait}{Ctrl+Right}{Wait}", 600);
    assert_eq!(starts(&said), 2, "both videos shown:\n{said}");
    assert!(!said.contains("opened paused"), "the next is not paused when this one was playing:\n{said}");
    // Paused by the user, on to the next: paused too.
    let said = said_playing(&dir, "{Wait}{Space}{Wait}{Ctrl+Right}{Wait}", 600);
    assert!(said.contains("opened paused"), "the next stays paused after a pause:\n{said}");
    // Sent to just short of its end while it plays: it plays out, and the
    // next comes on, playing; not held paused on the last frame.
    let said = said_playing(&dir, "{Wait}{Jump:0.99}{Wait}{Wait}{Wait}", 600);
    assert_eq!(starts(&said), 2, "the next came on after the jump to the end:\n{said}");
    assert!(!said.contains("opened paused"), "and plays:\n{said}");
    // Played into its end, the next plays, not held on the last frame.
    let said = said_playing(&dir, "{Wait}{Wait}{Wait}{Wait}{Wait}", 800);
    assert_eq!(starts(&said), 2, "the next came on of itself:\n{said}");
    assert!(!said.contains("opened paused"), "and plays:\n{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 4-second video, made once for the seek bar's tests.
fn bar_video(name: &str) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("jog-player").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=4", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(dir.join("clip.mp4"))
        .status()
        .ok()?;
    made.success().then_some(dir)
}

/// What jog drew, run in `dir` on `file` (as given: relative or not) with
/// these keys: the video paused first, so that each picture is of one frame.
fn bar_shot(dir: &Path, file: &Path, keys: &str) -> RgbImage {
    let out = dir.join(format!("shot-{}.ppm", keys.len()));
    let output = Command::new(env!("CARGO_BIN_EXE_jog"))
        .args(["--no-animations", "--no-positions", "-g", "640x400"])
        .arg(file)
        .current_dir(dir)
        .env("JOG_CONFIG", "/dev/null")
        .env("JOG_STATE", dir.join("state"))
        .env("JOG_SNAPSHOT", &out)
        .env("JOG_SNAPSHOT_KEYS", format!("{{Wait}}{{Space}}{{Wait}}{{Wait}}{{Wait}}{keys}{{Wait}}"))
        .env("JOG_SNAPSHOT_WAIT", "700")
        .output()
        .expect("jog runs");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    image::open(&out).unwrap_or_else(|error| panic!("no snapshot: {error}")).to_rgb8()
}

/// The share of the bottom rows of `a` and `b` that differ: the seek bar
/// up in one and not the other.
fn bottom_differs(a: &RgbImage, b: &RgbImage) -> f64 {
    let (w, h) = a.dimensions();
    let rows = (h - h / 8)..h;
    let differ = rows.clone().flat_map(|y| (0..w).map(move |x| (x, y))).filter(|(x, y)| a.get_pixel(*x, *y).0.iter().zip(b.get_pixel(*x, *y).0).any(|(p, q)| p.abs_diff(q) > 24)).count();
    differ as f64 / f64::from(w * (rows.end - rows.start))
}

#[test]
fn the_seek_bar_comes_up_at_the_bottom_and_not_elsewhere() {
    if !display() || !libmpv() {
        return;
    }
    let Some(dir) = bar_video("bar") else { return };
    let clip = dir.join("clip.mp4");
    let middle = bar_shot(&dir, &clip, "{Move:320,150}{Move:330,160}");
    let bottom = bar_shot(&dir, &clip, "{Move:320,150}{Move:330,390}");
    let elsewhere = bar_shot(&dir, &clip, "{Move:320,150}{Move:330,100}");
    assert!(bottom_differs(&middle, &bottom) > 0.2, "the bar is up with the pointer at the bottom");
    assert!(bottom_differs(&middle, &elsewhere) < 0.02, "and not with it anywhere else");
    // Opened by a name relative to where jog runs, as typed.
    let relative = bar_shot(&dir, Path::new("clip.mp4"), "{Move:320,150}{Move:330,390}");
    assert!(bottom_differs(&middle, &relative) > 0.2, "the bar is up for a file opened by a relative name");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_seek_bar_follows_the_window_to_a_new_size() {
    if !display() || !libmpv() {
        return;
    }
    let Some(dir) = bar_video("bar-size") else { return };
    let clip = dir.join("clip.mp4");
    // Made smaller, as leaving fullscreen does; the pointer then at the
    // bottom of the window as it is now.
    let still = bar_shot(&dir, &clip, "{Size:480x300}{Wait}{Move:240,100}{Move:250,110}");
    let up = bar_shot(&dir, &clip, "{Size:480x300}{Wait}{Move:240,100}{Move:250,292}");
    assert_eq!(up.dimensions(), (480, 300), "drawn at the size the window was made");
    assert!(bottom_differs(&still, &up) > 0.2, "the bar is up at the bottom of the window as it is now");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_bar_keeps_its_track_buttons_when_a_track_is_changed_while_paused() {
    if !display() || !libmpv() {
        return;
    }
    // Two audio tracks, so that the bar has a button for them.
    let dir = std::env::temp_dir().join("jog-player").join("bar-tracks");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=4"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4", "-f", "lavfi", "-i", "sine=frequency=660:duration=4"])
        .args(["-map", "0", "-map", "1", "-map", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(dir.join("clip.mkv"))
        .status();
    if !made.is_ok_and(|status| status.success()) {
        return;
    }
    let clip = dir.join("clip.mkv");
    let before = bar_shot(&dir, &clip, "{Move:320,150}{Move:330,390}");
    let after = bar_shot(&dir, &clip, "J{Wait}{Wait}{Wait}{Move:320,150}{Move:330,390}");
    // The buttons at the right of the bar, as they were: none gone.
    let (w, h) = before.dimensions();
    let buttons = |image: &RgbImage| image::imageops::crop_imm(image, w * 3 / 4, h - h / 10, w / 4, h / 10).to_image();
    let (a, b) = (buttons(&before), buttons(&after));
    let differ = a.pixels().zip(b.pixels()).filter(|(p, q)| p.0.iter().zip(q.0).any(|(x, y)| x.abs_diff(y) > 24)).count();
    assert!(differ < (a.width() * a.height()) as usize / 50, "the bar's buttons changed: {differ} pixels");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_track_picked_from_the_bar_is_played_and_kept_for_next_time() {
    if !display() || !libmpv() {
        return;
    }
    // Two audio tracks, so the bar has its audio button; at 1600x1000 it
    // stands at 1457,974 (no subtitles, so no button for them beside it),
    // the picker opening over it.
    let dir = std::env::temp_dir().join("jog-player").join("bar-pick");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("videos")).unwrap();
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=6"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=6", "-f", "lavfi", "-i", "sine=frequency=660:duration=6"])
        .args(["-map", "0", "-map", "1", "-map", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(dir.join("videos/clip.mkv"))
        .status();
    if !made.is_ok_and(|status| status.success()) {
        return;
    }
    let run = |keys: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_jog"))
            .args(["--no-animations", "-g", "1600x1000"])
            .arg(dir.join("videos"))
            .env("JOG_CONFIG", "/dev/null")
            .env("JOG_STATE", dir.join("state"))
            .env("JOG_SNAPSHOT", "/dev/null")
            .env("JOG_SNAPSHOT_KEYS", format!("{{Wait}}{{Space}}{{Wait}}{{Wait}}{{Wait}}{{Move:800,500}}{{Move:800,960}}{{Wait}}{keys}{{Wait}}"))
            .env("JOG_SNAPSHOT_WAIT", "600")
            .output()
            .expect("jog runs");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        // The audio track kept for the video: the first field of its line.
        let kept = std::fs::read_to_string(dir.join("state/tracks")).unwrap_or_default();
        kept.lines().next().and_then(|line| line.split('\t').next()).unwrap_or_default().to_string()
    };
    // The second track picked from the picker over the button.
    assert_eq!(run("{Move:1457,974}{Click}{Wait}{Down}{Down}{Enter}"), "2", "the track picked is kept");
    // Opened again on it: the next track from there is the first.
    assert_eq!(run("J"), "1", "opened again, the video plays the track kept");
    let _ = std::fs::remove_dir_all(&dir);
}
