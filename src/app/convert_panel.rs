//! The Convert panel: the settings of each format to try on the picture on
//! view before converting it, with what they make of it, its size, a
//! score for how near the original it stays and the two side by side.
//!
//! Each change of a setting is tried on another thread, the latest only:
//! an AVIF of a large photo takes seconds, and settings changed meanwhile
//! make the one under way old news.

use std::sync::mpsc;
use std::sync::Arc;

use image::{DynamicImage, RgbImage};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};

use super::*;
use crate::convert::{Effort, Settings, Trial};

/// The picture the panel is about, decoded once: as it is, over white as
/// the trials are compared with it, and the file's own bytes for a JPEG
/// that JPEG XL can repack.
pub struct PanelSource {
    image: DynamicImage,
    flat: RgbImage,
    bytes: Vec<u8>,
    jpeg: bool,
}

/// What a click on the panel lands on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Format(Format),
    /// A row of settings, and on it a choice, or a point along a slider
    /// as a share of its length.
    Choice(usize, usize),
    Slider(usize, [i32; 2]),
    Close,
    Reset,
    Save,
    Scope(bool),
    Convert,
    Preview,
}

pub struct ConvertPanel {
    id: usize,
    format: Format,
    settings: Settings,
    /// The row the keyboard is on: 0 the formats, then each setting.
    row: usize,
    /// Whether Convert does the marked files rather than this one.
    marked: bool,
    source: Option<Arc<PanelSource>>,
    failed: Option<String>,
    /// The latest try asked for, and the latest come back, by number.
    asked: u64,
    trial: Option<(u64, Format, Result<Trial, String>)>,
    jobs: mpsc::Sender<(u64, Format, Settings)>,
    /// How the comparison is shown: 0 the whole picture, else that many
    /// screen pixels to one of the picture's; and where, as shares of
    /// the picture's width and height.
    zoom: u32,
    center: (f64, f64),
    drawn: Vec<(Hit, [i32; 4])>,
}

/// A setting of a format.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Knob {
    Mode,
    Quality,
    Effort,
    Speed,
    Depth,
    Chroma,
    Progressive,
    Method,
}

/// How a setting is shown and changed: a slider over a range, or a few
/// choices, one of them chosen.
enum Look {
    Slider { min: f32, max: f32, value: f32, step: f32 },
    Choices { labels: Vec<&'static str>, chosen: usize },
}

pub(super) const ZOOMS: [u32; 4] = [0, 1, 2, 4];

fn knobs(format: Format, settings: &Settings, jpeg: bool) -> Vec<Knob> {
    match format {
        Format::Jxl => {
            // A JPEG repacked has nothing to set: its pixels are its own.
            let repack = jpeg && settings.jxl.recompress_jpeg && !settings.jxl.lossless;
            let lossy = !settings.jxl.lossless && !repack;
            [Some(Knob::Mode), lossy.then_some(Knob::Quality), (!repack).then_some(Knob::Effort)].into_iter().flatten().collect()
        }
        Format::Avif => vec![Knob::Quality, Knob::Speed, Knob::Depth],
        Format::Jpeg => vec![Knob::Quality, Knob::Chroma, Knob::Progressive],
        Format::Png => Vec::new(),
        Format::WebP => [Some(Knob::Mode), (!settings.webp.lossless).then_some(Knob::Quality), Some(Knob::Method)].into_iter().flatten().collect(),
    }
}

/// The modes JPEG XL offers for this picture: repacking only for a JPEG.
fn jxl_modes(jpeg: bool) -> Vec<&'static str> {
    if jpeg { vec!["Repack the JPEG", "Lossy", "Lossless"] } else { vec!["Lossy", "Lossless"] }
}

fn label(knob: Knob) -> &'static str {
    match knob {
        Knob::Mode => "Mode",
        Knob::Quality => "Quality",
        Knob::Effort => "Effort",
        Knob::Speed => "Speed",
        Knob::Depth => "Bit depth",
        Knob::Chroma => "Color",
        Knob::Progressive => "Progressive",
        Knob::Method => "Effort",
    }
}

fn look(knob: Knob, format: Format, settings: &Settings, jpeg: bool) -> Look {
    let slider = |min: f32, max: f32, value: f32| Look::Slider { min, max, value, step: 1.0 };
    let choices = |labels: Vec<&'static str>, chosen: usize| Look::Choices { labels, chosen };
    match (knob, format) {
        (Knob::Mode, Format::Jxl) => {
            let jxl = &settings.jxl;
            let chosen = match (jpeg, jxl.lossless, jxl.recompress_jpeg) {
                (_, true, _) => if jpeg { 2 } else { 1 },
                (true, false, true) => 0,
                (true, false, false) => 1,
                (false, false, _) => 0,
            };
            choices(jxl_modes(jpeg), chosen)
        }
        (Knob::Mode, _) => choices(vec!["Lossy", "Lossless"], usize::from(settings.webp.lossless)),
        (Knob::Quality, Format::Jxl) => slider(0.0, 100.0, settings.jxl.quality),
        (Knob::Quality, Format::Avif) => slider(0.0, 100.0, settings.avif.quality),
        (Knob::Quality, Format::Jpeg) => slider(0.0, 100.0, settings.jpeg.quality),
        (Knob::Quality, _) => slider(0.0, 100.0, settings.webp.quality),
        (Knob::Effort, _) => choices(Effort::ALL.iter().map(|effort| effort.name()).collect(), Effort::ALL.iter().position(|e| *e == settings.jxl.effort).unwrap_or(2)),
        (Knob::Speed, _) => slider(1.0, 10.0, f32::from(settings.avif.speed)),
        (Knob::Depth, _) => choices(vec!["8-bit", "10-bit"], usize::from(settings.avif.depth >= 10)),
        (Knob::Chroma, _) => choices(vec!["4:4:4", "4:2:0"], usize::from(!settings.jpeg.full_chroma)),
        (Knob::Progressive, _) => choices(vec!["Off", "On"], usize::from(settings.jpeg.progressive)),
        (Knob::Method, _) => slider(0.0, 6.0, f32::from(settings.webp.method)),
    }
}

