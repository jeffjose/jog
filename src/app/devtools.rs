//! Inspect as jog's developer tools: one panel to open, do everything in,
//! and close. Its tabs bring together what jog has elsewhere (the file
//! list, the tracks, the HDR charts, Convert and Compare, the debug
//! figures) and add what only a power user wants (Issues); a transport bar
//! along its foot plays and steps whatever tab is up. Everything stays
//! where it was as well.

use super::glossary::{Role, Tip};
use super::inspect::{Card, Hit};
use super::*;

/// The tabs, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Pane {
    Files,
    #[default]
    Overview,
    Tracks,
    Color,
    Timeline,
    Convert,
    Compare,
    Structure,
    Metadata,
    Performance,
    Issues,
    Console,
}

impl Pane {
    pub const ALL: [Pane; 12] = [
        Pane::Overview,
        Pane::Tracks,
        Pane::Color,
        Pane::Timeline,
        Pane::Structure,
        Pane::Metadata,
        Pane::Issues,
        Pane::Convert,
        Pane::Compare,
        Pane::Files,
        Pane::Console,
        Pane::Performance,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Pane::Files => "Playlist",
            Pane::Overview => "Overview",
            Pane::Tracks => "Tracks",
            Pane::Color => "Color",
            Pane::Timeline => "Timeline",
            Pane::Convert => "Convert",
            Pane::Compare => "Compare",
            Pane::Structure => "Structure",
            Pane::Metadata => "Metadata",
            Pane::Performance => "Performance",
            Pane::Issues => "Issues",
            Pane::Console => "Console",
        }
    }

    /// Whether the tab has anything for a picture, or for a video.
    pub fn applies(self, picture: bool) -> bool {
        !(picture && matches!(self, Pane::Tracks | Pane::Timeline | Pane::Structure))
    }

    /// The tabs there are for a picture, or for a video: those about the
    /// file, then the tools. What is about jog itself (the playlist, the
    /// console, how it performs) is in the drawer.
    pub fn shown(picture: bool) -> Vec<Pane> {
        Pane::ALL.into_iter().filter(|pane| pane.applies(picture) && pane.drawer().is_none()).collect()
    }

    /// For a pane that now lives in the drawer, which of its tabs.
    pub fn drawer(self) -> Option<DrawerPane> {
        match self {
            Pane::Files => Some(DrawerPane::Playlist),
            Pane::Console => Some(DrawerPane::Console),
            Pane::Performance => Some(DrawerPane::Performance),
            _ => None,
        }
    }

    /// The group a tab is in, for the label before the first of it.
    pub fn group(self) -> &'static str {
        match self {
            Pane::Convert | Pane::Compare => "Tools",
            Pane::Files | Pane::Console | Pane::Performance => "jog",
            _ => "File",
        }
    }

    /// Which of the inspector's first tabs this one's cards and charts
    /// are: a video's Summary, Streams, HDR, Structure, Tags; a picture's
    /// Summary, EXIF, Color, XMP. None for one drawn afresh here.
    pub fn legacy(self, picture: bool) -> Option<usize> {
        match (self, picture) {
            (Pane::Overview, _) => Some(0),
            (Pane::Tracks, false) => Some(1),
            (Pane::Color, _) => Some(2),
            (Pane::Structure, false) => Some(3),
            (Pane::Metadata, false) => Some(4),
            (Pane::Metadata, true) => Some(1),
            _ => None,
        }
    }

    /// Its name in the panels file, to open on it next time.
    pub fn from_name(name: &str) -> Option<Pane> {
        Pane::ALL.into_iter().find(|pane| pane.name().eq_ignore_ascii_case(name))
    }
}

/// The drawer's tabs: what is about jog rather than the file on view.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DrawerPane {
    #[default]
    Playlist,
    Console,
    Performance,
}

impl DrawerPane {
    pub const ALL: [DrawerPane; 3] = [DrawerPane::Playlist, DrawerPane::Console, DrawerPane::Performance];

    pub fn name(self) -> &'static str {
        match self {
            DrawerPane::Playlist => "Playlist",
            DrawerPane::Console => "Console",
            DrawerPane::Performance => "Performance",
        }
    }

    pub fn from_name(name: &str) -> Option<DrawerPane> {
        DrawerPane::ALL.into_iter().find(|pane| pane.name().eq_ignore_ascii_case(name))
    }
}

/// How bad an issue is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Severity {
    Note,
    Warning,
}

impl App {
    /// What is off about a file, as a careful look at it would find.
    /// Each with how to put it right.
    pub(super) fn issues(&self, found: &super::inspect::Inspection) -> Vec<(Severity, String, &'static str)> {
        let mut issues = Vec::new();
        if let Some(video) = found.video() {
            let missing = |key: &str| video[key].as_str().is_none_or(|value| value == "unknown");
            if missing("color_primaries") || missing("color_transfer") || missing("color_space") {
                issues.push((Severity::Warning, "Color not tagged: players guess its primaries, curve or matrix, and may guess wrong".to_string(), "Tag it as it was made: ffmpeg -c copy with -color_primaries, -color_trc and -colorspace"));
            }
            let (rate, average) = (video["r_frame_rate"].as_str().unwrap_or_default(), video["avg_frame_rate"].as_str().unwrap_or_default());
            if !rate.is_empty() && !average.is_empty() && rate != average && average != "0/0" {
                issues.push((Severity::Note, format!("Variable frame rate: {rate} at most, {average} on average"), "Fine to watch; for an editor that wants one rate, convert with ffmpeg -fps_mode cfr"));
            }
            let transfer = video["color_transfer"].as_str().unwrap_or_default();
            let hdr = found.hdr.as_ref();
            if transfer == "smpte2084" && hdr.is_some_and(|hdr| hdr.mastering.is_none()) && found.side("Mastering display metadata").is_none() {
                issues.push((Severity::Note, "PQ without mastering display metadata: tone mapping has to guess the peak".to_string(), "Set when encoding: x265 master-display, SVT-AV1 mastering-display"));
            }
            if transfer == "smpte2084" && hdr.is_some_and(|hdr| hdr.light.is_none()) && found.side("Content light level metadata").is_none() {
                issues.push((Severity::Note, "PQ without MaxCLL and MaxFALL: a screen cannot tell how bright the brightest frame is".to_string(), "Set when encoding: x265 max-cll, SVT-AV1 content-light"));
            }
            if transfer != "smpte2084" && transfer != "arib-std-b67" && hdr.is_some_and(|hdr| !hdr.hdr10plus.is_empty() || !hdr.agtm.is_empty()) {
                issues.push((Severity::Warning, "HDR metadata in a video not coded as HDR: players ignore it, or show it wrong".to_string(), "Its transfer tag is wrong: set it to PQ (ffmpeg -color_trc smpte2084 -c copy)"));
            }
        }
        // An MP4 whose index is at its end has to be read through before
        // it can play from a server.
        if let Ok(nodes) = found.tree.as_ref() {
            let order: Vec<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
            if let (Some(moov), Some(mdat)) = (order.iter().position(|name| *name == "moov"), order.iter().position(|name| *name == "mdat")) {
                if moov > mdat {
                    issues.push((Severity::Note, "Index (moov) after the media: not quick to start over a network".to_string(), "Move it first, losing nothing: ffmpeg -c copy -movflags +faststart; jog's Convert writes it first"));
                }
            }
        }
        if let Some(picture) = found.picture.as_ref() {
            if picture.icc.is_none() && picture.color.cicp.is_none() && picture.color.png_gamma.is_none() {
                issues.push((Severity::Note, "No color profile: shown as sRGB".to_string(), "Nothing to do for sRGB; a wide-gamut picture needs its profile embedded"));
            }
        }
        issues
    }


