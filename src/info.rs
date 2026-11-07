//! `--info`: what each file is, as the seek bar's details say it of a
//! video; and no window. One file is told of in a line; more, in a table
//! whose columns line up, with the names last, as `ls -l` has them.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rayon::prelude::*;
use serde_json::Value;

use crate::app::{clock, codec_name};
use crate::files;
use crate::meta::human_size;

/// What is told of a file: each part there or not.
#[derive(Default, Debug, PartialEq)]
struct Row {
    /// Width × height, as shown.
    pixels: Option<String>,
    length: Option<String>,
    /// The picture's format, or the video's codec and depth.
    format: Option<String>,
    /// Frames a second, as a number.
    rate: Option<String>,
    /// HDR, Dolby Vision, Ultra HDR.
    hdr: Vec<String>,
    bitrate: Option<String>,
    /// The sound's codec and channels, and its sample rate.
    audio: Option<String>,
    hz: Option<String>,
    /// How large the file is.
    bytes: Option<String>,
}

impl Row {
    /// Its parts in a line, with `·` between.
    fn line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(self.pixels.clone());
        parts.extend(self.length.clone());
        parts.extend(self.format.clone());
        parts.extend(self.rate.as_ref().map(|rate| format!("{rate} fps")));
        parts.extend(self.hdr.iter().cloned());
        parts.extend(self.bitrate.clone());
        parts.extend(self.audio.clone());
        parts.extend(self.hz.clone());
        parts.extend(self.bytes.clone());
        parts.join(" · ")
    }
}

/// A column of the table: its heading, what it shows of a row, and
/// whether it lines up on the right, as numbers do.
struct Column {
    heading: &'static str,
    cell: fn(&Row) -> String,
    right: bool,
}

const COLUMNS: &[Column] = &[
    Column { heading: "W×H", cell: |row| row.pixels.clone().unwrap_or_default(), right: true },
    Column { heading: "Length", cell: |row| row.length.clone().unwrap_or_default(), right: true },
    Column { heading: "Format", cell: |row| row.format.clone().unwrap_or_default(), right: false },
    Column { heading: "fps", cell: |row| row.rate.clone().unwrap_or_default(), right: true },
    Column { heading: "HDR", cell: |row| row.hdr.join(" "), right: false },
    Column { heading: "Bitrate", cell: |row| row.bitrate.clone().unwrap_or_default(), right: true },
    Column { heading: "Audio", cell: |row| [row.audio.clone(), row.hz.clone()].into_iter().flatten().collect::<Vec<_>>().join(" "), right: false },
    Column { heading: "Size", cell: |row| row.bytes.clone().unwrap_or_default(), right: true },
];

/// Tells of each file, in the order given, each worked out on a core of
/// its own; whether every one could be read.
pub fn print(paths: &[PathBuf]) -> bool {
    let rows: Vec<Result<Row, String>> = paths.par_iter().map(|path| if files::is_video(path) { video(path) } else { picture(path) }).collect();
    let mut read = Vec::new();
    for (path, row) in paths.iter().zip(rows) {
        match row {
            Ok(row) => read.push((path.display().to_string(), row)),
            Err(error) => eprintln!("{}: {error}", path.display()),
        }
    }
    let all = read.len() == paths.len();
    match read.as_slice() {
        [] => {}
        [(name, row)] if paths.len() == 1 => println!("{name}: {}", row.line()),
        rows => {
            // The heading dim on a terminal, plain where it is read by
            // something else.
            let dim = std::io::stdout().is_terminal();
            for line in table(rows, dim) {
                println!("{line}");
            }
        }
    }
    all
}

/// The table's lines: a heading, then a line for each file. A column no
/// file has anything in is left out.
fn table(rows: &[(String, Row)], dim: bool) -> Vec<String> {
    let width = |text: &str| text.chars().count();
    let cells: Vec<Vec<String>> = rows.iter().map(|(_, row)| COLUMNS.iter().map(|column| (column.cell)(row)).collect()).collect();
    let used: Vec<usize> = (0..COLUMNS.len()).filter(|&at| cells.iter().any(|row| !row[at].is_empty())).collect();
    let widths: Vec<usize> = used.iter().map(|&at| cells.iter().map(|row| width(&row[at])).max().unwrap_or(0).max(width(COLUMNS[at].heading))).collect();
    let line = |texts: &[&str], name: &str| {
        let mut out = String::new();
        for ((&at, &w), text) in used.iter().zip(&widths).zip(texts) {
            let pad = " ".repeat(w - width(text));
            match COLUMNS[at].right {
                true => out.push_str(&format!("{pad}{text}  ")),
                false => out.push_str(&format!("{text}{pad}  ")),
            }
        }
        out.push_str(name);
        out.trim_end().to_string()
    };
    let headings: Vec<&str> = used.iter().map(|&at| COLUMNS[at].heading).collect();
    let heading = line(&headings, "Name");
    let mut lines = vec![if dim { format!("\x1b[2m{heading}\x1b[0m") } else { heading }];
    for ((name, _), row) in rows.iter().zip(&cells) {
        let texts: Vec<&str> = used.iter().map(|&at| row[at].as_str()).collect();
        lines.push(line(&texts, name));
    }
    lines
}

