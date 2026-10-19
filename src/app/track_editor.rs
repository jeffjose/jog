//! The tracks editor: a video's streams in a list, to keep or drop, put in
//! another order, give a language and a title, mark default or forced, add
//! from a file or take out to one. Written as a new file beside the video
//! by ffmpeg copying the streams as they are, so nothing is encoded again
//! and it takes as long as reading the file does.

use serde_json::Value;
use winit::keyboard::{Key, NamedKey};

use super::tracks::{channels_name, codec_name, language_name, TrackKind};
use super::*;

/// One stream of the file to be written.
#[derive(Clone, Debug)]
pub struct EditRow {
    /// Where it comes from: the video's own stream by its index, or a file
    /// added, moved in time by `delay` seconds.
    from: Source,
    kind: TrackKind,
    codec: String,
    channels: Option<i64>,
    lang: Option<String>,
    title: Option<String>,
    default: bool,
    forced: bool,
    keep: bool,
    /// Sound: mixed down to stereo, and its loudness evened out, which
    /// encode it again. Subtitles: drawn into the picture, which encodes
    /// the video again.
    stereo: bool,
    loudness: bool,
    burn: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Source {
    Stream(usize),
    File { path: PathBuf, delay: f64 },
}


/// What a click in the editor lands on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Row(usize),
    Keep(usize),
    Write,
    Replace,
    Close,
}