    /// The Performance tab's cards: the debug overlay's figures.
    pub(super) fn performance_cards(&mut self) -> Vec<Card> {
        let mut cards: Vec<Card> = self
            .debug_groups()
            .into_iter()
            .map(|(title, rows)| {
                let mut card = Card::new(title);
                for (name, value) in rows {
                    card.row(name, Some(value));
                }
                card
            })
            .collect();
        // The decoder for the video on view: the GPU's, which kind, or the
        // CPU's, and why.
        let mut playback = Card::new("Playback");
        let playing = self.video.as_ref().map(|video| &self.files[video.id]);
        let crashed = playing.and_then(|path| crate::mpv::crashed_on(path));
        let current = self.player.as_ref().and_then(|player| player.text("hwdec-current")).filter(|decoder| !decoder.is_empty() && decoder != "no");
        let decoder = match (crate::mpv::HWDEC.load(atomic::Ordering::Relaxed), crashed, current, playing) {
            (false, ..) => "CPU · --no-hwdec".to_string(),
            (_, Some(_), ..) => "CPU · it crashed the GPU's decoder".to_string(),
            (_, _, Some(decoder), _) => format!("GPU · {decoder}"),
            (_, _, None, Some(_)) => "CPU · no GPU decoder for it".to_string(),
            (_, _, None, None) => "GPU where it can, else CPU".to_string(),
        };
        playback.row("Decoding", Some(decoder));
        let crashes = crate::mpv::crashes_this_run();
        if crashes > 0 {
            playback.row("Crashed", Some(format!("{crashes} {} this run · jog went on", if crashes == 1 { "player" } else { "players" })));
        }
        cards.insert(0, playback);
        // Each player process, which it is, and what it takes.
        let mut players = Card::new("Players");
        let pid = |player: Option<&Player>| player.map(Player::pid);
        let (main, spare, preview) = (pid(self.player.as_ref()), pid(self.spare.as_ref().map(|spare| &spare.player)), pid(self.preview.as_ref()));
        for usage in crate::mpv::usage() {
            let name = match usage.pid {
                pid if Some(pid) == main => "Playing",
                pid if Some(pid) == spare => "Next, held",
                pid if Some(pid) == preview => "Seek preview",
                _ => match usage.role {
                    crate::mpv::Role::Preview => "Scenes, grid",
                    _ => "Step back",
                },
            };
            let cpu = usage.cpu.map_or("…".to_string(), |cpu| format!("{:.1} {}", cpu / 100.0, if (cpu / 100.0 - 1.0).abs() < 0.05 { "core" } else { "cores" }));
            let memory = usage.memory.map(|bytes| format!(" · {}", meta::human_size(bytes))).unwrap_or_default();
            let up = match usage.age.as_secs() {
                seconds @ 0..=59 => format!("{seconds} s"),
                seconds @ 60..=3599 => format!("{} min", seconds / 60),
                seconds => format!("{} h", seconds / 3600),
            };
            players.row(name, Some(format!("{cpu}{memory} · up {up} · pid {}", usage.pid)));
        }
        if !players.rows.is_empty() {
            cards.insert(1, players);
        }
        cards
    }

    /// The files that crashed a player, each to be forgotten and tried on
    /// the GPU again.
    pub(super) fn crash_actions(&self) -> Vec<(String, Vec<Action>)> {
        // Only those of this run's list, not every file that ever crashed one.
        let ours = self.ours(crate::mpv::all_crashes().into_iter().map(|(path, _)| path).collect());
        let crashes: Vec<_> = crate::mpv::all_crashes().into_iter().filter(|(path, _)| ours.contains(path)).collect();
        if crashes.is_empty() {
            return Vec::new();
        }
        let rows = crashes
            .into_iter()
            .map(|(path, crash)| {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let what = match crash {
                    crate::mpv::Crash::Gpu => "on the CPU",
                    crate::mpv::Crash::Software => "not played",
                };
                Action { words: format!("{name}  ·  {what}  ·  forget"), hit: Hit::Forget(path) }
            })
            .collect();
        vec![("Crashed the player".to_string(), rows)]
    }
}

/// An action row: a pill the width of the panel, its words, what it runs.
pub(super) struct Action {
    pub words: String,
    pub hit: Hit,
}

impl App {
    /// The Convert tab's actions, for a picture or a video.
    pub(super) fn convert_actions(&self, picture: bool) -> Vec<(String, Vec<Action>)> {
        use crate::convert::Format;
        use crate::transcode::{Codec, Container, Target};
        let run = |words: &str, command: Command| Action { words: words.to_string(), hit: Hit::Run(command) };
        if picture {
            vec![
                ("Convert".to_string(), vec![run("Open the Convert window…   K", Command::ConvertPanel)]),
                (
                    "At once, with the settings last used".to_string(),
                    [("JPEG XL", Format::Jxl), ("AVIF", Format::Avif), ("JPEG", Format::Jpeg), ("PNG", Format::Png), ("WebP", Format::WebP)]
                        .into_iter()
                        .map(|(name, format)| run(name, Command::Convert(format)))
                        .collect(),
                ),
                ("Save".to_string(), vec![run("Save a copy   S", Command::Save)]),
            ]
        } else {
            let video = |words: &str, codec, container| run(words, Command::ConvertVideo(Target::new(codec, container)));
            vec![
                (
                    "Convert the video".to_string(),
                    vec![
                        video("MP4  ·  H.264", Codec::H264, Container::Mp4),
                        video("MP4  ·  HEVC", Codec::Hevc, Container::Mp4),
                        video("MKV  ·  AV1", Codec::Av1, Container::Mkv),
                        video("WebM  ·  AV1", Codec::Av1, Container::WebM),
                        run("Cancel the encode", Command::CancelEncode),
                    ],
                ),
                ("Save".to_string(), vec![run("Save this frame   S", Command::Save), run("Edit tracks…", Command::EditTracks)]),
            ]
        }
    }

