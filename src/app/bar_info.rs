//! What the seek bar says of the video playing, while it is up: a title
//! and the time at its left, the time left at its right, and over the track
//! what the file is: codec, size, rate, HDR, bitrate, audio, its size on
//! disk. A click on any of them copies it; a right click, the file's name,
//! path or folder, or all of it at once.

use super::*;

/// A title made from a file's name: a show's name and its season and
/// episode where the name has them (`Show.Name.S02E05.1080p…`), a film's
/// name and year (`Film.Name.2019.1080p…`), else the name as it is.
pub(super) fn title_of(file_name: &str) -> String {
    let stem = Path::new(file_name).file_stem().map_or_else(|| file_name.to_string(), |stem| stem.to_string_lossy().into_owned());
    let words: Vec<&str> = stem.split(['.', '_', ' ']).filter(|word| !word.is_empty()).collect();
    let joined = |words: &[&str]| words.join(" ").trim_end_matches([' ', '-']).to_string();
    for (at, word) in words.iter().enumerate().skip(1) {
        if let Some((season, episode)) = episode_of(word) {
            return format!("{} · S{season} E{episode}", joined(&words[..at]));
        }
        let year = word.len() == 4 && word.chars().all(|c| c.is_ascii_digit()) && (word.starts_with("19") || word.starts_with("20"));
        if year {
            return format!("{} ({word})", joined(&words[..at]));
        }
    }
    stem
}

/// `S02E05`, in either case, as season 2 and episode 5.
fn episode_of(word: &str) -> Option<(u32, u32)> {
    let lower = word.to_ascii_lowercase();
    let rest = lower.strip_prefix('s')?;
    let (season, episode) = rest.split_once('e')?;
    let all_digits = |part: &str| !part.is_empty() && part.len() <= 3 && part.chars().all(|c| c.is_ascii_digit());
    (all_digits(season) && all_digits(episode)).then(|| (season.parse().ok(), episode.parse().ok())).and_then(|(s, e)| s.zip(e))
}

/// An audio codec as it is usually written.
fn audio_name(codec: &str) -> String {
    match codec {
        "aac" => "AAC".to_string(),
        "ac3" => "AC-3".to_string(),
        "eac3" => "E-AC-3".to_string(),
        "opus" => "Opus".to_string(),
        "vorbis" => "Vorbis".to_string(),
        "flac" => "FLAC".to_string(),
        "mp3" => "MP3".to_string(),
        "truehd" => "TrueHD".to_string(),
        "dts" => "DTS".to_string(),
        other => other.to_uppercase(),
    }
}

/// So many channels as they are usually said.
fn channels_name(count: f64) -> String {
    match count as u32 {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        6 => "5.1".to_string(),
        8 => "7.1".to_string(),
        other => format!("{other} ch"),
    }
}

/// Where the title and the time stand, and the details over the track:
/// worked out with the bar, written with the rest of the text.
pub(super) struct BarInfo {
    /// The title's left edge, its top, and how wide it may be.
    pub left: i32,
    pub top: i32,
    pub room: i32,
    pub title: String,
    pub time: String,
    /// The right end of the details, and their top.
    pub right: i32,
    pub details_top: i32,
    pub details: Vec<String>,
    /// Where the sections' flags stand across, the top of their letters,
    /// and the span of a section's buttons when they are up: the details
    /// go over a letter under them, and give way to the buttons.
    pub flags: Vec<i32>,
    pub letters_top: i32,
    pub tools: Option<(i32, i32)>,
}

impl App {
    /// The title of the video playing, from its file's name.
    pub(super) fn bar_title(&self) -> String {
        title_of(&self.name())
    }

    /// What the file playing is, each to be copied on its own: codec and
    /// depth, size, rate, HDR, bitrate over the whole file, audio, size.
    pub(super) fn bar_details(&mut self) -> Vec<String> {
        let Some(video) = &self.video else { return Vec::new() };
        let id = video.id;
        let Some(player) = &self.player else { return Vec::new() };
        let number = |property| player.number(property);
        let mut details = Vec::new();
        if let Some(codec) = self.codec_name() {
            // As `yuv420p10` or `p010`; `nv12` is 8 bits.
            let deep = player.text("video-params/pixelformat").is_some_and(|format| ["p10", "p010", "p12", "p016"].iter().any(|deep| format.contains(deep)));
            details.push(if deep { format!("{codec} 10-bit") } else { codec });
        }
        if let Some((w, h)) = number("width").zip(number("height")) {
            details.push(format!("{w:.0}×{h:.0}"));
        }
        if let Some(rate) = number("container-fps").filter(|rate| *rate > 1.0) {
            let shown = format!("{rate:.3}");
            details.push(format!("{} fps", shown.trim_end_matches('0').trim_end_matches('.')));
        }
        match player.text("video-params/gamma").as_deref() {
            Some("pq") => details.push("HDR10".to_string()),
            Some("hlg") => details.push("HLG".to_string()),
            _ => {}
        }
        // Read once a file; the bitrate is over the whole of it.
        let bytes = self.file_bytes.get(&id).copied().unwrap_or_else(|| std::fs::metadata(&self.files[id]).map_or(0, |meta| meta.len()));
        if let (Some(length), true) = (number("duration").filter(|length| *length > 0.0), bytes > 0) {
            details.push(format!("{:.1} Mb/s", bytes as f64 * 8.0 / length / 1e6));
        }
        if let Some(codec) = player.text("audio-codec-name") {
            let channels = number("audio-params/channel-count").map(channels_name);
            let rate = number("audio-params/samplerate").map(|rate| format!("{} kHz", (rate / 100.0).round() / 10.0));
            details.push([Some(audio_name(&codec)), channels].into_iter().flatten().collect::<Vec<_>>().join(" "));
            details.extend(rate);
        }
        self.file_bytes.insert(id, bytes);
        if bytes > 0 {
            details.push(meta::human_size(bytes));
        }
        details
    }

