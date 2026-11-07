//! `--info`: what each file is, as the seek bar's details say it of a
//! video; and no window. One file is told of in a line; more, in a table
//! whose columns line up, the names first. `--info-sort` orders the table
//! by a column and `--info-filter` keeps the files that pass a test;
//! what ffprobe says of a video is kept in the state folder, so a folder
//! asked about again is told of at once.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rayon::prelude::*;
use serde_json::{json, Value};

use crate::app::{clock, codec_name};
use crate::files;
use crate::meta::human_size;

/// What is known of a file, as numbers where it is one.
#[derive(Default, Debug, Clone, PartialEq)]
struct Facts {
    /// As shown: a video turned a quarter has these the other way round.
    width: Option<u64>,
    height: Option<u64>,
    seconds: Option<f64>,
    /// The picture's format, or the video's codec and depth.
    format: Option<String>,
    /// The codec's profile, when it is not the everyday one.
    profile: Option<String>,
    fps: Option<f64>,
    /// HDR, Dolby Vision, Ultra HDR.
    hdr: Vec<String>,
    /// Bits a second, all streams.
    bitrate: Option<f64>,
    /// The sound's codec and channels, its bits a second, its sample rate.
    audio: Option<String>,
    audio_bitrate: Option<f64>,
    hz: Option<f64>,
    bytes: Option<u64>,
}

impl Facts {
    fn pixels(&self) -> Option<String> {
        Some(format!("{}×{}", self.width?, self.height?))
    }

    fn ratio(&self) -> Option<String> {
        Some(aspect(self.width?, self.height?))
    }

    fn rate(&self) -> Option<String> {
        self.fps.map(|fps| match (fps - fps.round()).abs() < 0.01 {
            true => format!("{fps:.0}"),
            false => format!("{fps:.3}").trim_end_matches('0').to_string(),
        })
    }

    fn sound(&self) -> Vec<String> {
        let mut parts: Vec<String> = self.audio.iter().cloned().collect();
        parts.extend(self.audio_bitrate.map(|bits| format!("{:.0} kb/s", bits / 1000.0)));
        parts.extend(self.hz.map(|rate| format!("{} kHz", format!("{:.1}", rate / 1000.0).trim_end_matches(".0"))));
        parts
    }

    /// Its parts in a line, with `·` between.
    fn line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(self.pixels().map(|pixels| format!("{pixels} ({})", self.ratio().unwrap_or_default())));
        parts.extend(self.seconds.map(clock));
        parts.extend([self.format.clone(), self.profile.clone()].into_iter().flatten().reduce(|codec, profile| format!("{codec} {profile}")));
        parts.extend(self.rate().map(|rate| format!("{rate} fps")));
        parts.extend(self.hdr.iter().cloned());
        parts.extend(self.bitrate.map(megabits));
        parts.extend(self.sound());
        parts.extend(self.bytes.map(human_size));
        parts.join(" · ")
    }

    fn to_json(&self) -> Value {
        json!({
            "width": self.width, "height": self.height, "seconds": self.seconds, "format": self.format, "profile": self.profile,
            "fps": self.fps, "hdr": self.hdr, "bitrate": self.bitrate, "audio": self.audio, "audio_bitrate": self.audio_bitrate,
            "hz": self.hz, "bytes": self.bytes,
        })
    }

    fn from_json(value: &Value) -> Facts {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Facts {
            width: value["width"].as_u64(),
            height: value["height"].as_u64(),
            seconds: value["seconds"].as_f64(),
            format: text("format"),
            profile: text("profile"),
            fps: value["fps"].as_f64(),
            hdr: value["hdr"].as_array().into_iter().flatten().filter_map(|badge| badge.as_str().map(str::to_string)).collect(),
            bitrate: value["bitrate"].as_f64(),
            audio: text("audio"),
            audio_bitrate: value["audio_bitrate"].as_f64(),
            hz: value["hz"].as_f64(),
            bytes: value["bytes"].as_u64(),
        }
    }
}

fn megabits(bits: f64) -> String {
    format!("{:.1} Mb/s", bits / 1e6)
}