    /// Draws groups of action rows on `sub` from `y`; returns where they end.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_actions(&self, sub: &mut Canvas, text: &mut Text, px: u32, groups: &[(String, Vec<Action>)], inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        // A section, as the facts are: its name and a hairline, then a row
        // for each, quiet until the pointer is on it, a '›' at its end.
        let row_h = line + pad / 3;
        let mut y = y;
        for (title, actions) in groups {
            y = section_head(sub, text, px, x, y, inner_w, title);
            for action in actions {
                let place = [x + offset.0, y + offset.1, inner_w, row_h];
                let over = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
                if over {
                    sub.round_fill(x - pad / 3, y, inner_w + pad * 2 / 3, row_h, pad / 3, 0x2C2D35, 256);
                }
                let words = clipped(text, &action.words, inner_w - pad * 2, px);
                text.draw(sub, x, y + (row_h - line) / 2, px, TEXT_COLOR, &words);
                let more = "›";
                let more_w = text.width(more, px);
                text.draw(sub, x + inner_w - more_w, y + (row_h - line) / 2, px, if over { accent_soft() } else { 0x5C5D66 }, more);
                drawn.push((action.hit.clone(), place));
                y += row_h;
            }
            y += pad;
        }
        y
    }

    /// The Timeline tab: chapters and sections kept, each a row to click
    /// to go there.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_timeline(&mut self, sub: &mut Canvas, text: &mut Text, px: u32, inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let entries = self.entries();
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        let row_h = line + pad / 3;
        let mut y = section_head(sub, text, px, x, y, inner_w, &format!("Chapters and sections · {}", entries.len()));
        if entries.is_empty() {
            text.draw(sub, x, y + (row_h - line) / 2, px, DIM_TEXT_COLOR, "None · mark one with Shift+A and Shift+B");
            y += row_h;
        }
        let when_w = px as i32 * 9;
        for entry in &entries {
            let place = [x + offset.0, y + offset.1, inner_w, row_h];
            if (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1) {
                sub.round_fill(x - pad / 3, y, inner_w + pad * 2 / 3, row_h, pad / 3, 0x2C2D35, 256);
            }
            text.draw(sub, x, y + (row_h - line) / 2, px, DIM_TEXT_COLOR, &format!("{} – {}", clock(entry.start), clock(entry.end)));
            let kind = if entry.kept { "section" } else { "chapter" };
            let title = entry.title.clone().unwrap_or_else(|| kind.to_string());
            let title = clipped(text, &title, inner_w - when_w, px);
            text.draw(sub, x + when_w, y + (row_h - line) / 2, px, if entry.kept { accent_soft() } else { TEXT_COLOR }, &title);
            drawn.push((Hit::Seek(entry.start), place));
            y += row_h;
        }
        y + pad
    }

    /// The transport bar along the inspector's foot, drawn every frame:
    /// a video's file, seek and play buttons with its time on a thin bar,
    /// its loop and sound; a picture's previous and next. Returns its hits.
    pub(super) fn draw_transport(&self, canvas: &mut Canvas, text: &mut Text, px: u32, [left, top, width, height]: [i32; 4], picture: bool) -> Vec<(Hit, [i32; 4])> {
        let pad = px as i32;
        let side = pad * 3 / 2;
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        canvas.fill(left, top, width, height, 0x121317);
        canvas.fill(left, top, width, 1, MENU_EDGE_COLOR);
        let y = top + (height - side) / 2;
        let mut x = left + pad / 2;
        let mut hits = Vec::new();
        let paused = self.player.as_ref().is_none_or(|player| player.flag("pause") != Some(false));
        let mut buttons: Vec<(Command, &str, bool)> = vec![(Command::Previous, commands::icons::SKIP_BACK, false)];
        if !picture {
            buttons.push((Command::SeekBack, commands::icons::REWIND, false));
            buttons.push((Command::PlayPause, if paused { commands::icons::PLAY } else { commands::icons::PAUSE }, true));
            buttons.push((Command::SeekForward, commands::icons::FAST_FORWARD, false));
        }
        buttons.push((Command::Next, commands::icons::SKIP_FORWARD, false));
        for (command, icon, main) in buttons {
            let place = Button { x, y, width: side, height: side, command };
            let over = (x..x + side).contains(&mouse.0) && (y..y + side).contains(&mouse.1);
            draw::round_button(canvas, text, &place, icon, px, over, main, false);
            hits.push((Hit::Run(command), [x, y, side, side]));
            x += side + pad / 3;
        }
        // At the right: loop and sound, for a video.
        let mut right = left + width - pad / 2;
        if !picture {
            for (command, icon, on) in [
                (Command::ToggleMute, if self.muted { commands::icons::VOLUME_X } else { commands::icons::VOLUME_2 }, self.muted),
                (Command::CycleLoop, if self.looping == Looping::File { commands::icons::REPEAT_1 } else { commands::icons::REPEAT }, self.looping != Looping::Off),
            ] {
                right -= side;
                let place = Button { x: right, y, width: side, height: side, command };
                let over = (right..right + side).contains(&mouse.0) && (y..y + side).contains(&mouse.1);
                draw::round_button(canvas, text, &place, icon, px, over, on, false);
                hits.push((Hit::Run(command), [right, y, side, side]));
                right -= pad / 3;
            }
        }
        // Between them: the time along, and a thin bar to click or drag on.
        let small = px;
        let words = match self.video_time() {
            Some((at, length)) if !picture => format!("{} / {}", clock(at), clock(length)),
            _ => format!("{} / {}", self.pos + 1, self.list.len()),
        };
        let words_w = text.width(&words, small);
        let time_x = right - pad / 2 - words_w;
        text.draw(canvas, time_x, top + (height - Text::line_height(small)) / 2, small, DIM_TEXT_COLOR, &words);
        let (bar_x, bar_w) = (x + pad / 3, time_x - pad - x - pad / 3);
        if bar_w > pad * 2 {
            let bar_h = (pad / 4).max(3);
            let bar_y = top + (height - bar_h) / 2;
            canvas.round_fill(bar_x, bar_y, bar_w, bar_h, bar_h / 2, 0x2C2D35, 256);
            let along = match self.video_time() {
                Some((at, length)) if !picture && length > 0.0 => at / length,
                _ => (self.pos + 1) as f64 / self.list.len().max(1) as f64,
            };
            let done = (f64::from(bar_w) * along.clamp(0.0, 1.0)) as i32;
            canvas.round_fill(bar_x, bar_y, done.max(bar_h), bar_h, bar_h / 2, accent_soft(), 256);
            if !picture {
                hits.push((Hit::Scrub(bar_x, bar_w), [bar_x, top, bar_w, height]));
            }
        }
        hits
    }
}

impl App {
    /// Whether Inspect's Files tab is up where the list is being drawn or
    /// answered: the main window, or Inspect's own if it is out.
    pub(super) fn files_tab_up(&self) -> bool {
        if self.inspector_list_drawing.get() {
            return true;
        }
        let here = (self.drawing_popout == Some(Popped::Inspector)) == self.popped(Popped::Inspector);
        here && self.inspector.is_some() && self.inspect_drawer && self.drawer_pane == DrawerPane::Playlist
    }
}