pub struct TrackEditor {
    id: usize,
    rows: Vec<EditRow>,
    cursor: usize,
    seconds: f64,
    /// The container, as ffprobe names it: "matroska,webm", "mov,mp4,…".
    format: String,
    failed: Option<String>,
    /// Replace the original, asked once and confirmed by asking again.
    replacing: bool,
    drawn: Vec<(Hit, [i32; 4])>,
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The streams of a file as the editor's rows, its length and container.
fn read_rows(path: &Path) -> Result<(Vec<EditRow>, f64, String), String> {
    let output = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("ffprobe did not run: {error}"))?;
    let probe: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let rows = probe["streams"]
        .as_array()
        .map(|streams| {
            streams
                .iter()
                .filter_map(|s| {
                    let kind = match s["codec_type"].as_str()? {
                        "video" => TrackKind::Video,
                        "audio" => TrackKind::Audio,
                        "subtitle" => TrackKind::Sub,
                        _ => return None,
                    };
                    let tag = |name: &str| s["tags"].as_object().and_then(|tags| tags.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))).and_then(|(_, v)| text_of(v));
                    Some(EditRow {
                        from: Source::Stream(s["index"].as_u64()? as usize),
                        kind,
                        codec: text_of(&s["codec_name"]).unwrap_or_default(),
                        channels: s["channels"].as_i64(),
                        lang: tag("language").filter(|lang| lang != "und"),
                        title: tag("title"),
                        default: s["disposition"]["default"] == 1,
                        forced: s["disposition"]["forced"] == 1,
                        keep: true,
                        stereo: false,
                        loudness: false,
                        burn: false,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let seconds = text_of(&probe["format"]["duration"]).and_then(|d| d.parse().ok()).unwrap_or(0.0);
    Ok((rows, seconds, text_of(&probe["format"]["format_name"]).unwrap_or_default()))
}

/// The file extension a stream taken out on its own is written with.
fn extract_extension(row: &EditRow) -> &'static str {
    match row.codec.as_str() {
        "subrip" => "srt",
        "ass" | "ssa" => "ass",
        "webvtt" => "vtt",
        "hdmv_pgs_subtitle" => "sup",
        "aac" => "m4a",
        "opus" => "opus",
        "flac" => "flac",
        "ac3" => "ac3",
        "eac3" => "eac3",
        "mp3" => "mp3",
        _ if row.kind == TrackKind::Sub => "mks",
        _ if row.kind == TrackKind::Audio => "mka",
        _ => "mkv",
    }
}

impl App {
    /// Opens the editor on the video on view.
    pub(super) fn open_track_editor(&mut self) {
        let id = self.list[self.focus()];
        if !files::is_video(&self.files[id]) {
            return self.toast("Tracks are a video's");
        }
        let (rows, seconds, format, failed) = match read_rows(&self.files[id]) {
            Ok((rows, seconds, format)) => (rows, seconds, format, None),
            Err(error) => (Vec::new(), 0.0, String::new(), Some(error)),
        };
        self.track_picker = None;
        self.menu = None;
        self.track_editor = Some(TrackEditor { id, rows, cursor: 0, seconds, format, failed, replacing: false, drawn: Vec::new() });
        self.redraw();
    }

    pub(super) fn track_editor_key(&mut self, key: Key<&str>) {
        let alt = self.modifiers.alt_key();
        let Some(editor) = &mut self.track_editor else { return };
        let last = editor.rows.len().saturating_sub(1);
        let at = editor.cursor;
        match key {
            Key::Named(NamedKey::Escape) => {
                if editor.replacing {
                    editor.replacing = false;
                } else {
                    self.track_editor = None;
                }
            }
            // Alt with the arrows moves the track, among its own kind.
            Key::Named(NamedKey::ArrowUp) if alt && at > 0 && editor.rows[at - 1].kind == editor.rows[at].kind => {
                editor.rows.swap(at, at - 1);
                editor.cursor -= 1;
            }
            Key::Named(NamedKey::ArrowDown) if alt && at < last && editor.rows[at + 1].kind == editor.rows[at].kind => {
                editor.rows.swap(at, at + 1);
                editor.cursor += 1;
            }
            Key::Named(NamedKey::ArrowUp) => editor.cursor = at.saturating_sub(1),
            Key::Named(NamedKey::ArrowDown) => editor.cursor = (at + 1).min(last),
            Key::Named(NamedKey::Space) => toggle_keep(editor, at),
            Key::Character("d") => {
                // One default of each kind: this one, or none.
                if let Some(kind) = editor.rows.get(at).map(|row| row.kind) {
                    let was = editor.rows[at].default;
                    editor.rows.iter_mut().filter(|row| row.kind == kind).for_each(|row| row.default = false);
                    editor.rows[at].default = !was;
                }
            }
            Key::Character("f") => {
                if let Some(row) = editor.rows.get_mut(at).filter(|row| row.kind != TrackKind::Video) {
                    row.forced = !row.forced;
                }
            }
            Key::Character("l") => return self.ask_track_text(true),
            Key::Character("t") => return self.ask_track_text(false),
            Key::Character("a") => return self.ask_track_file(),
            Key::Character("x") => return self.extract_track(),
            Key::Character("s") => match editor.rows.get_mut(at) {
                Some(row) if row.kind == TrackKind::Audio => row.stereo = !row.stereo,
                _ => return self.toast("Stereo is for sound"),
            },
            Key::Character("n") => match editor.rows.get_mut(at) {
                Some(row) if row.kind == TrackKind::Audio => row.loudness = !row.loudness,
                _ => return self.toast("Loudness is for sound"),
            },
            Key::Character("b") => {
                let text = editor.rows.get(at).is_some_and(|row| row.kind == TrackKind::Sub && matches!(row.codec.as_str(), "subrip" | "ass" | "ssa" | "webvtt" | "mov_text"));
                if !text {
                    return self.toast("Only text subtitles can be burned in");
                }
                // One drawn in at a time.
                let was = editor.rows[at].burn;
                editor.rows.iter_mut().for_each(|row| row.burn = false);
                editor.rows[at].burn = !was;
            }
            Key::Named(NamedKey::Enter) => return self.write_tracks(),
            Key::Character("R") => editor.replacing = !editor.replacing,
            _ => return,
        }
        self.redraw();
    }

    pub(super) fn track_editor_click(&mut self, x: f64, y: f64) {
        let Some(editor) = &mut self.track_editor else { return };
        let on = |[left, top, w, h]: [i32; 4]| (left..left + w).contains(&(x as i32)) && (top..top + h).contains(&(y as i32));
        let Some((hit, _)) = editor.drawn.iter().rev().find(|(_, place)| on(*place)).copied() else { return };
        match hit {
            Hit::Row(at) => editor.cursor = at,
            Hit::Keep(at) => {
                editor.cursor = at;
                toggle_keep(editor, at);
            }
            Hit::Write => return self.write_tracks(),
            Hit::Replace => editor.replacing = !editor.replacing,
            Hit::Close => self.track_editor = None,
        }
        self.redraw();
    }

    /// l and t: a box for the language or the title of the track at the
    /// cursor.
    fn ask_track_text(&mut self, language: bool) {
        let Some(editor) = &self.track_editor else { return };
        let Some(row) = editor.rows.get(editor.cursor) else { return };
        let now = if language { row.lang.clone() } else { row.title.clone() }.unwrap_or_default();
        let text: Vec<char> = now.chars().collect();
        let kind = if language { PromptKind::TrackLanguage(editor.cursor) } else { PromptKind::TrackTitle(editor.cursor) };
        self.prompt = Some(Prompt { id: editor.id, kind, field: TextField::new(text.clone(), text.len()) });
        self.redraw();
    }

    /// a: a box for a subtitle or audio file to add, starting in the
    /// video's folder.
    fn ask_track_file(&mut self) {
        let Some(editor) = &self.track_editor else { return };
        let dir = self.files[editor.id].parent().map(short_dir).unwrap_or_default();
        let text: Vec<char> = if dir.is_empty() { Vec::new() } else { format!("{dir}/").chars().collect() };
        self.prompt = Some(Prompt { id: editor.id, kind: PromptKind::TrackFile, field: TextField::new(text.clone(), text.len()) });
        self.redraw();
    }

    /// What was typed in one of the editor's boxes.
    pub(super) fn track_prompt_done(&mut self, kind: PromptKind, typed: &str) {
        let typed = typed.trim();
        let Some(editor) = &mut self.track_editor else { return };
        match kind {
            PromptKind::TrackLanguage(at) => {
                if let Some(row) = editor.rows.get_mut(at) {
                    row.lang = Some(typed.to_lowercase()).filter(|lang| !lang.is_empty());
                }
            }
            PromptKind::TrackTitle(at) => {
                if let Some(row) = editor.rows.get_mut(at) {
                    row.title = Some(typed.to_string()).filter(|title| !title.is_empty());
                }
            }
            PromptKind::TrackFile => {
                // "file.srt" or "file.srt +0.35": a file, and how far later.
                let (name, delay) = match typed.rsplit_once(' ') {
                    Some((name, delay)) if delay.parse::<f64>().is_ok() => (name.trim(), delay.parse().unwrap_or(0.0)),
                    _ => (typed, 0.0),
                };
                let path = crate::options::expand_home(name);
                if !path.is_file() {
                    return self.toast("No such file");
                }
                let ext = path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
                let (kind, codec) = match ext.as_str() {
                    "srt" => (TrackKind::Sub, "subrip"),
                    "ass" | "ssa" => (TrackKind::Sub, "ass"),
                    "vtt" => (TrackKind::Sub, "webvtt"),
                    "sup" => (TrackKind::Sub, "hdmv_pgs_subtitle"),
                    "m4a" | "aac" => (TrackKind::Audio, "aac"),
                    "opus" => (TrackKind::Audio, "opus"),
                    "flac" => (TrackKind::Audio, "flac"),
                    "mp3" => (TrackKind::Audio, "mp3"),
                    "ac3" => (TrackKind::Audio, "ac3"),
                    "eac3" => (TrackKind::Audio, "eac3"),
                    "wav" => (TrackKind::Audio, "pcm_s16le"),
                    "mka" => (TrackKind::Audio, "matroska"),
                    _ => return self.toast("Not a subtitle or audio file"),
                };
                // A language in the name, as in concert.en.srt.
                let lang = path.file_stem().and_then(|stem| Path::new(stem).extension()).map(|lang| lang.to_string_lossy().to_lowercase()).filter(|lang| (2..=3).contains(&lang.len()));
                let row = EditRow { from: Source::File { path, delay }, kind, codec: codec.to_string(), channels: None, lang, title: None, default: false, forced: false, keep: true, stereo: false, loudness: false, burn: false };
                // After the last of its kind.
                let at = editor.rows.iter().rposition(|other| other.kind == kind).map_or(editor.rows.len(), |at| at + 1);
                editor.rows.insert(at, row);
                editor.cursor = at;
            }
            _ => {}
        }
        self.redraw();
    }

    /// The ffmpeg arguments, after the video's own `-i`, that write the
    /// rows: the added files as more inputs, then each stream kept in its
    /// order with its language, title and flags.
    fn remux_args(editor: &TrackEditor, mp4: bool, video: &Path) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let mut inputs = Vec::new();
        for row in &editor.rows {
            if let (Source::File { path, delay }, true) = (&row.from, row.keep) {
                if *delay != 0.0 {
                    args.extend(["-itsoffset".to_string(), format!("{delay:.3}")]);
                }
                args.extend(["-i".to_string(), path.display().to_string()]);
                inputs.push(path.clone());
            }
        }
        let kept: Vec<&EditRow> = editor.rows.iter().filter(|row| row.keep).collect();
        for row in &kept {
            match &row.from {
                Source::Stream(index) => args.extend(["-map".to_string(), format!("0:{index}")]),
                Source::File { path, .. } => {
                    let input = inputs.iter().position(|other| other == path).map_or(1, |at| at + 1);
                    args.extend(["-map".to_string(), format!("{input}:0")]);
                }
            }
        }
        args.extend(["-c".to_string(), "copy".to_string()]);
        // MP4 takes text subtitles only as mov_text.
        if mp4 {
            args.extend(["-c:s".to_string(), "mov_text".to_string()]);
        }
        // Sound mixed down or evened out is encoded again, as AAC in MP4
        // and Opus elsewhere; loudnorm works at 192 kHz, so back to 48.
        for (out, row) in kept.iter().enumerate().filter(|(_, row)| row.stereo || row.loudness) {
            let codec = if mp4 { "aac" } else { "libopus" };
            args.extend([format!("-c:{out}"), codec.to_string(), format!("-b:{out}"), if row.stereo { "192k" } else { "320k" }.to_string()]);
            if row.stereo {
                args.extend([format!("-ac:{out}"), "2".to_string()]);
            }
            if row.loudness {
                args.extend([format!("-filter:{out}"), "loudnorm=I=-16:TP=-1.5:LRA=11,aresample=48000".to_string()]);
            }
        }
        // Subtitles drawn into the picture: the video encoded again, with
        // them, as H.264.
        if let (Some(burn), Some(video_out)) = (editor.rows.iter().find(|row| row.burn), kept.iter().position(|row| row.kind == TrackKind::Video)) {
            let source = match &burn.from {
                Source::Stream(index) => {
                    // The filter counts a file's subtitle streams from 0.
                    let nth = editor.rows.iter().filter(|row| row.kind == TrackKind::Sub && matches!(row.from, Source::Stream(other) if other < *index)).count();
                    format!("filename={}:si={nth}", filter_escape(&video.display().to_string()))
                }
                Source::File { path, .. } => format!("filename={}", filter_escape(&path.display().to_string())),
            };
            args.extend([format!("-c:{video_out}"), "libx264".to_string(), "-crf".to_string(), "18".to_string(), "-preset".to_string(), "medium".to_string()]);
            args.extend([format!("-filter:{video_out}"), format!("subtitles={source}")]);
        }
        for (out, row) in kept.iter().enumerate() {
            args.push(format!("-metadata:s:{out}"));
            args.push(format!("language={}", row.lang.as_deref().unwrap_or("und")));
            args.push(format!("-metadata:s:{out}"));
            args.push(format!("title={}", row.title.as_deref().unwrap_or_default()));
            let mut flags = Vec::new();
            if row.default {
                flags.push("default");
            }
            if row.forced {
                flags.push("forced");
            }
            args.push(format!("-disposition:{out}"));
            args.push(if flags.is_empty() { "0".to_string() } else { flags.join("+") });
        }
        args
    }

    /// Enter: writes the file, on another thread, beside the video; or,
    /// asked to, in its place.
    fn write_tracks(&mut self) {
        let Some(editor) = &self.track_editor else { return };
        if !editor.rows.iter().any(|row| row.keep && row.kind == TrackKind::Video) {
            return self.toast("Keep the video track");
        }
        let video = self.files[editor.id].clone();
        let stem = video.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        // Text subtitles added to an MP4 go as mov_text; anything else
        // that MP4 cannot hold means Matroska.
        let mp4 = editor.format.contains("mp4");
        let fits_mp4 = editor.rows.iter().filter(|row| row.keep && row.kind == TrackKind::Sub).all(|row| matches!(row.codec.as_str(), "subrip" | "mov_text" | "webvtt" | "ass"));
        let (ext, muxer, mov_text) = match (mp4, fits_mp4) {
            (true, true) => ("mp4", "mp4", true),
            (true, false) => ("mkv", "matroska", false),
            _ if editor.format.contains("webm") && video.extension().is_some_and(|e| e == "webm") => ("mkv", "matroska", false),
            _ => ("mkv", "matroska", false),
        };
        let args = Self::remux_args(editor, mov_text, &video);
        let output = video.with_file_name(format!("{stem}.tracks.{ext}"));
        let (replacing, seconds) = (editor.replacing, editor.seconds);
        if self.dry(|_| vec![format!("remux {} -> {}", video.display(), output.display())], "would write the tracks") {
            return;
        }
        if output.exists() && !replacing {
            return self.toast(format!("{} exists", output.file_name().unwrap_or_default().to_string_lossy()));
        }
        self.track_editor = None;
        self.toast("Writing the tracks…");
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let name = output.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let (mut said, started) = (None, Instant::now());
            let result = crate::transcode::remux(&video, &args, muxer, &output, seconds, |share| {
                let percent = (share * 100.0) as u32;
                if said != Some(percent) {
                    said = Some(percent);
                    let _ = proxy.send_event(Event::VideoProgress(format!("{name}  ·  {}", progress_words(share, started))));
                }
            });
            // In place of the original: the new file takes its name. The
            // extension stays the original's when the container does.
            let result = result.and_then(|made| match replacing {
                false => Ok(made),
                true => {
                    let target = if made.extension() == video.extension() { video.clone() } else { video.with_extension(ext) };
                    std::fs::rename(&made, &target).map_err(|error| error.to_string())?;
                    if target != video {
                        std::fs::remove_file(&video).map_err(|error| error.to_string())?;
                    }
                    Ok(target)
                }
            });
            let _ = proxy.send_event(Event::TracksWritten(result, replacing));
        });
    }

