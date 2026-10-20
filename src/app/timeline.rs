//! The Timeline tab's deeper look at a video: its frames near where it is
//! playing, each with its type and size; where its scenes cut; and its
//! sound, as a waveform and how loud.
//!
//! Each is read by ffprobe or ffmpeg in a process of its own, on a thread
//! of its own, at low priority: the frames only for the few seconds on
//! view, as they need decoding; the cuts in one pass over the whole video,
//! told of as it goes; the sound in one pass, which is quick.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use super::inspect::Hit;
use super::*;

/// A frame as ffprobe decodes it: when it is shown, how many bytes it
/// took, its type (I, P or B), and whether it is a keyframe.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FrameInfo {
    pub at: f64,
    pub size: u32,
    pub kind: char,
    pub key: bool,
}

/// The frames of a stretch of a video.
#[derive(Clone, Debug)]
pub struct Frames {
    pub from: f64,
    pub to: f64,
    pub frames: Vec<FrameInfo>,
}

/// The sound: its loudness as EBU R128 measures it, and its waveform as
/// the loudest of each of so many equal parts, from 0 to 1.
#[derive(Clone, Debug, Default)]
pub struct Sound {
    pub integrated: Option<f64>,
    pub range: Option<f64>,
    pub peak: Option<f64>,
    pub waveform: Vec<f32>,
}

/// How many parts the waveform is cut into.
const WAVEFORM: usize = 1000;

/// What the Timeline knows of the deeper look, and what is under way.
#[derive(Default)]
pub struct Deep {
    pub frames: Option<Result<Frames, String>>,
    /// The stretch asked for last, while it is being read.
    pub frames_asked: Option<(f64, f64)>,
    /// How many seconds the frames shown span.
    pub span: f64,
    pub cuts: Option<Result<Vec<f64>, String>>,
    /// The cuts found so far, while the pass goes on, and how far along it
    /// is, in thousandths.
    pub cuts_found: Arc<Mutex<Vec<f64>>>,
    pub cuts_along: Arc<AtomicU32>,
    pub cuts_asked: bool,
    pub sound: Option<Result<Sound, String>>,
    pub sound_asked: bool,
    /// The pass over the whole video, to be stopped when it is no longer
    /// wanted.
    pub running: Arc<Mutex<Option<u32>>>,
}

/// The spans the frames can be shown over, in seconds: zoomed in to out.
pub const SPANS: [f64; 4] = [1.0, 2.0, 4.0, 10.0];

