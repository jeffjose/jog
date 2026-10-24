//! Converting videos: H.264, HEVC or AV1, with Opus or AAC sound, in MP4,
//! MKV or WebM. None of it is built into jog: it is ffmpeg, whichever is
//! on the PATH, so a newer ffmpeg brings newer encoders with it. jog is
//! only the easy way in, with settings that suit most videos.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::frame::Transform;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Container {
    Mp4,
    Mkv,
    WebM,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Audio {
    Opus,
    Aac,
    /// The sound as it is, not encoded again.
    Copy,
}

/// What a video is converted to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub codec: Codec,
    pub container: Container,
    /// `None`: the usual for the container, from the settings.
    pub audio: Option<Audio>,
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
        }
    }
}

impl Container {
    pub fn name(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mkv => "MKV",
            Container::WebM => "WebM",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::WebM => "webm",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "matroska",
            Container::WebM => "webm",
        }
    }

    /// The codec a container gets when only it is named.
    fn usual_codec(self) -> Codec {
        match self {
            Container::Mp4 => Codec::H264,
            Container::Mkv => Codec::Hevc,
            Container::WebM => Codec::Av1,
        }
    }
}

impl Audio {
    pub fn name(self) -> &'static str {
        match self {
            Audio::Opus => "Opus",
            Audio::Aac => "AAC",
            Audio::Copy => "sound as it is",
        }
    }

    pub fn parse(word: &str) -> Option<Audio> {
        match word.to_ascii_lowercase().as_str() {
            "opus" => Some(Audio::Opus),
            "aac" => Some(Audio::Aac),
            "copy" => Some(Audio::Copy),
            _ => None,
        }
    }
}

impl Target {
    pub const fn new(codec: Codec, container: Container) -> Target {
        Target { codec, container, audio: None }
    }

    /// From `av1.mkv`, `hevc.mp4`, or a container alone: `mp4`, `mkv`,
    /// `webm`.
    pub fn parse(spec: &str) -> Option<Target> {
        let spec = spec.to_ascii_lowercase();
        let (codec, container) = match spec.split_once('.') {
            Some((codec, container)) => (Some(codec), container),
            None => (None, spec.as_str()),
        };
        let container = match container {
            "mp4" => Container::Mp4,
            "mkv" => Container::Mkv,
            "webm" => Container::WebM,
            _ => return None,
        };
        let codec = match codec {
            None => container.usual_codec(),
            Some("h264" | "avc" | "x264") => Codec::H264,
            Some("hevc" | "h265" | "x265") => Codec::Hevc,
            Some("av1") => Codec::Av1,
            Some(_) => return None,
        };
        Some(Target::new(codec, container))
    }

    /// The sound it gets.
    pub fn audio(&self, settings: &VideoSettings) -> Audio {
        match (self.audio, self.container) {
            (Some(audio), _) => audio,
            (None, Container::Mp4) => settings.mp4_audio,
            (None, _) => Audio::Opus,
        }
    }

    /// Why this cannot be made, if it cannot: WebM holds only AV1 (of
    /// these) and Opus.
    pub fn problem(&self, settings: &VideoSettings) -> Option<String> {
        if self.container == Container::WebM {
            if self.codec != Codec::Av1 {
                return Some(format!("WebM cannot hold {}: AV1 only", self.codec.name()));
            }
            if self.audio(settings) == Audio::Aac {
                return Some("WebM cannot hold AAC: Opus only".to_string());
            }
        }
        None
    }

    pub fn describe(&self, settings: &VideoSettings) -> String {
        format!("{} {}, {}", self.container.name(), self.codec.name(), self.audio(settings).name())
    }

    /// Where the video at `path` is written: beside it, named for the
    /// codec too, so that clip.mp4 made H.264 does not meet itself.
    pub fn output(&self, path: &Path) -> PathBuf {
        let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        path.with_file_name(format!("{stem}.{}.{}", self.codec.word(), self.container.extension()))
    }
}

/// The quality of each codec, as the constant rate factor ffmpeg's
/// encoders take (lower is better and larger), and the sound for MP4,
/// where Opus is the better codec but AAC the one everything plays.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VideoSettings {
    pub h264_crf: u8,
    pub hevc_crf: u8,
    pub av1_crf: u8,
    pub mp4_audio: Audio,
}

impl Default for VideoSettings {
    fn default() -> VideoSettings {
        // Each about where its encoder looks very good at a sensible size:
        // x264's 20, x265's 22, and 30 for AV1, which is 10-bit as well.
        VideoSettings { h264_crf: 20, hevc_crf: 22, av1_crf: 30, mp4_audio: Audio::Aac }
    }
}

impl VideoSettings {
    /// From `video = { h264-crf = 20, hevc-crf = 22, av1-crf = 30,
    /// mp4-audio = "aac" }` in `[convert]`.
    pub fn apply(&mut self, fields: &toml::Table) -> Result<(), String> {
        for (key, value) in fields {
            let wrong = || format!("convert.video.{key}: not a usable value");
            let crf = || value.as_integer().filter(|n| (0..=63).contains(n)).map(|n| n as u8).ok_or_else(wrong);
            match key.replace('_', "-").as_str() {
                "h264-crf" => self.h264_crf = crf()?,
                "hevc-crf" => self.hevc_crf = crf()?,
                "av1-crf" => self.av1_crf = crf()?,
                "mp4-audio" => self.mp4_audio = value.as_str().and_then(Audio::parse).ok_or_else(wrong)?,
                _ => return Err(format!("convert.video.{key}: no such setting")),
            }
        }
        Ok(())
    }

    pub fn to_toml(&self) -> String {
        let audio = match self.mp4_audio {
            Audio::Opus => "opus",
            Audio::Aac => "aac",
            Audio::Copy => "copy",
        };
        format!("video = {{ h264-crf = {}, hevc-crf = {}, av1-crf = {}, mp4-audio = \"{audio}\" }}\n", self.h264_crf, self.hevc_crf, self.av1_crf)
    }
}