impl App {
    /// The Tracks tab, under its cards: the audio and subtitles playing,
    /// each a row to click to play that one instead, with what else there
    /// is to do with the tracks.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_track_choices(&self, sub: &mut Canvas, text: &mut Text, px: u32, id: usize, inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let playing = self.video.as_ref().is_some_and(|video| video.id == id && video.showing);
        let tracks = match (&self.player, playing) {
            (Some(player), true) => tracks::track_list(player),
            _ => Vec::new(),
        };
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        let row_h = line + pad / 3;
        let mut y = y;
        // Each kind a section; the one playing marked in the accent, the
        // rest quiet until the pointer is on them.
        let mut group = |sub: &mut Canvas, text: &mut Text, y: &mut i32, title: &str, rows: Vec<(String, bool, Hit)>| {
            if rows.is_empty() {
                return;
            }
            *y = section_head(sub, text, px, x, *y, inner_w, title);
            for (words, on, hit) in rows {
                let place = [x + offset.0, *y + offset.1, inner_w, row_h];
                let over = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
                if over {
                    sub.round_fill(x - pad / 3, *y, inner_w + pad * 2 / 3, row_h, pad / 3, 0x2C2D35, 256);
                }
                let mark = if on { "●" } else { "○" };
                let words = clipped(text, &format!("{mark}  {words}"), inner_w, px);
                text.draw(sub, x, *y + (row_h - line) / 2, px, if on { accent_soft() } else { TEXT_COLOR }, &words);
                drawn.push((hit, place));
                *y += row_h;
            }
            *y += pad;
        };
        if !playing {
            text.draw(sub, x, y, px, DIM_TEXT_COLOR, "Play the video to choose its tracks");
            y += line + pad;
        }
        let audio: Vec<(String, bool, Hit)> = tracks
            .iter()
            .filter(|track| track.kind == tracks::TrackKind::Audio)
            .map(|track| (tracks::track_label(track), track.selected, Hit::Track(true, Some(track.id))))
            .collect();
        group(sub, text, &mut y, "Audio", audio);
        let mut subs: Vec<(String, bool, Hit)> = vec![("Off".to_string(), !tracks.iter().any(|track| track.kind == tracks::TrackKind::Sub && track.selected), Hit::Track(false, None))];
        subs.extend(tracks.iter().filter(|track| track.kind == tracks::TrackKind::Sub).map(|track| (tracks::track_label(track), track.selected, Hit::Track(false, Some(track.id)))));
        if playing {
            group(sub, text, &mut y, "Subtitles", subs);
        }
        let run = |words: &str, command: Command| Action { words: words.to_string(), hit: Hit::Run(command) };
        let actions = vec![(
            "Tracks".to_string(),
            vec![run("Edit tracks…  keep, order, tag, burn in", Command::EditTracks), run("Larger subtitles", Command::SubtitlesLarger), run("Smaller subtitles", Command::SubtitlesSmaller)],
        )];
        self.draw_actions(sub, text, px, &actions, inner_w, y, offset, mouse, drawn)
    }

    /// Plays another audio track, or subtitles, or none, as the track
    /// picker does, and keeps the choice for the video.
    pub(super) fn choose_track(&mut self, audio: bool, id: Option<i64>) {
        let Some(player) = &self.player else { return };
        match (audio, id) {
            (true, Some(id)) => player.set("aid", &id.to_string()),
            (false, Some(id)) => {
                player.set("sid", &id.to_string());
                self.subtitles = true;
            }
            (_, None) => {
                player.set("sid", "no");
                self.subtitles = false;
            }
        }
        self.remember_tracks();
        // What the info line and Inspect say is playing, read again.
        let words = self.player.as_ref().and_then(|player| tracks::playing_words(&tracks::track_list(player), self.subtitles));
        if let Some(video) = &mut self.video {
            video.track_words = words;
        }
        self.redraw();
    }
}

