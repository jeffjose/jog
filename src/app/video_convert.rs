//! The Convert window for a video, as the one for a picture: the codecs
//! along the top, the settings below, the picture, and Convert. What each
//! codec comes to is found as soon as the window opens, and again as the
//! settings change: a few seconds of each encoded, the codec chosen first,
//! a round no longer wanted stopping before its next codec. A frame
//! converted beside the same frame as it is is found only when asked for,
//! Compare: the picture of it is for looking at, not to be made for nothing.
//!
//! The choices are the Convert tab's in Inspect, kept as one: changed in
//! either, they are changed in both.

use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use winit::keyboard::{Key, NamedKey};

use super::convert_panel::{cut, draw_region, in_rect, shown_region};
use super::convert_tab::{about, Trial, HEIGHTS};
use super::*;
use crate::transcode::{Audio, Codec, Compared, Container, Estimate, Knobs, Speed, VideoFacts};

/// What a click on the window lands on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Codec(Codec),
    Container(Container),
    /// The quality slider: where it starts and how wide.
    Quality(i32, i32),
    Speed(Speed),
    Height(Option<u32>),
    Sdr(bool),
    Audio(Audio),
    Whole(bool),
    Compare,
    Convert,
    Cancel,
    Close,
}

/// How many seconds the trials encode.
const TRIAL: f64 = 5.0;

pub struct VideoPanel {
    id: usize,
    /// How long it is, HDR or not, with sound or not: found off the main
    /// thread when the window opens.
    facts: Option<VideoFacts>,
    bytes: u64,
    /// What each codec was estimated at, by the trial it was of.
    estimates: Vec<(Trial, Option<Result<Estimate, String>>)>,
    /// The frame compared, by the trial it was of, once it has come.
    compared: Option<(Trial, Option<Result<Arc<Compared>, String>>)>,
    /// Which round of estimates is wanted: one begun for settings changed
    /// since stops before its next codec.
    round: Arc<AtomicU64>,
    drawn: Vec<(Hit, [i32; 4])>,
}

impl VideoPanel {
    pub(super) fn hits(&self) -> Vec<[i32; 4]> {
        self.drawn.iter().map(|(_, place)| *place).collect()
    }
}

