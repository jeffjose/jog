//! The size of the screen jog was last shown on, kept between runs so
//! that the first image can be decoded for it before the display has
//! even been opened: opening one takes a good part of the time a small
//! image takes to come up.

use std::path::PathBuf;

use crate::positions::state_dir;

fn file() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("screen"))
}

/// The screen size of the last run, if one was kept.
pub fn remembered() -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(file()?).ok()?;
    let (width, height) = text.trim().split_once(' ')?;
    let size = (width.parse().ok()?, height.parse().ok()?);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// Keeps a screen size for the next run, when it is news.
pub fn remember((width, height): (u32, u32)) {
    if remembered() == Some((width, height)) {
        return;
    }
    let Some(file) = file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, format!("{width} {height}\n"));
}