/// How much time an encoder spends to make a file smaller.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Speed {
    Fast,
    #[default]
    Medium,
    Slow,
}

impl Speed {
    pub const ALL: [Speed; 3] = [Speed::Fast, Speed::Medium, Speed::Slow];

    pub fn name(self) -> &'static str {
        match self {
            Speed::Fast => "Fast",
            Speed::Medium => "Medium",
            Speed::Slow => "Slow",
        }
    }
}

/// What Inspect's Convert tab turns, past the codec and container: each
/// at its default the same as converting from the menu.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Knobs {
    /// The constant rate factor, instead of the codec's from the settings.
    pub crf: Option<u8>,
    pub speed: Speed,
    /// No taller than this, the width in proportion; never made larger.
    pub height: Option<u32>,
    /// HDR taken down to SDR, for screens and sites that show HDR gray.
    pub to_sdr: bool,
}

impl Knobs {
    /// The rate factor `codec` gets.
    pub fn crf(&self, codec: Codec, settings: &VideoSettings) -> u8 {
        self.crf.unwrap_or(match codec {
            Codec::H264 => settings.h264_crf,
            Codec::Hevc => settings.hevc_crf,
            Codec::Av1 => settings.av1_crf,
        })
    }
}

impl Codec {
    /// The rate factors worth offering, best first: about where each
    /// encoder's pictures are near the original to where they visibly
    /// are not.
    pub fn crf_range(self) -> (u8, u8) {
        match self {
            Codec::H264 => (14, 32),
            Codec::Hevc => (16, 34),
            Codec::Av1 => (18, 48),
        }
    }
}

/// The encoders the ffmpeg on the PATH has, asked once.
fn encoders() -> &'static str {
    static FOUND: OnceLock<String> = OnceLock::new();
    FOUND.get_or_init(|| {
        let output = Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).stdin(Stdio::null()).output();
        output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
    })
}

fn has(encoder: &str) -> bool {
    encoders().lines().any(|line| line.split_whitespace().nth(1) == Some(encoder))
}

/// What is known of the video before it is converted: its length, for
/// how far along it is, and whether it has more than 8 bits, which HEVC
/// keeps.
struct Probe {
    seconds: f64,
    deep: bool,
    has_audio: bool,
    /// HDR, which an 8-bit animation has to be tone mapped from.
    hdr: bool,
    /// Frames a second, as the video's own rate gives it: what frame of
    /// the output a time is, for HDR10+ written again a frame at a time.
    fps: f64,
}

fn probe(path: &Path) -> Probe {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration:stream=codec_type,pix_fmt,color_transfer,avg_frame_rate", "-of", "default=nw=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output();
    let text = output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    let field = |key: &'static str| text.lines().filter_map(move |line| line.strip_prefix(key));
    // The first that is a rate: sound has none, as 0/0.
    let fps = field("avg_frame_rate=").find_map(|rate| {
        let (n, d) = rate.split_once('/')?;
        let fps = n.parse::<f64>().ok()? / d.parse::<f64>().ok().filter(|d| *d > 0.0)?;
        (fps > 0.0).then_some(fps)
    });
    Probe {
        seconds: field("duration=").find_map(|value| value.parse().ok()).unwrap_or(0.0),
        deep: field("pix_fmt=").any(|format| format.contains("10") || format.contains("12")),
        has_audio: field("codec_type=").any(|kind| kind == "audio"),
        hdr: field("color_transfer=").any(|transfer| transfer == "smpte2084" || transfer == "arib-std-b67"),
        fps: fps.unwrap_or(30.0),
    }
}

/// HDR10+ for x265 to write (`dhdr10-info`), as the JSON it reads: for
/// each frame of the output, from `from` to `to` seconds of the video at
/// `fps`, the metadata the video has at that time, with a scene for each
/// run of frames that share it. None when the video has no HDR10+.
pub fn hdr10plus_json(info: &crate::hdrmeta::Info, (from, to): (f64, f64), fps: f64) -> Option<String> {
    use serde_json::json;
    if info.hdr10plus.is_empty() || to <= from || fps <= 0.0 {
        return None;
    }
    let frames = ((to - from) * fps).round() as usize;
    let mut scenes = Vec::new();
    let (mut scene, mut first) = (0usize, Vec::new());
    let mut last: Option<&crate::hdrmeta::Hdr10Plus> = None;
    for frame in 0..frames {
        // The middle of the frame, clear of where one meets the next.
        let at = from + (frame as f64 + 0.5) / fps;
        let Some(plus) = info.hdr10plus_at(at) else { continue };
        if last.is_some_and(|last| last != plus) {
            scene += 1;
        }
        if last != Some(plus) {
            first.push(frame);
        }
        last = Some(plus);
        // Back to the numbers as stored: tenths of a nit, and the curve's
        // points out of 4095 and 1023.
        let tenths = |nits: f64| (nits * 10.0).round() as u64;
        scenes.push(json!({
            "NumberOfWindows": 1,
            "TargetedSystemDisplayMaximumLuminance": plus.target.round() as u64,
            "LuminanceParameters": {
                "MaxScl": plus.maxscl.map(tenths),
                "AverageRGB": tenths(plus.average),
                "LuminanceDistributions": {
                    "DistributionIndex": plus.distribution.iter().map(|(share, _)| share).collect::<Vec<_>>(),
                    "DistributionValues": plus.distribution.iter().map(|(_, value)| value).collect::<Vec<_>>(),
                },
            },
            "BezierCurveData": {
                "KneePointX": (plus.knee.0 * 4095.0).round() as u64,
                "KneePointY": (plus.knee.1 * 4095.0).round() as u64,
                "Anchors": plus.anchors.iter().map(|anchor| (anchor * 1023.0).round() as u64).collect::<Vec<_>>(),
            },
            "SceneId": scene,
            "SceneFrameIndex": frame - first.last().copied().unwrap_or(0),
            "SequenceFrameIndex": frame,
        }));
    }
    if scenes.is_empty() {
        return None;
    }
    let lengths: Vec<usize> = first.windows(2).map(|pair| pair[1] - pair[0]).chain(first.last().map(|start| frames - start)).collect();
    let whole = json!({
        "JSONInfo": { "HDR10plusProfile": "B", "Version": "1.0" },
        "SceneInfo": scenes,
        "SceneInfoSummary": { "SceneFirstFrameIndex": first, "SceneFrameNumbers": lengths },
        "ToolInfo": { "Tool": "jog", "Version": env!("CARGO_PKG_VERSION") },
    });
    Some(whole.to_string())
}