/// Sets a slider's value, or picks a choice by its number.
fn set(knob: Knob, format: Format, settings: &mut Settings, jpeg: bool, value: f32) {
    let choice = value.round().max(0.0) as usize;
    match (knob, format) {
        (Knob::Mode, Format::Jxl) => {
            let modes = jxl_modes(jpeg);
            let mode = modes[choice.min(modes.len() - 1)];
            settings.jxl.lossless = mode == "Lossless";
            if jpeg {
                settings.jxl.recompress_jpeg = mode == "Repack the JPEG";
            }
        }
        (Knob::Mode, _) => settings.webp.lossless = choice >= 1,
        (Knob::Quality, f) => settings.set_quality(f, value.clamp(0.0, 100.0).round()),
        (Knob::Effort, _) => settings.jxl.effort = Effort::ALL[choice.min(Effort::ALL.len() - 1)],
        (Knob::Speed, _) => settings.avif.speed = value.round().clamp(1.0, 10.0) as u8,
        (Knob::Depth, _) => settings.avif.depth = if choice >= 1 { 10 } else { 8 },
        (Knob::Chroma, _) => settings.jpeg.full_chroma = choice == 0,
        (Knob::Progressive, _) => settings.jpeg.progressive = choice >= 1,
        (Knob::Method, _) => settings.webp.method = value.round().clamp(0.0, 6.0) as u8,
    }
}

/// A word on a setting: whether it is the one recommended, or what going
/// further that way costs.
fn note(knob: Knob, format: Format, settings: &Settings) -> &'static str {
    let usual = Settings::default();
    match (knob, format) {
        (Knob::Quality, f) => {
            let value = |s: &Settings| match f {
                Format::Jxl => s.jxl.quality,
                Format::Avif => s.avif.quality,
                Format::Jpeg => s.jpeg.quality,
                _ => s.webp.quality,
            };
            if value(settings) == value(&usual) { "recommended" } else { "" }
        }
        (Knob::Effort, _) => match settings.jxl.effort {
            Effort::Medium => "recommended",
            Effort::Slow | Effort::ExtraSlow => "slower, a little smaller",
            _ => "quicker, larger",
        },
        (Knob::Speed, _) if settings.avif.speed == usual.avif.speed => "recommended",
        (Knob::Speed, _) if settings.avif.speed < 4 => "much slower",
        (Knob::Speed, _) if settings.avif.speed > 7 => "larger",
        (Knob::Chroma, _) if !settings.jpeg.full_chroma => "smaller, color edges soften",
        (Knob::Method, _) if settings.webp.method == usual.webp.method => "recommended",
        _ => "",
    }
}

/// The words for a SSIMULACRA2 score.
pub(super) fn verdict(score: f64) -> &'static str {
    match score {
        s if s >= 90.0 => "visually lossless",
        s if s >= 80.0 => "very good",
        s if s >= 70.0 => "good",
        s if s >= 50.0 => "visible loss",
        _ => "poor",
    }
}

/// A part of a picture as a frame to draw: `rect` in the picture's pixels.
pub(super) fn cut(image: &RgbImage, rect: [f64; 4]) -> Option<Frame> {
    let (iw, ih) = (image.width() as f64, image.height() as f64);
    let x0 = rect[0].floor().clamp(0.0, iw - 1.0) as u32;
    let y0 = rect[1].floor().clamp(0.0, ih - 1.0) as u32;
    let x1 = (rect[0] + rect[2]).ceil().clamp(f64::from(x0) + 1.0, iw) as u32;
    let y1 = (rect[1] + rect[3]).ceil().clamp(f64::from(y0) + 1.0, ih) as u32;
    let (w, h) = (x1 - x0, y1 - y0);
    let mut pixels = Vec::with_capacity(w as usize * h as usize);
    for y in y0..y1 {
        let row = &image.as_raw()[(y as usize * image.width() as usize + x0 as usize) * 3..][..w as usize * 3];
        pixels.extend(row.chunks_exact(3).map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])));
    }
    Some(Frame::still(w, h, (w, h), pixels))
}

/// Where the format last chosen in the panel is kept.
fn last_format_path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("convert-format"))
}

