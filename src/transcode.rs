//! Converting videos: H.264, HEVC or AV1, with Opus or AAC sound, in MP4,
//! MKV or WebM. None of it is built into jog: it is ffmpeg, whichever is
//! on the PATH, so a newer ffmpeg brings newer encoders with it. jog is
//! only the easy way in, with settings that suit most videos.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

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
}

fn probe(path: &Path) -> Probe {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration:stream=codec_type,pix_fmt", "-of", "default=nw=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output();
    let text = output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    let field = |key: &'static str| text.lines().filter_map(move |line| line.strip_prefix(key));
    Probe {
        seconds: field("duration=").find_map(|value| value.parse().ok()).unwrap_or(0.0),
        deep: field("pix_fmt=").any(|format| format.contains("10") || format.contains("12")),
        has_audio: field("codec_type=").any(|kind| kind == "audio"),
    }
}

/// The arguments for ffmpeg, from after `-i input` to before the output.
fn arguments(target: &Target, settings: &VideoSettings, probe: &Probe) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(|item| item.to_string()));
    // The first video, all the sound, and in MKV the subtitles too; the
    // file's tags and chapters kept.
    push(&["-map", "0:v:0", "-map", "0:a?", "-map_metadata", "0", "-map_chapters", "0"]);
    if target.container == Container::Mkv {
        push(&["-map", "0:s?", "-c:s", "copy"]);
    }
    match target.codec {
        Codec::H264 => {
            if !has("libx264") {
                return Err("this ffmpeg has no H.264 encoder (libx264)".into());
            }
            // 8-bit 4:2:0: what every player plays.
            let crf = settings.h264_crf.to_string();
            push(&["-c:v", "libx264", "-preset", "slow", "-crf", &crf, "-pix_fmt", "yuv420p"]);
        }
        Codec::Hevc => {
            if !has("libx265") {
                return Err("this ffmpeg has no HEVC encoder (libx265)".into());
            }
            let crf = settings.hevc_crf.to_string();
            let depth = if probe.deep { "yuv420p10le" } else { "yuv420p" };
            push(&["-c:v", "libx265", "-preset", "medium", "-crf", &crf, "-pix_fmt", depth]);
            // Apple's players want HEVC in MP4 tagged so.
            if target.container == Container::Mp4 {
                push(&["-tag:v", "hvc1"]);
            }
        }
        Codec::Av1 => {
            let crf = settings.av1_crf.to_string();
            // SVT-AV1 is many times quicker than libaom at the same size;
            // newer ffmpegs have it.
            if has("libsvtav1") {
                push(&["-c:v", "libsvtav1", "-preset", "6", "-crf", &crf]);
            } else if has("libaom-av1") {
                push(&["-c:v", "libaom-av1", "-crf", &crf, "-b:v", "0", "-cpu-used", "6", "-row-mt", "1", "-tiles", "2x2"]);
            } else {
                return Err("this ffmpeg has no AV1 encoder (libsvtav1 or libaom-av1)".into());
            }
            push(&["-pix_fmt", "yuv420p10le"]);
        }
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
pub fn transcode(path: &Path, target: &Target, settings: &VideoSettings, mut progress: impl FnMut(f64)) -> Result<PathBuf, String> {
    if let Some(problem) = target.problem(settings) {
        return Err(problem);
    }
    let output = target.output(path);
    if output.exists() {
        return Err(format!("{} is there already", output.display()));
    }
    let probe = probe(path);
    let args = arguments(target, settings, &probe)?;
    let partial = output.with_extension(format!("{}.part", target.container.extension()));
    let mut command = Command::new("ffmpeg");
    command
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error", "-progress", "pipe:1", "-y", "-i"])
        .arg(path)
        .args(&args)
        .args(["-f", target.container.muxer()])
        .arg(&partial)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // ffmpeg goes when jog does, rather than going on with the CPU alone.
    // SAFETY: prctl is async-signal-safe and touches nothing shared.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| format!("cannot run ffmpeg: {error}"))?;
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
            if let Some(micros) = at.and_then(|value| value.parse::<f64>().ok()).filter(|_| probe.seconds > 0.0) {
                progress((micros / 1e6 / probe.seconds).clamp(0.0, 1.0));
            }
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let said = errors.join().unwrap_or_default();
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        let last = said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("ffmpeg failed");
        return Err(last.to_string());
    }
    std::fs::rename(&partial, &output).map_err(|error| error.to_string())?;
    Ok(output)
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
        let result = transcode(path, target, settings, |share| eprint!("\r{count} {shown}  {:.0}%\x1b[K", share * 100.0));
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
