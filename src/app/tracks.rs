//! A video's audio and subtitle tracks: the picker over it (Ctrl+J), the
//! tracks and timing picked for each video kept for the next time it is
//! opened, and the language picked last preferred for videos not seen.

use super::*;

/// Which kind of stream a track is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackKind {
    Video,
    Audio,
    Sub,
}

/// One track of the file a player has open, as the player tells of it.
#[derive(Clone, PartialEq, Debug)]
pub struct TrackInfo {
    pub kind: TrackKind,
    /// The player's number for it, counted within its kind from 1.
    pub id: i64,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub codec: String,
    pub channels: Option<i64>,
    pub default: bool,
    pub forced: bool,
    /// Loaded from a file beside the video rather than inside it.
    pub external: bool,
    pub selected: bool,
}

/// Every track of the file a player has open, in the file's order: as the
/// player keeps jog told of them, so reading them waits on nothing.
pub fn track_list(player: &Player) -> Vec<TrackInfo> {
    player.text("track-list").map(|list| parse_tracks(&list)).unwrap_or_default()
}

/// The tracks from mpv's `track-list`, which it gives as JSON.
fn parse_tracks(list: &str) -> Vec<TrackInfo> {
    let Ok(serde_json::Value::Array(tracks)) = serde_json::from_str(list) else { return Vec::new() };
    tracks
        .iter()
        .filter_map(|track| {
            let text = |name: &str| track[name].as_str().filter(|value| !value.is_empty()).map(str::to_string);
            let flag = |name: &str| track[name].as_bool() == Some(true);
            let kind = match track["type"].as_str()? {
                "video" => TrackKind::Video,
                "audio" => TrackKind::Audio,
                "sub" => TrackKind::Sub,
                _ => return None,
            };
            Some(TrackInfo {
                kind,
                id: track["id"].as_i64()?,
                lang: text("lang"),
                title: text("title"),
                codec: text("codec").unwrap_or_default(),
                channels: track["demux-channel-count"].as_i64(),
                default: flag("default"),
                forced: flag("forced"),
                external: flag("external"),
                selected: flag("selected"),
            })
        })
        .collect()
}

/// A codec as people know it: TrueHD, E-AC-3, PGS, SRT.
pub fn codec_name(codec: &str) -> String {
    match codec {
        "truehd" => "TrueHD",
        "eac3" => "E-AC-3",
        "ac3" => "AC-3",
        "aac" => "AAC",
        "opus" => "Opus",
        "flac" => "FLAC",
        "mp3" => "MP3",
        "vorbis" => "Vorbis",
        "dts" => "DTS",
        "alac" => "ALAC",
        "pcm_s16le" | "pcm_s24le" | "pcm_s32le" | "pcm_f32le" => "PCM",
        "hdmv_pgs_subtitle" => "PGS",
        "dvd_subtitle" => "VobSub",
        "dvb_subtitle" => "DVB",
        "subrip" => "SRT",
        "ass" | "ssa" => "ASS",
        "webvtt" => "WebVTT",
        "mov_text" => "TX3G",
        "hevc" => "HEVC",
        "h264" => "H.264",
        "av1" => "AV1",
        "vp9" => "VP9",
        other => return other.to_uppercase(),
    }
    .to_string()
}

/// How many channels, as an audio track is described: 7.1, 5.1, 2.0.
pub fn channels_name(count: i64) -> String {
    match count {
        1 => "1.0".to_string(),
        2 => "2.0".to_string(),
        6 => "5.1".to_string(),
        8 => "7.1".to_string(),
        n => format!("{n} ch"),
    }
}