impl App {
    /// The Console tab: what jog has said, the latest first, with how long
    /// ago; and the ways to tell it something.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_console(&self, sub: &mut Canvas, text: &mut Text, px: u32, inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let run = |words: &str, command: Command| Action { words: words.to_string(), hit: Hit::Run(command) };
        let actions = vec![("Tell jog".to_string(), vec![run("Run a command…   Ctrl+P", Command::CommandPalette), run("Run a shell command on it…   !", Command::Shell), run("Repeat the last   .", Command::RepeatShell)])];
        let mut y = self.draw_actions(sub, text, px, &actions, inner_w, y, offset, mouse, drawn);
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = px;
        let x = flow_x();
        let shown: Vec<&(Instant, String)> = self.messages.iter().rev().take(200).collect();
        let row_h = line + pad / 4;
        y = section_head(sub, text, px, x, y, inner_w, &format!("Messages · {}", self.messages.len()));
        if shown.is_empty() {
            text.draw(sub, x, y, px, DIM_TEXT_COLOR, "Nothing yet");
            y += row_h;
        }
        let when_w = px as i32 * 3;
        for (at, words) in shown {
            let ago = at.elapsed().as_secs();
            let when = match ago {
                0..=59 => format!("{ago}s"),
                60..=3599 => format!("{}m", ago / 60),
                _ => format!("{}h", ago / 3600),
            };
            text.draw(sub, x, y + (line - Text::line_height(small)) / 2, small, 0x6C6D78, &when);
            let words = clipped(text, words, inner_w - when_w, px);
            text.draw(sub, x + when_w, y, px, TEXT_COLOR, &words);
            y += row_h;
        }
        y + pad
    }

    /// The Color tab's graphs of the picture on view, as toggles.
    pub(super) fn scope_actions(&self) -> Vec<(String, Vec<Action>)> {
        let names = ["Histogram", "Histogram in rows", "Waveform", "RGB parade", "Vectorscope"];
        let rows = names
            .iter()
            .enumerate()
            .map(|(at, name)| Action { words: format!("{}  {name}", if self.scopes.get(at).copied().unwrap_or(false) { "●" } else { "○" }), hit: Hit::Run(Command::Scope(at)) })
            .collect();
        vec![("Graphs of the frame on view".to_string(), rows)]
    }

    /// The Overview's anatomy: what the file is made of, as boxes within
    /// boxes, the way a browser draws an element's margin, border and
    /// padding: the container, its streams, each one's codec, and a
    /// video's color and HDR within that.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_anatomy(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &super::inspect::Inspection, inner_w: i32, y: i32, tips: &mut Vec<Tip>) -> i32 {
        let streams = found.streams();
        if streams.is_empty() {
            return y;
        }
        let pad = px as i32;
        // Its words at the size of all that is read.
        let small = px;
        let small_line = Text::line_height(small);
        let x = flow_x();
        let word = |value: &serde_json::Value| value.as_str().map(str::to_string).filter(|v| !v.is_empty() && v != "unknown");
        let container = found.probe["format"]["format_long_name"].as_str().or(found.probe["format"]["format_name"].as_str()).unwrap_or("file").to_string();
        // Each stream's layers, outermost first: what it is, then within it
        // its codec, then for a video its color and its HDR.
        // Each layer: its words, its color, and for its tooltip a title and
        // each term with what it is.
        type Layer = (String, u32, &'static str, Vec<(Role, String)>);
        let layers: Vec<Vec<Layer>> = streams
            .iter()
            .map(|stream| {
                let kind = word(&stream["codec_type"]).unwrap_or_else(|| "stream".into());
                let mut layers: Vec<Layer> = vec![(format!("#{} {kind}", stream["index"]), 0x2E3A55, "The stream", vec![(Role::Stream, kind.clone())])];
                let codec_name = word(&stream["codec_name"]).unwrap_or_default();
                let mut codec = codec_name.to_uppercase();
                let mut terms = vec![(Role::Codec, codec_name)];
                if let Some(profile) = word(&stream["profile"]) {
                    codec.push_str(&format!(" · {profile}"));
                    terms.push((Role::Profile, profile));
                }
                if let (Some(w), Some(h)) = (stream["width"].as_i64(), stream["height"].as_i64()) {
                    codec.push_str(&format!(" · {w}×{h}"));
                    terms.push((Role::Size, format!("{w}×{h}")));
                }
                if let Some(channels) = stream["channels"].as_i64() {
                    codec.push_str(&format!(" · {channels} ch"));
                    terms.push((Role::Channels, word(&stream["channel_layout"]).unwrap_or_else(|| format!("{channels} ch"))));
                }
                layers.push((codec, 0x3A3354, "Its codec", terms));
                if kind == "video" {
                    let parts = [(Role::Primaries, word(&stream["color_primaries"])), (Role::Transfer, word(&stream["color_transfer"])), (Role::Matrix, word(&stream["color_space"])), (Role::Pixels, word(&stream["pix_fmt"]))];
                    let terms: Vec<(Role, String)> = parts.into_iter().filter_map(|(role, term)| Some((role, term?))).collect();
                    if !terms.is_empty() {
                        let color = terms.iter().map(|(_, term)| term.as_str()).collect::<Vec<_>>().join(" · ");
                        layers.push((color, 0x2F4A44, "Its color", terms));
                    }
                    if let Some(hdr) = found.hdr.as_ref().filter(|hdr| hdr.is_hdr()) {
                        let badges = hdr.badges();
                        layers.push((badges.join(" · "), 0x553A2E, "Its HDR", badges.into_iter().map(|badge| (Role::Hdr, badge)).collect()));
                    }
                }
                layers
            })
            .collect();
        // Boxes nest by a little each layer; streams stack inside the file.
        let inset = pad / 2;
        let label_h = small_line + pad / 3;
        let stream_h = |layers: &Vec<Layer>| layers.len() as i32 * (label_h + inset) + inset;
        let inside: i32 = layers.iter().map(stream_h).sum::<i32>() + inset * (layers.len() as i32 + 1);
        let card_h = pad + small_line + pad / 2 + label_h + inside + pad;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, WELL, 256);
        text.draw(sub, x + pad, y + pad, small, HEAD_COLOR, "Anatomy");
        let (fx, fw) = (x + pad, inner_w - pad * 2);
        let mut fy = y + pad + small_line + pad / 2;
        let file_h = label_h + inside;
        sub.round_fill(fx, fy, fw, file_h, pad / 3, 0x1B2233, 256);
        sub.round_outline(fx, fy, fw, file_h, pad / 3, 1, 0x3E63DD);
        tips.push(Tip::new([fx, fy, fw, file_h], "The container", vec![(Role::Container, container.clone())]));
        let container = clipped(text, &container, fw - inset * 2, small);
        text.draw(sub, fx + inset, fy + pad / 6, small, TEXT_COLOR, &container);
        fy += label_h + inset;
        for stream in &layers {
            let mut bx = fx + inset;
            let mut bw = fw - inset * 2;
            let mut by = fy;
            let mut bh = stream_h(stream);
            for (words, color, title, terms) in stream {
                sub.round_fill(bx, by, bw, bh, pad / 3, *color, 256);
                sub.round_outline(bx, by, bw, bh, pad / 3, 1, 0x4A4A55);
                tips.push(Tip::new([bx, by, bw, bh], title, terms.clone()));
                let words = clipped(text, words, bw - inset * 2, small);
                text.draw(sub, bx + inset, by + pad / 6, small, TEXT_COLOR, &words);
                bx += inset;
                bw -= inset * 2;
                by += label_h;
                bh -= label_h + inset;
            }
            fy += stream_h(stream) + inset;
        }
        y + card_h + pad
    }
}

/// A video's packets, as ffprobe lists them: when each is shown, how big
/// it is, and which are keyframes. For the Timeline's bitrate and seeking.
#[derive(Debug, Default)]
pub struct Packets {
    pub times: Vec<f64>,
    pub sizes: Vec<u32>,
    pub keys: Vec<f64>,
}

impl Packets {
    /// Reads them, on whatever thread calls.
    pub fn read(path: &Path) -> Result<Packets, String> {
        let output = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,dts_time,size,flags", "-of", "csv=p=0"])
            .arg(path)
            .output()
            .map_err(|error| format!("ffprobe: {error}"))?;
        let mut packets = Packets::default();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let fields: Vec<&str> = line.split(',').collect();
            let [pts, dts, size, flags] = fields[..] else { continue };
            let Some(time) = pts.parse::<f64>().ok().or_else(|| dts.parse().ok()) else { continue };
            packets.times.push(time);
            packets.sizes.push(size.parse().unwrap_or(0));
            if flags.starts_with('K') {
                packets.keys.push(time);
            }
        }
        if packets.times.is_empty() {
            return Err("no video packets".into());
        }
        packets.keys.sort_by(f64::total_cmp);
        Ok(packets)
    }

    pub fn length(&self) -> f64 {
        self.times.iter().copied().fold(0.0, f64::max)
    }

    /// Bits a second in each of `bins` equal stretches of the video.
    pub fn bitrate(&self, bins: usize) -> Vec<f64> {
        let length = self.length().max(1e-3);
        let mut bytes = vec![0u64; bins.max(1)];
        for (time, size) in self.times.iter().zip(&self.sizes) {
            let bin = ((time / length) * bins as f64).floor().clamp(0.0, (bins - 1) as f64) as usize;
            bytes[bin] += u64::from(*size);
        }
        let span = length / bins as f64;
        bytes.into_iter().map(|bytes| bytes as f64 * 8.0 / span).collect()
    }
}

/// Bits a second as people read them.
fn rate(bits: f64) -> String {
    if bits >= 1e6 { format!("{:.1} Mb/s", bits / 1e6) } else { format!("{:.0} kb/s", bits / 1e3) }
}

