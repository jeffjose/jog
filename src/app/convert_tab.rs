//! Inspect's Convert tab for a video: a small HandBrake. The video as it
//! is, the whole of it or the section marked; what to make of it, codec,
//! quality, speed, size, HDR and sound; and what that comes to, tried on
//! a few seconds before the whole is begun.
//!
//! The trial and the encode both run on other threads; the tab draws what
//! it knows and says so when it does not.

use super::devtools::{flow_x, section_head, WELL};
use super::inspect::{Hit, Inspection};
use super::*;
use crate::transcode::{Audio, Clip, Codec, Container, Estimate, Knobs, Speed, Target};

/// A choice on the tab.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Knob {
    /// The whole video, or the section marked.
    Whole(bool),
    Container(Container),
    Codec(Codec),
    /// The quality slider: where it starts and how wide.
    Quality(i32, i32),
    Speed(Speed),
    Height(Option<u32>),
    Sdr(bool),
    Audio(Audio),
    Estimate,
    Start,
}

/// What a trial was of: the same again is the same answer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Trial {
    id: usize,
    target: Target,
    knobs: Knobs,
    span: Option<(f64, f64)>,
}

/// The tab's choices, kept from video to video as HandBrake keeps them.
pub struct VideoConvert {
    target: Target,
    knobs: Knobs,
    /// The whole video even when a section is marked.
    whole: bool,
    /// The trial asked for last, and its answer once it has one.
    trial: Option<(Trial, Option<Result<Estimate, String>>)>,
}

impl Default for VideoConvert {
    fn default() -> VideoConvert {
        VideoConvert { target: Target::new(Codec::H264, Container::Mp4), knobs: Knobs::default(), whole: false, trial: None }
    }
}

/// Heights worth offering, as their common names have them.
const HEIGHTS: [u32; 4] = [2160, 1440, 1080, 720];

/// A video's facts as the tab needs them, from its inspection.
struct Source {
    seconds: f64,
    bytes: u64,
    height: u32,
    hdr: bool,
    audio: bool,
}