/// A language's name from the code a file tags it with, when it is one
/// of the common ones; otherwise the code.
pub fn language_name(code: &str) -> String {
    let name = match code.to_lowercase().as_str() {
        "en" | "eng" => "English",
        "ko" | "kor" => "Korean",
        "ja" | "jpn" => "Japanese",
        "zh" | "chi" | "zho" => "Chinese",
        "fr" | "fre" | "fra" => "French",
        "de" | "ger" | "deu" => "German",
        "es" | "spa" => "Spanish",
        "it" | "ita" => "Italian",
        "pt" | "por" => "Portuguese",
        "ru" | "rus" => "Russian",
        "hi" | "hin" => "Hindi",
        "ar" | "ara" => "Arabic",
        "nl" | "dut" | "nld" => "Dutch",
        "sv" | "swe" => "Swedish",
        "pl" | "pol" => "Polish",
        "tr" | "tur" => "Turkish",
        "th" | "tha" => "Thai",
        "vi" | "vie" => "Vietnamese",
        "id" | "ind" => "Indonesian",
        "ta" | "tam" => "Tamil",
        "ml" | "mal" => "Malayalam",
        "te" | "tel" => "Telugu",
        "und" => "Unknown",
        _ => return code.to_string(),
    };
    name.to_string()
}

/// A track's name: its language and its title, as far as it has them.
pub fn track_label(track: &TrackInfo) -> String {
    let lang = track.lang.as_deref().map(language_name);
    match (lang, track.title.as_deref()) {
        (Some(lang), Some(title)) if !title.eq_ignore_ascii_case(&lang) => format!("{lang}  ·  {title}"),
        (Some(lang), _) => lang,
        (None, Some(title)) => title.to_string(),
        (None, None) => format!("Track {}", track.id),
    }
}

/// What a track is made of, for the right of its row: codec and, for
/// sound, its channels.
pub fn track_codec(track: &TrackInfo) -> String {
    match (track.kind, track.channels) {
        (TrackKind::Audio, Some(count)) => format!("{} {}", codec_name(&track.codec), channels_name(count)),
        _ => codec_name(&track.codec),
    }
}

/// A row of the picker.
#[derive(Clone, PartialEq, Debug)]
enum Row {
    Audio(i64),
    SubOff,
    Sub(i64),
    /// A subtitle file beside the video, not loaded yet.
    External(PathBuf),
}

/// The track picker over a video.
pub struct TrackPicker {
    tracks: Vec<TrackInfo>,
    rows: Vec<Row>,
    cursor: usize,
    /// When the list is to be read again: after a file is loaded into it.
    reread: Option<Instant>,
    drawn: Vec<(usize, [i32; 4])>,
    place: [i32; 4],
}

/// The tracks and timing kept for a video: its audio and subtitle tracks
/// by the player's numbers (no subtitles is `Some(None)`), and how far
/// each is moved, in seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackChoice {
    pub aid: Option<i64>,
    pub sid: Option<Option<i64>>,
    pub sub_delay: f64,
    pub audio_delay: f64,
}

/// Where the choices are kept: a line each, audio, subtitles, the two
/// delays and the video's path, by tabs.
fn memory_path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("tracks"))
}

fn read_memory() -> HashMap<PathBuf, TrackChoice> {
    let text = memory_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(5, '\t');
            let aid = parts.next()?.parse().ok();
            let sid = match parts.next()? {
                "no" => Some(None),
                number => number.parse().ok().map(Some),
            };
            let (sub_delay, audio_delay) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
            Some((PathBuf::from(parts.next()?), TrackChoice { aid, sid, sub_delay, audio_delay }))
        })
        .collect()
}

fn write_memory(memory: &HashMap<PathBuf, TrackChoice>) {
    let Some(path) = memory_path() else { return };
    let mut lines: Vec<String> = memory
        .iter()
        .map(|(video, choice)| {
            let aid = choice.aid.map_or("-".to_string(), |id| id.to_string());
            let sid = match choice.sid {
                None => "-".to_string(),
                Some(None) => "no".to_string(),
                Some(Some(id)) => id.to_string(),
            };
            format!("{aid}\t{sid}\t{:.3}\t{:.3}\t{}", choice.sub_delay, choice.audio_delay, video.display())
        })
        .collect();
    lines.sort();
    let _ = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, lines.join("\n") + "\n"));
}