/// A file written for ffmpeg to read, gone when this is.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What has to be written down beside ffmpeg's arguments to carry the
/// video's HDR metadata into `target`: its HDR10+, for x265, which reads
/// it from a file. The HDR10 figures ffmpeg passes on by itself.
fn carried(path: &Path, (from, to): (f64, f64), target: &Target, knobs: &Knobs, probe: &Probe) -> Option<Scratch> {
    if target.codec != Codec::Hevc || !probe.hdr || knobs.to_sdr {
        return None;
    }
    let info = crate::hdrmeta::read(path)?;
    let to = if to > from { to } else { probe.seconds };
    let json = hdr10plus_json(&info, (from, to), probe.fps)?;
    static FILES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = std::env::temp_dir().join(format!("jog-hdr10plus-{}-{count}.json", std::process::id()));
    std::fs::write(&file, json).ok()?;
    Some(Scratch(file))
}

/// What a conversion keeps of a video's HDR, and what it cannot: each kind
/// the video has, and whether it is kept, with why not when not. For the
/// Convert tab to say before it is begun.
pub fn hdr_kept(info: &crate::hdrmeta::Info, target: &Target, knobs: &Knobs) -> Vec<(&'static str, Result<(), &'static str>)> {
    let sdr = knobs.to_sdr || target.codec == Codec::H264;
    let mut kept = Vec::new();
    if info.mastering.is_some() || info.light.is_some() {
        kept.push(("HDR10", if sdr { Err("taken to SDR") } else { Ok(()) }));
    }
    if !info.hdr10plus.is_empty() {
        kept.push((
            "HDR10+",
            match (sdr, target.codec) {
                (true, _) => Err("taken to SDR"),
                (false, Codec::Hevc) => Ok(()),
                _ => Err("HEVC only"),
            },
        ));
    }
    if !info.agtm.is_empty() {
        kept.push(("2094-50", Err("no encoder writes it yet")));
    }
    kept
}

/// The arguments for ffmpeg, from after `-i input` to before the output;
/// `turn` the filters that turn the picture as it is shown, if any.
fn arguments(target: &Target, settings: &VideoSettings, knobs: &Knobs, probe: &Probe, turn: Option<String>, hdr10plus: Option<&Path>) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(|item| item.to_string()));
    // The first video, all the sound, and in MKV the subtitles too; the
    // file's tags and chapters kept.
    push(&["-map", "0:v:0", "-map", "0:a?", "-map_metadata", "0", "-map_chapters", "0"]);
    if target.container == Container::Mkv {
        push(&["-map", "0:s?", "-c:s", "copy"]);
    }
    let crf = knobs.crf(target.codec, settings).to_string();
    // H.264 is 8-bit, and HDR in 8 bits is banded and shown gray: it is always
    // taken to SDR, however it is asked for.
    let sdr = (knobs.to_sdr || target.codec == Codec::H264) && probe.hdr;
    match target.codec {
        Codec::H264 => {
            if !has("libx264") {
                return Err("this ffmpeg has no H.264 encoder (libx264)".into());
            }
            // 8-bit 4:2:0: what every player plays.
            let preset = ["veryfast", "slow", "slower"][knobs.speed as usize];
            push(&["-c:v", "libx264", "-preset", preset, "-crf", &crf, "-pix_fmt", "yuv420p"]);
        }
        Codec::Hevc => {
            if !has("libx265") {
                return Err("this ffmpeg has no HEVC encoder (libx265)".into());
            }
            let depth = if probe.deep { "yuv420p10le" } else { "yuv420p" };
            let preset = ["fast", "medium", "slow"][knobs.speed as usize];
            push(&["-c:v", "libx265", "-preset", preset, "-crf", &crf, "-pix_fmt", depth]);
            // Apple's players want HEVC in MP4 tagged so.
            if target.container == Container::Mp4 {
                push(&["-tag:v", "hvc1"]);
            }
            // The video's HDR10+, written again a frame at a time.
            if let Some(json) = hdr10plus.filter(|_| !sdr) {
                push(&["-x265-params", &format!("dhdr10-info={}", json.display())]);
            }
        }
        Codec::Av1 => {
            // SVT-AV1 is many times quicker than libaom at the same size;
            // newer ffmpegs have it.
            if has("libsvtav1") {
                let preset = ["9", "6", "4"][knobs.speed as usize];
                push(&["-c:v", "libsvtav1", "-preset", preset, "-crf", &crf]);
            } else if has("libaom-av1") {
                let used = ["8", "6", "4"][knobs.speed as usize];
                push(&["-c:v", "libaom-av1", "-crf", &crf, "-b:v", "0", "-cpu-used", used, "-row-mt", "1", "-tiles", "2x2"]);
            } else {
                return Err("this ffmpeg has no AV1 encoder (libsvtav1 or libaom-av1)".into());
            }
            push(&["-pix_fmt", "yuv420p10le"]);
        }
    }
    // Turned, made smaller, taken down to SDR: one chain of filters.
    let mut filters: Vec<String> = turn.into_iter().collect();
    if let Some(height) = knobs.height {
        filters.push(format!("scale=-2:'min(ih,{height})':flags=lanczos"));
    }
    if sdr {
        filters.push(tone_map(probe).trim_end_matches(',').to_string());
        // Taken to SDR, it says nothing of HDR: the encoders would write the
        // HDR figures they are handed, and a player see HDR that is not.
        if has_filter("sidedata") {
            for kind in ["MASTERING_DISPLAY_METADATA", "CONTENT_LIGHT_LEVEL", "DYNAMIC_HDR_PLUS", "DOVI_METADATA", "DOVI_RPU_BUFFER"] {
                filters.push(format!("sidedata=mode=delete:type={kind}"));
            }
        }
        push(&["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709"]);
    }
    filters.retain(|filter| !filter.is_empty());
    if !filters.is_empty() {
        push(&["-vf", &filters.join(",")]);
    }
    if probe.has_audio {
        match target.audio(settings) {
            Audio::Opus => {
                if !has("libopus") {
                    return Err("this ffmpeg has no Opus encoder (libopus)".into());
                }
                push(&["-c:a", "libopus", "-b:a", "128k"]);
            }
            Audio::Aac => push(&["-c:a", "aac", "-b:a", "192k"]),
            Audio::Copy => push(&["-c:a", "copy"]),
        }
    }
    if target.container == Container::Mp4 {
        // Playable before it has all arrived.
        push(&["-movflags", "+faststart"]);
    }
    Ok(args)
}