    /// Writes the title, the time and the details, and notes where each
    /// is, for a click to copy it. The one under the pointer is lit.
    pub(super) fn draw_bar_info(&mut self, canvas: &mut Canvas, text: &mut Text, info: &BarInfo, px: u32) {
        let small = (px * 5 / 6).max(11);
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
        let over = |[x, y, w, h]: [i32; 4]| (x..x + w).contains(&mx) && (y..y + h).contains(&my);
        let mut hits = Vec::new();
        // The title, cut short to fit its room: the name, not the episode
        // or the year after it.
        let mut title = info.title.clone();
        if text.width(&title, px) > info.room {
            let split = title.rfind(" · ").or_else(|| title.rfind(" (")).unwrap_or(title.len());
            let (mut head, tail) = (title[..split].to_string(), title[split..].to_string());
            while !head.is_empty() && text.width(&format!("{}…{tail}", head.trim_end()), px) > info.room {
                head.pop();
            }
            title = format!("{}…{tail}", head.trim_end());
        }
        let place = [info.left, info.top, text.width(&title, px), Text::line_height(px)];
        text.draw(canvas, info.left, info.top, px, if over(place) { TEXT_COLOR } else { 0xF2F3F5 }, &title);
        hits.push((place, Command::CopyBarTitle));
        let time_top = info.top + Text::line_height(px);
        let place = [info.left, time_top, text.width(&info.time, small), Text::line_height(small)];
        text.draw(canvas, info.left, time_top, small, if over(place) { TEXT_COLOR } else { DIM_TEXT_COLOR }, &info.time);
        hits.push((place, Command::CopyTime));
        // The details, back from the right end, with a dot between.
        let gap = "  ·  ";
        let gap_w = text.width(gap, small);
        let widths: Vec<i32> = info.details.iter().map(|detail| text.width(detail, small)).collect();
        let total = widths.iter().sum::<i32>() + gap_w * (widths.len() as i32 - 1).max(0);
        let mut x = info.right - total;
        let span = x - px as i32..info.right + px as i32;
        let details_top = match info.flags.iter().any(|flag| span.contains(flag)) {
            true => info.letters_top - Text::line_height(small),
            false => info.details_top,
        };
        let under_tools = info.tools.is_some_and(|(left, right)| left < info.right && right > x);
        let details = if under_tools { &[][..] } else { &info.details[..] };
        for (index, (detail, w)) in details.iter().zip(&widths).enumerate() {
            if index > 0 {
                text.draw(canvas, x, details_top, small, MENU_EDGE_COLOR, gap);
                x += gap_w;
            }
            let place = [x, details_top, *w, Text::line_height(small)];
            text.draw(canvas, x, details_top, small, if over(place) { TEXT_COLOR } else { DIM_TEXT_COLOR }, detail);
            hits.push((place, Command::CopyDetail(index as u8)));
            x += w;
        }
        self.bar_hits = hits;
    }

    /// What of the bar's words is under a point, to be copied.
    pub(super) fn bar_hit(&self, x: f64, y: f64) -> Option<Command> {
        self.hover?;
        let (x, y) = (x as i32, y as i32);
        self.bar_hits.iter().find(|([left, top, w, h], _)| (*left..left + w).contains(&x) && (*top..top + h).contains(&y)).map(|(_, command)| *command)
    }

    /// Copies a piece of what the bar says, or the file's folder.
    pub(super) fn copy_bar(&mut self, command: Command) {
        let words = match command {
            Command::CopyBarTitle => self.bar_title(),
            Command::CopyTime => self.video_time().map(|(at, _)| clock(at)).unwrap_or_default(),
            Command::CopyDetail(index) => self.bar_details().get(usize::from(index)).cloned().unwrap_or_default(),
            Command::CopyDetails => {
                let details = self.bar_details().join(" · ");
                format!("{} · {details}", self.bar_title())
            }
            _ => {
                let path = &self.files[self.id()];
                let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                path.parent().map(|folder| folder.display().to_string()).unwrap_or_default()
            }
        };
        if words.is_empty() {
            return;
        }
        println!("[copy] {words}");
        if self.snapshot.is_some() || ops::set_clipboard(&words) {
            self.toast(format!("Copied {words}"));
        } else {
            self.toast("No clipboard tool (xclip, xsel, wl-copy)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_from_names() {
        assert_eq!(title_of("Friends.S04E12.1080p.Bluray.x265.mkv"), "Friends · S4 E12");
        assert_eq!(title_of("The.Long.Way.Home.s02e05.HDR.mkv"), "The Long Way Home · S2 E5");
        assert_eq!(title_of("Some_Film_2019_1080p.mp4"), "Some Film (2019)");
        assert_eq!(title_of("2001.A.Space.Odyssey.1968.mkv"), "2001 A Space Odyssey (1968)");
        assert_eq!(title_of("holiday clip.mp4"), "holiday clip");
        assert_eq!(title_of("eun.mp4"), "eun");
    }
}