    pub(super) fn tracks_written(&mut self, result: Result<PathBuf, String>, replaced: bool) {
        match (result, replaced) {
            (Err(error), _) => self.toast(format!("Not written · {error}")),
            (Ok(path), true) => {
                println!("[tracks] replaced {}", path.display());
                self.toast(format!("Replaced {}", path.file_name().unwrap_or_default().to_string_lossy()));
                self.refresh();
            }
            (Ok(path), false) => {
                println!("[tracks] {}", path.display());
                let said = format!("{}  ·  {}", path.file_name().unwrap_or_default().to_string_lossy(), meta::human_size(std::fs::metadata(&path).map_or(0, |m| m.len())));
                self.made(path, said);
            }
        }
    }

    /// x: the track at the cursor written out on its own, beside the video.
    fn extract_track(&mut self) {
        let Some(editor) = &self.track_editor else { return };
        let Some(row) = editor.rows.get(editor.cursor).cloned() else { return };
        let Source::Stream(index) = row.from else { return self.toast("Already a file") };
        if row.kind == TrackKind::Video {
            return self.toast("Only audio and subtitles");
        }
        let video = self.files[editor.id].clone();
        let stem = video.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let label = row.lang.clone().or(row.title.clone()).unwrap_or_else(|| format!("track{index}"));
        let ext = extract_extension(&row);
        let output = video.with_file_name(format!("{stem}.{label}.{ext}"));
        if output.exists() {
            return self.toast(format!("{} exists", output.file_name().unwrap_or_default().to_string_lossy()));
        }
        let muxer = match ext {
            "srt" => "srt",
            "ass" => "ass",
            "vtt" => "webvtt",
            "sup" => "sup",
            "m4a" => "ipod",
            "opus" => "opus",
            "flac" => "flac",
            "ac3" => "ac3",
            "eac3" => "eac3",
            "mp3" => "mp3",
            _ => "matroska",
        };
        let args = vec!["-map".to_string(), format!("0:{index}"), "-c".to_string(), "copy".to_string()];
        let seconds = editor.seconds;
        let proxy = self.proxy.clone();
        self.toast("Taking it out…");
        std::thread::spawn(move || {
            let result = crate::transcode::remux(&video, &args, muxer, &output, seconds, |_| {});
            let _ = proxy.send_event(Event::TracksWritten(result, false));
        });
    }