/// Converts the video at `path`, telling `progress` how far along it is
/// (0 to 1) as it goes. Written under another name and renamed when done,
/// so an unfinished one is never left looking finished.
/// The filters that turn and mirror a video as `transform` says: as jog
/// shows it, mirrored first, then turned clockwise. None for none.
pub fn turn_filter(transform: Transform) -> Option<String> {
    let mirror = transform.mirrored().then_some("hflip");
    let turn = match transform.turns() {
        1 => Some("transpose=clock"),
        2 => Some("hflip,vflip"),
        3 => Some("transpose=cclock"),
        _ => None,
    };
    let steps: Vec<&str> = mirror.into_iter().chain(turn).collect();
    (!steps.is_empty()).then(|| steps.join(","))
}

/// The arguments, before the input, that tell players to show a video
/// copied as it is turned and mirrored as `transform` says: the picture
/// itself cannot be turned without being encoded again.
fn display_turn(transform: Transform) -> Vec<String> {
    if transform.is_identity() {
        return Vec::new();
    }
    // Counter-clockwise, as the metadata has it; the flip is applied
    // after the turn, which mirroring first and then turning comes to
    // when the turn goes the other way.
    let turns = if transform.mirrored() { transform.turns() } else { (4 - transform.turns()) % 4 };
    let mut args = vec!["-display_rotation:v:0".to_string(), (u32::from(turns) * 90).to_string()];
    if transform.mirrored() {
        args.push("-display_hflip:v:0".to_string());
    }
    args
}

pub fn transcode(path: &Path, target: &Target, settings: &VideoSettings, knobs: &Knobs, transform: Transform, progress: impl FnMut(f64)) -> Result<PathBuf, String> {
    if let Some(problem) = target.problem(settings) {
        return Err(problem);
    }
    let output = target.output(path);
    if output.exists() {
        return Err(format!("{} is there already", output.display()));
    }
    let probe = probe(path);
    let carry = carried(path, (0.0, probe.seconds), target, knobs, &probe);
    let args = arguments(target, settings, knobs, &probe, turn_filter(transform), carry.as_ref().map(|carry| carry.0.as_path()))?;
    run(path, &[], &args, target.container.muxer(), &output, probe.seconds, progress)
}

/// What a trial of a few seconds says of a whole encode.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Estimate {
    /// Bytes for each second of video.
    pub bytes_per_second: f64,
    /// Seconds of encoding for each second of video.
    pub pace: f64,
}

/// Encodes `seconds` (or fewer) of `from`..`to` of the video at `path` as
/// the whole would be, to files thrown away after, to say how large the
/// whole will be and how long it will take. A long stretch is tried in
/// three pieces, a fifth, half and four fifths of the way through, and
/// what they say put together: the middle alone may be a quiet scene, or
/// a busy one, and say too little or too much of the whole.
pub fn trial(path: &Path, (from, to): (f64, f64), target: &Target, settings: &VideoSettings, knobs: &Knobs, transform: Transform, seconds: f64) -> Result<Estimate, String> {
    let to = if to > from { to } else { probe(path).seconds };
    let length = to - from;
    let spans: Vec<(f64, f64)> = if length >= seconds * 6.0 {
        let piece = seconds / 3.0;
        [0.2, 0.5, 0.8].iter().map(|at| (from + length * at - piece / 2.0, from + length * at + piece / 2.0)).collect()
    } else {
        vec![(from, to)]
    };
    let piece = seconds / spans.len() as f64;
    let (mut bytes, mut took, mut covered) = (0.0, 0.0, 0.0);
    for span in spans {
        let (estimate, output, (_, length)) = encode_trial(path, span, target, settings, knobs, transform, piece)?;
        let _ = std::fs::remove_file(&output);
        bytes += estimate.bytes_per_second * length;
        took += estimate.pace * length;
        covered += length;
    }
    let covered = covered.max(1e-6);
    Ok(Estimate { bytes_per_second: bytes / covered, pace: took / covered })
}

/// The trial encoded and kept, where it is, and from when in the video.
fn encode_trial(path: &Path, (from, to): (f64, f64), target: &Target, settings: &VideoSettings, knobs: &Knobs, transform: Transform, seconds: f64) -> Result<(Estimate, PathBuf, (f64, f64)), String> {
    if let Some(problem) = target.problem(settings) {
        return Err(problem);
    }
    let probe = probe(path);
    let to = if to > from { to } else { probe.seconds };
    let length = seconds.min(to - from).max(0.1);
    let start = from + ((to - from) - length).max(0.0) / 2.0;
    let mut args = vec!["-t".to_string(), format!("{length:.3}")];
    let carry = carried(path, (start, start + length), target, knobs, &probe);
    args.extend(arguments(target, settings, knobs, &probe, turn_filter(transform), carry.as_ref().map(|carry| carry.0.as_path()))?);
    static TRIALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = TRIALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let output = std::env::temp_dir().join(format!("jog-trial-{}-{count}.{}", std::process::id(), target.container.extension()));
    let began = std::time::Instant::now();
    run(path, &["-ss".to_string(), format!("{start:.3}")], &args, target.container.muxer(), &output, length, |_| {})?;
    let bytes = size(&output) as f64;
    Ok((Estimate { bytes_per_second: bytes / length, pace: began.elapsed().as_secs_f64() / length }, output, (start, length)))
}

