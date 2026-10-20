//! Inspect as jog's developer tools: one panel to open, do everything in,
//! and close. Its tabs bring together what jog has elsewhere (the file
//! list, the tracks, the HDR charts, Convert and Compare, the debug
//! figures) and add what only a power user wants (Issues); a transport bar
//! along its foot plays and steps whatever tab is up. Everything stays
//! where it was as well.

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
    pub(super) fn issues(&self, found: &super::inspect::Inspection) -> Vec<(Severity, String)> {
        let mut issues = Vec::new();
        if let Some(video) = found.video() {
            let missing = |key: &str| video[key].as_str().is_none_or(|value| value == "unknown");
            if missing("color_primaries") || missing("color_transfer") || missing("color_space") {
                issues.push((Severity::Warning, "Color not tagged: players guess its primaries, curve or matrix, and may guess wrong".to_string()));
            }
            let (rate, average) = (video["r_frame_rate"].as_str().unwrap_or_default(), video["avg_frame_rate"].as_str().unwrap_or_default());
            if !rate.is_empty() && !average.is_empty() && rate != average && average != "0/0" {
                issues.push((Severity::Note, format!("Variable frame rate: {rate} at most, {average} on average")));
            }
            let transfer = video["color_transfer"].as_str().unwrap_or_default();
            let hdr = found.hdr.as_ref();
            if transfer == "smpte2084" && hdr.is_some_and(|hdr| hdr.mastering.is_none()) && found.side("Mastering display metadata").is_none() {
                issues.push((Severity::Note, "PQ without mastering display metadata: tone mapping has to guess the peak".to_string()));
            }
            if transfer == "smpte2084" && hdr.is_some_and(|hdr| hdr.light.is_none()) && found.side("Content light level metadata").is_none() {
                issues.push((Severity::Note, "PQ without MaxCLL and MaxFALL".to_string()));
            }
            if transfer != "smpte2084" && transfer != "arib-std-b67" && hdr.is_some_and(|hdr| !hdr.hdr10plus.is_empty() || !hdr.agtm.is_empty()) {
                issues.push((Severity::Warning, "HDR metadata in a video not coded as HDR".to_string()));
            }
        }
        // An MP4 whose index is at its end has to be read through before
        // it can play from a server.
        if let Ok(nodes) = found.tree.as_ref() {
            let order: Vec<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
            if let (Some(moov), Some(mdat)) = (order.iter().position(|name| *name == "moov"), order.iter().position(|name| *name == "mdat")) {
                if moov > mdat {
                    issues.push((Severity::Note, "Index (moov) after the media: not quick to start over a network".to_string()));
                }
            }
        }
        if let Some(picture) = found.picture.as_ref() {
            if picture.icc.is_none() && picture.color.cicp.is_none() && picture.color.png_gamma.is_none() {
                issues.push((Severity::Note, "No color profile: shown as sRGB".to_string()));
            }
        }
        issues
    }

    /// The Issues tab's cards.
    pub(super) fn issue_cards(&self, found: &super::inspect::Inspection) -> Vec<Card> {
        let issues = self.issues(found);
        let mut card = Card::new(format!("Issues · {}", issues.len()));
        if issues.is_empty() {
            card.row("Found", Some("Nothing off".into()));
        }
        for (severity, words) in issues {
            card.row(if severity == Severity::Warning { "Warning" } else { "Note" }, Some(words));
        }
        vec![card]
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
        let decoder = if crate::mpv::HWDEC.load(atomic::Ordering::Relaxed) { "GPU where it can, else CPU" } else { "CPU (--no-hwdec)" };
        let mut playback = Card::new("Playback");
        playback.row("Decoding", Some(decoder.to_string()));
        cards.insert(0, playback);
        cards
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

    /// The Compare tab's actions.
    pub(super) fn compare_actions(&self) -> Vec<(String, Vec<Action>)> {
        let marked = self.marks.len();
        let words = match marked {
            0 => "Mark a file (x), then compare it with this one".to_string(),
            1 => "Compare with the marked file".to_string(),
            n => format!("Compare the first two of {n} marked"),
        };
        vec![("Compare".to_string(), vec![Action { words, hit: Hit::Run(Command::Compare) }])]
    }

    /// Draws groups of action rows on `sub` from `y`; returns where they end.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_actions(&self, sub: &mut Canvas, text: &mut Text, px: u32, groups: &[(String, Vec<Action>)], inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let line = Text::line_height(px);
        let x = pad * 3 / 2;
        let row_h = line + pad / 2;
        let mut y = y;
        for (title, actions) in groups {
            text.draw(sub, x, y, small, DIM_TEXT_COLOR, &title.to_uppercase());
            y += small_line + pad / 3;
            for action in actions {
                let place = [x + offset.0, y + offset.1, inner_w, row_h];
                let over = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
                sub.round_fill(x, y, inner_w, row_h, row_h / 2, if over { accent() } else { 0x26262B }, 256);
                text.draw(sub, x + pad, y + (row_h - line) / 2, px, if over { on_accent() } else { TEXT_COLOR }, &action.words);
                drawn.push((action.hit.clone(), place));
                y += row_h + pad / 3;
            }
            y += pad / 2;
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
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let row_h = line + pad / 3;
        let rows = entries.len().max(1) as i32;
        let card_h = pad + small_line + pad / 2 + rows * row_h + pad / 2;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, 0x26262B, 256);
        text.draw(sub, x + pad, y + pad, small, DIM_TEXT_COLOR, &format!("CHAPTERS AND SECTIONS · {}", entries.len()));
        let mut ry = y + pad + small_line + pad / 2;
        if entries.is_empty() {
            text.draw(sub, x + pad, ry, px, DIM_TEXT_COLOR, "None · mark one with Shift+A and Shift+B");
        }
        for entry in &entries {
            let place = [x + offset.0, ry + offset.1, inner_w, row_h];
            if (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1) {
                sub.round_fill(x + pad / 2, ry, inner_w - pad, row_h, pad / 4, 0x3A3A40, 256);
            }
            text.draw(sub, x + pad, ry, px, DIM_TEXT_COLOR, &format!("{} – {}", clock(entry.start), clock(entry.end)));
            let kind = if entry.kept { "section" } else { "chapter" };
            let title = entry.title.clone().unwrap_or_else(|| kind.to_string());
            text.draw(sub, x + pad + px as i32 * 9, ry, px, TEXT_COLOR, &title);
            drawn.push((Hit::Seek(entry.start), place));
            ry += row_h;
        }
        y + card_h + pad
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
        let small = (px * 5 / 6).max(11);
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
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let row_h = line + pad / 3;
        let mut y = y;
        let mut group = |sub: &mut Canvas, text: &mut Text, y: &mut i32, title: &str, rows: Vec<(String, bool, Hit)>| {
            if rows.is_empty() {
                return;
            }
            let card_h = pad + small_line + pad / 2 + rows.len() as i32 * row_h + pad / 2;
            sub.round_fill(x, *y, inner_w, card_h, pad / 2, 0x26262B, 256);
            text.draw(sub, x + pad, *y + pad, small, DIM_TEXT_COLOR, title);
            let mut ry = *y + pad + small_line + pad / 2;
            for (words, on, hit) in rows {
                let place = [x + offset.0, ry + offset.1, inner_w, row_h];
                let over = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
                if on || over {
                    sub.round_fill(x + pad / 2, ry, inner_w - pad, row_h, pad / 3, if on { accent() } else { 0x3A3A40 }, 256);
                }
                let mark = if on { "●" } else { "○" };
                text.draw(sub, x + pad, ry + (row_h - line) / 2, px, if on { on_accent() } else { TEXT_COLOR }, &format!("{mark}  {words}"));
                drawn.push((hit, place));
                ry += row_h;
            }
            *y += card_h + pad;
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
        group(sub, text, &mut y, "PLAYING · AUDIO", audio);
        let mut subs: Vec<(String, bool, Hit)> = vec![("Off".to_string(), !tracks.iter().any(|track| track.kind == tracks::TrackKind::Sub && track.selected), Hit::Track(false, None))];
        subs.extend(tracks.iter().filter(|track| track.kind == tracks::TrackKind::Sub).map(|track| (tracks::track_label(track), track.selected, Hit::Track(false, Some(track.id)))));
        if playing {
            group(sub, text, &mut y, "PLAYING · SUBTITLES", subs);
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
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let shown: Vec<&(Instant, String)> = self.messages.iter().rev().take(200).collect();
        let row_h = line + pad / 4;
        let card_h = pad + small_line + pad / 2 + shown.len().max(1) as i32 * row_h + pad / 2;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, 0x26262B, 256);
        text.draw(sub, x + pad, y + pad, small, DIM_TEXT_COLOR, &format!("MESSAGES · {}", self.messages.len()));
        let mut ry = y + pad + small_line + pad / 2;
        if shown.is_empty() {
            text.draw(sub, x + pad, ry, px, DIM_TEXT_COLOR, "Nothing yet");
        }
        let when_w = px as i32 * 4;
        for (at, words) in shown {
            let ago = at.elapsed().as_secs();
            let when = match ago {
                0..=59 => format!("{ago}s"),
                60..=3599 => format!("{}m", ago / 60),
                _ => format!("{}h", ago / 3600),
            };
            text.draw(sub, x + pad, ry, small, DIM_TEXT_COLOR, &when);
            let words = clipped(text, words, inner_w - pad * 2 - when_w, px);
            text.draw(sub, x + pad + when_w, ry, px, TEXT_COLOR, &words);
            ry += row_h;
        }
        y += card_h + pad;
        y
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
    pub(super) fn draw_anatomy(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &super::inspect::Inspection, inner_w: i32, y: i32) -> i32 {
        let streams = found.streams();
        if streams.is_empty() {
            return y;
        }
        let pad = px as i32;
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let word = |value: &serde_json::Value| value.as_str().map(str::to_string).filter(|v| !v.is_empty() && v != "unknown");
        let container = found.probe["format"]["format_long_name"].as_str().or(found.probe["format"]["format_name"].as_str()).unwrap_or("file").to_string();
        // Each stream's layers, outermost first: what it is, then within it
        // its codec, then for a video its color and its HDR.
        let layers: Vec<Vec<(String, u32)>> = streams
            .iter()
            .map(|stream| {
                let kind = word(&stream["codec_type"]).unwrap_or_else(|| "stream".into());
                let mut layers = vec![(format!("#{} {kind}", stream["index"]), 0x2E3A55)];
                let mut codec = word(&stream["codec_name"]).unwrap_or_default().to_uppercase();
                if let Some(profile) = word(&stream["profile"]) {
                    codec.push_str(&format!(" · {profile}"));
                }
                if let (Some(w), Some(h)) = (stream["width"].as_i64(), stream["height"].as_i64()) {
                    codec.push_str(&format!(" · {w}×{h}"));
                }
                if let Some(channels) = stream["channels"].as_i64() {
                    codec.push_str(&format!(" · {channels} ch"));
                }
                layers.push((codec, 0x3A3354));
                if kind == "video" {
                    let color = [word(&stream["color_primaries"]), word(&stream["color_transfer"]), word(&stream["color_space"]), word(&stream["pix_fmt"])].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    if !color.is_empty() {
                        layers.push((color, 0x2F4A44));
                    }
                    if let Some(hdr) = found.hdr.as_ref().filter(|hdr| hdr.is_hdr()) {
                        layers.push((hdr.badges().join(" · "), 0x553A2E));
                    }
                }
                layers
            })
            .collect();
        // Boxes nest by a little each layer; streams stack inside the file.
        let inset = pad / 2;
        let label_h = small_line + pad / 3;
        let stream_h = |layers: &Vec<(String, u32)>| layers.len() as i32 * (label_h + inset) + inset;
        let inside: i32 = layers.iter().map(stream_h).sum::<i32>() + inset * (layers.len() as i32 + 1);
        let card_h = pad + small_line + pad / 2 + label_h + inside + pad;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, 0x26262B, 256);
        text.draw(sub, x + pad, y + pad, small, DIM_TEXT_COLOR, "ANATOMY");
        let (fx, fw) = (x + pad, inner_w - pad * 2);
        let mut fy = y + pad + small_line + pad / 2;
        let file_h = label_h + inside;
        sub.round_fill(fx, fy, fw, file_h, pad / 3, 0x1B2233, 256);
        sub.round_outline(fx, fy, fw, file_h, pad / 3, 1, 0x3E63DD);
        let container = clipped(text, &container, fw - inset * 2, small);
        text.draw(sub, fx + inset, fy + pad / 6, small, TEXT_COLOR, &container);
        fy += label_h + inset;
        for stream in &layers {
            let mut bx = fx + inset;
            let mut bw = fw - inset * 2;
            let mut by = fy;
            let mut bh = stream_h(stream);
            for (words, color) in stream {
                sub.round_fill(bx, by, bw, bh, pad / 3, *color, 256);
                sub.round_outline(bx, by, bw, bh, pad / 3, 1, 0x4A4A55);
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
    pub(super) fn draw_bitrate(&self, sub: &mut Canvas, text: &mut Text, px: u32, packets: Option<&Result<Packets, String>>, id: usize, inner_w: i32, y: i32, offset: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let (cx, cw) = (x + pad, inner_w - 2 * pad);
        let chart_h = px as i32 * 8;
        let row = small_line + pad / 4;
        let card_h = pad + small_line + pad / 2 + chart_h + pad / 3 + pad / 2 + 3 * row + pad;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, 0x26262B, 256);
        text.draw(sub, cx, y + pad, small, DIM_TEXT_COLOR, "BITRATE AND KEYFRAMES");
        let top = y + pad + small_line + pad / 2;
        let packets = match packets {
            None => {
                text.draw(sub, cx, top, px, DIM_TEXT_COLOR, "Reading the packets…");
                return y + card_h + pad;
            }
            Some(Err(error)) => {
                text.draw(sub, cx, top, px, DIM_TEXT_COLOR, &format!("Not readable · {error}"));
                return y + card_h + pad;
            }
            Some(Ok(packets)) => packets,
        };
        let bins = (cw / 3).max(1) as usize;
        let rates = packets.bitrate(bins);
        let peak = rates.iter().copied().fold(1.0, f64::max);
        let mean = packets.sizes.iter().map(|size| f64::from(*size)).sum::<f64>() * 8.0 / packets.length().max(1e-3);
        let length = packets.length().max(1e-3);
        sub.fill(cx, top, cw, chart_h, 0x0B0B0B);
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
        // Where the video is.
        let at = self.video.as_ref().filter(|video| video.id == id).and(self.video_time()).map(|(at, _)| at);
        if let Some(at) = at {
            let ax = cx + ((at / length) * f64::from(cw)) as i32;
            sub.fill(ax.min(cx + cw - 1), top, 2, chart_h, accent());
        }
        let mut ty = ticks + pad / 3 + pad / 2;
        let gaps: Vec<f64> = packets.keys.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let (gap_mean, gap_most) = (gaps.iter().sum::<f64>() / gaps.len().max(1) as f64, gaps.iter().copied().fold(0.0, f64::max));
        let lines = [
            format!("Mean {}  ·  busiest stretch {}", rate(mean), rate(peak)),
            format!("{} keyframes  ·  one every {:.1} s, at most {:.1} s apart", packets.keys.len(), gap_mean, gap_most),
            match at {
                Some(at) => {
                    let back = packets.keys.iter().rev().find(|key| **key <= at + 1e-6).map_or(at, |key| at - key);
                    format!("A seek here decodes from {:.1} s back  ·  click the chart to go there", back)
                }
                None => "Click the chart to go there".to_string(),
            },
        ];
        for words in lines {
            let words = clipped(text, &words, cw, small);
            text.draw(sub, cx, ty, small, TEXT_COLOR, &words);
            ty += row;
        }
        y + card_h + pad
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
        let small = (px * 5 / 6).max(11);
        let tiny = (px * 2 / 3).max(10);
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
        let base = left + pad;
        for (at, name, x, w) in &items {
            let x = base + x - shift;
            if x + w <= left || x >= left + width - pad {
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