    /// Draws the editor in the middle of the window.
    pub(super) fn draw_track_editor(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        let name = self.track_editor.as_ref().map(|editor| self.name_of(editor.id)).unwrap_or_default();
        let Some(editor) = &mut self.track_editor else { return };
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let row_h = line + pad * 2 / 3;
        let width = (px as i32 * 52).min(canvas.width - 4 * pad);
        let rows_h = editor.rows.len().max(1) as i32 * (row_h + pad / 4);
        let height = (pad + line + pad + small_line + pad / 2 + rows_h + pad / 2 + pad / 2 + line + pad / 2 + small_line + pad).min(canvas.height - 2 * pad);
        let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);
        panel(canvas, left, top, width, height, pad * 2 / 3);
        editor.drawn.clear();
        let inner = left + pad * 3 / 2;
        let inner_w = width - pad * 3;
        let mut y = top + pad;

        let heading = format!("{}  Tracks", commands::icons::AUDIO_LINES);
        text.draw(canvas, inner, y, px, TEXT_COLOR, &heading);
        let about = format!("{name}  ·  {}", clock(editor.seconds));
        let hx = inner + text.width(&heading, px) + pad;
        text.draw(canvas, hx, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, &about);
        let x_w = text.width(commands::icons::X, px);
        text.draw(canvas, inner + inner_w - x_w, y, px, DIM_TEXT_COLOR, commands::icons::X);
        editor.drawn.push((Hit::Close, [inner + inner_w - line, y, line, line]));
        y += line + pad;
        if let Some(error) = &editor.failed {
            text.draw(canvas, inner, y, px, DIM_TEXT_COLOR, &format!("Not readable · {error}"));
            return;
        }