/// A frame of the video as converted, and the same frame as it is, for
/// telling what converting loses; with what the trial said of size and
/// time.
pub struct Compared {
    pub estimate: Estimate,
    pub original: image::RgbImage,
    pub converted: image::RgbImage,
    /// Where in the video the frame is, in seconds.
    pub at: f64,
}

/// Encodes a few seconds as `trial` does, keeping them, and takes the
/// frame in their middle from both it and the video: the video's turned as
/// it is shown and, HDR, taken down to SDR as the trial's is, or both so
/// when the trial keeps HDR; so that what differs is what the encoding did.
pub fn compare(path: &Path, span: (f64, f64), target: &Target, settings: &VideoSettings, knobs: &Knobs, transform: Transform, seconds: f64) -> Result<Compared, String> {
    let (estimate, output, (start, length)) = encode_trial(path, span, target, settings, knobs, transform, seconds)?;
    let probe = probe(path);
    let sdr = (knobs.to_sdr || target.codec == Codec::H264) && probe.hdr;
    let middle = length / 2.0;
    let shown = |turn: Option<String>, tone: bool| {
        let mut filters: Vec<String> = turn.into_iter().collect();
        if tone {
            filters.push(tone_map(&probe).trim_end_matches(',').to_string());
        }
        filters.retain(|filter| !filter.is_empty());
        filters.join(",")
    };
    let original = frame_at(path, start + middle, &shown(turn_filter(transform), probe.hdr));
    // The trial is turned already; HDR still if it kept it.
    let converted = frame_at(&output, middle, &shown(None, probe.hdr && !sdr));
    let _ = std::fs::remove_file(&output);
    Ok(Compared { estimate, original: original?, converted: converted?, at: start + middle })
}

/// The frame of the video at `path` at `at` seconds, through `filters`.
fn frame_at(path: &Path, at: f64, filters: &str) -> Result<image::RgbImage, String> {
    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-ss", &format!("{at:.3}"), "-i"]).arg(path).args(["-frames:v", "1"]);
    if !filters.is_empty() {
        command.args(["-vf", filters]);
    }
    let output = command.args(["-f", "image2pipe", "-c:v", "png", "-"]).stdin(Stdio::null()).output().map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("no frame").to_string());
    }
    image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png).map(|picture| picture.to_rgb8()).map_err(|error| error.to_string())
}

/// What a video is, as the Convert window says it: how long, and whether
/// HDR and with sound.
pub struct VideoFacts {
    pub seconds: f64,
    pub hdr: bool,
    pub has_audio: bool,
}

pub fn video_facts(path: &Path) -> VideoFacts {
    let probe = probe(path);
    VideoFacts { seconds: probe.seconds, hdr: probe.hdr, has_audio: probe.has_audio }
}

/// The files ffmpeg is writing now, and the process group of the ffmpeg
/// writing each. jog quits without unwinding: `abandon`, on the way out,
/// stops those ffmpegs and takes their half files away. The group, not
/// the process: `ffmpeg` on the PATH may be a shim (mise's) that runs the
/// real one as a child, which nothing told to go with jog would reach.
static UNFINISHED: std::sync::Mutex<Vec<(PathBuf, i32)>> = std::sync::Mutex::new(Vec::new());

/// A file being written, known to `abandon` for as long as this lives.
struct Unfinished(PathBuf);

impl Unfinished {
    fn new(path: &Path, group: u32) -> Unfinished {
        UNFINISHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((path.to_path_buf(), group as i32));
        Unfinished(path.to_path_buf())
    }
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        UNFINISHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|(path, _)| *path != self.0);
    }
}