impl App {
    /// Opens the Convert panel on the picture on view.
    pub(super) fn open_convert_panel(&mut self) {
        let id = self.list[self.focus()];
        let path = self.files[id].clone();
        if files::is_video(&path) {
            return self.toast("Videos: File > Convert");
        }
        let (jobs, inbox) = mpsc::channel::<(u64, Format, Settings)>();
        let proxy = self.proxy.clone();
        // Turned and mirrored as it is shown, as Convert will write it.
        let transform = self.transforms.get(&id).copied().unwrap_or_default();
        // One thread for the panel, working through what it is asked, the
        // latest first and the rest forgotten, until the panel closes.
        std::thread::spawn(move || {
            let loaded = (|| -> Result<PanelSource, String> {
                let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                let image = convert::turned(crate::decode::open_guarded(&path).map_err(|error| error.to_string())?, transform);
                let flat = convert::flat(&image);
                // A JPEG turned is encoded afresh, not repacked, so it is
                // not offered as one.
                let jpeg = transform.is_identity() && bytes.starts_with(b"\xFF\xD8\xFF");
                let bytes = if transform.is_identity() { bytes } else { Vec::new() };
                Ok(PanelSource { image, flat, bytes, jpeg })
            })();
            let source = match loaded {
                Ok(source) => Arc::new(source),
                Err(error) => {
                    let _ = proxy.send_event(Event::ConvertSource(id, Err(error)));
                    return;
                }
            };
            let _ = proxy.send_event(Event::ConvertSource(id, Ok(source.clone())));
            let mut waiting = None;
            loop {
                let Some(mut job) = waiting.take().or_else(|| inbox.recv().ok()) else { return };
                while let Ok(later) = inbox.try_recv() {
                    job = later;
                }
                let (asked, format, settings) = job;
                let result = convert::trial_unscored(&source.image, Some(&source.bytes).filter(|bytes| !bytes.is_empty()).map(Vec::as_slice), format, &settings).map_err(|error| error.to_string());
                let scoring = result.as_ref().ok().map(|trial| (trial.decoded.is_some(), !trial.how.contains("repacked")));
                let decoded = result.as_ref().ok().and_then(|trial| trial.decoded.clone());
                let how = result.as_ref().map(|trial| trial.how.clone()).unwrap_or_default();
                if proxy.send_event(Event::ConvertTrial(asked, format, result)).is_err() {
                    return;
                }
                // The score, unless the settings have changed meanwhile:
                // then the new ones are tried first.
                if scoring == Some((true, true)) {
                    if let Ok(later) = inbox.try_recv() {
                        waiting = Some(later);
                        continue;
                    }
                    let trial = Trial { size: 0, how, took: Default::default(), decoded, score: None };
                    let score = convert::trial_score(&source.flat, &trial);
                    if proxy.send_event(Event::ConvertScore(asked, score)).is_err() {
                        return;
                    }
                }
            }
        });
        // The format chosen last time; else JPEG XL, the one most worth
        // it. Never the picture's own: a JPEG XL is offered as AVIF.
        let own = Format::of(&self.files[id]);
        let last = last_format_path().and_then(|path| std::fs::read_to_string(path).ok()).and_then(|ext| Format::of(Path::new(&format!("x.{}", ext.trim()))));
        let format = match last.filter(|last| Some(*last) != own) {
            Some(last) => last,
            None if own == Some(Format::Jxl) => Format::Avif,
            None => Format::Jxl,
        };
        self.convert_panel = Some(ConvertPanel {
            id,
            format,
            settings: self.options.convert,
            row: 0,
            marked: self.marks.len() > 1 && self.marks.contains(&id),
            source: None,
            failed: None,
            asked: 0,
            trial: None,
            jobs,
            zoom: 1,
            center: (0.5, 0.5),
            drawn: Vec::new(),
        });
        self.menu = None;
        self.redraw();
    }

    /// The picture is decoded: the first try is made.
    pub(super) fn convert_source(&mut self, id: usize, source: Result<Arc<PanelSource>, String>) {
        let Some(panel) = self.convert_panel.as_mut().filter(|panel| panel.id == id) else { return };
        match source {
            Ok(source) => {
                panel.source = Some(source);
                self.try_settings();
            }
            Err(error) => panel.failed = Some(error),
        }
        self.redraw();
    }

    pub(super) fn convert_trial(&mut self, asked: u64, format: Format, trial: Result<Trial, String>) {
        let Some(panel) = &mut self.convert_panel else { return };
        // An older try is still better than nothing to show, until the
        // latest comes.
        if panel.trial.as_ref().is_none_or(|(had, ..)| *had <= asked) {
            panel.trial = Some((asked, format, trial));
        }
        self.redraw();
    }

    /// The score of a try, which comes after it.
    pub(super) fn convert_score(&mut self, asked: u64, score: Option<f64>) {
        let Some(panel) = &mut self.convert_panel else { return };
        if let Some((had, _, Ok(trial))) = &mut panel.trial {
            if *had == asked {
                trial.score = score;
                self.redraw();
            }
        }
    }

    /// Asks for the settings as they are now to be tried.
    fn try_settings(&mut self) {
        let Some(panel) = &mut self.convert_panel else { return };
        if panel.source.is_none() {
            return;
        }
        panel.asked += 1;
        let _ = panel.jobs.send((panel.asked, panel.format, panel.settings));
    }

    fn panel_knobs(&self) -> Vec<Knob> {
        let Some(panel) = &self.convert_panel else { return Vec::new() };
        knobs(panel.format, &panel.settings, panel.source.as_ref().is_some_and(|source| source.jpeg))
    }

    fn panel_jpeg(&self) -> bool {
        self.convert_panel.as_ref().and_then(|panel| panel.source.as_ref()).is_some_and(|source| source.jpeg)
    }

    fn panel_format(&mut self, format: Format) {
        let Some(panel) = &mut self.convert_panel else { return };
        if panel.format != format {
            panel.format = format;
            panel.row = panel.row.min(knobs(format, &panel.settings, false).len());
            self.try_settings();
            // The panel opens on it next time.
            if let Some(path) = last_format_path() {
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
                let _ = std::fs::write(path, format.extension());
            }
        }
        self.redraw();
    }