/// A picture: its size and format, and how large the file is.
fn picture(path: &Path) -> Result<Row, String> {
    let found = crate::picmeta::read(path)?;
    let mut row = Row { pixels: found.size.map(|(w, h)| format!("{w}×{h}")), format: found.format.clone(), bytes: Some(human_size(found.bytes)), ..Row::default() };
    if found.gain.is_some() {
        row.hdr.push("Ultra HDR".to_string());
    }
    Ok(row)
}

/// A video, as ffprobe tells of it: its size as shown, length, codec and
/// depth, rate, HDR, bitrate, sound, and how large the file is.
fn video(path: &Path) -> Result<Row, String> {
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
    let mut row = Row::default();
    if let Some(stream) = picture {
        if let (Some(w), Some(h)) = (stream["width"].as_u64(), stream["height"].as_u64()) {
            // Turned a quarter, as phones record upright video, it is shown
            // the other way round.
            let rotation = stream["side_data_list"].as_array().into_iter().flatten().find_map(|data| number(&data["rotation"])).or_else(|| number(&stream["tags"]["rotate"]));
            let turned = rotation.is_some_and(|degrees| (degrees as i64).rem_euclid(180) == 90);
            row.pixels = Some(if turned { format!("{h}×{w}") } else { format!("{w}×{h}") });
        }
    }
    row.length = number(&probe["format"]["duration"]).map(clock);
    if let Some(stream) = picture {
        let mut codec = stream["codec_name"].as_str().map(codec_name).unwrap_or_default();
        let format = stream["pix_fmt"].as_str().unwrap_or_default();
        if format.contains("10") || format.contains("12") {
            codec.push_str(if format.contains("12") { " 12-bit" } else { " 10-bit" });
        }
        row.format = Some(codec).filter(|codec| !codec.is_empty());
        let rate = stream["avg_frame_rate"].as_str().and_then(|rate| {
            let (n, d) = rate.split_once('/')?;
            Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok().filter(|d| *d > 0.0)?)
        });
        row.rate = rate.filter(|fps| *fps > 0.0).map(|fps| match (fps - fps.round()).abs() < 0.01 {
            true => format!("{fps:.0}"),
            false => format!("{fps:.3}").trim_end_matches('0').to_string(),
        });
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
        row.hdr = hdr;
        let dolby = stream["side_data_list"].as_array().into_iter().flatten().any(|data| data["side_data_type"].as_str().is_some_and(|kind| kind.contains("DOVI")));
        if dolby {
            row.hdr.push("Dolby Vision".to_string());
        }
    }
    row.bitrate = number(&probe["format"]["bit_rate"]).map(|bits| format!("{:.1} Mb/s", bits / 1e6));
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
        row.audio = Some(audio);
        row.hz = number(&stream["sample_rate"]).map(|rate| format!("{} kHz", format!("{:.1}", rate / 1000.0).trim_end_matches(".0")));
    }
    row.bytes = std::fs::metadata(path).ok().map(|meta| human_size(meta.len()));
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn the_table_lines_up_and_leaves_out_what_no_file_has() {
        let clip = Row { pixels: some("1080×1920"), length: some("0:04"), format: some("H.264"), rate: some("30"), bitrate: some("16.9 Mb/s"), audio: some("AAC mono"), hz: some("44.1 kHz"), bytes: some("8.1 MB"), ..Row::default() };
        let photo = Row { pixels: some("640×640"), format: some("PNG"), bytes: some("337 B"), ..Row::default() };
        let lines = table(&[("clip.mp4".to_string(), clip), ("small.png".to_string(), photo)], false);
        assert_eq!(
            lines,
            [
                "      W×H  Length  Format  fps    Bitrate  Audio                Size  Name",
                "1080×1920    0:04  H.264    30  16.9 Mb/s  AAC mono 44.1 kHz  8.1 MB  clip.mp4",
                "  640×640          PNG                                         337 B  small.png",
            ]
        );
    }
}