/// Goes up by one each time what is being encoded is called off: a job
/// that sees it move on from what it was when the job began stops.
static CANCELED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times encoding has been called off; for a job to compare with.
pub fn generation() -> u64 {
    CANCELED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Calls off what ffmpeg is encoding, and whatever was to follow it.
/// Whether anything was.
pub fn cancel() -> bool {
    CANCELED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let running = UNFINISHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for (_, group) in running.iter() {
        // SAFETY: kill takes plain numbers and touches no memory.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    !running.is_empty() || BATCHES.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// Whether ffmpeg is encoding, or pictures are being converted.
pub fn busy() -> bool {
    !UNFINISHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_empty() || BATCHES.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// How many batches of pictures are being converted, which a cancel
/// stops between one picture and the next.
static BATCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A batch being converted, counted for as long as this lives.
pub struct Batch;

impl Batch {
    pub fn begin() -> Batch {
        BATCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Batch
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        BATCHES.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// What a job called off says, rather than how ffmpeg failed.
pub const CANCELED_WORD: &str = "canceled";

/// Stops what ffmpeg is in the middle of, and removes what it was
/// writing, as jog quits.
pub fn abandon() {
    for (path, group) in UNFINISHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain(..) {
        // SAFETY: kill takes plain numbers and touches no memory.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        let _ = std::fs::remove_file(path);
    }
}

/// Runs ffmpeg on `input` into `output`, with `before` ahead of the input
/// (where to start reading) and `args` after it, saying how far along it
/// is out of `seconds`. Written under another name until it is done, and
/// gone if it fails.
fn run(input: &Path, before: &[String], args: &[String], muxer: &str, output: &Path, seconds: f64, mut progress: impl FnMut(f64)) -> Result<PathBuf, String> {
    let extension = output.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let partial = output.with_extension(format!("{extension}.part"));
    let began = generation();
    let mut command = Command::new("ffmpeg");
    command
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error", "-progress", "pipe:1", "-y"])
        .args(before)
        .arg("-i")
        .arg(input)
        .args(args)
        .args(["-f", muxer])
        .arg(&partial)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // ffmpeg goes when jog does, rather than going on with the CPU alone:
    // told to by the system if jog dies, and stopped with all its group by
    // `abandon` when jog quits.
    // SAFETY: prctl is async-signal-safe and touches nothing shared.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| format!("cannot run ffmpeg: {error}"))?;
    let _unfinished = Unfinished::new(&partial, child.id());
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
        }
        text
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // Microseconds, in both of the names ffmpegs have used.
            let at = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms="));
            if let Some(micros) = at.and_then(|value| value.parse::<f64>().ok()).filter(|_| seconds > 0.0) {
                progress((micros / 1e6 / seconds).clamp(0.0, 1.0));
            }
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let said = errors.join().unwrap_or_default();
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        if generation() != began {
            return Err(CANCELED_WORD.to_string());
        }
        let last = said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("ffmpeg failed");
        return Err(last.to_string());
    }
    std::fs::rename(&partial, output).map_err(|error| error.to_string())?;
    Ok(output.to_path_buf())
}

/// What a section of a video is saved as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Clip {
    /// The video and sound as they are, in seconds and losing nothing,
    /// from the keyframe at or before the start: a little early, often.
    Quick,
    /// Encoded again, from the very frame marked.
    Exact(Target),
    Gif,
    WebP,
    Avif,
}

impl Clip {
    pub fn describe(&self, settings: &VideoSettings) -> String {
        match self {
            Clip::Quick => "a clip, from the keyframe before".to_string(),
            Clip::Exact(target) => target.describe(settings),
            Clip::Gif => "GIF".to_string(),
            Clip::WebP => "animated WebP".to_string(),
            Clip::Avif => "animated AVIF".to_string(),
        }
    }
}

/// A time for a file name: 1m05.2s.
fn stamp(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round() as u64;
    format!("{}m{:02}.{}s", tenths / 600, tenths / 10 % 60, tenths % 10)
}

/// The container a quick clip keeps: the video's own, when it is one
/// that takes the streams as they are; else MKV, which takes anything.
fn own_container(path: &Path) -> (&'static str, &'static str) {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp4" | "m4v") => ("mp4", "mp4"),
        Some("mov") => ("mov", "mov"),
        Some("webm") => ("webm", "webm"),
        _ => ("mkv", "matroska"),
    }
}

/// Where a section from `a` to `b` of the video at `path` is saved:
/// beside it, named for the stretch, clip.1m05.2s-1m12.0s.mp4.
pub fn clip_output(path: &Path, a: f64, b: f64, clip: &Clip) -> PathBuf {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let ending = match clip {
        Clip::Quick => own_container(path).0.to_string(),
        Clip::Exact(target) => format!("{}.{}", target.codec.word(), target.container.extension()),
        Clip::Gif => "gif".to_string(),
        Clip::WebP => "webp".to_string(),
        Clip::Avif => "avif".to_string(),
    };
    path.with_file_name(format!("{stem}.{}-{}.{ending}", stamp(a), stamp(b)))
}

/// Takes HDR down to SDR, for an 8-bit animation: without it an HLG or PQ
/// video comes out gray and washed out. Nothing for SDR, or an ffmpeg
/// without zscale.
/// The filters the ffmpeg on the PATH has, asked once.
fn filters() -> &'static str {
    static FOUND: OnceLock<String> = OnceLock::new();
    FOUND.get_or_init(|| {
        let output = Command::new("ffmpeg").args(["-hide_banner", "-filters"]).stdin(Stdio::null()).output();
        output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
    })
}

fn has_filter(name: &str) -> bool {
    filters().lines().any(|line| line.split_whitespace().nth(1) == Some(name))
}

fn tone_map(probe: &Probe) -> &'static str {
    if probe.hdr && has_filter("zscale") {
        "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p,"
    } else {
        ""
    }
}