    /// Changes the setting on a row: a slider by `steps`, a choice to the
    /// next or the one before.
    fn panel_step(&mut self, row: usize, steps: f32) {
        let knobs = self.panel_knobs();
        let jpeg = self.panel_jpeg();
        let Some(panel) = &mut self.convert_panel else { return };
        let Some(&knob) = knobs.get(row.wrapping_sub(1)) else { return };
        let value = match look(knob, panel.format, &panel.settings, jpeg) {
            Look::Slider { min, max, value, step } => (value + steps * step).clamp(min, max),
            Look::Choices { labels, chosen } => (chosen as f32 + steps.signum()).clamp(0.0, labels.len() as f32 - 1.0),
        };
        set(knob, panel.format, &mut panel.settings, jpeg, value);
        let rows = knobs_now(panel, jpeg);
        panel.row = panel.row.min(rows);
        self.try_settings();
        self.redraw();
    }

    fn panel_set(&mut self, row: usize, value: f32) {
        let knobs = self.panel_knobs();
        let jpeg = self.panel_jpeg();
        let Some(panel) = &mut self.convert_panel else { return };
        let Some(&knob) = knobs.get(row.wrapping_sub(1)) else { return };
        set(knob, panel.format, &mut panel.settings, jpeg, value);
        panel.row = row.min(knobs_now(panel, jpeg));
        self.try_settings();
        self.redraw();
    }

    fn panel_reset(&mut self) {
        let Some(panel) = &mut self.convert_panel else { return };
        let usual = Settings::default();
        match panel.format {
            Format::Jxl => panel.settings.jxl = usual.jxl,
            Format::Avif => panel.settings.avif = usual.avif,
            Format::Jpeg => panel.settings.jpeg = usual.jpeg,
            Format::WebP => panel.settings.webp = usual.webp,
            Format::Png => {}
        }
        let name = panel.format.name();
        self.try_settings();
        self.toast(format!("{name} reset"));
    }

    fn panel_save(&mut self) {
        let Some(panel) = &self.convert_panel else { return };
        let settings = panel.settings;
        match settings.save() {
            Ok(path) => {
                self.options.convert = settings;
                self.toast(format!("Saved as default ({})", short_dir(&path)));
            }
            Err(error) => self.toast(format!("Not saved · {error}")),
        }
    }

    /// Converts with the panel's settings, and closes it.
    fn panel_convert(&mut self) {
        let Some(panel) = self.convert_panel.take() else { return };
        let ids = if panel.marked { self.targets() } else { vec![panel.id] };
        self.convert_ids(ids, panel.format, panel.settings);
    }