/// The languages picked last, audio and subtitles, preferred for a video
/// with nothing kept of its own.
fn prefs_path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("track-languages"))
}

pub fn read_prefs() -> (Option<String>, Option<String>) {
    let text = prefs_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    let get = |key: &str| text.lines().find_map(|line| line.strip_prefix(key)).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    (get("audio="), get("subtitles="))
}

fn write_prefs(audio: Option<&str>, subtitles: Option<&str>) {
    let Some(path) = prefs_path() else { return };
    let text = format!("audio={}\nsubtitles={}\n", audio.unwrap_or_default(), subtitles.unwrap_or_default());
    let _ = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text));
}

/// Subtitle files beside a video that are named after it: `concert.srt`,
/// `concert.en.srt`, `concert.English.ass`.
pub fn subtitle_files(video: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (video.parent(), video.file_stem().map(|stem| stem.to_string_lossy().into_owned())) else { return Vec::new() };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let ext = path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase);
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            matches!(ext.as_deref(), Some("srt" | "ass" | "ssa" | "vtt" | "sub")) && name.starts_with(&stem) && name[stem.len()..].starts_with('.')
        })
        .collect();
    found.sort();
    found
}

impl App {
    /// Ctrl+J: the picker of the video's tracks, over it.
    pub(super) fn open_track_picker(&mut self) {
        let Some(player) = self.player.as_ref().filter(|_| self.video.as_ref().is_some_and(|video| video.showing)) else {
            return self.toast("No video here");
        };
        let tracks = track_list(player);
        let mut picker = TrackPicker { tracks, rows: Vec::new(), cursor: 0, reread: None, drawn: Vec::new(), place: [0; 4] };
        self.fill_rows(&mut picker);
        self.menu = None;
        self.track_picker = Some(picker);
        self.redraw();
    }

    /// The rows from the tracks: the audio, then no subtitles, the
    /// subtitles, and the subtitle files beside the video not loaded yet.
    /// The cursor starts on the audio playing.
    fn fill_rows(&self, picker: &mut TrackPicker) {
        let tracks = &picker.tracks;
        let mut rows: Vec<Row> = tracks.iter().filter(|t| t.kind == TrackKind::Audio).map(|t| Row::Audio(t.id)).collect();
        rows.push(Row::SubOff);
        rows.extend(tracks.iter().filter(|t| t.kind == TrackKind::Sub).map(|t| Row::Sub(t.id)));
        if let Some(video) = &self.video {
            let loaded: Vec<String> = tracks.iter().filter(|t| t.external).filter_map(|t| t.title.clone()).collect();
            for file in subtitle_files(&self.files[video.id]) {
                let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                if !loaded.iter().any(|title| *title == name) {
                    rows.push(Row::External(file));
                }
            }
        }
        let playing = rows.iter().position(|row| matches!(row, Row::Audio(id) if tracks.iter().any(|t| t.kind == TrackKind::Audio && t.id == *id && t.selected)));
        picker.cursor = picker.cursor.min(rows.len().saturating_sub(1));
        if picker.rows.is_empty() {
            picker.cursor = playing.unwrap_or(0);
        }
        picker.rows = rows;
    }