fn source(found: &Inspection) -> Source {
    let format = &found.probe["format"];
    let video = found.video();
    Source {
        seconds: format["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0),
        bytes: format["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
        height: video.and_then(|v| v["height"].as_u64()).unwrap_or(0) as u32,
        hdr: video.and_then(|v| v["color_transfer"].as_str()).is_some_and(|t| t == "smpte2084" || t == "arib-std-b67"),
        audio: found.streams().iter().any(|s| s["codec_type"] == "audio"),
    }
}

/// About how long, as people say it.
fn about(seconds: f64) -> String {
    match seconds {
        s if s < 90.0 => format!("about {:.0} s", s.max(1.0)),
        s if s < 5400.0 => format!("about {:.0} min", s / 60.0),
        s => format!("about {:.1} h", s / 3600.0),
    }
}

impl App {
    /// The span to convert: the section, when one is marked and wanted.
    fn convert_span(&self, id: usize) -> Option<(f64, f64)> {
        if self.video_convert.whole {
            return None;
        }
        self.sections.get(&id).and_then(|section| section.span())
    }

    /// What is converted, as the tab's choices say: H.264 has 8 bits, and
    /// HDR in 8 bits is banded and gray, so it always goes to SDR.
    fn convert_knobs(&self) -> Knobs {
        let mut knobs = self.video_convert.knobs;
        knobs.to_sdr |= self.video_convert.target.codec == Codec::H264;
        knobs
    }

    pub(super) fn convert_knob(&mut self, knob: Knob, x: f64) {
        let Some(id) = self.inspector.as_ref().map(|inspector| inspector.id()) else { return };
        let state = &mut self.video_convert;
        match knob {
            Knob::Whole(whole) => state.whole = whole,
            Knob::Container(container) => {
                state.target.container = container;
                // WebM holds AV1 and Opus, of these.
                if container == Container::WebM {
                    state.target.codec = Codec::Av1;
                    state.target.audio = Some(Audio::Opus);
                } else if state.target.audio == Some(Audio::Opus) && container == Container::Mp4 {
                    state.target.audio = None;
                }
            }
            Knob::Codec(codec) => {
                // A rate factor for one codec means another thing to the next.
                if codec != state.target.codec {
                    state.knobs.crf = None;
                }
                state.target.codec = codec;
            }
            Knob::Quality(left, width) => {
                let (best, worst) = state.target.codec.crf_range();
                let along = ((x - f64::from(left)) / f64::from(width.max(1))).clamp(0.0, 1.0);
                // Left is smaller, right is better: the slider reads as quality.
                state.knobs.crf = Some((f64::from(worst) - along * f64::from(worst - best)).round() as u8);
            }
            Knob::Speed(speed) => state.knobs.speed = speed,
            Knob::Height(height) => state.knobs.height = height,
            Knob::Sdr(sdr) => state.knobs.to_sdr = sdr,
            Knob::Audio(audio) => state.target.audio = Some(audio),
            Knob::Estimate => return self.try_convert(id),
            Knob::Start => return self.start_convert(id),
        }
        self.redraw();
    }

    /// Encodes a few seconds as the whole would be, on another thread, to
    /// say how large and how long.
    fn try_convert(&mut self, id: usize) {
        let trial = Trial { id, target: self.video_convert.target, knobs: self.convert_knobs(), span: self.convert_span(id) };
        if self.video_convert.trial.as_ref().is_some_and(|(asked, _)| *asked == trial) {
            return;
        }
        self.video_convert.trial = Some((trial, None));
        let path = self.files[id].clone();
        let settings = self.options.convert.video;
        let transform = self.transforms.get(&id).copied().unwrap_or_default();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = crate::transcode::trial(&path, trial.span.unwrap_or((0.0, 0.0)), &trial.target, &settings, &trial.knobs, transform, 5.0);
            let _ = proxy.send_event(Event::VideoTrial(trial, result));
        });
        self.redraw();
    }

    pub(super) fn convert_tried(&mut self, trial: Trial, result: Result<Estimate, String>) {
        if let Some((asked, answer)) = &mut self.video_convert.trial {
            if *asked == trial {
                *answer = Some(result);
                self.redraw();
            }
        }
    }

    fn start_convert(&mut self, id: usize) {
        let (target, knobs) = (self.video_convert.target, self.convert_knobs());
        // A section is saved from the video playing, which is this one.
        let playing = self.video.as_ref().is_some_and(|video| video.id == id);
        match self.convert_span(id) {
            Some(_) if playing => self.save_section(Clip::Exact(target), knobs),
            _ => self.convert_these_videos(vec![id], target, knobs),
        }
    }

    /// What the tab draws from, for knowing when to draw it again.
    pub(super) fn convert_tab_key(&self) -> u128 {
        let state = &self.video_convert;
        let words = format!("{:?}{:?}{}{:?}{}", state.target, state.knobs, state.whole, state.trial, crate::transcode::busy());
        words.bytes().fold(0u128, |hash, byte| hash.wrapping_mul(31).wrapping_add(u128::from(byte)))
    }

    /// The video as it is: its facts, and which of it to convert.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_convert_source(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &Inspection, id: usize, width: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        let facts = source(found);
        let mut y = section_head(sub, text, px, x, y, width, "Source");
        let name = self.files[id].file_name().unwrap_or_default().to_string_lossy().into_owned();
        let name = clipped(text, &name, width, px);
        text.draw(sub, x, y, px, TEXT_COLOR, &name);
        y += line + pad / 4;
        let video = found.video();
        let codec = video.and_then(|v| v["codec_name"].as_str()).unwrap_or("video").to_uppercase();
        let size = video.and_then(|v| Some(format!("{}×{}", v["width"].as_u64()?, v["height"].as_u64()?)));
        let color = if facts.hdr { found.hdr.as_ref().map(|hdr| hdr.badges().join(" · ")).filter(|b| !b.is_empty()).unwrap_or_else(|| "HDR".into()) } else { "SDR".into() };
        let under = [Some(codec), size, Some(color)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let under = clipped(text, &under, width, px);
        text.draw(sub, x, y, px, DIM_TEXT_COLOR, &under);
        y += line + pad / 4;
        let rate = if facts.seconds > 0.0 { format!(" · {:.1} Mb/s", facts.bytes as f64 * 8.0 / facts.seconds / 1e6) } else { String::new() };
        text.draw(sub, x, y, px, DIM_TEXT_COLOR, &format!("{} · {}{rate}", clock(facts.seconds), meta::human_size(facts.bytes)));
        y += line + pad;
        let span = self.sections.get(&id).and_then(|section| section.span());
        let name_w = text.width("Quality", px) + pad;
        let options = vec![
            (format!("Whole · {}", clock(facts.seconds)), self.video_convert.whole || span.is_none(), Some(Knob::Whole(true))),
            match span {
                Some((a, b)) => (format!("A–B · {}–{}", clock(a), clock(b)), !self.video_convert.whole, Some(Knob::Whole(false))),
                None => ("A–B · none marked".to_string(), false, None),
            },
        ];
        segments(sub, text, px, x, y, width, name_w, "Range", options, offset, mouse, drawn)
    }

    /// What to make of it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_convert_output(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &Inspection, width: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        let facts = source(found);
        let state = &self.video_convert;
        let settings = self.options.convert.video;
        let (target, knobs) = (state.target, self.convert_knobs());
        let name_w = text.width("Quality", px) + pad;
        let mut y = section_head(sub, text, px, x, y, width, "Output");
        let pick = |on: bool, words: &str, knob: Knob| (words.to_string(), on, Some(knob));
        let containers = [Container::Mp4, Container::Mkv, Container::WebM].map(|c| pick(target.container == c, c.name(), Knob::Container(c)));
        y = segments(sub, text, px, x, y, width, name_w, "Format", containers.to_vec(), offset, mouse, drawn);
        let codecs = [Codec::H264, Codec::Hevc, Codec::Av1].map(|c| {
            let allowed = target.container != Container::WebM || c == Codec::Av1;
            (c.name().to_string(), target.codec == c, allowed.then_some(Knob::Codec(c)))
        });
        y = segments(sub, text, px, x, y, width, name_w, "Video", codecs.to_vec(), offset, mouse, drawn);
        // Quality: a slider from smaller to better, the rate factor named.
        let (best, worst) = target.codec.crf_range();
        let crf = knobs.crf(target.codec, &settings);
        let usual = Knobs::default().crf(target.codec, &settings);
        let h = line + pad / 3;
        text.draw(sub, x, y + (h - line) / 2, px, DIM_TEXT_COLOR, "Quality");
        let said = format!("CRF {crf}{}", if crf == usual { " · usual" } else { "" });
        let said_w = text.width("CRF 00 · usual", px);
        let (bar_x, bar_w) = (x + name_w + pad / 3, (width - name_w - said_w - pad - pad / 3).max(pad * 3));
        let bar_h = (pad / 4).max(3);
        let bar_y = y + (h - bar_h) / 2;
        sub.round_fill(bar_x, bar_y, bar_w, bar_h, bar_h / 2, 0x2C2D35, 256);
        let along = f64::from(worst - crf.clamp(best, worst)) / f64::from((worst - best).max(1));
        let knob_x = bar_x + (f64::from(bar_w) * along) as i32;
        sub.round_fill(bar_x, bar_y, knob_x - bar_x, bar_h, bar_h / 2, accent_soft(), 256);
        let usual_x = bar_x + (f64::from(bar_w) * f64::from(worst - usual) / f64::from((worst - best).max(1))) as i32;
        sub.fill(usual_x, bar_y - pad / 4, 1, bar_h + pad / 2, DIM_TEXT_COLOR);
        let dot = pad * 2 / 3;
        sub.round_fill(knob_x - dot / 2, y + (h - dot) / 2, dot, dot, dot / 2, accent(), 256);
        text.draw(sub, bar_x + bar_w + pad * 2 / 3, y + (h - line) / 2, px, TEXT_COLOR, &said);
        drawn.push((Hit::Convert(Knob::Quality(bar_x + offset.0, bar_w)), [bar_x - dot / 2 + offset.0, y + offset.1, bar_w + dot, h]));
        y += h + pad / 2;
        let speeds = Speed::ALL.map(|s| pick(knobs.speed == s, s.name(), Knob::Speed(s)));
        y = segments(sub, text, px, x, y, width, name_w, "Speed", speeds.to_vec(), offset, mouse, drawn);
        let mut sizes = vec![pick(knobs.height.is_none(), "Same", Knob::Height(None))];
        sizes.extend(HEIGHTS.iter().filter(|h| **h < facts.height).map(|h| pick(knobs.height == Some(*h), &format!("{h}p"), Knob::Height(Some(*h)))));
        y = segments(sub, text, px, x, y, width, name_w, "Size", sizes, offset, mouse, drawn);
        if facts.hdr {
            let keep = target.codec != Codec::H264;
            let hdr = vec![(if keep { "Keep" } else { "Keep · not in H.264" }.to_string(), !knobs.to_sdr, keep.then_some(Knob::Sdr(false))), pick(knobs.to_sdr, "To SDR", Knob::Sdr(true))];
            y = segments(sub, text, px, x, y, width, name_w, "HDR", hdr, offset, mouse, drawn);
        }
        if facts.audio {
            let now = target.audio(&settings);
            let audios = [Audio::Aac, Audio::Opus, Audio::Copy].map(|a| {
                let allowed = target.container != Container::WebM || a == Audio::Opus;
                let name = if a == Audio::Copy { "As is" } else { a.name() };
                (name.to_string(), now == a, allowed.then_some(Knob::Audio(a)))
            });
            y = segments(sub, text, px, x, y, width, name_w, "Audio", audios.to_vec(), offset, mouse, drawn);
        }
        y + pad / 2
    }

    /// What it comes to: the file, how large and how long once tried, and
    /// the button to begin.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_convert_result(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &Inspection, id: usize, width: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        let facts = source(found);
        let state = &self.video_convert;
        let (target, knobs) = (state.target, self.convert_knobs());
        let span = self.convert_span(id);
        let path = &self.files[id];
        let output = match span {
            Some((a, b)) => crate::transcode::clip_output(path, a, b, &Clip::Exact(target)),
            None => target.output(path),
        };
        let mut y = section_head(sub, text, px, x, y, width, "Result");
        let name_w = text.width("Quality", px) + pad;
        let row = |sub: &mut Canvas, text: &mut Text, y: &mut i32, name: &str, words: &str, color: u32| {
            text.draw(sub, x, *y, px, DIM_TEXT_COLOR, name);
            let words = clipped(text, words, width - name_w, px);
            text.draw(sub, x + name_w, *y, px, color, &words);
            *y += line + pad / 4;
        };
        let file = output.file_name().unwrap_or_default().to_string_lossy().into_owned();
        row(sub, text, &mut y, "File", &file, TEXT_COLOR);
        if output.exists() {
            row(sub, text, &mut y, "", "Not converted · there already", 0xE0A040);
        }
        let length = span.map_or(facts.seconds, |(a, b)| b - a);
        let trial = Trial { id, target, knobs, span };
        let (size, time) = match &state.trial {
            Some((asked, Some(Ok(estimate)))) if *asked == trial => {
                let bytes = estimate.bytes_per_second * length;
                let share = if span.is_none() && facts.bytes > 0 { format!(" · {:.0}% of the source", bytes / facts.bytes as f64 * 100.0) } else { String::new() };
                (format!("≈ {}{share}", meta::human_size(bytes as u64)), about(estimate.pace * length))
            }
            Some((asked, Some(Err(error)))) if *asked == trial => (format!("Not tried · {error}"), String::new()),
            Some((asked, None)) if *asked == trial => ("Trying 5 s…".to_string(), String::new()),
            _ => ("Try 5 s to know".to_string(), String::new()),
        };
        row(sub, text, &mut y, "Size", &size, if size.starts_with('≈') { TEXT_COLOR } else { DIM_TEXT_COLOR });
        if !time.is_empty() {
            row(sub, text, &mut y, "Time", &time, TEXT_COLOR);
        }
        // Which of the video's HDR metadata the result keeps: a row of what
        // is kept, and one for each kind that is not, and why.
        if let Some(info) = found.hdr.as_ref().filter(|info| info.is_hdr()) {
            let said = crate::transcode::hdr_kept(info, &target, &knobs);
            let kept: Vec<&str> = said.iter().filter(|(_, kept)| kept.is_ok()).map(|(name, _)| *name).collect();
            if !kept.is_empty() {
                row(sub, text, &mut y, "HDR", &format!("Keeps {}", kept.join(" · ")), TEXT_COLOR);
            }
            for (name, why) in said.iter().filter_map(|(name, kept)| kept.err().map(|why| (name, why))) {
                let first = kept.is_empty() && said.iter().find(|(_, kept)| kept.is_err()).is_some_and(|(first, _)| first == name);
                row(sub, text, &mut y, if first { "HDR" } else { "" }, &format!("Not {name} · {why}"), 0xE0A040);
            }
        }
        y += pad / 2;
        // The buttons: try, and begin; or call off what is under way.
        let h = line + pad * 2 / 3;
        let busy = crate::transcode::busy();
        let buttons: Vec<(&str, bool, Hit)> = if busy {
            vec![("Cancel", false, Hit::Run(Command::CancelEncode))]
        } else {
            vec![("Try 5 s", false, Hit::Convert(Knob::Estimate)), ("Convert", true, Hit::Convert(Knob::Start))]
        };
        let mut bx = x;
        let gap = pad / 2;
        let count = buttons.len() as i32;
        for (words, main, hit) in buttons {
            // The main one takes what the others leave.
            let w = if main || count == 1 { width - (bx - x) } else { (width - gap) / 3 };
            let place = [bx + offset.0, y + offset.1, w, h];
            let over = (place[0]..place[0] + w).contains(&mouse.0) && (place[1]..place[1] + h).contains(&mouse.1);
            let fill = match (main, over) {
                (true, false) => accent(),
                (true, true) => accent_soft(),
                (false, false) => WELL,
                (false, true) => 0x2C2D35,
            };
            sub.round_fill(bx, y, w, h, pad / 3, fill, 256);
            let words_w = text.width(words, px);
            text.draw(sub, bx + (w - words_w) / 2, y + (h - line) / 2, px, if main { on_accent() } else { TEXT_COLOR }, words);
            drawn.push((hit, place));
            bx += w + gap;
        }
        y += h + pad / 2;
        if busy {
            text.draw(sub, x, y, (px * 5 / 6).max(11), DIM_TEXT_COLOR, "Converting · how far in the toast");
            y += line;
        }
        y + pad
    }
}