/// Width to height as people say it: 16:9, 9:16, 4:3, 21:9; one near
/// enough to a usual shape (a few pixels off, as video sizes often are)
/// is called that; any other as a number to one, the long side first.
fn aspect(width: u64, height: u64) -> String {
    const USUAL: &[(u64, u64, &str)] = &[
        (16, 9, "16:9"), (9, 16, "9:16"), (4, 3, "4:3"), (3, 4, "3:4"), (1, 1, "1:1"), (3, 2, "3:2"), (2, 3, "2:3"),
        (4, 5, "4:5"), (5, 4, "5:4"), (16, 10, "16:10"), (10, 16, "10:16"), (21, 9, "21:9"), (9, 21, "9:21"), (2, 1, "2:1"), (1, 2, "1:2"),
    ];
    if width == 0 || height == 0 {
        return String::new();
    }
    let shape = width as f64 / height as f64;
    if let Some((_, _, name)) = USUAL.iter().find(|(w, h, _)| (shape / (*w as f64 / *h as f64) - 1.0).abs() < 0.02) {
        return name.to_string();
    }
    match shape >= 1.0 {
        true => format!("{shape:.2}:1"),
        false => format!("1:{:.2}", 1.0 / shape),
    }
}

/// A codec's profile, unless it is the one nearly every file of that codec
/// has, which says nothing.
fn notable_profile(codec: &str, profile: &str) -> Option<String> {
    let everyday: &[&str] = match codec {
        "h264" => &["High", "Main", "Baseline", "Constrained Baseline"],
        "hevc" => &["Main", "Main 10"],
        "vp9" => &["Profile 0"],
        "av1" => &["Main"],
        "mpeg4" => &["Simple Profile", "Advanced Simple Profile"],
        _ => &[],
    };
    (!profile.is_empty() && profile != "unknown" && !everyday.contains(&profile)).then(|| profile.to_string())
}

/// A column of the table: its heading, what it shows of a file, and
/// whether it lines up on the right, as numbers do.
struct Column {
    heading: &'static str,
    cell: fn(&Facts) -> String,
    right: bool,
}

const COLUMNS: &[Column] = &[
    Column { heading: "W×H", cell: |facts| facts.pixels().unwrap_or_default(), right: true },
    Column { heading: "Ratio", cell: |facts| facts.ratio().unwrap_or_default(), right: false },
    Column { heading: "Length", cell: |facts| facts.seconds.map(clock).unwrap_or_default(), right: true },
    Column { heading: "Format", cell: |facts| facts.format.clone().unwrap_or_default(), right: false },
    Column { heading: "Profile", cell: |facts| facts.profile.clone().unwrap_or_default(), right: false },
    Column { heading: "fps", cell: |facts| facts.rate().unwrap_or_default(), right: true },
    Column { heading: "HDR", cell: |facts| facts.hdr.join(" "), right: false },
    Column { heading: "Bitrate", cell: |facts| facts.bitrate.map(megabits).unwrap_or_default(), right: true },
    Column { heading: "Audio", cell: |facts| facts.sound().join(" "), right: false },
    Column { heading: "Size", cell: |facts| facts.bytes.map(human_size).unwrap_or_default(), right: true },
];

/// A name longer than this is cut in the middle in the table, keeping its
/// start and its end, which tell files apart.
const NAME_AT_MOST: usize = 60;

/// What the table can be sorted and filtered by.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Key {
    Name,
    Width,
    Height,
    Pixels,
    Ratio,
    Length,
    Format,
    Fps,
    Hdr,
    Bitrate,
    Audio,
    Size,
}

const KEYS: &[(&str, Key)] = &[
    ("name", Key::Name), ("width", Key::Width), ("height", Key::Height), ("pixels", Key::Pixels), ("resolution", Key::Pixels),
    ("ratio", Key::Ratio), ("length", Key::Length), ("duration", Key::Length), ("format", Key::Format), ("codec", Key::Format),
    ("fps", Key::Fps), ("hdr", Key::Hdr), ("bitrate", Key::Bitrate), ("audio", Key::Audio), ("size", Key::Size),
];

fn key_named(name: &str) -> Result<Key, String> {
    KEYS.iter().find(|(word, _)| word.eq_ignore_ascii_case(name)).map(|(_, key)| *key).ok_or_else(|| {
        let words: Vec<&str> = KEYS.iter().map(|(word, _)| *word).collect();
        format!("no column {name:?}: one of {}", words.join(", "))
    })
}

/// A file's value in a column: a number, or words.
enum Cell {
    Number(f64),
    Words(String),
}