    /// The keys of the picker. Whether it took the key.
    pub(super) fn picker_key(&mut self, key: &Key<&str>) -> bool {
        let shift = self.modifiers.shift_key();
        let Some(picker) = &mut self.track_picker else { return false };
        let last = picker.rows.len().saturating_sub(1);
        match key {
            Key::Named(NamedKey::Escape) => self.track_picker = None,
            Key::Named(NamedKey::ArrowUp) if shift => return self.shift_delay(0.1),
            Key::Named(NamedKey::ArrowDown) if shift => return self.shift_delay(-0.1),
            Key::Named(NamedKey::ArrowUp) => picker.cursor = picker.cursor.saturating_sub(1),
            Key::Named(NamedKey::ArrowDown) => picker.cursor = (picker.cursor + 1).min(last),
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                let at = picker.cursor;
                self.pick_row(at);
            }
            Key::Character("j") if self.modifiers.control_key() => self.track_picker = None,
            Key::Character("e") => {
                self.open_track_editor();
                return true;
            }
            _ => return false,
        }
        self.redraw();
        true
    }

    /// A click on a row picks it; one outside the picker closes it.
    pub(super) fn picker_click(&mut self, x: f64, y: f64) -> bool {
        let Some(picker) = &self.track_picker else { return false };
        let on = |[left, top, w, h]: [i32; 4]| (left..left + w).contains(&(x as i32)) && (top..top + h).contains(&(y as i32));
        if let Some(&(row, _)) = picker.drawn.iter().find(|(_, place)| on(*place)) {
            if let Some(picker) = &mut self.track_picker {
                picker.cursor = row;
            }
            self.pick_row(row);
        } else if !on(picker.place) {
            self.track_picker = None;
        }
        self.redraw();
        true
    }

    /// Puts the track of a row on, and keeps it as this video's choice;
    /// its language is preferred for videos with no choice of their own.
    fn pick_row(&mut self, at: usize) {
        let Some(picker) = &mut self.track_picker else { return };
        let Some(row) = picker.rows.get(at).cloned() else { return };
        let Some(player) = &self.player else { return };
        let lang_of = |id: i64, kind: TrackKind| picker.tracks.iter().find(|t| t.kind == kind && t.id == id).and_then(|t| t.lang.clone());
        let (mut audio_lang, mut sub_lang) = read_prefs();
        match &row {
            Row::Audio(id) => {
                player.set("aid", &id.to_string());
                audio_lang = lang_of(*id, TrackKind::Audio).or(audio_lang);
                picker.tracks.iter_mut().filter(|t| t.kind == TrackKind::Audio).for_each(|t| t.selected = t.id == *id);
            }
            Row::Sub(id) => {
                player.set("sid", &id.to_string());
                sub_lang = lang_of(*id, TrackKind::Sub).or(sub_lang);
                picker.tracks.iter_mut().filter(|t| t.kind == TrackKind::Sub).for_each(|t| t.selected = t.id == *id);
                self.subtitles = true;
            }
            Row::SubOff => {
                player.set("sid", "no");
                picker.tracks.iter_mut().filter(|t| t.kind == TrackKind::Sub).for_each(|t| t.selected = false);
                self.subtitles = false;
            }
            Row::External(file) => {
                player.command(&[b"sub-add", file.as_os_str().as_encoded_bytes(), b"select"]);
                picker.reread = Some(Instant::now() + Duration::from_millis(250));
                self.anim_due = Some(Instant::now() + Duration::from_millis(260));
                self.subtitles = true;
            }
        }
        write_prefs(audio_lang.as_deref(), sub_lang.as_deref());
        for player in self.players() {
            player.set("alang", audio_lang.as_deref().unwrap_or_default());
            player.set("slang", sub_lang.as_deref().unwrap_or_default());
        }
        self.apply_subtitles();
        self.remember_tracks();
    }

    /// Shift+↑↓ in the picker: the timing of the subtitles, or of the
    /// sound when the cursor is on an audio track, a tenth of a second.
    fn shift_delay(&mut self, by: f64) -> bool {
        let Some(picker) = &self.track_picker else { return false };
        let audio = matches!(picker.rows.get(picker.cursor), Some(Row::Audio(_)));
        let Some(player) = &self.player else { return true };
        let property = if audio { "audio-delay" } else { "sub-delay" };
        let now = player.number(property).unwrap_or(0.0) + by;
        let now = (now * 10.0).round() / 10.0;
        player.set(property, &format!("{now:.3}"));
        self.toast(format!("{}  {now:+.1} s", if audio { "Sound" } else { "Subtitles" }));
        self.remember_tracks();
        true
    }

    /// Keeps the tracks and timing of the video on view for the next time
    /// it is opened.
    pub(super) fn remember_tracks(&mut self) {
        // Picked in the picker, as it has them: the player, told just now,
        // may not have them on yet.
        let picked = |kind| self.track_picker.as_ref().and_then(|picker| picker.tracks.iter().find(|t| t.kind == kind && t.selected)).map(|t| t.id);
        let (aid, sid) = (picked(TrackKind::Audio), picked(TrackKind::Sub));
        self.remember_tracks_as(aid, sid);
    }

    /// As `remember_tracks`, with the audio and subtitle tracks just picked
    /// where they are known: asked of the player straight after it is told
    /// of them, it says what it had.
    pub(super) fn remember_tracks_as(&mut self, aid: Option<i64>, sid: Option<i64>) {
        let (Some(player), Some(video)) = (&self.player, &self.video) else { return };
        let path = std::path::absolute(&self.files[video.id]).unwrap_or_else(|_| self.files[video.id].clone());
        let number = |property: &str| player.text(property).and_then(|value| value.parse::<i64>().ok());
        let sid = match self.subtitles {
            false => Some(None),
            true => sid.or_else(|| number("sid")).map(Some),
        };
        let choice = TrackChoice {
            aid: aid.or_else(|| number("aid")),
            sid,
            sub_delay: player.number("sub-delay").unwrap_or(0.0),
            audio_delay: player.number("audio-delay").unwrap_or(0.0),
        };
        let memory = self.track_memory.get_or_insert_with(read_memory);
        memory.insert(path, choice);
        write_memory(memory);
        // The info overlay's words for them: from the picker, which knows
        // what was just picked; otherwise said again as the player tells of
        // the change. How many tracks there are stays: forgotten, the bar's
        // buttons for them went until the player was asked again, which
        // paused could be a long while.
        let words = self.track_picker.as_ref().map(|picker| playing_words(&picker.tracks, self.subtitles));
        if let (Some(video), Some(words)) = (&mut self.video, words) {
            video.track_words = words;
        }
    }

    /// The tracks and timing kept for a video, as a player is told them
    /// before it opens it: `Player::choose_next`'s arguments.
    pub(super) fn kept_tracks(&mut self, id: usize) -> Option<(String, String, f64, f64)> {
        let path = std::path::absolute(&self.files[id]).unwrap_or_else(|_| self.files[id].clone());
        let choice = self.track_memory.get_or_insert_with(read_memory).get(&path).copied()?;
        let aid = choice.aid.map_or("auto".to_string(), |id| id.to_string());
        let sid = match choice.sid {
            None => "auto".to_string(),
            Some(None) => "no".to_string(),
            Some(Some(id)) => id.to_string(),
        };
        Some((aid, sid, choice.sub_delay, choice.audio_delay))
    }

    /// The preferred languages, told to a player as it starts.
    pub(super) fn apply_track_prefs(&self) {
        let (audio, subtitles) = read_prefs();
        for player in self.players() {
            player.set("alang", audio.as_deref().unwrap_or_default());
            player.set("slang", subtitles.as_deref().unwrap_or_default());
        }
    }

    /// Draws the picker, at the bottom right above the seek bar.
    pub(super) fn draw_track_picker(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        // Read again once a file loaded into the player shows in its list.
        let reread = self.track_picker.as_ref().and_then(|picker| picker.reread).is_some_and(|at| at <= Instant::now());
        if reread {
            if let (Some(player), Some(mut picker)) = (&self.player, self.track_picker.take()) {
                picker.tracks = track_list(player);
                picker.reread = None;
                self.fill_rows(&mut picker);
                self.track_picker = Some(picker);
            }
        }
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        let Some(picker) = &mut self.track_picker else { return };
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let row_h = line + pad * 2 / 3;
        let heading_h = line + pad / 2;
        let width = (px as i32 * 30).min(canvas.width - 2 * pad);
        let audio_rows = picker.rows.iter().filter(|row| matches!(row, Row::Audio(_))).count() as i32;
        let other_rows = picker.rows.len() as i32 - audio_rows;
        // The keys along the foot, on as many lines as they take.
        let hints = wrap_hints(text, "↑↓ pick  ·  Enter use  ·  Shift+↑↓ timing ±0.1 s  ·  e edit tracks  ·  Esc close", small, width - 2 * pad);
        let height = pad + heading_h + audio_rows.max(1) * row_h + pad / 2 + 1 + pad / 2 + heading_h + other_rows * row_h + pad / 2 + small_line * hints.len() as i32 + pad;
        let (left, top) = (canvas.width - width - pad, (canvas.height - height - pad * 6).max(pad));
        picker.place = [left, top, width, height];
        panel(canvas, left, top, width, height, pad * 2 / 3);
        let inner = left + pad;
        let inner_w = width - 2 * pad;
        let mut y = top + pad;
        picker.drawn.clear();
        let heading = |canvas: &mut Canvas, text: &mut Text, y: i32, title: &str, keys: &str| {
            text.draw_strong(canvas, inner, y, px, TEXT_COLOR, title);
            let kw = text.width(keys, small);
            text.draw(canvas, inner + inner_w - kw, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, keys);
        };
        heading(canvas, text, y, "Audio", "J cycles");
        y += heading_h;
        if audio_rows == 0 {
            text.draw(canvas, inner, y + (row_h - line) / 2, px, DIM_TEXT_COLOR, "No sound");
            y += row_h;
        }
        let mut subs_heading = false;
        for (at, row) in picker.rows.iter().enumerate() {
            if !matches!(row, Row::Audio(_)) && !subs_heading {
                subs_heading = true;
                y += pad / 2;
                canvas.fill(inner, y, inner_w, 1, MENU_EDGE_COLOR);
                y += 1 + pad / 2;
                heading(canvas, text, y, "Subtitles", "j cycles");
                y += heading_h;
            }
            let track = match row {
                Row::Audio(id) => picker.tracks.iter().find(|t| t.kind == TrackKind::Audio && t.id == *id),
                Row::Sub(id) => picker.tracks.iter().find(|t| t.kind == TrackKind::Sub && t.id == *id),
                _ => None,
            };
            let on = match row {
                Row::SubOff => !picker.tracks.iter().any(|t| t.kind == TrackKind::Sub && t.selected) || !self.subtitles,
                Row::Sub(_) => track.is_some_and(|t| t.selected) && self.subtitles,
                _ => track.is_some_and(|t| t.selected),
            };
            let place = [inner - pad / 3, y, inner_w + pad * 2 / 3, row_h];
            let hovered = in_place(mouse, place) || at == picker.cursor;
            let fill = if on { Some(accent()) } else if hovered { Some(PLACEHOLDER_COLOR) } else { None };
            if let Some(color) = fill {
                canvas.round_fill(place[0], place[1], place[2], place[3], pad / 2, color, 256);
            }
            if at == picker.cursor && !on {
                canvas.round_outline(place[0], place[1], place[2], place[3], pad / 2, 1, accent());
            }
            let fg = if on { on_accent() } else { TEXT_COLOR };
            let dim = if on { on_accent() } else { DIM_TEXT_COLOR };
            // A dot, filled for the one on.
            let dot = (line / 3).max(6);
            let (dx, dy) = (inner + pad / 6, y + (row_h - dot) / 2);
            match on {
                true => canvas.round_fill(dx, dy, dot, dot, dot / 2, fg, 256),
                false => canvas.round_outline(dx, dy, dot, dot, dot / 2, 1, dim),
            }
            let name_x = dx + dot + pad * 2 / 3;
            let (name, right, flags): (String, String, Vec<&str>) = match row {
                Row::SubOff => ("Off".to_string(), String::new(), Vec::new()),
                Row::External(file) => (format!("{}  ·  beside the video", file.file_name().unwrap_or_default().to_string_lossy()), "load".to_string(), Vec::new()),
                _ => match track {
                    Some(t) => {
                        let mut flags = Vec::new();
                        if t.default {
                            flags.push("default");
                        }
                        if t.forced {
                            flags.push("forced");
                        }
                        if t.external {
                            flags.push("file");
                        }
                        (track_label(t), track_codec(t), flags)
                    }
                    None => (String::new(), String::new(), Vec::new()),
                },
            };
            let right_w = text.width(&right, small);
            text.draw(canvas, inner + inner_w - right_w, y + (row_h - small_line) / 2, small, dim, &right);
            let mut x = name_x;
            let name_room = inner + inner_w - right_w - pad - name_x;
            let name = clipped(text, &name, name_room, px);
            text.draw(canvas, x, y + (row_h - line) / 2, px, if matches!(row, Row::External(_)) && !on { DIM_TEXT_COLOR } else { fg }, &name);
            x += text.width(&name, px) + pad / 2;
            let tiny = (px * 4 / 5).max(10);
            for flag in flags {
                let w = text.width(flag, tiny) + pad / 2;
                if x + w > inner + inner_w - right_w - pad / 2 {
                    break;
                }
                let bh = Text::line_height(tiny) + pad / 6;
                let color = if flag == "forced" { 0x8E5BD6 } else { badge_color(flag) };
                canvas.round_fill(x, y + (row_h - bh) / 2, w, bh, bh / 3, if on { 0x000000 } else { color }, if on { 60 } else { 230 });
                text.draw(canvas, x + pad / 4, y + (row_h - bh) / 2 + pad / 12, tiny, fg, flag);
                x += w + pad / 3;
            }
            picker.drawn.push((at, place));
            y += row_h;
        }
        y += pad / 2;
        for hint in &hints {
            text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, hint);
            y += small_line;
        }
    }
}