    pub(super) fn panel_key(&mut self, key: Key<&str>, _event_loop: &ActiveEventLoop) {
        let rows = self.panel_knobs().len();
        let shift = self.modifiers.shift_key();
        let many = self.marks.len() > 1;
        let Some(panel) = &mut self.convert_panel else { return };
        let row = panel.row;
        let order = Format::ALL;
        let at = order.iter().position(|f| *f == panel.format).unwrap_or(0);
        let next = |forward: bool| order[(at + if forward { 1 } else { order.len() - 1 }) % order.len()];
        match key {
            Key::Named(NamedKey::Escape) => self.convert_panel = None,
            Key::Named(NamedKey::Enter) => return self.panel_convert(),
            Key::Named(NamedKey::Tab) => return self.panel_format(next(!shift)),
            Key::Named(NamedKey::ArrowUp) => panel.row = row.saturating_sub(1),
            Key::Named(NamedKey::ArrowDown) => panel.row = (row + 1).min(rows),
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) => {
                let forward = matches!(key, Key::Named(NamedKey::ArrowRight));
                if row == 0 {
                    return self.panel_format(next(forward));
                }
                let by = if shift { 10.0 } else { 1.0 };
                return self.panel_step(row, if forward { by } else { -by });
            }
            Key::Character("r") => return self.panel_reset(),
            Key::Character("f") => return self.ask_fit_size(),
            Key::Character("s") => return self.panel_save(),
            Key::Character("z") => {
                let at = ZOOMS.iter().position(|z| *z == panel.zoom).unwrap_or(0);
                panel.zoom = ZOOMS[(at + 1) % ZOOMS.len()];
            }
            Key::Character("m") if many => panel.marked = !panel.marked,
            _ => return,
        }
        self.redraw();
    }

    pub(super) fn panel_click(&mut self, x: f64, y: f64) {
        let Some(panel) = &self.convert_panel else { return };
        let (xi, yi) = (x as i32, y as i32);
        let on = |[left, top, w, h]: [i32; 4]| (left..left + w).contains(&xi) && (top..top + h).contains(&yi);
        let Some((hit, rect)) = panel.drawn.iter().rev().find(|(_, rect)| on(*rect)).copied() else { return };
        match hit {
            Hit::Format(format) => self.panel_format(format),
            Hit::Choice(row, choice) => self.panel_set(row, choice as f32),
            Hit::Slider(row, [from, to]) => {
                let knobs = self.panel_knobs();
                let jpeg = self.panel_jpeg();
                let Some(panel) = &self.convert_panel else { return };
                if let Some(Look::Slider { min, max, .. }) = knobs.get(row.wrapping_sub(1)).map(|knob| look(*knob, panel.format, &panel.settings, jpeg)) {
                    let share = ((x - f64::from(from)) / f64::from((to - from).max(1))).clamp(0.0, 1.0) as f32;
                    self.panel_set(row, min + share * (max - min));
                }
            }
            Hit::Close => {
                self.convert_panel = None;
                self.redraw();
            }
            Hit::Reset => self.panel_reset(),
            Hit::Save => self.panel_save(),
            Hit::Scope(marked) => {
                if let Some(panel) = &mut self.convert_panel {
                    panel.marked = marked;
                }
                self.redraw();
            }
            Hit::Convert => self.panel_convert(),
            Hit::Preview => {
                // Looked at more closely, there.
                let Some(panel) = &mut self.convert_panel else { return };
                let Some(source) = &panel.source else { return };
                let size = (source.flat.width() as f64, source.flat.height() as f64);
                let [px_, py_, pw, ph] = rect;
                let (shown, _) = shown_region(size, [px_, py_, pw, ph], panel.zoom, panel.center);
                let picture = (shown[0] + (x - f64::from(px_)) / f64::from(pw) * shown[2], shown[1] + (y - f64::from(py_)) / f64::from(ph) * shown[3]);
                panel.center = (picture.0 / size.0, picture.1 / size.1);
                if panel.zoom == 0 {
                    panel.zoom = 1;
                }
                self.redraw();
            }
        }
    }

    pub(super) fn panel_wheel(&mut self, lines: f64) {
        let Some(panel) = &mut self.convert_panel else { return };
        let at = ZOOMS.iter().position(|z| *z == panel.zoom).unwrap_or(0);
        let to = if lines > 0.0 { (at + 1).min(ZOOMS.len() - 1) } else { at.saturating_sub(1) };
        if to != at {
            panel.zoom = ZOOMS[to];
            self.redraw();
        }
    }

    pub(super) fn draw_convert_panel(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let knobs = self.panel_knobs();
        let jpeg = self.panel_jpeg();
        let marks = self.marks.len();
        let mouse = self.mouse;
        let Some(panel) = &self.convert_panel else { return };
        let name = self.name_of(panel.id);
        let file_size = std::fs::metadata(&self.files[panel.id]).map_or(0, |meta| meta.len());
        let mut drawn: Vec<(Hit, [i32; 4])> = Vec::new();

        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let width = (canvas.width * 3 / 4).clamp((px as i32 * 40).min(canvas.width - 2 * pad), canvas.width - 2 * pad);
        let height = (canvas.height * 7 / 8).min(canvas.height - 2 * pad);
        let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);
        panel_box(canvas, left, top, width, height, pad);
        let inner = left + 3 * pad / 2;
        let inner_w = width - 3 * pad;
        let mut y = top + pad;

        // The heading: what the panel is, the file, and a way out.
        text.draw(canvas, inner, y, px, TEXT_COLOR, &format!("{}  Convert", commands::icons::REPLACE));
        let about = format!("{name} · {}", meta::human_size(file_size));
        let heading_w = text.width(&format!("{}  Convert", commands::icons::REPLACE), px);
        text.draw(canvas, inner + heading_w + pad, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, &about);
        let close = line;
        let close_x = inner + inner_w - close;
        let over_close = in_rect(mouse, [close_x, y, close, close]);
        if over_close {
            canvas.round_fill(close_x, y, close, close, close / 2, PLACEHOLDER_COLOR, 256);
        }
        let x_w = text.width(commands::icons::X, px);
        text.draw(canvas, close_x + (close - x_w) / 2, y, px, TEXT_COLOR, commands::icons::X);
        drawn.push((Hit::Close, [close_x, y, close, close]));
        y += line + pad;

        // The formats, as a row of buttons: the one chosen filled in.
        let tab_h = line + pad * 2 / 3;
        let mut x = inner;
        for format in Format::ALL {
            let label = format.name();
            let w = text.width(label, px) + 2 * pad;
            let chosen = format == panel.format;
            let hovered = in_rect(mouse, [x, y, w, tab_h]);
            canvas.round_fill(x, y, w, tab_h, pad / 2, if chosen { accent() } else if hovered { PLACEHOLDER_COLOR } else { 0x161616 }, 256);
            if panel.row == 0 && chosen {
                canvas.round_outline(x - 2, y - 2, w + 4, tab_h + 4, pad / 2 + 2, 2, accent_soft());
            }
            text.draw(canvas, x + pad, y + (tab_h - line) / 2, px, if chosen { on_accent() } else { TEXT_COLOR }, label);
            drawn.push((Hit::Format(format), [x, y, w, tab_h]));
            x += w + pad / 3;
        }
        y += tab_h + pad;

        // The settings of the format, a row each.
        let label_w = px as i32 * 8;
        let row_h = line + pad * 2 / 3;
        if knobs.is_empty() {
            text.draw(canvas, inner, y + (row_h - small_line) / 2, small, DIM_TEXT_COLOR, "PNG is lossless  ·  nothing to set");
            y += row_h;
        }
        for (index, knob) in knobs.iter().enumerate() {
            let row = index + 1;
            let focused = panel.row == row;
            if focused {
                canvas.round_fill(inner - pad / 2, y, inner_w + pad, row_h, pad / 2, 0x262626, 256);
            }
            let middle = y + (row_h - line) / 2;
            text.draw(canvas, inner, middle, px, if focused { TEXT_COLOR } else { DIM_TEXT_COLOR }, label(*knob));
            let mut x = inner + label_w;
            let hint = note(*knob, panel.format, &panel.settings);
            match look(*knob, panel.format, &panel.settings, jpeg) {
                Look::Slider { min, max, value, .. } => {
                    let track_w = (inner_w - label_w - px as i32 * 14).max(px as i32 * 8);
                    let thick = (px as i32 / 4).max(3);
                    let track_y = y + row_h / 2 - thick / 2;
                    let share = ((value - min) / (max - min)).clamp(0.0, 1.0);
                    let filled = (share * track_w as f32) as i32;
                    canvas.round_fill(x, track_y, track_w, thick, thick / 2, 0x3A3A3A, 256);
                    canvas.round_fill(x, track_y, filled.max(thick), thick, thick / 2, accent(), 256);
                    let knob_d = thick * 3;
                    canvas.round_fill(x + filled - knob_d / 2, y + row_h / 2 - knob_d / 2, knob_d, knob_d, knob_d / 2, TEXT_COLOR, 256);
                    drawn.push((Hit::Slider(row, [x, x + track_w]), [x - knob_d, y, track_w + 2 * knob_d, row_h]));
                    x += track_w + pad * 3 / 2;
                    let shown = format!("{value:.0}");
                    text.draw(canvas, x, middle, px, TEXT_COLOR, &shown);
                    x += text.width("100", px) + pad;
                }
                Look::Choices { labels, chosen } => {
                    for (which, word) in labels.iter().enumerate() {
                        let w = text.width(word, small) + pad * 3 / 2;
                        let h = small_line + pad / 2;
                        let top_c = y + (row_h - h) / 2;
                        let picked = which == chosen;
                        let hovered = in_rect(mouse, [x, top_c, w, h]);
                        canvas.round_fill(x, top_c, w, h, h / 2, if picked { accent() } else if hovered { PLACEHOLDER_COLOR } else { 0x181818 }, 256);
                        if !picked {
                            canvas.round_outline(x, top_c, w, h, h / 2, 1, MENU_EDGE_COLOR);
                        }
                        text.draw(canvas, x + pad * 3 / 4, top_c + pad / 4, small, if picked { on_accent() } else { TEXT_COLOR }, word);
                        drawn.push((Hit::Choice(row, which), [x, top_c, w, h]));
                        x += w + pad / 3;
                    }
                    x += pad / 2;
                }
            }
            if !hint.is_empty() {
                text.draw(canvas, x, y + (row_h - small_line) / 2, small, DIM_TEXT_COLOR, hint);
            }
            y += row_h;
        }
        y += pad / 2;

        // What it makes of the picture, side by side with the original.
        let footer_h = line + pad * 2 / 3;
        let bottom = top + height - pad - small_line - pad / 2 - footer_h - pad - line - pad / 2;
        let preview = [inner, y, inner_w, (bottom - y).max(px as i32 * 4)];
        canvas.round_fill(preview[0], preview[1], preview[2], preview[3], pad / 2, 0x0C0C0C, 256);
        drawn.push((Hit::Preview, preview));
        let trial = panel.trial.as_ref().filter(|(_, format, _)| *format == panel.format);
        let current = trial.is_some_and(|(asked, ..)| *asked == panel.asked);
        if let Some(source) = &panel.source {
            let size = (f64::from(source.flat.width()), f64::from(source.flat.height()));
            let (shown, dest) = shown_region(size, preview, panel.zoom, panel.center);
            // The divider follows the pointer over the picture; elsewhere
            // it stands in the middle.
            let split = if in_rect(mouse, preview) { (mouse.0 as i32).clamp(dest[0], dest[0] + dest[2]) } else { dest[0] + dest[2] / 2 };
            if let Some(frame) = cut(&source.flat, shown) {
                draw_region(canvas, &frame, dest, panel.zoom);
            }
            let decoded = trial.and_then(|(_, _, result)| result.as_ref().ok()).and_then(|trial| trial.decoded.as_ref());
            if let Some(decoded) = decoded {
                let share = f64::from(split - dest[0]) / f64::from(dest[2].max(1));
                let right = [shown[0] + shown[2] * share, shown[1], shown[2] * (1.0 - share), shown[3]];
                if let Some(frame) = cut(decoded, right).filter(|_| split < dest[0] + dest[2]) {
                    draw_region(canvas, &frame, [split, dest[1], dest[0] + dest[2] - split, dest[3]], panel.zoom);
                }
                canvas.fill(split - 1, dest[1], 2, dest[3], TEXT_COLOR);
            }
            let tag = |canvas: &mut Canvas, text: &mut Text, words: &str, right: bool| {
                let w = text.width(words, small) + pad;
                let x = if right { preview[0] + preview[2] - w - pad / 2 } else { preview[0] + pad / 2 };
                let ty = preview[1] + preview[3] - small_line - pad;
                canvas.round_fill(x, ty, w, small_line + pad / 2, pad / 3, 0x000000, 170);
                text.draw(canvas, x + pad / 2, ty + pad / 4, small, TEXT_COLOR, words);
            };
            tag(canvas, text, "Original", false);
            let right_words = match (decoded.is_some(), trial.map(|(_, _, result)| result.is_ok())) {
                (true, _) => format!("{}  {}", panel.format.name(), panel.settings.describe(panel.format)),
                (false, Some(true)) => format!("{}: no decoder here to show it", panel.format.name()),
                _ => format!("{}…", panel.format.name()),
            };
            tag(canvas, text, &right_words, true);
            let zoom_words = match panel.zoom {
                0 => "whole picture · click to look closer".to_string(),
                z => format!("{}% · click to look there · wheel or z to zoom", z * 100),
            };
            let zw = text.width(&zoom_words, small) + pad;
            canvas.round_fill(preview[0] + preview[2] - zw - pad / 2, preview[1] + pad / 2, zw, small_line + pad / 2, pad / 3, 0x000000, 170);
            text.draw(canvas, preview[0] + preview[2] - zw, preview[1] + pad / 2 + pad / 4, small, DIM_TEXT_COLOR, &zoom_words);
        } else {
            let words = panel.failed.clone().map_or("Reading…".to_string(), |error| format!("Not readable · {error}"));
            text.draw(canvas, preview[0] + pad, preview[1] + pad, px, DIM_TEXT_COLOR, &words);
        }
        y = preview[1] + preview[3] + pad / 2;

        // The numbers: size, score, time.
        let mut x = inner;
        let mut say = |canvas: &mut Canvas, text: &mut Text, words: &str, color: u32| {
            text.draw(canvas, x, y, px, color, words);
            x += text.width(words, px) + pad * 3 / 2;
        };
        match trial.map(|(_, _, result)| result) {
            Some(Ok(trial)) => {
                let change = (trial.size as f64 / file_size.max(1) as f64 - 1.0) * 100.0;
                say(canvas, text, &format!("{} → {}", meta::human_size(file_size), meta::human_size(trial.size as u64)), TEXT_COLOR);
                say(canvas, text, &format!("{change:+.0}%"), if change > 0.0 { MARK_COLOR } else { KEPT_COLOR });
                match trial.score {
                    _ if trial.how.contains("repacked") => say(canvas, text, "nothing lost: the JPEG comes back out of it", DIM_TEXT_COLOR),
                    Some(score) => say(canvas, text, &format!("SSIMULACRA2 {score:.1}, {}", verdict(score)), TEXT_COLOR),
                    None if trial.decoded.is_some() && current => say(canvas, text, "scoring…", DIM_TEXT_COLOR),
                    None => {}
                }
                let took = trial.took.as_secs_f64();
                let took = if took < 1.0 { format!("{:.0} ms", took * 1000.0) } else { format!("{took:.1} s") };
                say(canvas, text, &format!("encoded in {took}"), DIM_TEXT_COLOR);
                if !current {
                    say(canvas, text, "trying the new settings…", accent_soft());
                }
            }
            Some(Err(error)) => say(canvas, text, &format!("Not encoded · {error}"), DANGER_COLOR),
            None if panel.source.is_some() => say(canvas, text, "Encoding…", DIM_TEXT_COLOR),
            None => {}
        }
        y += line + pad;

        // The buttons: back to the recommended settings, keep these, which
        // files, and convert.
        let button = |canvas: &mut Canvas, text: &mut Text, x: i32, words: &str, primary: bool| -> i32 {
            let w = text.width(words, px) + 2 * pad;
            let hovered = in_rect(mouse, [x, y, w, footer_h]);
            let fill = if primary { accent() } else if hovered { PLACEHOLDER_COLOR } else { 0x181818 };
            canvas.round_fill(x, y, w, footer_h, pad / 2, fill, 256);
            if !primary {
                canvas.round_outline(x, y, w, footer_h, pad / 2, 1, MENU_EDGE_COLOR);
            }
            text.draw(canvas, x + pad, y + (footer_h - line) / 2, px, if primary { on_accent() } else { TEXT_COLOR }, words);
            w
        };
        let w = button(canvas, text, inner, "Reset", false);
        drawn.push((Hit::Reset, [inner, y, w, footer_h]));
        let save_x = inner + w + pad / 2;
        let w = button(canvas, text, save_x, "Save as default", false);
        drawn.push((Hit::Save, [save_x, y, w, footer_h]));
        let go = if panel.marked { format!("Convert {marks}  ⏎") } else { "Convert  ⏎".to_string() };
        let go_w = text.width(&go, px) + 2 * pad;
        let go_x = inner + inner_w - go_w;
        button(canvas, text, go_x, &go, true);
        drawn.push((Hit::Convert, [go_x, y, go_w, footer_h]));
        if marks > 1 {
            let mut x = go_x - pad;
            for (marked, words) in [(true, format!("{marks} marked")), (false, "This file".to_string())] {
                let w = text.width(&words, small) + pad * 3 / 2;
                x -= w;
                let h = small_line + pad / 2;
                let top_c = y + (footer_h - h) / 2;
                let picked = panel.marked == marked;
                canvas.round_fill(x, top_c, w, h, h / 2, if picked { accent() } else { 0x181818 }, 256);
                text.draw(canvas, x + pad * 3 / 4, top_c + pad / 4, small, if picked { on_accent() } else { TEXT_COLOR }, &words);
                drawn.push((Hit::Scope(marked), [x, top_c, w, h]));
                x -= pad / 3;
            }
        }
        y += footer_h + pad / 2;
        let keys = "↑↓ setting  ·  ←→ change (Shift ×10)  ·  Tab format  ·  z zoom  ·  f fit under a size  ·  r reset  ·  s save as default  ·  Enter convert  ·  Esc close";
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, keys);

        if let Some(panel) = &mut self.convert_panel {
            panel.drawn = drawn;
        }
    }
}