/// Saves the section from `a` to `b` of the video at `path`, turned and
/// mirrored as `transform` says, telling `progress` how far along it is.
pub fn cut(path: &Path, a: f64, b: f64, clip: &Clip, settings: &VideoSettings, knobs: &Knobs, transform: Transform, progress: impl FnMut(f64)) -> Result<PathBuf, String> {
    let output = clip_output(path, a, b, clip);
    if output.exists() {
        return Err(format!("{} is there already", output.display()));
    }
    let probe = probe(path);
    let length = (b - a).max(0.0);
    let to_strings = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<String>>();
    // The turn, first of the filters, for those that are encoded again.
    let turn = turn_filter(transform).map(|steps| format!("{steps},")).unwrap_or_default();
    // From the start, for as long as the section: read from there, which
    // copying does from the keyframe before and encoding to the frame.
    let mut before = to_strings(&["-ss", &format!("{a:.3}")]);
    let mut args = to_strings(&["-t", &format!("{length:.3}")]);
    // HDR10+ written down for x265, for as long as it encodes.
    let _carry: Option<Scratch>;
    let muxer = match clip {
        Clip::Quick => {
            // Copied as it is, the turn goes in as how to show it.
            before.extend(display_turn(transform));
            let (extension, muxer) = own_container(path);
            args.extend(to_strings(&["-map", "0:v:0", "-map", "0:a?", "-map_metadata", "0", "-c", "copy", "-avoid_negative_ts", "make_zero"]));
            if extension == "mkv" {
                args.extend(to_strings(&["-map", "0:s?"]));
            }
            if extension == "mp4" || extension == "mov" {
                args.extend(to_strings(&["-movflags", "+faststart"]));
            }
            muxer
        }
        Clip::Exact(target) => {
            if let Some(problem) = target.problem(settings) {
                return Err(problem);
            }
            _carry = carried(path, (a, b), target, knobs, &probe);
            args.extend(arguments(target, settings, knobs, &probe, turn_filter(transform), _carry.as_ref().map(|carry| carry.0.as_path()))?);
            target.container.muxer()
        }
        Clip::Gif => {
            // A palette made for these frames, and dithered to it: GIF has
            // 256 colors.
            // Made small first, then tone mapped: four times quicker than
            // mapping every pixel of a 4K frame and throwing most away.
            let filter = format!(
                "{turn}fps=15,scale='min(iw,640)':-2:flags=lanczos,{}split[s0][s1];[s0]palettegen=stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=4",
                tone_map(&probe)
            );
            args.extend(to_strings(&["-an", "-vf", &filter, "-loop", "0"]));
            "gif"
        }
        Clip::WebP => {
            if !has("libwebp_anim") {
                return Err("this ffmpeg has no animated WebP encoder (libwebp_anim)".into());
            }
            let filter = format!("{turn}fps=20,scale='min(iw,960)':-2:flags=lanczos,{}", tone_map(&probe)).trim_end_matches(',').to_string();
            args.extend(to_strings(&["-an", "-vf", &filter, "-c:v", "libwebp_anim", "-quality", "80", "-compression_level", "4", "-loop", "0"]));
            "webp"
        }
        Clip::Avif => {
            // 10-bit AV1, HDR kept, at a size for passing around.
            args.extend(to_strings(&["-an", "-vf", &format!("{turn}fps=30,scale='min(iw,1280)':-2:flags=lanczos")]));
            if has("libsvtav1") {
                args.extend(to_strings(&["-c:v", "libsvtav1", "-preset", "8", "-crf", "35"]));
            } else if has("libaom-av1") {
                args.extend(to_strings(&["-c:v", "libaom-av1", "-crf", "35", "-b:v", "0", "-cpu-used", "8", "-row-mt", "1"]));
            } else {
                return Err("this ffmpeg has no AV1 encoder (libsvtav1 or libaom-av1)".into());
            }
            args.extend(to_strings(&["-pix_fmt", "yuv420p10le"]));
            "avif"
        }
    };
    run(path, &before, &args, muxer, &output, length, progress)
}