fn in_place((x, y): (i32, i32), [left, top, w, h]: [i32; 4]) -> bool {
    (left..left + w).contains(&x) && (top..top + h).contains(&y)
}

/// The audio and subtitles playing, in words: "Korean · Opus 2.0  ·
/// CC Korean". None for a video with one track and no subtitles.
pub fn playing_words(tracks: &[TrackInfo], subtitles: bool) -> Option<String> {
    let audio_count = tracks.iter().filter(|t| t.kind == TrackKind::Audio).count();
    let sub_count = tracks.iter().filter(|t| t.kind == TrackKind::Sub).count();
    if audio_count < 2 && sub_count == 0 {
        return None;
    }
    let audio = tracks.iter().find(|t| t.kind == TrackKind::Audio && t.selected);
    let sub = tracks.iter().find(|t| t.kind == TrackKind::Sub && t.selected).filter(|_| subtitles);
    let audio = audio.map_or("No sound".to_string(), |t| format!("{}  {}", track_label(t), track_codec(t)));
    let sub = sub.map_or("CC off".to_string(), |t| format!("CC {}", track_label(t)));
    Some(format!("{audio}   ·   {sub}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_read_from_the_players_list() {
        // As mpv gives it, trimmed.
        let list = r#"[{"id":1,"type":"video","default":true,"external":false,"selected":true,"codec":"hevc"},{"id":1,"type":"audio","title":"Surround","lang":"eng","default":true,"selected":true,"codec":"aac","demux-channel-count":6},{"id":2,"type":"sub","lang":"","forced":true,"external":true,"selected":false,"codec":"subrip"}]"#;
        let tracks = parse_tracks(list);
        assert_eq!(tracks.len(), 3);
        assert_eq!((tracks[1].kind, tracks[1].id, tracks[1].channels, tracks[1].title.as_deref(), tracks[1].lang.as_deref()), (TrackKind::Audio, 1, Some(6), Some("Surround"), Some("eng")));
        assert!(tracks[1].selected && tracks[1].default);
        assert_eq!((tracks[2].kind, tracks[2].lang.as_deref(), tracks[2].forced, tracks[2].external), (TrackKind::Sub, None, true, true), "an empty language is none");
        assert!(parse_tracks("").is_empty());
    }
}