/// How many rows of settings the panel has now, after a change that may
/// have taken one away.
fn knobs_now(panel: &ConvertPanel, jpeg: bool) -> usize {
    knobs(panel.format, &panel.settings, jpeg).len()
}

pub(super) fn in_rect(at: (f64, f64), [left, top, w, h]: [i32; 4]) -> bool {
    let (x, y) = (at.0 as i32, at.1 as i32);
    (left..left + w).contains(&x) && (top..top + h).contains(&y)
}

fn panel_box(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, radius: i32) {
    panel(canvas, x, y, w, h, radius);
}

/// The part of a picture of `size` that a preview box shows, and where in
/// the box it goes: all of it fitted at zoom 0, else the box's own size
/// over `zoom`, around `center`, kept inside the picture.
pub(super) fn shown_region(size: (f64, f64), [x, y, w, h]: [i32; 4], zoom: u32, center: (f64, f64)) -> ([f64; 4], [i32; 4]) {
    let (sw, sh) = size;
    if zoom == 0 {
        let scale = (f64::from(w) / sw).min(f64::from(h) / sh);
        let (dw, dh) = ((sw * scale) as i32, (sh * scale) as i32);
        return ([0.0, 0.0, sw, sh], [x + (w - dw) / 2, y + (h - dh) / 2, dw, dh]);
    }
    let z = f64::from(zoom);
    let (vw, vh) = ((f64::from(w) / z).min(sw), (f64::from(h) / z).min(sh));
    let left = (center.0 * sw - vw / 2.0).clamp(0.0, sw - vw);
    let top = (center.1 * sh - vh / 2.0).clamp(0.0, sh - vh);
    let (dw, dh) = ((vw * z) as i32, (vh * z) as i32);
    ([left, top, vw, vh], [x + (w - dw) / 2, y + (h - dh) / 2, dw, dh])
}