impl App {
    /// The Timeline's bitrate over the video, its keyframes under it, and
    /// what that means for seeking from where the video is.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_bitrate(&self, sub: &mut Canvas, text: &mut Text, px: u32, packets: Option<&Result<Packets, String>>, id: usize, inner_w: i32, y: i32, offset: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>, (cuts, finding): (&[f64], Option<u32>)) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let x = flow_x();
        let (cx, cw) = (x, inner_w);
        let chart_h = px as i32 * 8;
        let row = line + pad / 4;
        let card_h = chart_h + pad / 3 + pad / 2 + 4 * row;
        let top = section_head(sub, text, px, x, y, inner_w, "Bitrate and keyframes");
        let end = top + card_h + pad;
        let packets = match packets {
            None => {
                text.draw(sub, cx, top, px, DIM_TEXT_COLOR, "Reading the packets…");
                return end;
            }
            Some(Err(error)) => {
                text.draw(sub, cx, top, px, DIM_TEXT_COLOR, &format!("Not readable · {error}"));
                return end;
            }
            Some(Ok(packets)) => packets,
        };
        // A bar for every 3 pixels, but none thinner than two frames, so
        // a short clip's bars aren't single frames.
        let bins = ((cw / 3) as usize).min(packets.times.len() / 2).max(1);
        let rates = packets.bitrate(bins);
        let peak = rates.iter().copied().fold(1.0, f64::max);
        let length = packets.length().max(1e-3);
        let mean = packets.sizes.iter().map(|size| f64::from(*size)).sum::<f64>() * 8.0 / length;
        // The busiest second, as a stream has to carry it.
        let busiest = {
            let window = 1.0_f64.min(length);
            let (mut most, mut bytes, mut from) = (0u64, 0u64, 0);
            let order: Vec<(f64, u32)> = {
                let mut order: Vec<(f64, u32)> = packets.times.iter().copied().zip(packets.sizes.iter().copied()).collect();
                order.sort_by(|a, b| a.0.total_cmp(&b.0));
                order
            };
            for &(time, size) in &order {
                bytes += u64::from(size);
                while order[from].0 <= time - window {
                    bytes -= u64::from(order[from].1);
                    from += 1;
                }
                most = most.max(bytes);
            }
            most as f64 * 8.0 / window
        };
        sub.round_fill(cx, top, cw, chart_h, pad / 3, WELL, 256);
        for (bin, value) in rates.iter().enumerate() {
            let (bx, next) = (cx + bin as i32 * cw / bins as i32, cx + (bin as i32 + 1) * cw / bins as i32);
            let h = ((value / peak) * f64::from(chart_h)).max(1.0) as i32;
            sub.fill(bx, top + chart_h - h, (next - bx).max(1), h, 0x3E7BD6);
            drawn.push((Hit::Seek(length * bin as f64 / bins as f64), [bx + offset.0, top + offset.1, (next - bx).max(1), chart_h]));
        }
        // The keyframes, a tick each along the foot.
        let ticks = top + chart_h + 2;
        for key in &packets.keys {
            let kx = cx + ((key / length) * f64::from(cw)) as i32;
            sub.fill(kx.min(cx + cw - 1), ticks, 1, pad / 3, TEXT_COLOR);
        }
        // Where the scenes cut, a mark each down from the top.
        for cut in cuts {
            let kx = cx + ((cut / length) * f64::from(cw)) as i32;
            sub.fill(kx.min(cx + cw - 1), top, 1, chart_h / 3, 0xE0A040);
        }
        // Where the video is.
        let at = self.video.as_ref().filter(|video| video.id == id).and(self.video_time()).map(|(at, _)| at);
        if let Some(at) = at {
            let ax = cx + ((at / length) * f64::from(cw)) as i32;
            sub.fill(ax.min(cx + cw - 1), top, 2, chart_h, accent());
        }
        let mut ty = ticks + pad / 3 + pad / 2;
        // Gaps to the end too: one keyframe at the start of a long video
        // is a long way to seek.
        let mut gaps: Vec<f64> = packets.keys.windows(2).map(|pair| pair[1] - pair[0]).collect();
        if let Some(last) = packets.keys.last() {
            gaps.push(length - last);
        }
        let gap_most = gaps.iter().copied().fold(0.0, f64::max);
        let keys = match packets.keys.len() {
            0 => "No keyframes".to_string(),
            1 => format!("1 keyframe · at {}", clock(packets.keys[0])),
            n => format!("every {:.1} s · longest gap {:.1} s", length / n as f64, gap_most),
        };
        let facts = [
            ("Bitrate", format!("{} mean · {} busiest second", rate(mean), rate(busiest))),
            ("Keyframes", if packets.keys.len() > 1 { format!("{} · {keys}", packets.keys.len()) } else { keys }),
            (
                "Seek here",
                match at {
                    Some(at) => {
                        let back = packets.keys.iter().rev().find(|key| **key <= at + 1e-6).map_or(at, |key| at - key);
                        format!("decodes {back:.1} s of video first")
                    }
                    None => "Click the chart to go there".to_string(),
                },
            ),
            (
                "Scenes",
                match (finding, cuts.len()) {
                    (Some(along), found) => format!("finding cuts · {}% · {found} so far", along / 10),
                    (None, 0) => "no cuts found".to_string(),
                    (None, 1) => "1 cut".to_string(),
                    (None, found) => format!("{found} cuts · one every {:.0} s on average", length / (found + 1) as f64),
                },
            ),
        ];
        let name_w = facts.iter().map(|(name, _)| text.width(name, px)).max().unwrap_or(0) + pad;
        for (name, words) in facts {
            text.draw(sub, cx, ty, px, DIM_TEXT_COLOR, name);
            let words = clipped(text, &words, cw - name_w, px);
            text.draw(sub, cx + name_w, ty, px, TEXT_COLOR, &words);
            ty += row;
        }
        end
    }
}

impl App {
    /// A row of tabs, a browser's developer tools' way: the names in a
    /// line, the one up in full white and underlined, a small label before
    /// each group (File, Tools), a hairline under all; when they are wider
    /// than the room, the row is moved along to keep the one up in view,
    /// with a fade and a » at the cut. Returns where under it to go on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_tab_row(&self, canvas: &mut Canvas, text: &mut Text, px: u32, [left, top, width]: [i32; 3], panes: &[Pane], current: usize, mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        // The names at the size of what is under them; the group labels
        // small capitals, as the sections' heads are.
        let small = px;
        let tiny = (px * 5 / 6).max(11);
        let line = Text::line_height(small);
        let row_h = line + pad * 3 / 4;
        let gap = pad;
        // Where each name and group label would go, from 0.
        let mut items: Vec<(Option<usize>, &str, i32, i32)> = Vec::new();
        let mut x = 0;
        let mut group = "";
        for (at, pane) in panes.iter().enumerate() {
            if pane.group() != group {
                group = pane.group();
                let label = if group == "jog" { group.to_string() } else { group.to_uppercase() };
                let w = text.width(&label, tiny);
                items.push((None, group, x, w));
                x += w + gap / 2;
            }
            let w = text.width(pane.name(), small);
            items.push((Some(at), pane.name(), x, w));
            x += w + gap;
        }
        let total = x - gap;
        let room = width - pad * 2;
        // Along far enough that the one up ends inside the room.
        let shift = items
            .iter()
            .find(|(at, ..)| *at == Some(current))
            .map_or(0, |(_, _, x, w)| (x + w + gap * 2 - room).max(0))
            .min((total - room).max(0));
        // Shifted, it starts at a whole name, after a « for those before.
        let back = if shift > 0 { text.width("«", small) + gap / 2 } else { 0 };
        let shift = match shift {
            0 => 0,
            _ => items.iter().map(|(_, _, x, _)| *x).find(|x| *x >= shift + back).map_or(shift, |x| x - back),
        };
        let base = left + pad;
        if shift > 0 {
            // A click on it goes a tab back.
            text.draw(canvas, base, top + (row_h - line) / 2, small, DIM_TEXT_COLOR, "«");
            drawn.push((Hit::Tab(current.saturating_sub(1)), [base - pad / 2, top, back + pad / 2, row_h]));
        }
        for (at, name, x, w) in &items {
            if shift > 0 && *x < shift + back {
                continue;
            }
            let x = base + x - shift;
            // Whole names only, the » keeping the room after the last.
            let end = if total - shift > room { left + width - pad * 2 } else { left + width };
            if x + w > end {
                continue;
            }
            match at {
                None => text.draw(canvas, x, top + (row_h - Text::line_height(tiny)) / 2 + 1, tiny, 0x5C5D66, &if *name == "jog" { name.to_string() } else { name.to_uppercase() }),
                Some(at) => {
                    let on = *at == current;
                    let hovered = (x - gap / 2..x + w + gap / 2).contains(&mouse.0) && (top..top + row_h).contains(&mouse.1);
                    let color = if on || hovered { TEXT_COLOR } else { DIM_TEXT_COLOR };
                    text.draw(canvas, x, top + (row_h - line) / 2, small, color, name);
                    if on {
                        canvas.fill(x, top + row_h - 2, *w, 2, accent());
                    }
                    drawn.push((Hit::Tab(*at), [x - gap / 2, top, w + gap, row_h]));
                }
            }
        }
        // A cut: the names fade into the panel, and » says there is more.
        if total - shift > room {
            let edge = left + width - pad * 2;
            canvas.fill(edge, top, pad * 2, row_h - 1, GRID_COLOR);
            text.draw(canvas, edge + pad / 2, top + (row_h - line) / 2, small, DIM_TEXT_COLOR, "»");
        }
        canvas.fill(left, top + row_h - 1, width, 1, MENU_EDGE_COLOR);
        top + row_h + pad / 2
    }
}