        // Columns: keep, kind, language and title, codec, flags.
        let kind_x = inner + px as i32 * 3;
        let name_x = kind_x + px as i32 * 5;
        let codec_x = inner + inner_w - px as i32 * 20;
        let flags_x = inner + inner_w - px as i32 * 13;
        for (label, x) in [("Keep", inner), ("Kind", kind_x), ("Language  ·  title", name_x), ("Codec", codec_x), ("Flags", flags_x)] {
            text.draw(canvas, x, y, small, DIM_TEXT_COLOR, label);
        }
        y += small_line + pad / 2;
        for (at, row) in editor.rows.iter().enumerate() {
            let place = [inner - pad / 3, y, inner_w + pad * 2 / 3, row_h];
            let selected = at == editor.cursor;
            let hovered = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 2, if selected { 0x262A3D } else if hovered { 0x2A2A30 } else { 0x222226 }, 256);
            if selected {
                canvas.round_outline(place[0], place[1], place[2], place[3], pad / 2, 1, accent());
            }
            editor.drawn.push((Hit::Row(at), place));
            // The box: ticked to keep; the video's always is.
            let side = line * 3 / 4;
            let (bx, by) = (inner, y + (row_h - side) / 2);
            let added = matches!(row.from, Source::File { .. });
            match (row.keep, row.kind == TrackKind::Video) {
                (true, true) => canvas.round_fill(bx, by, side, side, side / 4, 0x55565E, 256),
                (true, false) => canvas.round_fill(bx, by, side, side, side / 4, if added { 0x2F9E6A } else { accent() }, 256),
                (false, _) => canvas.round_outline(bx, by, side, side, side / 4, 1, DIM_TEXT_COLOR),
            }
            if row.keep {
                let mark = if added { "+" } else { "✓" };
                let mw = text.width(mark, small);
                text.draw(canvas, bx + (side - mw) / 2, by + (side - small_line) / 2, small, on_accent(), mark);
            }
            editor.drawn.push((Hit::Keep(at), [bx - pad / 4, y, side + pad / 2, row_h]));
            let fg = if row.keep { TEXT_COLOR } else { DIM_TEXT_COLOR };
            let ty = y + (row_h - line) / 2;
            let kind = match row.kind {
                TrackKind::Video => "Video",
                TrackKind::Audio => "Audio",
                TrackKind::Sub => "Subtitle",
            };
            text.draw(canvas, kind_x, ty + (line - small_line) / 2, small, DIM_TEXT_COLOR, kind);
            let lang = row.lang.as_deref().map(language_name);
            let mut words = match (lang, &row.title) {
                (Some(lang), Some(title)) => format!("{lang}  ·  {title}"),
                (Some(lang), None) => lang,
                (None, Some(title)) => title.clone(),
                (None, None) if row.kind == TrackKind::Video => "—".to_string(),
                (None, None) => "No language".to_string(),
            };
            if let Source::File { path, delay } = &row.from {
                words = format!("{words}  ·  {}{}", path.file_name().unwrap_or_default().to_string_lossy(), if *delay != 0.0 { format!("  {delay:+.2} s") } else { String::new() });
            }
            if !row.keep {
                words = format!("{words}  ·  dropped");
            }
            let words = clipped(text, &words, codec_x - name_x - pad, px);
            text.draw(canvas, name_x, ty, px, fg, &words);
            let codec = match (row.kind, row.channels) {
                (TrackKind::Audio, Some(count)) => format!("{} {}", codec_name(&row.codec), channels_name(count)),
                _ => codec_name(&row.codec),
            };
            text.draw(canvas, codec_x, ty + (line - small_line) / 2, small, DIM_TEXT_COLOR, &codec);
            let mut fx = flags_x;
            for (on, flag, color) in [(row.default, "default", accent()), (row.forced, "forced", 0x8E5BD6), (row.stereo, "stereo", 0xD9822B), (row.loudness, "loudness", 0xD9822B), (row.burn, "burn in", 0xE5484D)] {
                if on {
                    let w = text.width(flag, small) + pad / 2;
                    canvas.round_fill(fx, y + (row_h - small_line) / 2, w, small_line, pad / 4, color, 256);
                    text.draw(canvas, fx + pad / 4, y + (row_h - small_line) / 2, small, on_accent(), flag);
                    fx += w + pad / 3;
                }
            }
            y += row_h + pad / 4;
        }
        y += pad / 2;

        // What will be written, and the buttons.
        let kept = editor.rows.iter().filter(|row| row.keep).count();
        let dropped = editor.rows.iter().filter(|row| !row.keep).count();
        let added = editor.rows.iter().filter(|row| row.keep && matches!(row.from, Source::File { .. })).count();
        let mut summary = format!("{kept} tracks");
        if dropped > 0 {
            summary.push_str(&format!("  ·  −{dropped}"));
        }
        if added > 0 {
            summary.push_str(&format!("  ·  +{added}"));
        }
        let encodes = match (editor.rows.iter().any(|row| row.burn), editor.rows.iter().any(|row| row.keep && (row.stereo || row.loudness))) {
            (true, _) => "  ·  the video encoded again: slow",
            (false, true) => "  ·  sound encoded again",
            (false, false) => "  ·  copied, not encoded",
        };
        summary.push_str(if editor.replacing { "  ·  replaces the original, no undo" } else { "  ·  a new file beside it" });
        summary.push_str(encodes);
        canvas.fill(inner, y, inner_w, 1, MENU_EDGE_COLOR);
        y += pad / 2;
        text.draw(canvas, inner, y + (line - small_line) / 2, small, if editor.replacing { DANGER_COLOR } else { DIM_TEXT_COLOR }, &summary);
        let write = if editor.replacing { "Replace  ⏎" } else { "Write  ⏎" };
        let ww = text.width(write, px) + pad;
        let wx = inner + inner_w - ww;
        canvas.round_fill(wx, y - pad / 6, ww, line + pad / 3, pad / 3, if editor.replacing { DANGER_COLOR } else { accent() }, 256);
        text.draw(canvas, wx + pad / 2, y, px, on_accent(), write);
        editor.drawn.push((Hit::Write, [wx, y - pad / 6, ww, line + pad / 3]));
        let replace = "Replace original";
        let rw = text.width(replace, small) + pad;
        let rx = wx - pad / 2 - rw;
        canvas.round_fill(rx, y, rw, line, pad / 3, if editor.replacing { 0x3A2020 } else { PLACEHOLDER_COLOR }, 256);
        text.draw(canvas, rx + pad / 2, y + (line - small_line) / 2, small, TEXT_COLOR, replace);
        editor.drawn.push((Hit::Replace, [rx, y, rw, line]));
        y += line + pad / 2;
        let keys = "Space keep  ·  Alt+↑↓ move  ·  d default  ·  f forced  ·  l language  ·  t title  ·  a add  ·  x take out  ·  s stereo  ·  n loudness  ·  b burn in  ·  Esc";
        let keys = clipped(text, keys, inner_w, small);
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, &keys);
    }
}

/// Space or a click on the box: keep the track or drop it. The video's
/// first track stays.
fn toggle_keep(editor: &mut TrackEditor, at: usize) {
    let Some(row) = editor.rows.get(at) else { return };
    let videos = editor.rows.iter().filter(|row| row.kind == TrackKind::Video && row.keep).count();
    if row.kind == TrackKind::Video && row.keep && videos == 1 {
        return;
    }
    editor.rows[at].keep = !editor.rows[at].keep;
}

/// A path as a filter's option wants it: the characters that mean
/// something to a filter graph each behind a backslash, twice over, once
/// for the option and once for the graph.
fn filter_escape(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        if matches!(c, '\\' | ':' | '\'' | ',' | '[' | ']' | ';' | '=') {
            out.push_str("\\\\");
        }
        out.push(c);
    }
    out
}