/// Draws a cut of the picture into a rectangle: smoothly when it is made
/// smaller, as it is when the whole picture is shown; with its pixels
/// blended when made larger, which close up is what is wanted.
pub(super) fn draw_region(canvas: &mut Canvas, frame: &Frame, [x, y, w, h]: [i32; 4], zoom: u32) {
    if zoom == 0 {
        canvas.image(frame, x, y, w, h);
    } else {
        canvas.image_fast(frame, x, y, w, h);
    }
}

/// A size as typed: 500, 500k, 500 KB, 1.2 MB. In bytes.
fn parse_size(typed: &str) -> Option<u64> {
    let typed = typed.trim().to_lowercase().replace(' ', "");
    let digits = typed.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(typed.len());
    let (number, unit) = typed.split_at(digits);
    let number: f64 = number.parse().ok()?;
    // A bare number is in KB, as the panel speaks of sizes.
    let scale = match unit {
        "" | "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "b" => 1.0,
        _ => return None,
    };
    Some((number * scale) as u64).filter(|bytes| *bytes > 0)
}

impl App {
    /// f in the panel: asks for the size to fit the picture under.
    pub(super) fn ask_fit_size(&mut self) {
        let Some(panel) = &self.convert_panel else { return };
        if panel.format == Format::Png {
            return self.toast("PNG is lossless  ·  nothing to fit");
        }
        let id = panel.id;
        self.prompt = Some(Prompt { id, kind: PromptKind::FitSize, field: TextField::new(Vec::new(), 0) });
        self.redraw();
    }