/// A row of choices, its name against the edge and each choice a pill,
/// the one chosen in the accent; one with no knob can't be chosen.
/// Wraps when the row is too narrow. Returns where the next row goes.
#[allow(clippy::too_many_arguments)]
fn segments(sub: &mut Canvas, text: &mut Text, px: u32, x: i32, y: i32, width: i32, name_w: i32, name: &str, options: Vec<(String, bool, Option<Knob>)>, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
    let pad = px as i32;
    let line = Text::line_height(px);
    let h = line + pad / 3;
    text.draw(sub, x, y + (h - line) / 2, px, DIM_TEXT_COLOR, name);
    let (mut sx, mut sy) = (x + name_w, y);
    for (words, on, knob) in options {
        let w = text.width(&words, px) + pad;
        if sx + w > x + width && sx > x + name_w {
            sx = x + name_w;
            sy += h + pad / 4;
        }
        let place = [sx + offset.0, sy + offset.1, w, h];
        let over = knob.is_some() && (place[0]..place[0] + w).contains(&mouse.0) && (place[1]..place[1] + h).contains(&mouse.1);
        let fill = match (on, over) {
            (true, _) => accent(),
            (false, true) => 0x2C2D35,
            (false, false) => WELL,
        };
        sub.round_fill(sx, sy, w, h, pad / 3, fill, 256);
        let color = match (on, knob.is_some()) {
            (true, _) => on_accent(),
            (false, true) => TEXT_COLOR,
            (false, false) => 0x5C5D66,
        };
        text.draw(sub, sx + pad / 2, sy + (h - line) / 2, px, color, &words);
        if let Some(knob) = knob {
            drawn.push((Hit::Convert(knob), place));
        }
        sx += w + pad / 4;
    }
    sy + h + pad / 2
}