impl App {
    /// Opens the Convert window for the video on view.
    pub(super) fn open_video_convert(&mut self, id: usize) {
        let path = self.files[id].clone();
        let bytes = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        self.video_panel = Some(VideoPanel { id, facts: None, bytes, estimates: Vec::new(), compared: None, round: Arc::new(AtomicU64::new(0)), drawn: Vec::new() });
        // What each codec comes to, found at once, a few seconds of each.
        self.estimate_codecs(id);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || drop(proxy.send_event(Event::VideoFacts(id, crate::transcode::video_facts(&path)))));
        self.menu = None;
        self.redraw();
    }

    pub(super) fn video_facts(&mut self, id: usize, facts: VideoFacts) {
        if let Some(panel) = self.video_panel.as_mut().filter(|panel| panel.id == id) {
            panel.facts = Some(facts);
            self.redraw();
        }
    }

    /// The trial a codec is tried with now: the settings as they are, with
    /// that codec, and its own usual quality if it is not the one chosen.
    fn trial_of(&self, id: usize, codec: Codec) -> Trial {
        let state = &self.video_convert;
        let mut target = state.target;
        let mut knobs = self.convert_knobs();
        if codec != target.codec {
            knobs.crf = None;
            knobs.to_sdr = state.knobs.to_sdr || codec == Codec::H264;
        }
        target.codec = codec;
        Trial { id, target, knobs, span: self.convert_span(id) }
    }

    /// The codecs the format takes: WebM only AV1.
    fn codecs_now(&self) -> Vec<Codec> {
        match self.video_convert.target.container {
            Container::WebM => vec![Codec::Av1],
            _ => vec![Codec::H264, Codec::Hevc, Codec::Av1],
        }
    }

    /// Each codec tried as the settings are, one after another on a thread
    /// of their own; those tried already as they are not tried again.
    fn estimate_codecs(&mut self, id: usize) {
        // The codec chosen first: its size is the one most looked for.
        let chosen = self.video_convert.target.codec;
        let mut codecs = self.codecs_now();
        codecs.sort_by_key(|codec| *codec != chosen);
        let trials: Vec<Trial> = codecs.into_iter().map(|codec| self.trial_of(id, codec)).collect();
        let Some(panel) = &mut self.video_panel else { return };
        // Those asked for before and not yet begun are not wanted now: the
        // settings they were of are gone.
        panel.estimates.retain(|(_, answer)| answer.is_some());
        let fresh: Vec<Trial> = trials.into_iter().filter(|trial| !panel.estimates.iter().any(|(had, _)| had == trial)).collect();
        let round = panel.round.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        if fresh.is_empty() {
            return self.redraw();
        }
        panel.estimates.extend(fresh.iter().map(|trial| (*trial, None)));
        let wanted = panel.round.clone();
        let path = self.files[id].clone();
        let settings = self.options.convert.video;
        let transform = self.transforms.get(&id).copied().unwrap_or_default();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            for trial in fresh {
                // Settings changed since: the next round has these.
                if wanted.load(AtomicOrdering::Relaxed) != round {
                    return;
                }
                let result = crate::transcode::trial(&path, trial.span.unwrap_or((0.0, 0.0)), &trial.target, &settings, &trial.knobs, transform, TRIAL);
                if proxy.send_event(Event::VideoTrial(trial, result)).is_err() {
                    return;
                }
            }
        });
        self.redraw();
    }

    /// A trial done: what its codec comes to, on its button.
    pub(super) fn video_panel_tried(&mut self, trial: Trial, result: &Result<Estimate, String>) {
        let Some(panel) = &mut self.video_panel else { return };
        if let Some((_, answer)) = panel.estimates.iter_mut().find(|(had, _)| *had == trial) {
            *answer = Some(result.clone());
            self.redraw();
        }
    }

    /// A frame of the video converted as the settings are, and the same
    /// frame as it is: an encode of a few seconds, on another thread.
    fn compare_frames(&mut self, id: usize) {
        let trial = self.trial_of(id, self.video_convert.target.codec);
        let Some(panel) = &mut self.video_panel else { return };
        if panel.compared.as_ref().is_some_and(|(had, _)| *had == trial) {
            return;
        }
        panel.compared = Some((trial, None));
        let path = self.files[id].clone();
        let settings = self.options.convert.video;
        let transform = self.transforms.get(&id).copied().unwrap_or_default();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = crate::transcode::compare(&path, trial.span.unwrap_or((0.0, 0.0)), &trial.target, &settings, &trial.knobs, transform, TRIAL).map(Arc::new);
            let _ = proxy.send_event(Event::VideoCompared(trial, result));
        });
        self.redraw();
    }

    pub(super) fn video_compared(&mut self, trial: Trial, result: Result<Arc<Compared>, String>) {
        let Some(panel) = &mut self.video_panel else { return };
        if panel.compared.as_ref().is_some_and(|(had, _)| *had == trial) {
            // What it says of size and time goes on the codec's button too.
            if let Ok(compared) = &result {
                panel.estimates.retain(|(had, _)| *had != trial);
                panel.estimates.push((trial, Some(Ok(compared.estimate))));
            }
            panel.compared = Some((trial, Some(result)));
            self.redraw();
        }
    }

    fn video_panel_hit(&mut self, hit: Hit, x: f64) {
        let Some(id) = self.video_panel.as_ref().map(|panel| panel.id) else { return };
        let changes = matches!(hit, Hit::Codec(_) | Hit::Container(_) | Hit::Quality(..) | Hit::Speed(_) | Hit::Height(_) | Hit::Sdr(_) | Hit::Audio(_) | Hit::Whole(_));
        let state = &mut self.video_convert;
        match hit {
            Hit::Codec(codec) => {
                // A rate factor for one codec means another thing to the next.
                if codec != state.target.codec {
                    state.knobs.crf = None;
                }
                state.target.codec = codec;
            }
            Hit::Container(container) => {
                state.target.container = container;
                // WebM holds AV1 and Opus, of these.
                if container == Container::WebM {
                    state.target.codec = Codec::Av1;
                    state.target.audio = Some(Audio::Opus);
                } else if state.target.audio == Some(Audio::Opus) && container == Container::Mp4 {
                    state.target.audio = None;
                }
            }
            Hit::Quality(left, width) => {
                let (best, worst) = state.target.codec.crf_range();
                let along = ((x - f64::from(left)) / f64::from(width.max(1))).clamp(0.0, 1.0);
                // Left is smaller, right is better: the slider reads as quality.
                state.knobs.crf = Some((f64::from(worst) - along * f64::from(worst - best)).round() as u8);
            }
            Hit::Speed(speed) => state.knobs.speed = speed,
            Hit::Height(height) => state.knobs.height = height,
            Hit::Sdr(sdr) => state.knobs.to_sdr = sdr,
            Hit::Audio(audio) => state.target.audio = Some(audio),
            Hit::Whole(whole) => state.whole = whole,
            Hit::Compare => return self.compare_frames(id),
            Hit::Convert => {
                self.video_panel = None;
                self.start_convert(id);
            }
            Hit::Cancel => {
                crate::transcode::cancel();
            }
            Hit::Close => self.video_panel = None,
        }
        // Settings changed: what each codec comes to, found again.
        if changes && self.video_panel.is_some() {
            return self.estimate_codecs(id);
        }
        self.redraw();
    }

    pub(super) fn video_panel_click(&mut self, x: f64, y: f64) {
        let Some(panel) = &self.video_panel else { return };
        if let Some((hit, _)) = panel.drawn.iter().rev().find(|(_, place)| in_rect((x, y), *place)).copied() {
            self.video_panel_hit(hit, x);
        }
    }

    pub(super) fn video_panel_key(&mut self, key: Key<&str>) {
        let Some(id) = self.video_panel.as_ref().map(|panel| panel.id) else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.video_panel = None,
            Key::Named(NamedKey::Enter) => return self.video_panel_hit(Hit::Convert, 0.0),
            // The next codec the format takes.
            Key::Named(NamedKey::Tab) => {
                let codecs = self.codecs_now();
                let at = codecs.iter().position(|codec| *codec == self.video_convert.target.codec).map_or(0, |at| (at + 1) % codecs.len());
                return self.video_panel_hit(Hit::Codec(codecs[at]), 0.0);
            }
            // Quality a step better or smaller.
            Key::Named(way @ (NamedKey::ArrowLeft | NamedKey::ArrowRight)) => {
                let (best, worst) = self.video_convert.target.codec.crf_range();
                let crf = self.convert_knobs().crf(self.video_convert.target.codec, &self.options.convert.video);
                let crf = if way == NamedKey::ArrowRight { crf.saturating_sub(1).max(best) } else { (crf + 1).min(worst) };
                self.video_convert.knobs.crf = Some(crf);
                return self.estimate_codecs(id);
            }
            Key::Character("e") => return self.estimate_codecs(id),
            Key::Character("c") => return self.compare_frames(id),
            _ => return,
        }
        self.redraw();
    }

    pub(super) fn draw_video_panel(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let mouse = self.mouse;
        let Some(panel) = &self.video_panel else { return };
        let id = panel.id;
        let state = &self.video_convert;
        let settings = self.options.convert.video;
        let (target, knobs) = (state.target, self.convert_knobs());
        let span = self.sections.get(&id).and_then(|section| section.span());
        let converting = self.convert_span(id);
        let native = self.shown.as_ref().filter(|(shown, _)| *shown == id).map(|(_, frame)| frame.native);
        let still = self.shown.as_ref().filter(|(shown, _)| *shown == id).map(|(_, frame)| frame.clone());
        let mut drawn: Vec<(Hit, [i32; 4])> = Vec::new();

        let pad = px as i32;
        let line = Text::line_height(px);
        // Read closely, not glanced at: hardly smaller than the rest.
        let small = (px * 11 / 12).max(12);
        let small_line = Text::line_height(small);
        let width = (canvas.width * 3 / 4).clamp((px as i32 * 40).min(canvas.width - 2 * pad), canvas.width - 2 * pad);
        let height = (canvas.height * 7 / 8).min(canvas.height - 2 * pad);
        let (left, top) = self.place_dialog("video convert", (canvas.width, canvas.height), (width, height), pad * 2 + line);
        draw::panel(canvas, left, top, width, height, pad);
        let inner = left + 3 * pad / 2;
        let inner_w = width - 3 * pad;
        let mut y = top + pad;

        // The heading: what the window is, the file, and a way out.
        let heading = format!("{}  Convert", commands::icons::REPLACE);
        text.draw_strong(canvas, inner, y, px, TEXT_COLOR, &heading);
        let length = panel.facts.as_ref().map(|facts| facts.seconds);
        let mut about_it = vec![self.name_of(id), meta::human_size(panel.bytes)];
        if let Some(seconds) = length {
            about_it.push(clock(seconds));
        }
        if let Some((w, h)) = native {
            about_it.push(format!("{w}×{h}"));
        }
        if panel.facts.as_ref().is_some_and(|facts| facts.hdr) {
            about_it.push("HDR".to_string());
        }
        let heading_w = text.width_strong(&heading, px);
        text.draw(canvas, inner + heading_w + pad, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, &about_it.join(" · "));
        let close = pad * 3 / 2;
        let (close_x, close_y) = (inner + inner_w - close, y + (line - close) / 2);
        let place = Button { x: close_x, y: close_y, width: close, height: close, command: Command::ConvertPanel };
        draw::round_button(canvas, text, &place, commands::icons::X, px, in_rect(mouse, [close_x, close_y, close, close]), false, false);
        drawn.push((Hit::Close, [close_x, close_y, close, close]));
        y += line + pad;

        // The codecs, as buttons: the one chosen filled in, each with what
        // the video comes to in it once estimated.
        let seconds = match (converting, length) {
            (Some((a, b)), _) => Some(b - a),
            (None, length) => length,
        };
        let tab_h = line + small_line + pad * 2 / 3;
        let mut x = inner;
        for codec in [Codec::H264, Codec::Hevc, Codec::Av1] {
            let allowed = self.codecs_now().contains(&codec);
            let trial = self.trial_of(id, codec);
            let estimate = panel.estimates.iter().find(|(had, _)| *had == trial).map(|(_, answer)| answer);
            let (said, change) = match (estimate, seconds) {
                (Some(Some(Ok(estimate))), Some(seconds)) => {
                    let bytes = estimate.bytes_per_second * seconds;
                    // Against the file, when it is the whole of it.
                    let change = (converting.is_none() && panel.bytes > 0).then(|| (bytes / panel.bytes as f64 - 1.0) * 100.0);
                    let change_said = change.map(|change| format!("  {change:+.0}%")).unwrap_or_default();
                    (format!("≈ {}{change_said}", meta::human_size(bytes as u64)), change)
                }
                (Some(Some(Err(_))), _) => ("—".to_string(), None),
                (Some(None), _) => ("trying…".to_string(), None),
                _ if !allowed => ("not in WebM".to_string(), None),
                _ => ("…".to_string(), None),
            };
            let w = text.width_strong(codec.name(), px).max(text.width(&said, small)) + 2 * pad;
            let chosen = codec == target.codec;
            let hovered = allowed && in_rect(mouse, [x, y, w, tab_h]);
            canvas.round_fill(x, y, w, tab_h, pad / 2, if chosen { accent() } else if hovered { PLACEHOLDER_COLOR } else { 0x161616 }, 256);
            let text_y = y + (tab_h - line - small_line) / 2;
            text.draw_strong(canvas, x + pad, text_y, px, if chosen { on_accent() } else if allowed { TEXT_COLOR } else { DIM_TEXT_COLOR }, codec.name());
            let color = match change {
                _ if chosen => on_accent(),
                Some(change) if change.abs() < 0.5 => DIM_TEXT_COLOR,
                Some(change) if change > 0.0 => MARK_COLOR,
                Some(_) => KEPT_COLOR,
                None => DIM_TEXT_COLOR,
            };
            text.draw(canvas, x + pad, text_y + line, small, color, &said);
            if allowed {
                drawn.push((Hit::Codec(codec), [x, y, w, tab_h]));
            }
            x += w + pad / 3;
        }
        y += tab_h + pad;

        // The settings, a row each: pills to choose from, and the quality
        // slider.
        let label_w = px as i32 * 7;
        let row_h = line + pad / 2;
        let row = |canvas: &mut Canvas, text: &mut Text, drawn: &mut Vec<(Hit, [i32; 4])>, y: &mut i32, name: &str, options: Vec<(String, bool, Option<Hit>)>| {
            text.draw(canvas, inner, *y + (row_h - line) / 2, px, DIM_TEXT_COLOR, name);
            let mut x = inner + label_w;
            for (word, picked, hit) in options {
                let w = text.width(&word, small) + pad * 3 / 2;
                let h = small_line + pad / 2;
                let top_c = *y + (row_h - h) / 2;
                let hovered = hit.is_some() && in_rect(mouse, [x, top_c, w, h]);
                canvas.round_fill(x, top_c, w, h, h / 2, if picked { accent() } else if hovered { PLACEHOLDER_COLOR } else { 0x181818 }, 256);
                if !picked {
                    canvas.round_outline(x, top_c, w, h, h / 2, 1, MENU_EDGE_COLOR);
                }
                let color = if picked { on_accent() } else if hit.is_some() { TEXT_COLOR } else { DIM_TEXT_COLOR };
                text.draw(canvas, x + pad * 3 / 4, top_c + pad / 4, small, color, &word);
                if let Some(hit) = hit {
                    drawn.push((hit, [x, top_c, w, h]));
                }
                x += w + pad / 3;
            }
            *y += row_h;
        };
        let pick = |on: bool, words: &str, hit: Hit| (words.to_string(), on, Some(hit));
        row(canvas, text, &mut drawn, &mut y, "Format", [Container::Mp4, Container::Mkv, Container::WebM].map(|c| pick(target.container == c, c.name(), Hit::Container(c))).to_vec());
        // Quality: from smaller to better, the rate factor named.
        {
            let (best, worst) = target.codec.crf_range();
            let crf = knobs.crf(target.codec, &settings);
            let usual = Knobs::default().crf(target.codec, &settings);
            text.draw(canvas, inner, y + (row_h - line) / 2, px, DIM_TEXT_COLOR, "Quality");
            let said = format!("CRF {crf}{}", if crf == usual { " · usual" } else { "" });
            let said_w = text.width("CRF 00 · usual", px);
            let (bar_x, bar_w) = (inner + label_w, (inner_w - label_w - said_w - pad * 2).max(pad * 6));
            let thick = (pad / 4).max(3);
            let bar_y = y + row_h / 2 - thick / 2;
            let along = f64::from(worst - crf.clamp(best, worst)) / f64::from((worst - best).max(1));
            let knob_x = bar_x + (f64::from(bar_w) * along) as i32;
            canvas.round_fill(bar_x, bar_y, bar_w, thick, thick / 2, 0x3A3A3A, 256);
            canvas.round_fill(bar_x, bar_y, (knob_x - bar_x).max(thick), thick, thick / 2, accent(), 256);
            let usual_x = bar_x + (f64::from(bar_w) * f64::from(worst - usual) / f64::from((worst - best).max(1))) as i32;
            canvas.fill(usual_x, bar_y - pad / 4, 1, thick + pad / 2, DIM_TEXT_COLOR);
            let dot = thick * 3;
            canvas.round_fill(knob_x - dot / 2, y + row_h / 2 - dot / 2, dot, dot, dot / 2, TEXT_COLOR, 256);
            text.draw(canvas, bar_x + bar_w + pad, y + (row_h - line) / 2, px, TEXT_COLOR, &said);
            drawn.push((Hit::Quality(bar_x, bar_w), [bar_x - dot, y, bar_w + 2 * dot, row_h]));
            y += row_h;
        }
        row(canvas, text, &mut drawn, &mut y, "Speed", Speed::ALL.map(|s| pick(knobs.speed == s, s.name(), Hit::Speed(s))).to_vec());
        let tall = native.map_or(0, |(_, h)| h);
        let mut sizes = vec![pick(knobs.height.is_none(), "Same", Hit::Height(None))];
        sizes.extend(HEIGHTS.iter().filter(|h| **h < tall).map(|h| pick(knobs.height == Some(*h), &format!("{h}p"), Hit::Height(Some(*h)))));
        row(canvas, text, &mut drawn, &mut y, "Size", sizes);
        if panel.facts.as_ref().is_some_and(|facts| facts.hdr) {
            let keep = target.codec != Codec::H264;
            let options = vec![(if keep { "Keep" } else { "Keep · not in H.264" }.to_string(), !knobs.to_sdr, keep.then_some(Hit::Sdr(false))), pick(knobs.to_sdr, "To SDR", Hit::Sdr(true))];
            row(canvas, text, &mut drawn, &mut y, "HDR", options);
        }
        if panel.facts.as_ref().is_some_and(|facts| facts.has_audio) {
            let now = target.audio(&settings);
            let audios = [Audio::Aac, Audio::Opus, Audio::Copy].map(|a| {
                let allowed = target.container != Container::WebM || a == Audio::Opus;
                let name = if a == Audio::Copy { "As is" } else { a.name() };
                (name.to_string(), now == a, allowed.then_some(Hit::Audio(a)))
            });
            row(canvas, text, &mut drawn, &mut y, "Audio", audios.to_vec());
        }
        if let Some((a, b)) = span {
            let (first, second) = self.active_letters();
            let whole = length.map_or("Whole".to_string(), |seconds| format!("Whole · {}", clock(seconds)));
            row(canvas, text, &mut drawn, &mut y, "Range", vec![pick(state.whole, &whole, Hit::Whole(true)), pick(!state.whole, &format!("{first}–{second} · {}–{}", clock(a), clock(b)), Hit::Whole(false))]);
        }
        y += pad / 2;

        // The picture: the video as it is, or once compared, a frame of it
        // converted beside the same frame as it is, the divider following
        // the pointer.
        let footer_h = line + pad * 2 / 3;
        let bottom = top + height - pad - small_line - pad / 2 - footer_h - pad - line - pad / 2;
        let preview = [inner, y, inner_w, (bottom - y).max(px as i32 * 4)];
        canvas.round_fill(preview[0], preview[1], preview[2], preview[3], pad / 2, 0x0C0C0C, 256);
        let current = self.trial_of(id, target.codec);
        let compared = panel.compared.as_ref().filter(|(had, _)| *had == current).map(|(_, answer)| answer);
        let tag = |canvas: &mut Canvas, text: &mut Text, words: &str, right: bool| {
            let w = text.width(words, small) + pad;
            let x = if right { preview[0] + preview[2] - w - pad / 2 } else { preview[0] + pad / 2 };
            let ty = preview[1] + preview[3] - small_line - pad;
            canvas.round_fill(x, ty, w, small_line + pad / 2, pad / 3, 0x000000, 170);
            text.draw(canvas, x + pad / 2, ty + pad / 4, small, TEXT_COLOR, words);
        };
        match compared {
            Some(Some(Ok(compared))) => {
                let size = (f64::from(compared.original.width()), f64::from(compared.original.height()));
                let (shown, dest) = shown_region(size, preview, 0, (0.5, 0.5));
                let split = if in_rect(mouse, preview) { (mouse.0 as i32).clamp(dest[0], dest[0] + dest[2]) } else { dest[0] + dest[2] / 2 };
                if let Some(frame) = cut(&compared.original, shown) {
                    draw_region(canvas, &frame, dest, 0);
                }
                // Both drawn whole into the same place, the original's left
                // of the divider kept and laid back over: neither shifts as
                // the divider moves. The converted one in the original's
                // terms, should it have been made smaller.
                let (kx, ky) = (f64::from(compared.converted.width()) / size.0, f64::from(compared.converted.height()) / size.1);
                let left_w = split - dest[0];
                let kept = canvas.copy(dest[0], dest[1], left_w, dest[3]);
                if let Some(frame) = cut(&compared.converted, [shown[0] * kx, shown[1] * ky, shown[2] * kx, shown[3] * ky]) {
                    draw_region(canvas, &frame, dest, 0);
                }
                canvas.paste(dest[0], dest[1], left_w, dest[3], &kept);
                canvas.fill(split - 1, dest[1], 2, dest[3], TEXT_COLOR);
                tag(canvas, text, &format!("Original · {}", clock(compared.at)), false);
                tag(canvas, text, &format!("{}  CRF {}", target.codec.name(), knobs.crf(target.codec, &settings)), true);
            }
            other => {
                if let Some(still) = &still {
                    let size = (f64::from(still.width), f64::from(still.height));
                    let (_, [x, y, w, h]) = shown_region(size, preview, 0, (0.5, 0.5));
                    canvas.image(still, x, y, w, h);
                    canvas.shade(x, y, w, h, 110);
                }
                let words = match other {
                    Some(Some(Err(error))) => format!("Not compared · {error}"),
                    Some(None) => format!("Converting {} s to compare…", TRIAL as u32),
                    _ => "c  Compare a frame converted with the same frame as it is".to_string(),
                };
                let ww = text.width(&words, px);
                text.draw(canvas, preview[0] + (preview[2] - ww) / 2, preview[1] + (preview[3] - line) / 2, px, TEXT_COLOR, &words);
                // Changed since it was compared: the frame is of other settings.
                if panel.compared.is_some() && compared.is_none() {
                    tag(canvas, text, "Settings changed · compare again", true);
                }
            }
        }
        y = preview[1] + preview[3] + pad / 2;

        // What the chosen codec comes to, once estimated or compared.
        let estimate = panel.estimates.iter().find(|(had, _)| *had == current).and_then(|(_, answer)| answer.clone());
        let said = match (estimate, seconds) {
            (Some(Ok(estimate)), Some(seconds)) => {
                let bytes = estimate.bytes_per_second * seconds;
                format!("{} → ≈ {}   ·   converts in {}", meta::human_size(panel.bytes), meta::human_size(bytes as u64), about(estimate.pace * seconds))
            }
            (Some(Err(error)), _) => format!("Not tried · {error}"),
            _ => format!("Trying {} s of each codec for the size and the time…", TRIAL as u32),
        };
        text.draw(canvas, inner, y, px, DIM_TEXT_COLOR, &said);
        y += line + pad;

        // The buttons: estimate, compare; cancel while one is converting;
        // and convert.
        let button = |canvas: &mut Canvas, text: &mut Text, x: i32, words: &str, primary: bool| -> i32 {
            let w = text.width(words, px) + 2 * pad;
            let hovered = in_rect(mouse, [x, y, w, footer_h]);
            canvas.round_fill(x, y, w, footer_h, pad / 2, if primary { accent() } else if hovered { PLACEHOLDER_COLOR } else { 0x181818 }, 256);
            if !primary {
                canvas.round_outline(x, y, w, footer_h, pad / 2, 1, MENU_EDGE_COLOR);
            }
            text.draw(canvas, x + pad, y + (footer_h - line) / 2, px, if primary { on_accent() } else { TEXT_COLOR }, words);
            w
        };
        let mut x = inner;
        for (words, hit) in [("Compare", Hit::Compare)] {
            let w = button(canvas, text, x, words, false);
            drawn.push((hit, [x, y, w, footer_h]));
            x += w + pad / 2;
        }
        // A conversion running, not the trials of this window.
        if self.activity_running() {
            let w = button(canvas, text, x, "Cancel converting", false);
            drawn.push((Hit::Cancel, [x, y, w, footer_h]));
        }
        let go = match converting {
            Some(_) => {
                let (first, second) = self.active_letters();
                format!("Convert {first}–{second}  ⏎")
            }
            None => "Convert  ⏎".to_string(),
        };
        let go = go.as_str();
        let go_w = text.width(go, px) + 2 * pad;
        let go_x = inner + inner_w - go_w;
        button(canvas, text, go_x, go, true);
        drawn.push((Hit::Convert, [go_x, y, go_w, footer_h]));
        y += footer_h + pad / 2;
        let keys = "Tab codec  ·  ←→ quality  ·  c compare  ·  Enter convert  ·  Esc close";
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, keys);

        if let Some(panel) = &mut self.video_panel {
            panel.drawn = drawn;
        }
    }
}