    /// Looks, on another thread, for the highest quality whose file is
    /// no larger than `typed` says, in the panel's format, lossy.
    pub(super) fn fit_size(&mut self, typed: &str) {
        let Some(target) = parse_size(typed) else { return self.toast("Not a size  ·  try 500 KB or 1.2 MB") };
        let Some(panel) = &mut self.convert_panel else { return };
        let Some(source) = panel.source.clone() else { return self.toast("Still reading…") };
        let (format, id, mut settings) = (panel.format, panel.id, panel.settings);
        // Fitting is of a lossy file: the lossless modes are left.
        settings.jxl.lossless = false;
        settings.jxl.recompress_jpeg = false;
        settings.webp.lossless = false;
        let proxy = self.proxy.clone();
        self.toast(format!("Fitting under {}…", meta::human_size(target)));
        std::thread::spawn(move || {
            let size = |quality: f32| -> Option<usize> {
                let mut tried = settings;
                tried.set_quality(format, quality);
                convert::trial_unscored(&source.image, None, format, &tried).ok().map(|trial| trial.size)
            };
            // The highest quality that fits, halving the range each time.
            let (mut low, mut high, mut best) = (1u32, 100u32, None);
            while low <= high {
                let middle = (low + high) / 2;
                match size(middle as f32) {
                    Some(bytes) if bytes as u64 <= target => {
                        best = Some((middle, bytes));
                        low = middle + 1;
                    }
                    Some(_) => high = middle - 1,
                    None => break,
                }
            }
            let _ = proxy.send_event(Event::ConvertFit(id, format, best.map(|(quality, bytes)| (quality as f32, bytes as u64)), target));
        });
    }

    /// The quality found to fit: set, and tried as any change is.
    pub(super) fn convert_fit(&mut self, id: usize, format: Format, found: Option<(f32, u64)>, target: u64) {
        let Some(panel) = self.convert_panel.as_mut().filter(|panel| panel.id == id && panel.format == format) else { return };
        let Some((quality, bytes)) = found else {
            return self.toast(format!("Nothing fits under {}", meta::human_size(target)));
        };
        panel.settings.jxl.lossless = false;
        panel.settings.jxl.recompress_jpeg = false;
        panel.settings.webp.lossless = false;
        panel.settings.set_quality(format, quality);
        self.toast(format!("Quality {quality:.0}  ·  {}", meta::human_size(bytes)));
        self.try_settings();
        self.redraw();
    }
}
