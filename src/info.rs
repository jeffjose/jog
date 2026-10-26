//! `--info`: a line on each file, what it is, as the seek bar's details
//! say it of a video; and no window.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rayon::prelude::*;
use serde_json::Value;

use crate::app::{clock, codec_name};
use crate::files;
use crate::meta::human_size;

/// Prints a line for each file, in the order given, each worked out on a
/// core of its own; whether every one could be read.
pub fn print(paths: &[PathBuf]) -> bool {
    let lines: Vec<Result<String, String>> = paths.par_iter().map(|path| if files::is_video(path) { video(path) } else { picture(path) }).collect();
    let mut all = true;
    for (path, line) in paths.iter().zip(lines) {
        match line {
            Ok(line) => println!("{}: {line}", path.display()),
            Err(error) => {
                eprintln!("{}: {error}", path.display());
                all = false;
            }
        }
    }
    all
}

/// A picture: its size and format, and how large the file is.
fn picture(path: &Path) -> Result<String, String> {
    let found = crate::picmeta::read(path)?;
    let mut parts = Vec::new();
    if let Some((w, h)) = found.size {
        parts.push(format!("{w}×{h}"));
    }
    parts.extend(found.format);
    if found.gain.is_some() {
        parts.push("Ultra HDR".to_string());
    }
    parts.push(human_size(found.bytes));
    Ok(parts.join(" · "))
}

/// A video, as ffprobe tells of it: its size as shown, length, codec and
/// depth, rate, HDR, bitrate, sound, and how large the file is.
fn video(path: &Path) -> Result<String, String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run ffprobe: {error}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(said.lines().last().unwrap_or("not a video ffprobe can read").to_string());
    }
    let probe: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let streams = probe["streams"].as_array().cloned().unwrap_or_default();
    // The picture, not a cover image stored with it.
    let picture = streams.iter().find(|stream| stream["codec_type"] == "video" && stream["disposition"]["attached_pic"] != 1);
    let sound = streams.iter().find(|stream| stream["codec_type"] == "audio");
    let number = |value: &Value| value.as_f64().or_else(|| value.as_str().and_then(|text| text.parse().ok()));
    let mut parts = Vec::new();
    if let Some(stream) = picture {
        if let (Some(w), Some(h)) = (stream["width"].as_u64(), stream["height"].as_u64()) {
            // Turned a quarter, as phones record upright video, it is shown
            // the other way round.
            let rotation = stream["side_data_list"].as_array().into_iter().flatten().find_map(|data| number(&data["rotation"])).or_else(|| number(&stream["tags"]["rotate"]));
            let turned = rotation.is_some_and(|degrees| (degrees as i64).rem_euclid(180) == 90);
            parts.push(if turned { format!("{h}×{w}") } else { format!("{w}×{h}") });
        }
    }
    if let Some(seconds) = number(&probe["format"]["duration"]) {
        parts.push(clock(seconds));
    }
    if let Some(stream) = picture {
        let mut codec = stream["codec_name"].as_str().map(codec_name).unwrap_or_default();
        let format = stream["pix_fmt"].as_str().unwrap_or_default();
        if format.contains("10") || format.contains("12") {
            codec.push_str(if format.contains("12") { " 12-bit" } else { " 10-bit" });
        }
        parts.push(codec);
        let rate = stream["avg_frame_rate"].as_str().and_then(|rate| {
            let (n, d) = rate.split_once('/')?;
            Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok().filter(|d| *d > 0.0)?)
        });
        if let Some(fps) = rate.filter(|fps| *fps > 0.0) {
            parts.push(match (fps - fps.round()).abs() < 0.01 {
                true => format!("{fps:.0} fps"),
                false => format!("{} fps", format!("{fps:.3}").trim_end_matches('0')),
            });
        }
        // HDR as the info overlay names it; or, where what the container
        // carries says nothing of it, as the stream's own colors do, in the
        // same words. Dolby Vision from the stream.
        let mut hdr = crate::hdrmeta::read(path).map(|info| info.badges()).unwrap_or_default();
        if hdr.is_empty() {
            let side = |kind: &str| stream["side_data_list"].as_array().into_iter().flatten().any(|data| data["side_data_type"].as_str().is_some_and(|name| name.contains(kind)));
            match stream["color_transfer"].as_str() {
                Some("smpte2084") if side("Mastering display") || side("Content light") => hdr.push("HDR10".to_string()),
                Some("smpte2084") => hdr.push("PQ".to_string()),
                Some("arib-std-b67") => hdr.push("HLG".to_string()),
                _ => {}
            }
            if stream["color_primaries"] == "bt2020" {
                hdr.push("BT.2020".to_string());
            }
        }
        parts.extend(hdr);
        let dolby = stream["side_data_list"].as_array().into_iter().flatten().any(|data| data["side_data_type"].as_str().is_some_and(|kind| kind.contains("DOVI")));
        if dolby {
            parts.push("Dolby Vision".to_string());
        }
    }
    if let Some(bits) = number(&probe["format"]["bit_rate"]) {
        parts.push(format!("{:.1} Mb/s", bits / 1e6));
    }
    if let Some(stream) = sound {
        let mut audio = stream["codec_name"].as_str().map(codec_name).unwrap_or_default();
        match stream["channels"].as_u64() {
            Some(1) => audio.push_str(" mono"),
            Some(2) => audio.push_str(" stereo"),
            Some(6) => audio.push_str(" 5.1"),
            Some(8) => audio.push_str(" 7.1"),
            Some(other) => audio.push_str(&format!(" {other} ch")),
            None => {}
        }
        parts.push(audio);
        if let Some(rate) = number(&stream["sample_rate"]) {
            parts.push(format!("{} kHz", format!("{:.1}", rate / 1000.0).trim_end_matches(".0")));
        }
    }
    if let Ok(meta) = std::fs::metadata(path) {
        parts.push(human_size(meta.len()));
    }
    Ok(parts.join(" · "))
}