/// Converts each of `paths`, one after another, saying how it goes; for
/// `jog --convert av1.mkv`. False if any failed.
pub fn transcode_all(paths: &[PathBuf], target: &Target, settings: &VideoSettings) -> bool {
    let todo: Vec<&PathBuf> = paths.iter().filter(|path| !target.output(path).exists()).collect();
    let done = paths.len() - todo.len();
    let before = if done > 0 { format!(", {done} done before") } else { String::new() };
    eprintln!("jog: converting {} to {}{before}", todo.len(), target.describe(settings));
    let mut failed = 0;
    for (index, path) in todo.iter().enumerate() {
        let count = format!("[{}/{}]", index + 1, todo.len());
        let started = std::time::Instant::now();
        let shown = path.display().to_string();
        let result = transcode(path, target, settings, &Knobs::default(), Transform::default(), |share| eprint!("\r{count} {shown}  {:.0}%\x1b[K", share * 100.0));
        match result {
            Ok(output) => {
                let (from, to) = (size(path), size(&output));
                let change = (to as f64 / from.max(1) as f64 - 1.0) * 100.0;
                eprintln!("\r{count} {shown} -> {}  {change:+.0}%  in {:.0?}\x1b[K", output.display(), started.elapsed());
            }
            Err(error) => {
                eprintln!("\r{count} {shown}: {error}\x1b[K");
                failed += 1;
            }
        }
    }
    failed == 0
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

/// Writes `output` from `input` with ffmpeg as `args` say, which may name
/// further inputs and map streams from any of them: the tracks editor's
/// remux. Saying how far along it is out of `seconds`, called off as any
/// encode is, and written under another name until it is done.
pub fn remux(input: &Path, args: &[String], muxer: &str, output: &Path, seconds: f64, progress: impl FnMut(f64)) -> Result<PathBuf, String> {
    run(input, &[], args, muxer, output, seconds, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_read_as_written() {
        assert_eq!(Target::parse("av1.mkv"), Some(Target::new(Codec::Av1, Container::Mkv)));
        assert_eq!(Target::parse("HEVC.mp4"), Some(Target::new(Codec::Hevc, Container::Mp4)));
        assert_eq!(Target::parse("mp4"), Some(Target::new(Codec::H264, Container::Mp4)));
        assert_eq!(Target::parse("webm"), Some(Target::new(Codec::Av1, Container::WebM)));
        assert_eq!(Target::parse("jxl"), None);
        assert_eq!(Target::parse("vp9.webm"), None);
    }

    #[test]
    fn webm_holds_only_av1_and_opus() {
        let settings = VideoSettings::default();
        assert!(Target::new(Codec::H264, Container::WebM).problem(&settings).is_some());
        assert!(Target::new(Codec::Av1, Container::WebM).problem(&settings).is_none());
        let aac = Target { audio: Some(Audio::Aac), ..Target::new(Codec::Av1, Container::WebM) };
        assert!(aac.problem(&settings).is_some());
    }

    #[test]
    fn the_sound_suits_the_container() {
        let settings = VideoSettings::default();
        assert_eq!(Target::new(Codec::H264, Container::Mp4).audio(&settings), Audio::Aac);
        assert_eq!(Target::new(Codec::Av1, Container::Mkv).audio(&settings), Audio::Opus);
        let opus = Target { audio: Some(Audio::Opus), ..Target::new(Codec::Hevc, Container::Mp4) };
        assert_eq!(opus.audio(&settings), Audio::Opus);
    }

    #[test]
    fn the_output_is_named_for_the_codec() {
        let target = Target::new(Codec::H264, Container::Mp4);
        assert_eq!(target.output(Path::new("/v/clip.mp4")), PathBuf::from("/v/clip.h264.mp4"));
        assert_eq!(Target::new(Codec::Av1, Container::Mkv).output(Path::new("a.b.mov")), PathBuf::from("a.b.av1.mkv"));
    }

    #[test]
    fn a_section_is_named_for_its_stretch() {
        let path = Path::new("/v/clip.mp4");
        assert_eq!(clip_output(path, 65.24, 72.0, &Clip::Quick), PathBuf::from("/v/clip.1m05.2s-1m12.0s.mp4"));
        let av1 = Clip::Exact(Target::new(Codec::Av1, Container::Mkv));
        assert_eq!(clip_output(path, 0.0, 3.5, &av1), PathBuf::from("/v/clip.0m00.0s-0m03.5s.av1.mkv"));
        assert_eq!(clip_output(Path::new("a.avi"), 1.0, 2.0, &Clip::Quick), PathBuf::from("a.0m01.0s-0m02.0s.mkv"), "copied into MKV");
        assert_eq!(clip_output(path, 1.0, 2.0, &Clip::Gif), PathBuf::from("/v/clip.0m01.0s-0m02.0s.gif"));
    }

    #[test]
    fn knobs_at_rest_change_nothing() {
        let settings = VideoSettings::default();
        let probe = Probe { seconds: 10.0, deep: true, has_audio: false, hdr: true, fps: 30.0 };
        let joined = |target: Target, knobs: &Knobs| arguments(&target, &settings, knobs, &probe, None, None).map(|args| args.join(" "));
        let Ok(plain) = joined(Target::new(Codec::Hevc, Container::Mkv), &Knobs::default()) else { return };
        assert!(plain.contains("-preset medium -crf 22"), "{plain}");
        assert!(!plain.contains("-vf"), "{plain}");
        let knobs = Knobs { crf: Some(30), speed: Speed::Slow, height: Some(720), to_sdr: true };
        let turned = joined(Target::new(Codec::Hevc, Container::Mkv), &knobs).unwrap();
        assert!(turned.contains("-preset slow -crf 30"), "{turned}");
        assert!(turned.contains("scale=-2:'min(ih,720)'"), "{turned}");
        assert!(turned.contains("-color_trc bt709"), "{turned}");
        assert_eq!(turned.matches("-vf").count(), 1, "one chain of filters: {turned}");
    }

    #[test]
    fn hdr10plus_is_written_for_every_frame_with_a_scene_for_each_run() {
        use crate::hdrmeta::{Hdr10Plus, Info, Timed};
        let scene = |peak: f64| Hdr10Plus { target: 400.0, maxscl: [peak, peak, peak], average: 12.5, knee: (0.25, 0.5), anchors: vec![0.5], distribution: vec![(1, 10), (99, 2000)], bright: 0 };
        // Two scenes, at 0 s and 1 s, in a video of 10 frames a second.
        let info = Info { hdr10plus: vec![Timed { at: 0.0, value: scene(100.0) }, Timed { at: 1.0, value: scene(500.0) }], ..Info::default() };
        let json: serde_json::Value = serde_json::from_str(&hdr10plus_json(&info, (0.5, 1.5), 10.0).unwrap()).unwrap();
        let frames = json["SceneInfo"].as_array().unwrap();
        assert_eq!(frames.len(), 10, "a frame from 0.5 s to 1.5 s at 10 a second");
        assert_eq!(frames[4]["LuminanceParameters"]["MaxScl"][0], 1000, "tenths of a nit, as stored");
        assert_eq!(frames[5]["LuminanceParameters"]["MaxScl"][0], 5000, "the second scene from 1 s on");
        assert_eq!((frames[4]["SceneId"].as_u64(), frames[5]["SceneId"].as_u64(), frames[5]["SceneFrameIndex"].as_u64()), (Some(0), Some(1), Some(0)));
        assert_eq!(frames[0]["BezierCurveData"]["KneePointX"], 1024);
        assert_eq!(frames[0]["LuminanceParameters"]["LuminanceDistributions"]["DistributionValues"][1], 2000);
        assert_eq!(json["SceneInfoSummary"]["SceneFrameNumbers"], serde_json::json!([5, 5]));
        assert!(hdr10plus_json(&Info::default(), (0.0, 1.0), 30.0).is_none(), "nothing to write without HDR10+");
    }

    #[test]
    fn hdr_taken_to_sdr_says_nothing_of_hdr() {
        let settings = VideoSettings::default();
        let probe = Probe { seconds: 10.0, deep: true, has_audio: false, hdr: true, fps: 30.0 };
        let sdr = arguments(&Target::new(Codec::H264, Container::Mp4), &settings, &Knobs::default(), &probe, None, None).unwrap().join(" ");
        assert!(sdr.contains("-color_trc bt709"), "H.264 is always taken to SDR: {sdr}");
        if has_filter("sidedata") {
            assert!(sdr.contains("type=MASTERING_DISPLAY_METADATA") && sdr.contains("type=DYNAMIC_HDR_PLUS"), "{sdr}");
        }
        let hevc = arguments(&Target::new(Codec::Hevc, Container::Mkv), &settings, &Knobs::default(), &probe, None, Some(Path::new("/tmp/x.json"))).unwrap().join(" ");
        assert!(hevc.contains("dhdr10-info=/tmp/x.json") && !hevc.contains("sidedata"), "{hevc}");
    }

    #[test]
    fn settings_come_from_the_config_file() {
        let mut settings = VideoSettings::default();
        let table: toml::Table = "av1-crf = 35\nmp4-audio = \"opus\"".parse().unwrap();
        settings.apply(&table).unwrap();
        assert_eq!((settings.av1_crf, settings.mp4_audio), (35, Audio::Opus));
        let back: toml::Table = settings.to_toml().parse().unwrap();
        let mut again = VideoSettings::default();
        again.apply(back["video"].as_table().unwrap()).unwrap();
        assert_eq!(again, settings);
        assert!(settings.apply(&"crf = 3".parse().unwrap()).is_err());
    }
}