impl Drop for Deep {
    fn drop(&mut self) {
        // The pass over a video no longer inspected is stopped: it would
        // only take cores for nothing.
        if let Some(pid) = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            // SAFETY: a signal to the process this started.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
}

/// A command for ffmpeg or ffprobe, at the lowest priority, and gone when
/// the thread that started it is: each is started from, and waited on by,
/// a thread of its own that lives as long as it runs, and goes with jog.
fn low(program: &str) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    // SAFETY: setpriority and prctl are async-signal-safe and touch only
    // this child.
    unsafe {
        command.pre_exec(|| {
            libc::setpriority(libc::PRIO_PROCESS, 0, 19);
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    command
}

/// The frames from `from` to `to` seconds, decoded from the keyframe
/// before: a type is only known by decoding.
pub fn read_frames(path: &Path, from: f64, to: f64) -> Result<Frames, String> {
    let output = low("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-read_intervals", &format!("{:.3}%{:.3}", from.max(0.0), to)])
        .args(["-show_entries", "frame=pts_time,best_effort_timestamp_time,pkt_size,pict_type,key_frame", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .map_err(|error| format!("ffprobe: {error}"))?;
    let frames: Vec<FrameInfo> = String::from_utf8_lossy(&output.stdout).lines().filter_map(parse_frame).filter(|frame| frame.at >= from - 0.001 && frame.at <= to).collect();
    if frames.is_empty() {
        return Err("no frames there".into());
    }
    Ok(Frames { from, to, frames })
}

/// A frame as ffprobe writes it in CSV: key_frame, pts_time,
/// best_effort_timestamp_time, pkt_size, pict_type, in the order the
/// fields come in its output.
fn parse_frame(line: &str) -> Option<FrameInfo> {
    let fields: Vec<&str> = line.split(',').collect();
    let [key, pts, best, size, kind, ..] = fields[..] else { return None };
    let at = pts.parse::<f64>().ok().or_else(|| best.parse().ok())?;
    Some(FrameInfo { at, size: size.parse().unwrap_or(0), kind: kind.chars().next().unwrap_or('?'), key: key == "1" })
}

/// Where the scenes cut, in one pass over the whole video, made small
/// first; `found` and `along` told of as it goes, and its process kept in
/// `running` so that it can be stopped.
pub fn read_cuts(path: &Path, length: f64, found: &Mutex<Vec<f64>>, along: Arc<AtomicU32>, running: &Mutex<Option<u32>>) -> Result<Vec<f64>, String> {
    let mut child = low("ffmpeg")
        .args(["-hide_banner", "-nostats", "-loglevel", "error", "-threads", "2", "-progress", "pipe:2"])
        .arg("-i")
        .arg(path)
        .args(["-map", "0:v:0", "-an", "-sn", "-vf", "scale=160:-2,select='gt(scene,0.32)',metadata=mode=print:file=-", "-f", "null", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("ffmpeg: {error}"))?;
    *running.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.id());
    // How far along, from the progress ffmpeg writes on its error output.
    let progress = child.stderr.take().map(|stderr| {
        let along = along.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(micros) = line.strip_prefix("out_time_us=").and_then(|value| value.parse::<f64>().ok()).filter(|_| length > 0.0) {
                    along.store((micros / 1e6 / length * 1000.0).clamp(0.0, 1000.0) as u32, Ordering::Relaxed);
                }
            }
        })
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // `frame:12  pts:...  pts_time:3.5`: a frame that cut.
            if let Some(at) = line.split_whitespace().find_map(|field| field.strip_prefix("pts_time:")).and_then(|value| value.parse::<f64>().ok()) {
                found.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(at);
            }
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    if let Some(progress) = progress {
        let _ = progress.join();
    }
    running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if !status.success() {
        return Err("ffmpeg could not read it through".into());
    }
    along.store(1000, Ordering::Relaxed);
    Ok(found.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
}

/// The first sound track's loudness and waveform.
pub fn read_sound(path: &Path, length: f64) -> Result<Sound, String> {
    // Loudness, as EBU R128: ffmpeg's summary at the end of its output.
    let output = low("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-af", "ebur128=peak=true:framelog=quiet", "-f", "null", "-"])
        .output()
        .map_err(|error| format!("ffmpeg: {error}"))?;
    let said = String::from_utf8_lossy(&output.stderr);
    if said.contains("matches no streams") {
        return Err("no sound".into());
    }
    let figure = |name: &str| said.lines().rev().find_map(|line| line.trim().strip_prefix(name)?.split_whitespace().next()?.parse::<f64>().ok());
    let mut sound = Sound { integrated: figure("I:"), range: figure("LRA:"), peak: figure("Peak:"), waveform: Vec::new() };
    // The waveform: the sound made mono at 4 kHz, which keeps what speech
    // and music are (thinned to the waveform's own few hundred a second,
    // all above that would be filtered away, and it would be flat), the
    // loudest of each part kept as it streams by: a film's worth is never
    // held at once.
    const RATE: f64 = 4000.0;
    let mut child = low("ffmpeg")
        .args(["-hide_banner", "-nostats", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-ac", "1", "-ar", "4000", "-f", "s16le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("ffmpeg: {error}"))?;
    let expected = (length.max(0.1) * RATE) as usize;
    let mut peaks = vec![0u16; WAVEFORM];
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        let mut buffer = vec![0u8; 1 << 16];
        // A sample split between one read and the next: its first byte.
        let mut carried: Option<u8> = None;
        let mut sample = 0usize;
        let mut take = |low: u8, high: u8| {
            let part = (sample * WAVEFORM / expected.max(1)).min(WAVEFORM - 1);
            peaks[part] = peaks[part].max(i16::from_le_bytes([low, high]).unsigned_abs());
            sample += 1;
        };
        loop {
            let read = match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let mut bytes = &buffer[..read];
            if let Some(low) = carried.take() {
                take(low, bytes[0]);
                bytes = &bytes[1..];
            }
            let mut pairs = bytes.chunks_exact(2);
            for pair in &mut pairs {
                take(pair[0], pair[1]);
            }
            carried = pairs.remainder().first().copied();
        }
    }
    let _ = child.wait();
    sound.waveform = peaks.into_iter().map(|peak| f32::from(peak) / 32768.0).collect();
    Ok(sound)
}

impl App {
    /// Asks for what the Timeline wants and does not have: the frames
    /// around where the video is, its cuts and its sound, each on another
    /// thread. Called as the tab is drawn.
    pub(super) fn tend_timeline(&self, id: usize, deep: &mut Deep, length: Option<f64>) {
        let path = self.files[id].clone();
        if deep.span == 0.0 {
            deep.span = SPANS[2];
        }
        // The frames: a stretch around where the video is, read again once
        // it plays out of it.
        let at = self.video.as_ref().filter(|video| video.id == id).and(self.video_time()).map(|(at, _)| at);
        if let Some(at) = at {
            let shown = deep.frames_asked.or(deep.frames.as_ref().and_then(|frames| frames.as_ref().ok()).map(|frames| (frames.from, frames.to)));
            let fits = shown.is_some_and(|(from, to)| at >= from && at <= to && (to - from - deep.span).abs() < 0.01);
            if !fits && deep.frames_asked.is_none() {
                let from = (at - deep.span / 2.0).max(0.0);
                let to = from + deep.span;
                deep.frames_asked = Some((from, to));
                let (proxy, path) = (self.proxy.clone(), path.clone());
                std::thread::spawn(move || drop(proxy.send_event(Event::TimelineFrames(id, read_frames(&path, from, to)))));
            }
        }
        let Some(length) = length else { return };
        if !deep.cuts_asked {
            deep.cuts_asked = true;
            let (found, along, running, proxy, path) = (deep.cuts_found.clone(), deep.cuts_along.clone(), deep.running.clone(), self.proxy.clone(), path.clone());
            std::thread::spawn(move || {
                let cuts = read_cuts(&path, length, &found, along, &running);
                let _ = proxy.send_event(Event::TimelineCuts(id, cuts));
            });
        }
        if !deep.sound_asked {
            deep.sound_asked = true;
            let proxy = self.proxy.clone();
            std::thread::spawn(move || drop(proxy.send_event(Event::TimelineSound(id, read_sound(&path, length)))));
        }
    }

    pub(super) fn timeline_frames(&mut self, id: usize, frames: Result<Frames, String>) {
        if let Some(deep) = self.inspected_deep(id) {
            deep.frames_asked = None;
            deep.frames = Some(frames);
            self.redraw();
        }
    }

    pub(super) fn timeline_cuts(&mut self, id: usize, cuts: Result<Vec<f64>, String>) {
        if let Some(deep) = self.inspected_deep(id) {
            deep.cuts = Some(cuts);
            self.redraw();
        }
    }

    pub(super) fn timeline_sound(&mut self, id: usize, sound: Result<Sound, String>) {
        if let Some(deep) = self.inspected_deep(id) {
            deep.sound = Some(sound);
            self.redraw();
        }
    }

    /// The frames shown over a longer or shorter span.
    pub(super) fn timeline_zoom(&mut self, out: bool) {
        if let Some(deep) = self.inspector.as_mut().map(|inspector| inspector.deep_mut()) {
            let at = SPANS.iter().position(|span| (*span - deep.span).abs() < 0.01).unwrap_or(2);
            let next = if out { (at + 1).min(SPANS.len() - 1) } else { at.saturating_sub(1) };
            deep.span = SPANS[next];
            self.redraw();
        }
    }
}

/// Seconds as a span is said: 1 s, 10 s.
pub fn span_words(span: f64) -> String {
    format!("{span:.0} s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_read_as_ffprobe_writes_them() {
        assert_eq!(parse_frame("1,59.831000,59.831000,66516,I,"), Some(FrameInfo { at: 59.831, size: 66516, kind: 'I', key: true }));
        assert_eq!(parse_frame("0,N/A,59.872000,495,B"), Some(FrameInfo { at: 59.872, size: 495, kind: 'B', key: false }), "a frame with no pts has its best guess");
        assert_eq!(parse_frame("nonsense"), None);
    }

    #[test]
    fn the_spans_go_from_in_to_out() {
        assert!(SPANS.windows(2).all(|pair| pair[0] < pair[1]));
    }
}

/// The color a frame of each type is drawn in.
fn kind_color(kind: char) -> u32 {
    match kind {
        'I' => 0xE5534B,
        'P' => 0x3E7BD6,
        'B' => 0x3DD68C,
        _ => 0x6C6D78,
    }
}

impl App {
    /// The frames near where the video is: a bar each, as tall as its
    /// share of the bytes, colored by its type, a click going to it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_frames(&self, sub: &mut Canvas, text: &mut Text, px: u32, id: usize, deep: &Deep, width: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = super::devtools::flow_x();
        let mut y = super::devtools::section_head(sub, text, px, x, y, width, &format!("Frames · {} around here", span_words(deep.span)));
        // Zoom: in and out, at the head's right.
        let side = line;
        for (at, (words, out)) in [("−", false), ("+", true)].into_iter().enumerate() {
            let bx = x + width - (2 - at as i32) * (side + pad / 4);
            // Level with the head's words, `section_head` having come down past them.
            let by = y - pad / 3 - pad / 4 - line;
            let place = [bx + offset.0, by + offset.1, side, side];
            let over = (place[0]..place[0] + side).contains(&mouse.0) && (place[1]..place[1] + side).contains(&mouse.1);
            sub.round_fill(bx, by, side, side, side / 2, if over { 0x2C2D35 } else { super::devtools::WELL }, 256);
            let w = text.width(words, px);
            text.draw(sub, bx + (side - w) / 2, by + (side - line) / 2, px, TEXT_COLOR, words);
            // Out is a longer span: "+" shows more.
            drawn.push((Hit::FramesZoom(out), place));
        }
        let frames = match &deep.frames {
            Some(Ok(frames)) => frames,
            Some(Err(error)) => {
                text.draw(sub, x, y, px, DIM_TEXT_COLOR, &format!("Not readable · {error}"));
                return y + line + pad;
            }
            None => {
                let words = if self.video.as_ref().is_some_and(|video| video.id == id) { "Decoding…" } else { "Play the video to see its frames" };
                text.draw(sub, x, y, px, DIM_TEXT_COLOR, words);
                return y + line + pad;
            }
        };
        let chart_h = px as i32 * 6;
        sub.round_fill(x, y, width, chart_h, pad / 3, super::devtools::WELL, 256);
        let span = (frames.to - frames.from).max(1e-3);
        // Heights by the square root of the bytes: a keyframe is tens of
        // times a B frame, which would otherwise not show at all.
        let most = frames.frames.iter().map(|frame| f64::from(frame.size).sqrt()).fold(1.0, f64::max);
        let each = ((f64::from(width) / frames.frames.len().max(1) as f64) - 1.0).clamp(1.0, f64::from(pad)) as i32;
        for frame in &frames.frames {
            let fx = x + ((frame.at - frames.from) / span * f64::from(width - each)) as i32;
            let h = ((f64::from(frame.size).sqrt() / most) * f64::from(chart_h - pad / 3)).max(2.0) as i32;
            sub.fill(fx, y + chart_h - h, each, h, kind_color(frame.kind));
            drawn.push((Hit::Seek(frame.at), [fx + offset.0, y + offset.1, each.max(2), chart_h]));
        }
        // Where the video is.
        let at = self.video.as_ref().filter(|video| video.id == id).and(self.video_time()).map(|(at, _)| at);
        if let Some(at) = at.filter(|at| *at >= frames.from && *at <= frames.to) {
            let ax = x + ((at - frames.from) / span * f64::from(width)) as i32;
            sub.fill(ax.min(x + width - 2), y, 2, chart_h, accent());
        }
        y += chart_h + pad / 2;
        // The legend, then what the frames say.
        let small = px;
        let mut lx = x;
        for (kind, words) in [('I', "I · whole picture"), ('P', "P · from before"), ('B', "B · from both sides")] {
            sub.round_fill(lx, y + (Text::line_height(small) - pad / 2) / 2, pad / 2, pad / 2, 2, kind_color(kind), 256);
            text.draw(sub, lx + pad * 2 / 3, y, small, DIM_TEXT_COLOR, words);
            lx += pad * 2 / 3 + text.width(words, small) + pad;
        }
        y += Text::line_height(small) + pad / 2;
        let count = |kind: char| frames.frames.iter().filter(|frame| frame.kind == kind).count();
        let keys: Vec<f64> = frames.frames.iter().filter(|frame| frame.key).map(|frame| frame.at).collect();
        let name_w = text.width("Keyframes", px) + pad;
        let fact = |sub: &mut Canvas, text: &mut Text, y: &mut i32, name: &str, words: String| {
            text.draw(sub, x, *y, px, DIM_TEXT_COLOR, name);
            let words = clipped(text, &words, width - name_w, px);
            text.draw(sub, x + name_w, *y, px, TEXT_COLOR, &words);
            *y += line + pad / 4;
        };
        fact(sub, text, &mut y, "Frames", format!("{} · I {} · P {} · B {}", frames.frames.len(), count('I'), count('P'), count('B')));
        let keyframes = match keys.len() {
            0 => "none in these seconds".to_string(),
            1 => "one in these seconds".to_string(),
            n => format!("{n}, every {:.2} s", (keys[n - 1] - keys[0]) / (n - 1) as f64),
        };
        fact(sub, text, &mut y, "Keyframes", keyframes);
        // The frame on view, its type and size.
        if let Some(here) = at.and_then(|at| frames.frames.iter().rev().find(|frame| frame.at <= at + 1e-3)) {
            fact(sub, text, &mut y, "Here", format!("{} frame · {}", here.kind, meta::human_size(u64::from(here.size))));
        }
        y + pad
    }

    /// The sound: a waveform with where the video is, and how loud it is.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_sound(&self, sub: &mut Canvas, text: &mut Text, px: u32, id: usize, deep: &Deep, length: Option<f64>, width: i32, y: i32, offset: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = super::devtools::flow_x();
        let mut y = super::devtools::section_head(sub, text, px, x, y, width, "Sound");
        let sound = match &deep.sound {
            Some(Ok(sound)) => sound,
            Some(Err(error)) => {
                text.draw(sub, x, y, px, DIM_TEXT_COLOR, &format!("None · {error}"));
                return y + line + pad;
            }
            None => {
                text.draw(sub, x, y, px, DIM_TEXT_COLOR, "Measuring…");
                return y + line + pad;
            }
        };
        let chart_h = px as i32 * 4;
        sub.round_fill(x, y, width, chart_h, pad / 3, super::devtools::WELL, 256);
        if !sound.waveform.is_empty() {
            let mid = y + chart_h / 2;
            let parts = sound.waveform.len();
            // Drawn to its own loudest, as waveforms are: how loud it is in
            // all is the figures' to say.
            let loudest = sound.waveform.iter().copied().fold(1e-6, f32::max);
            for column in 0..width {
                let part = (column as usize * parts / width.max(1) as usize).min(parts - 1);
                let h = (sound.waveform[part] / loudest * (chart_h / 2 - 2) as f32) as i32;
                sub.fill(x + column, mid - h, 1, h * 2 + 1, 0x3E7BD6);
            }
            if let Some(length) = length.filter(|length| *length > 0.0) {
                let at = self.video.as_ref().filter(|video| video.id == id).and(self.video_time()).map(|(at, _)| at);
                if let Some(at) = at {
                    sub.fill(x + ((at / length) * f64::from(width)) as i32, y, 2, chart_h, accent());
                }
                // A click goes there.
                for column in (0..width).step_by(3) {
                    drawn.push((Hit::Seek(length * f64::from(column) / f64::from(width)), [x + column + offset.0, y + offset.1, 3, chart_h]));
                }
            }
        }
        y += chart_h + pad / 2;
        let name_w = text.width("Loudness", px) + pad;
        let fact = |sub: &mut Canvas, text: &mut Text, y: &mut i32, name: &str, words: String| {
            text.draw(sub, x, *y, px, DIM_TEXT_COLOR, name);
            let words = clipped(text, &words, width - name_w, px);
            text.draw(sub, x + name_w, *y, px, TEXT_COLOR, &words);
            *y += line + pad / 4;
        };
        if let Some(integrated) = sound.integrated {
            // Against the two targets people mix to.
            let as_what = match integrated {
                i if i >= -16.0 => "as loud as streaming mixes (−14)",
                i if i > -20.0 => "between streaming and broadcast",
                _ => "as quiet as broadcast (−23)",
            };
            fact(sub, text, &mut y, "Loudness", format!("{integrated:.1} LUFS · {as_what}"));
        }
        if let Some(range) = sound.range {
            fact(sub, text, &mut y, "Range", format!("{range:.1} LU · {}", if range > 15.0 { "wide, as film" } else if range > 7.0 { "moderate" } else { "narrow, as music or speech is mixed" }));
        }
        if let Some(peak) = sound.peak {
            fact(sub, text, &mut y, "Peak", format!("{peak:.1} dBFS{}", if peak > -1.0 { " · near clipping" } else { "" }));
        }
        y + pad
    }
}
