//! Golden-image tests: jog is run off-screen on a few scripted scenes and
//! what it draws is checked against the pictures kept in tests/screens.
//! A scene drawn differently in more than a sliver of the window fails,
//! and leaves what it drew beside the golden one, as `*.new.png`, to look
//! at. `JOG_BLESS=1 cargo test --test screens` takes what is drawn now as
//! the golden pictures. With no display to open a window on, the scenes
//! are passed over.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{Rgb, RgbImage};

/// The window every scene is drawn in.
const SIZE: (u32, u32) = (1280, 800);
/// How far apart a pixel's channels may be and it still count as the same.
const NEAR: u8 = 24;
/// The share of the window that may differ: a timing or a score in a
/// corner, text a pixel to one side.
const ALLOWED: f64 = 0.004;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/screens")
}

/// A picture to look at that is the same on every machine: a gradient with
/// blocks of color, and, as `changed`, a patch of it painted over.
fn fixture(changed: bool) -> RgbImage {
    let (w, h) = (960, 600);
    let mut image = RgbImage::from_fn(w, h, |x, y| Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 160]));
    for (left, top, color) in [(100, 100, [230, 60, 60]), (400, 250, [60, 200, 90]), (700, 120, [250, 220, 40])] {
        for y in top..top + 140 {
            for x in left..left + 160 {
                image.put_pixel(x, y, Rgb(color));
            }
        }
    }
    if changed {
        for y in 380..440 {
            for x in 160..240 {
                image.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
    }
    image
}

/// A folder of fixtures, made afresh for a scene.
fn folder(name: &str) -> PathBuf {
    // Named the same each run: the info overlay shows the path.
    let dir = std::env::temp_dir().join("jog-screens").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("pictures")).unwrap();
    fixture(false).save(dir.join("pictures/a.png")).unwrap();
    fixture(true).save(dir.join("pictures/b.png")).unwrap();
    dir
}

fn display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Runs jog on the fixtures with these keys, and gives back what it drew.
fn draw(name: &str, keys: &str, wait_ms: u32) -> RgbImage {
    let dir = folder(name);
    let out = dir.join("out.ppm");
    let status = Command::new(env!("CARGO_BIN_EXE_jog"))
        .args(["--no-animations", "--no-positions", "-g", &format!("{}x{}", SIZE.0, SIZE.1)])
        .arg(dir.join("pictures"))
        .env("JOG_CONFIG", "/dev/null")
        .env("JOG_STATE", dir.join("state"))
        .env("JOG_SNAPSHOT", &out)
        .env("JOG_SNAPSHOT_KEYS", keys)
        .env("JOG_SNAPSHOT_WAIT", wait_ms.to_string())
        .status()
        .expect("jog runs");
    assert!(status.success(), "jog failed drawing {name}");
    let drawn = image::open(&out).unwrap_or_else(|error| panic!("{name}: no snapshot: {error}")).to_rgb8();
    let _ = std::fs::remove_dir_all(&dir);
    drawn
}

/// Checks a scene against its golden picture, or makes it that.
fn check(name: &str, keys: &str, wait_ms: u32) {
    if !display() {
        eprintln!("{name}: no display, passed over");
        return;
    }
    let drawn = draw(name, keys, wait_ms);
    let golden = golden_dir().join(format!("{name}.png"));
    if std::env::var_os("JOG_BLESS").is_some() || !golden.exists() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        drawn.save(&golden).unwrap();
        eprintln!("{name}: kept as the golden picture");
        return;
    }
    let kept = image::open(&golden).unwrap().to_rgb8();
    assert_eq!(kept.dimensions(), drawn.dimensions(), "{name}: drawn at another size");
    let differing = kept.pixels().zip(drawn.pixels()).filter(|(a, b)| a.0.iter().zip(b.0.iter()).any(|(x, y)| x.abs_diff(*y) > NEAR)).count();
    let share = differing as f64 / f64::from(SIZE.0 * SIZE.1);
    if share > ALLOWED {
        let new = golden.with_extension("new.png");
        drawn.save(&new).unwrap();
        panic!("{name}: {:.2}% of the window drawn differently; see {}", share * 100.0, new.display());
    }
}

#[test]
fn info_overlay() {
    check("info", "i{Wait}", 600);
}

#[test]
fn toast() {
    check("toast", "Z{Wait}", 500);
}

#[test]
fn grid() {
    check("grid", "g{Wait}{Wait}", 500);
}

#[test]
fn shortcuts() {
    check("shortcuts", "?{Wait}", 500);
}

#[test]
fn find_box() {
    check("find", "{Ctrl+k}>sort{Wait}", 500);
}

#[test]
fn rename_box() {
    check("rename", "{F2}{Ctrl+a}done/a.png{Wait}", 500);
}

#[test]
fn compare_difference() {
    check("compare", "x{Right}{Ctrl+k}>compare{Enter}{Wait}{Wait}d{Wait}", 700);
}

#[test]
fn convert_panel() {
    check("convert", "{Ctrl+k}>convert…{Enter}{Wait}{Wait}{Wait}", 900);
}