impl Key {
    fn of(self, name: &str, facts: &Facts) -> Option<Cell> {
        use Cell::*;
        Some(match self {
            Key::Name => Words(name.to_string()),
            Key::Width => Number(facts.width? as f64),
            Key::Height => Number(facts.height? as f64),
            Key::Pixels => Number((facts.width? * facts.height?) as f64),
            Key::Ratio => Number(facts.width? as f64 / facts.height?.max(1) as f64),
            Key::Length => Number(facts.seconds?),
            Key::Format => Words([facts.format.clone(), facts.profile.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ")),
            Key::Fps => Number(facts.fps?),
            Key::Hdr => Words(facts.hdr.join(" ")),
            // In the units the table shows: Mb/s.
            Key::Bitrate => Number(facts.bitrate? / 1e6),
            Key::Audio => Words(facts.sound().join(" ")),
            Key::Size => Number(facts.bytes? as f64),
        })
    }

    /// A value to compare with, as it is written: sizes with their units
    /// (`500KB`, `2GB`, plain is MB), lengths as seconds or `m:ss`, ratios
    /// as `9:16` or a number.
    fn parse(self, text: &str) -> Result<Cell, String> {
        let bad = || format!("{text:?} is not a value for {}", format!("{self:?}").to_lowercase());
        let number = |text: &str| text.trim().parse::<f64>().map_err(|_| bad());
        Ok(match self {
            Key::Name | Key::Format | Key::Hdr | Key::Audio => Cell::Words(text.to_string()),
            Key::Size => {
                let lower = text.trim().to_ascii_lowercase();
                let (kb, mb, gb) = (1024.0, 1024.0 * 1024.0, 1024.0 * 1024.0 * 1024.0);
                let units = [("kb", kb), ("mb", mb), ("gb", gb), ("k", kb), ("m", mb), ("g", gb), ("b", 1.0)];
                let (digits, scale) = units.iter().find_map(|(unit, scale)| lower.strip_suffix(unit).map(|digits| (digits, *scale))).unwrap_or((lower.as_str(), mb));
                Cell::Number(number(digits)? * scale)
            }
            Key::Length => match text.split_once(':') {
                Some((minutes, seconds)) => Cell::Number(number(minutes)? * 60.0 + number(seconds)?),
                None => Cell::Number(number(text)?),
            },
            Key::Ratio => match text.split_once(':') {
                Some((w, h)) => Cell::Number(number(w)? / number(h)?.max(f64::MIN_POSITIVE)),
                None => Cell::Number(number(text)?),
            },
            _ => Cell::Number(number(text)?),
        })
    }
}

/// One `--info-filter`: a column, a comparison and a value, as in
/// `bitrate>5`, `ratio<1`, `format=hevc`, `name~beach`.
struct Test {
    key: Key,
    op: &'static str,
    value: Cell,
}

impl Test {
    fn parse(text: &str) -> Result<Test, String> {
        let ops = [">=", "<=", "!=", ">", "<", "=", "~"];
        let (at, op) = ops
            .iter()
            .filter_map(|op| text.find(op).map(|at| (at, *op)))
            .min_by_key(|(at, op)| (*at, std::cmp::Reverse(op.len())))
            .ok_or_else(|| format!("{text:?}: a column, one of {} and a value, as in bitrate>5", ops.join(" ")))?;
        let key = key_named(text[..at].trim())?;
        let value = key.parse(text[at + op.len()..].trim())?;
        Ok(Test { key, op, value })
    }

    fn passes(&self, name: &str, facts: &Facts) -> bool {
        let Some(have) = self.key.of(name, facts) else { return self.op == "!=" };
        match (have, &self.value) {
            (Cell::Number(have), Cell::Number(want)) => {
                let same = (have - want).abs() < 1e-6 * want.abs().max(1.0);
                match self.op {
                    ">" => have > *want,
                    "<" => have < *want,
                    ">=" => have >= *want || same,
                    "<=" => have <= *want || same,
                    "=" => same,
                    "!=" => !same,
                    _ => false,
                }
            }
            (Cell::Words(have), Cell::Words(want)) => {
                let (have, want) = (have.to_lowercase(), want.to_lowercase());
                match self.op {
                    "=" => have == want,
                    "!=" => have != want,
                    "~" => have.contains(&want),
                    ">" => have > want,
                    "<" => have < want,
                    ">=" => have >= want,
                    "<=" => have <= want,
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

/// How the table is narrowed and ordered.
pub struct Asked<'a> {
    pub sort: Option<&'a str>,
    pub filters: &'a [String],
    pub reverse: bool,
}

/// Tells of each file, in the order given, each worked out on a core of
/// its own; whether every one could be read and every request made sense.
pub fn print(paths: &[PathBuf], asked: &Asked) -> bool {
    let tests: Result<Vec<Test>, String> = asked.filters.iter().map(|text| Test::parse(text)).collect();
    let sort = asked.sort.map(key_named).transpose();
    let (tests, sort) = match (tests, sort) {
        (Ok(tests), Ok(sort)) => (tests, sort),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("jog: {error}");
            return false;
        }
    };
    let mut cache = Cache::load();
    let found: Vec<Result<Facts, String>> = paths.par_iter().map(|path| facts(path, &cache)).collect();
    let mut read = Vec::new();
    for (path, facts) in paths.iter().zip(found) {
        match facts {
            Ok(facts) => {
                cache.keep(path, &facts);
                read.push((path.display().to_string(), facts));
            }
            Err(error) => eprintln!("{}: {error}", path.display()),
        }
    }
    cache.save();
    let all = read.len() == paths.len();
    let single = paths.len() == 1 && tests.is_empty();
    read.retain(|(name, facts)| tests.iter().all(|test| test.passes(name, facts)));
    if let Some(key) = sort {
        // Numbers largest first, words from A; what has none, last.
        read.sort_by(|(a_name, a), (b_name, b)| match (key.of(a_name, a), key.of(b_name, b)) {
            (Some(Cell::Number(a)), Some(Cell::Number(b))) => b.total_cmp(&a),
            (Some(Cell::Words(a)), Some(Cell::Words(b))) => a.to_lowercase().cmp(&b.to_lowercase()),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
        if asked.reverse {
            read.reverse();
        }
    }
    match read.as_slice() {
        [] => {}
        [(name, facts)] if single => println!("{name}: {}", facts.line()),
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

/// A name cut in the middle to at most `NAME_AT_MOST` characters.
fn shortened(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= NAME_AT_MOST {
        return name.to_string();
    }
    let tail = NAME_AT_MOST * 3 / 5;
    let head = NAME_AT_MOST - tail - 1;
    chars[..head].iter().chain(['…'].iter()).chain(chars[chars.len() - tail..].iter()).collect()
}

/// The table's lines: a heading, then a line for each file, the name
/// first. A column no file has anything in is left out.
fn table(rows: &[(String, Facts)], dim: bool) -> Vec<String> {
    let width = |text: &str| text.chars().count();
    let names: Vec<String> = rows.iter().map(|(name, _)| shortened(name)).collect();
    let cells: Vec<Vec<String>> = rows.iter().map(|(_, facts)| COLUMNS.iter().map(|column| (column.cell)(facts)).collect()).collect();
    let used: Vec<usize> = (0..COLUMNS.len()).filter(|&at| cells.iter().any(|row| !row[at].is_empty())).collect();
    let widths: Vec<usize> = used.iter().map(|&at| cells.iter().map(|row| width(&row[at])).max().unwrap_or(0).max(width(COLUMNS[at].heading))).collect();
    let name_width = names.iter().map(|name| width(name)).max().unwrap_or(0).max(width("Name"));
    let line = |texts: &[&str], name: &str| {
        let mut out = format!("{name}{}  ", " ".repeat(name_width - width(name)));
        for ((&at, &w), text) in used.iter().zip(&widths).zip(texts) {
            let pad = " ".repeat(w - width(text));
            match COLUMNS[at].right {
                true => out.push_str(&format!("{pad}{text}  ")),
                false => out.push_str(&format!("{text}{pad}  ")),
            }
        }
        out.trim_end().to_string()
    };
    let headings: Vec<&str> = used.iter().map(|&at| COLUMNS[at].heading).collect();
    let heading = line(&headings, "Name");
    let mut lines = vec![if dim { format!("\x1b[2m{heading}\x1b[0m") } else { heading }];
    for (name, row) in names.iter().zip(&cells) {
        let texts: Vec<&str> = used.iter().map(|&at| row[at].as_str()).collect();
        lines.push(line(&texts, name));
    }
    lines
}

/// What ffprobe said of videos before, by path, with the file's length and
/// time of change then: a file changed since is asked about afresh.
struct Cache {
    known: HashMap<String, Value>,
    changed: bool,
}

/// Videos kept at most: the ones asked about longest ago go first.
const CACHE_AT_MOST: usize = 50_000;

impl Cache {
    fn file() -> Option<PathBuf> {
        crate::positions::state_dir().map(|dir| dir.join("info-cache.json"))
    }

    fn load() -> Cache {
        let known = Self::file().and_then(|file| std::fs::read(file).ok()).and_then(|bytes| serde_json::from_slice::<HashMap<String, Value>>(&bytes).ok()).unwrap_or_default();
        Cache { known, changed: false }
    }

    /// The file's length and time of change, which say whether what was
    /// kept of it still holds.
    fn stamp(path: &Path) -> Option<(u64, String)> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
        Some((meta.len(), modified.to_string()))
    }

    fn key(path: &Path) -> String {
        std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string()
    }

    fn get(&self, path: &Path) -> Option<Facts> {
        let entry = self.known.get(&Self::key(path))?;
        let (len, modified) = Self::stamp(path)?;
        let same = entry["len"].as_u64() == Some(len) && entry["modified"].as_str() == Some(modified.as_str());
        same.then(|| Facts::from_json(&entry["facts"]))
    }

    fn keep(&mut self, path: &Path, facts: &Facts) {
        if !files::is_video(path) || files::is_url(path) {
            return;
        }
        let Some((len, modified)) = Self::stamp(path) else { return };
        let asked = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |at| at.as_secs());
        self.known.insert(Self::key(path), json!({ "len": len, "modified": modified, "asked": asked, "facts": facts.to_json() }));
        self.changed = true;
    }

    fn save(mut self) {
        if !self.changed {
            return;
        }
        if self.known.len() > CACHE_AT_MOST {
            let mut asked: Vec<(u64, String)> = self.known.iter().map(|(key, entry)| (entry["asked"].as_u64().unwrap_or(0), key.clone())).collect();
            asked.sort();
            for (_, key) in asked.iter().take(self.known.len() - CACHE_AT_MOST) {
                self.known.remove(key);
            }
        }
        let Some(file) = Self::file() else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Written whole beside it and moved over it, so that a run cut
        // short never leaves half a file.
        let new = file.with_extension("json.new");
        if serde_json::to_vec(&self.known).is_ok_and(|bytes| std::fs::write(&new, bytes).is_ok()) {
            let _ = std::fs::rename(&new, &file);
        }
    }
}

fn facts(path: &Path, cache: &Cache) -> Result<Facts, String> {
    if !files::is_video(path) {
        return picture(path);
    }
    match cache.get(path) {
        Some(facts) => Ok(facts),
        None => video(path),
    }
}

/// A picture: its size and format, and how large the file is.
fn picture(path: &Path) -> Result<Facts, String> {
    let found = crate::picmeta::read(path)?;
    let mut facts = Facts { width: found.size.map(|(w, _)| u64::from(w)), height: found.size.map(|(_, h)| u64::from(h)), format: found.format.clone(), bytes: Some(found.bytes), ..Facts::default() };
    if found.gain.is_some() {
        facts.hdr.push("Ultra HDR".to_string());
    }
    Ok(facts)
}

/// A video, as ffprobe tells of it: its size as shown, length, codec,
/// depth and profile, rate, HDR, bitrate, sound, and how large the file is.
fn video(path: &Path) -> Result<Facts, String> {
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
    let mut facts = Facts::default();
    if let Some(stream) = picture {
        if let (Some(w), Some(h)) = (stream["width"].as_u64(), stream["height"].as_u64()) {
            // Turned a quarter, as phones record upright video, it is shown
            // the other way round.
            let rotation = stream["side_data_list"].as_array().into_iter().flatten().find_map(|data| number(&data["rotation"])).or_else(|| number(&stream["tags"]["rotate"]));
            let turned = rotation.is_some_and(|degrees| (degrees as i64).rem_euclid(180) == 90);
            (facts.width, facts.height) = if turned { (Some(h), Some(w)) } else { (Some(w), Some(h)) };
        }
    }
    facts.seconds = number(&probe["format"]["duration"]);
    if let Some(stream) = picture {
        let name = stream["codec_name"].as_str().unwrap_or_default();
        let mut codec = codec_name(name);
        let format = stream["pix_fmt"].as_str().unwrap_or_default();
        if format.contains("10") || format.contains("12") {
            codec.push_str(if format.contains("12") { " 12-bit" } else { " 10-bit" });
        }
        facts.format = Some(codec).filter(|codec| !codec.is_empty());
        facts.profile = stream["profile"].as_str().and_then(|profile| notable_profile(name, profile));
        let rate = stream["avg_frame_rate"].as_str().and_then(|rate| {
            let (n, d) = rate.split_once('/')?;
            Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok().filter(|d| *d > 0.0)?)
        });
        facts.fps = rate.filter(|fps| *fps > 0.0);
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
        facts.hdr = hdr;
        let dolby = stream["side_data_list"].as_array().into_iter().flatten().any(|data| data["side_data_type"].as_str().is_some_and(|kind| kind.contains("DOVI")));
        if dolby {
            facts.hdr.push("Dolby Vision".to_string());
        }
    }
    facts.bitrate = number(&probe["format"]["bit_rate"]);
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
        facts.audio = Some(audio);
        // Matroska keeps a stream's rate in a tag of its own.
        facts.audio_bitrate = number(&stream["bit_rate"]).or_else(|| number(&stream["tags"]["BPS"]));
        facts.hz = number(&stream["sample_rate"]);
    }
    facts.bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> Facts {
        Facts { width: Some(1080), height: Some(1920), seconds: Some(4.0), format: Some("H.264".into()), fps: Some(30.0), bitrate: Some(16.9e6), audio: Some("AAC mono".into()), audio_bitrate: Some(69_000.0), hz: Some(44_100.0), bytes: Some(8_440_000), ..Facts::default() }
    }

    fn photo() -> Facts {
        Facts { width: Some(640), height: Some(640), format: Some("PNG".into()), bytes: Some(337), ..Facts::default() }
    }

    #[test]
    fn the_table_lines_up_and_leaves_out_what_no_file_has() {
        let lines = table(&[("clip.mp4".to_string(), clip()), ("small.png".to_string(), photo())], false);
        assert_eq!(
            lines,
            [
                "Name             W×H  Ratio  Length  Format  fps    Bitrate  Audio                        Size",
                "clip.mp4   1080×1920  9:16     0:04  H.264    30  16.9 Mb/s  AAC mono 69 kb/s 44.1 kHz  8.0 MB",
                "small.png    640×640  1:1            PNG                                                 337 B",
            ]
        );
    }

    #[test]
    fn shapes_are_named_as_people_say_them() {
        assert_eq!(aspect(1920, 1080), "16:9");
        assert_eq!(aspect(1080, 1920), "9:16");
        assert_eq!(aspect(1920, 1200), "16:10");
        assert_eq!(aspect(1200, 800), "3:2");
        // A few pixels off a usual shape is that shape.
        assert_eq!(aspect(1080, 1350), "4:5");
        assert_eq!(aspect(2560, 1080), "21:9");
        assert_eq!(aspect(1998, 1080), "1.85:1");
        assert_eq!(aspect(1080, 2400), "1:2.22");
    }

    #[test]
    fn a_long_name_keeps_its_start_and_its_end() {
        let name = format!("{}clip.mp4", "a".repeat(100));
        let short = shortened(&name);
        assert_eq!(short.chars().count(), NAME_AT_MOST);
        assert!(short.starts_with("aaaa") && short.ends_with("aaclip.mp4") && short.contains('…'));
        assert_eq!(shortened("short.mp4"), "short.mp4");
    }

    #[test]
    fn filters_compare_in_the_table_units() {
        let pass = |test: &str, facts: &Facts| Test::parse(test).unwrap().passes("clip.mp4", facts);
        assert!(pass("bitrate>5", &clip()) && !pass("bitrate>20", &clip()));
        assert!(pass("ratio<1", &clip()) && !pass("ratio<1", &photo()));
        assert!(pass("size>=8mb", &clip()) && pass("size<1kb", &photo()));
        assert!(pass("length<=0:05", &clip()) && pass("fps=30", &clip()));
        assert!(pass("format=h.264", &clip()) && pass("audio~aac", &clip()) && pass("name~clip", &clip()));
        // A file without the value passes only "is not".
        assert!(!pass("fps>1", &photo()) && pass("fps!=30", &photo()));
        assert!(Test::parse("speed>1").is_err() && Test::parse("bitrate").is_err() && Test::parse("bitrate>fast").is_err());
    }

    #[test]
    fn only_an_unusual_profile_is_named() {
        assert_eq!(notable_profile("h264", "High"), None);
        assert_eq!(notable_profile("h264", "High 4:4:4 Predictive").as_deref(), Some("High 4:4:4 Predictive"));
        assert_eq!(notable_profile("hevc", "Main 10"), None);
        assert_eq!(notable_profile("hevc", "Rext").as_deref(), Some("Rext"));
    }

    #[test]
    fn facts_come_back_from_the_cache_as_they_went_in() {
        let mut facts = clip();
        facts.hdr = vec!["HDR10".into(), "Dolby Vision".into()];
        facts.profile = Some("High 10".into());
        assert_eq!(Facts::from_json(&facts.to_json()), facts);
    }
}