impl DrawerPane {
    /// As the pane it was a tab of, for drawing its name in a row of tabs.
    pub fn as_pane(self) -> Pane {
        match self {
            DrawerPane::Playlist => Pane::Files,
            DrawerPane::Console => Pane::Console,
            DrawerPane::Performance => Pane::Performance,
        }
    }
}

/// Where the block being drawn in an Inspect tab starts from the left of
/// its sheet: the column the tab's layout put it in. Every block reads it
/// rather than standing at the one margin, so that a tab lays itself out
/// in as many columns as its width has room for.
static FLOW_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub(super) fn flow_x() -> i32 {
    FLOW_X.load(atomic::Ordering::Relaxed)
}

pub(super) fn set_flow_x(x: i32) {
    FLOW_X.store(x, atomic::Ordering::Relaxed);
}

/// One thing a tab shows, placed by the tab's layout in a column.
pub(super) enum Block<'a> {
    /// The Overview's opening line: what the file is.
    Hero,
    /// The Color tab's: what HDR it is.
    HdrHero,
    Issues,
    Card(&'a Card),
    Anatomy,
    Tracks,
    PictureCharts,
    SceneChart,
    Agtm,
    Plus,
    Ladder,
    Scopes,
    Bitrate,
    /// The Timeline's frames near where the video is, and its sound.
    Frames,
    Sound,
    Chapters,
    Convert,
    ConvertSource,
    ConvertOutput,
    ConvertResult,
    Compare,
}

/// The ground of a chart or a box that is a thing in itself; sections
/// of facts have none.
pub(super) const WELL: u32 = 0x1B1C21;

/// Inspect's type is one size for all that is read, its rank told by its
/// color: values in full white, the names of facts dim, the heads of
/// sections between, over a hairline. A smaller size is for what is only
/// glanced at: chips, a chart's scale.
pub(super) const HEAD_COLOR: u32 = 0xC4C6D0;

/// A section's head: its name, in `HEAD_COLOR`, a hairline under;
/// returns where its rows start.
#[allow(clippy::too_many_arguments)]
pub(super) fn section_head(sub: &mut Canvas, text: &mut Text, px: u32, x: i32, y: i32, width: i32, title: &str) -> i32 {
    let pad = px as i32;
    text.draw(sub, x, y, px, HEAD_COLOR, &sentence(title));
    let y = y + Text::line_height(px) + pad / 4;
    sub.fill(x, y, width, 1, MENU_EDGE_COLOR);
    y + pad / 3
}

/// A head as a sentence starts: its first letter up, the rest as it was,
/// and a head written in capitals brought down. Names in capitals within
/// it (HDR10+, AV1) are kept.
pub(super) fn sentence(title: &str) -> String {
    let shouted = title.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()) && title.chars().filter(|c| c.is_alphabetic()).count() > 6;
    let title = if shouted { title.to_lowercase() } else { title.to_string() };
    let mut chars = title.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

impl App {
    /// A section of facts: its name in small capitals with any chips after
    /// it and a hairline under, then each fact, its name dim against one
    /// edge and its value after, a click copying it. No box around it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_card(&self, sub: &mut Canvas, text: &mut Text, px: u32, card: &Card, width: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>, tips: &mut Vec<Tip>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let x = flow_x();
        let mut tx = x;
        // The head, as `section_head` draws it, with its chips after it.
        let title = sentence(&card.title);
        text.draw(sub, tx, y, px, HEAD_COLOR, &title);
        tx += text.width(&title, px) + pad / 2;
        for name in &card.badges {
            tx += chip(sub, text, tx, y + (line - Text::line_height(small)) / 2, small, name, badge_color(name)) + pad / 3;
        }
        let mut ry = y + line + pad / 4;
        sub.fill(x, ry, width, 1, MENU_EDGE_COLOR);
        ry += pad / 2;
        let row_h = line + pad / 5;
        let label_w = card.rows.iter().map(|(label, _)| text.width(label, px)).max().unwrap_or(0).min(width * 2 / 5);
        for (label, value) in &card.rows {
            // Over its name: what the fact is, and what this value means.
            if let Some(tip) = super::glossary::fact_tip([x, ry, label_w + pad / 2, row_h], label, value) {
                tips.push(tip);
            }
            let label = clipped(text, label, label_w, px);
            text.draw(sub, x, ry, px, DIM_TEXT_COLOR, &label);
            let vx = x + label_w + pad;
            let shown = clipped(text, value, x + width - vx, px);
            let vw = text.width(&shown, px);
            let place = [vx - pad / 4 + offset.0, ry + offset.1, vw + pad / 2, row_h];
            if (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1) {
                sub.round_fill(vx - pad / 4, ry, vw + pad / 2, row_h, pad / 4, 0x2C2D35, 256);
            }
            text.draw(sub, vx, ry, px, TEXT_COLOR, &shown);
            drawn.push((Hit::Copy(value.to_string()), place));
            ry += row_h;
        }
        ry + pad
    }

    /// The Overview's opening: the video's codec, size and rate large, its
    /// length, size on disk and bitrate under, then chips for what else
    /// it is. For a picture its format and size.
    pub(super) fn draw_overview_hero(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &super::inspect::Inspection, width: i32, y: i32) -> i32 {
        let pad = px as i32;
        let big = px * 3 / 2;
        let small = (px * 5 / 6).max(11);
        let x = flow_x();
        let format = &found.probe["format"];
        let (headline, under, chips) = match found.video() {
            Some(video) => {
                let codec = video["codec_name"].as_str().unwrap_or("video").to_uppercase();
                let size = match (video["width"].as_i64(), video["height"].as_i64()) {
                    (Some(w), Some(h)) => format!("{w}×{h}"),
                    _ => String::new(),
                };
                let rate = video["avg_frame_rate"].as_str().and_then(|rate| {
                    let (n, d) = rate.split_once('/')?;
                    let fps = n.parse::<f64>().ok()? / d.parse::<f64>().ok().filter(|d| *d > 0.0)?;
                    Some(fps_words(fps))
                });
                let headline = [Some(codec), (!size.is_empty()).then_some(size), rate].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let seconds = format["duration"].as_str().and_then(|d| d.parse::<f64>().ok());
                let bytes = format["size"].as_str().and_then(|s| s.parse::<u64>().ok());
                let bits = format["bit_rate"].as_str().and_then(|b| b.parse::<f64>().ok());
                let under = [seconds.map(clock), bytes.map(meta::human_size), bits.map(|b| format!("{:.1} Mb/s", b / 1e6))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let mut chips: Vec<String> = Vec::new();
                if let Some(name) = format["format_name"].as_str() {
                    chips.push(name.split(',').next().unwrap_or(name).to_uppercase());
                }
                if video["pix_fmt"].as_str().is_some_and(|f| f.contains("10")) {
                    chips.push("10-bit".into());
                }
                if let Some(hdr) = found.hdr.as_ref() {
                    chips.extend(hdr.badges());
                }
                let count = |kind: &str| found.streams().iter().filter(|s| s["codec_type"] == kind).count();
                chips.push(format!("{} audio", count("audio")));
                if count("subtitle") > 0 {
                    chips.push(format!("{} subtitles", count("subtitle")));
                }
                (headline, under, chips)
            }
            None => match found.picture.as_ref() {
                Some(picture) => {
                    let size = picture.size.map(|(w, h)| format!("{w}×{h}")).unwrap_or_default();
                    let headline = [picture.format.clone(), (!size.is_empty()).then_some(size)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    let mp = picture.size.map(|(w, h)| format!("{:.1} MP", f64::from(w) * f64::from(h) / 1e6));
                    let under = [mp, Some(meta::human_size(picture.bytes))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    let mut chips = Vec::new();
                    if picture.gain.is_some() {
                        chips.push("Ultra HDR".to_string());
                    }
                    if let Some((name, _)) = &picture.icc {
                        chips.push(name.clone());
                    }
                    (headline, under, chips)
                }
                None => return y,
            },
        };
        let headline = clipped(text, &headline, width, big);
        text.draw(sub, x, y, big, TEXT_COLOR, &headline);
        let mut ty = y + Text::line_height(big) + pad / 4;
        { let shown = clipped(text, &under, width, px); text.draw(sub, x, ty, px, DIM_TEXT_COLOR, &shown); }
        ty += Text::line_height(px) + pad / 2;
        let mut cx = x;
        for name in &chips {
            let w = text.width(name, small) + pad / 2;
            if cx + w > x + width && cx > x {
                cx = x;
                ty += Text::line_height(small) + pad / 3;
            }
            cx += chip(sub, text, cx, ty, small, name, badge_color(name)) + pad / 3;
        }
        ty + Text::line_height(small) + pad
    }

    /// The Color tab's opening: the HDR formats large, the curve, the
    /// primaries and the mastering peak under.
    pub(super) fn draw_hdr_hero(&self, sub: &mut Canvas, text: &mut Text, px: u32, info: Option<&crate::hdrmeta::Info>, width: i32, y: i32) -> i32 {
        let Some(info) = info else { return y };
        let pad = px as i32;
        let big = px * 3 / 2;
        let x = flow_x();
        let headline = info.badges().join(" · ");
        { let shown = clipped(text, &headline, width, big); text.draw(sub, x, y, big, TEXT_COLOR, &shown); }
        let curve = match info.transfer {
            Some(crate::hdrmeta::Transfer::Pq) => "PQ",
            Some(crate::hdrmeta::Transfer::Hlg) => "HLG",
            _ => "SDR",
        };
        let under = [Some(curve.to_string()), info.bt2020.then(|| "BT.2020".to_string()), info.mastering.map(|(peak, _)| format!("mastered at {peak:.0} nits"))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let ty = y + Text::line_height(big) + pad / 4;
        { let shown = clipped(text, &under, width, px); text.draw(sub, x, ty, px, DIM_TEXT_COLOR, &shown); }
        ty + Text::line_height(px) + pad
    }

    /// The Issues tab: how many, then each with a bar of its severity's
    /// color, what is off and what it means.
    pub(super) fn draw_issues(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &super::inspect::Inspection, width: i32, y: i32) -> i32 {
        let pad = px as i32;
        let big = px * 3 / 2;
        let small = px;
        let x = flow_x();
        let issues = self.issues(found);
        let headline = match issues.len() {
            0 => "Nothing off".to_string(),
            1 => "1 issue".to_string(),
            n => format!("{n} issues"),
        };
        text.draw(sub, x, y, big, if issues.is_empty() { 0x3DD68C } else { TEXT_COLOR }, &headline);
        let mut ty = y + Text::line_height(big) + pad;
        let small_line = Text::line_height(small);
        for (severity, words, fix) in issues {
            // The first clause is what is off, the rest what it means; then
            // how to put it right. Wrapped, not cut: a narrow dock keeps all.
            let (what, means) = words.split_once(':').map_or((words.as_str(), ""), |(a, b)| (a, b.trim()));
            let color = if severity == Severity::Warning { 0xE5A50A } else { 0x9DB1F5 };
            let inner = width - pad;
            let mut lines: Vec<(String, u32, u32)> = vec![(clipped(text, what, inner, px), px, TEXT_COLOR)];
            lines.extend(super::glossary::wrap(text, means, inner, small).into_iter().filter(|line| !line.is_empty()).map(|line| (line, small, DIM_TEXT_COLOR)));
            lines.extend(super::glossary::wrap(text, &format!("Fix · {fix}"), inner, small).into_iter().map(|line| (line, small, accent_soft())));
            let h: i32 = lines.iter().map(|(_, size, _)| if *size == px { Text::line_height(px) } else { small_line + pad / 8 }).sum();
            sub.round_fill(x, ty, 3, h, 1, color, 256);
            let mut ly = ty;
            for (line, size, ink) in lines {
                text.draw(sub, x + pad * 2 / 3, ly, size, ink, &line);
                ly += if size == px { Text::line_height(px) } else { small_line + pad / 8 };
            }
            ty += h + pad * 2 / 3;
        }
        ty + pad / 2
    }
}

/// A chip: a word on a small rounded ground of its color. Returns its width.
fn chip(sub: &mut Canvas, text: &mut Text, x: i32, y: i32, small: u32, name: &str, color: u32) -> i32 {
    let pad = small as i32;
    let w = text.width(name, small) + pad * 2 / 3;
    let h = Text::line_height(small);
    sub.round_fill(x, y, w, h, pad / 4, color, 210);
    text.draw(sub, x + pad / 3, y, small, TEXT_COLOR, name);
    w
}
