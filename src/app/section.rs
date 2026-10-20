//! A section of a video, marked with Shift+A at its start and Shift+B at
//! its end: played over and over while it is marked, shown on the seek
//! bar, its ends dragged there to move them.
//!
//! Sections can be kept, and are when saved as a file: they come back in
//! the Sections tab of the strip along the bottom (T, then T again), with
//! the chapters the file has of its own, a click marking one again.

use super::*;

/// What the strip along the bottom of a video shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StripTab {
    #[default]
    Scenes,
    Sections,
}

/// A stretch of a video in the Sections tab: kept from a section marked,
/// or a chapter of the file's own.
#[derive(Clone, PartialEq, Debug)]
pub struct Entry {
    pub start: f64,
    pub end: f64,
    /// The chapter's title, or the name a kept section was given.
    pub title: Option<String>,
    /// A section kept, which can be renamed or removed; not a chapter.
    pub kept: bool,
}

/// A section kept: its ends, and the name it was given, if any.
#[derive(Clone, PartialEq, Debug)]
pub struct Kept {
    pub a: f64,
    pub b: f64,
    pub name: Option<String>,
}

impl Kept {
    fn is(&self, a: f64, b: f64) -> bool {
        (self.a - a).abs() < 0.05 && (self.b - b).abs() < 0.05
    }
}

/// The frames of the entries of a video, for the cells of the tab.
#[derive(Default)]
pub struct EntryFrames {
    starts: Vec<f64>,
    frames: Vec<Option<Arc<Frame>>>,
}

/// Where the sections kept are written: a line each, start, end, the
/// video's path and the name given, if any, by tabs.
fn kept_path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("sections"))
}

pub(super) type KeptSections = HashMap<PathBuf, Vec<Kept>>;

fn read_kept() -> KeptSections {
    let text = kept_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    let mut kept = KeptSections::new();
    for line in text.lines() {
        let mut parts = line.splitn(4, '\t');
        let (Some(a), Some(b), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let name = parts.next().filter(|name| !name.is_empty()).map(str::to_string);
        if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
            kept.entry(PathBuf::from(path)).or_default().push(Kept { a, b, name });
        }
    }
    kept
}

fn write_kept(kept: &KeptSections) -> std::io::Result<()> {
    let path = kept_path().ok_or_else(|| std::io::Error::other("no folder to keep it in"))?;
    let mut paths: Vec<&PathBuf> = kept.keys().collect();
    paths.sort();
    let mut text = String::new();
    for video in paths {
        for Kept { a, b, name } in &kept[video] {
            let name = name.as_deref().unwrap_or_default().replace(['\t', '\n'], " ");
            text.push_str(&format!("{a:.3}\t{b:.3}\t{}\t{name}\n", video.display()));
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

/// Either end of a section.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    A,
    B,
}

/// The ends marked so far, in seconds into the video.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Section {
    pub a: Option<f64>,
    pub b: Option<f64>,
}

impl Section {
    /// Both ends, when both are marked.
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.a?, self.b?))
    }
}

/// The least a section can be, in seconds: less and mpv would be looping
/// a frame or two at most, which is a stop and not a section.
const SHORTEST: f64 = 0.1;

/// How near the pointer must be to an end on the seek bar, in pixels, to
/// take hold of it rather than seek.
fn grip(bar: &SeekBar) -> f64 {
    f64::from(bar.unit)
}

/// Where the sections marked when jog was last quit are kept, a video's
/// as start, end and path by tabs: marked again when it is opened.
fn marked_path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("marked-sections"))
}

fn read_marked() -> HashMap<PathBuf, (f64, f64)> {
    let text = marked_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let (a, b, path) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?);
            Some((PathBuf::from(path), (a, b)))
        })
        .collect()
}

impl App {
    /// A video opened: the section it had marked when jog was last quit,
    /// unless one is marked now.
    pub(super) fn restore_section(&mut self, id: usize) {
        if self.sections.contains_key(&id) {
            return;
        }
        let path = std::path::absolute(&self.files[id]).unwrap_or_else(|_| self.files[id].clone());
        if let Some(&(a, b)) = self.marked_sections.get_or_insert_with(read_marked).get(&path) {
            self.sections.insert(id, Section { a: Some(a), b: Some(b) });
        }
    }

    /// Keeps the sections marked, for the next run.
    pub(super) fn save_marked_sections(&mut self) {
        let mut marked = self.marked_sections.take().unwrap_or_else(read_marked);
        for (&id, section) in &self.sections {
            let path = std::path::absolute(&self.files[id]).unwrap_or_else(|_| self.files[id].clone());
            match section.span() {
                Some(span) => marked.insert(path, span),
                None => marked.remove(&path),
            };
        }
        let Some(file) = marked_path() else { return };
        let mut lines: Vec<String> = marked.iter().map(|(path, (a, b))| format!("{a:.3}\t{b:.3}\t{}", path.display())).collect();
        lines.sort();
        let _ = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, lines.join("\n") + "\n"));
        self.marked_sections = Some(marked);
    }

    /// The section of the video on view, if one is marked.
    pub(super) fn section(&self) -> Option<Section> {
        let video = self.video.as_ref()?;
        self.sections.get(&video.id).copied()
    }

    /// Marks an end of the section where the video is now.
    pub(super) fn mark_edge(&mut self, edge: Edge) {
        let Some(video) = &self.video else { return self.toast("Sections are for videos") };
        let id = video.id;
        let Some((at, length)) = self.video_time() else { return };
        let mut section = self.sections.get(&id).copied().unwrap_or_default();
        match edge {
            Edge::A => {
                section.a = Some(at);
                // An end before the new start is no end of it.
                if section.b.is_some_and(|b| b <= at + SHORTEST) {
                    section.b = None;
                }
            }
            Edge::B => {
                let a = section.a.unwrap_or(0.0);
                if at <= a + SHORTEST {
                    return self.toast(format!("End must come after {}", clock(a)));
                }
                section = Section { a: Some(a), b: Some(at.min(length)) };
            }
        }
        self.sections.insert(id, section);
        self.apply_loop();
        self.say_section();
        self.redraw();
    }

    /// Unmarks the section of the video on view. Whether there was one.
    pub(super) fn clear_section(&mut self) -> bool {
        let Some(video) = &self.video else { return false };
        let path = std::path::absolute(&self.files[video.id]).unwrap_or_else(|_| self.files[video.id].clone());
        if self.sections.remove(&video.id).is_none() {
            return false;
        }
        if let Some(marked) = &mut self.marked_sections {
            marked.remove(&path);
        }
        self.apply_loop();
        self.toast("Section cleared");
        self.redraw();
        true
    }

    /// Says what is marked, and what to do next.
    fn say_section(&mut self) {
        let Some(section) = self.section() else { return };
        match section.span() {
            Some((a, b)) => self.toast(format!("Looping {} – {}  ({:.1} s)", clock(a), clock(b), b - a)),
            None => self.toast(format!("A {}  ·  Shift+B ends it", clock(section.a.unwrap_or(0.0)))),
        }
    }

    /// Tells the player to loop the section, or not to loop anything of
    /// its own when there is none.
    pub(super) fn apply_section_loop(&self) {
        let Some(player) = &self.player else { return };
        let span = self.section().and_then(|section| section.span());
        let (a, b) = span.map_or(("no".to_string(), "no".to_string()), |(a, b)| (format!("{a:.3}"), format!("{b:.3}")));
        player.set("ab-loop-a", &a);
        player.set("ab-loop-b", &b);
    }

    /// The end of the section near a point on the seek bar, to drag.
    pub(super) fn section_edge_at(&self, bar: &SeekBar, x: f64) -> Option<Edge> {
        let section = self.section()?;
        let (_, length) = self.video_time()?;
        let near = |at: Option<f64>| at.map(|at| (f64::from(bar.x) + f64::from(bar.width) * at / length - x).abs()).filter(|off| *off <= grip(bar));
        match (near(section.a), near(section.b)) {
            (Some(a), Some(b)) => Some(if b < a { Edge::B } else { Edge::A }),
            (Some(_), None) => Some(Edge::A),
            (None, Some(_)) => Some(Edge::B),
            (None, None) => None,
        }
    }

    /// An end dragged along the seek bar, to `fraction` of the video.
    pub(super) fn drag_edge(&mut self, edge: Edge, fraction: f64) {
        let Some(video) = &self.video else { return };
        let id = video.id;
        let Some((_, length)) = self.video_time() else { return };
        let Some(section) = self.sections.get_mut(&id) else { return };
        let at = fraction.clamp(0.0, 1.0) * length;
        // Kept on its own side of the other end.
        match edge {
            Edge::A => section.a = Some(section.b.map_or(at, |b| at.min(b - SHORTEST)).max(0.0)),
            Edge::B => section.b = Some(section.a.map_or(at, |a| at.max(a + SHORTEST)).min(length)),
        }
        self.hover = Some(fraction);
        self.request_preview(fraction);
        self.apply_loop();
        self.redraw();
    }

    /// The drag is over: the video goes to the end moved, to show the
    /// very frame it is now.
    pub(super) fn drop_edge(&mut self, edge: Edge) {
        let Some(section) = self.section() else { return };
        let Some((_, length)) = self.video_time() else { return };
        let at = match edge {
            Edge::A => section.a,
            Edge::B => section.b,
        };
        if let Some(at) = at {
            self.seek_to(at / length, true);
        }
        self.say_section();
    }

    /// Saves the section as `clip`, by ffmpeg on another thread, saying in
    /// the toast how far along it is.
    pub(super) fn save_section(&mut self, clip: crate::transcode::Clip, knobs: crate::transcode::Knobs) {
        let Some(video) = &self.video else { return self.toast("Sections are for videos") };
        let Some((a, b)) = self.section().and_then(|section| section.span()) else {
            return self.toast("No section · Shift+A, Shift+B");
        };
        let path = self.files[video.id].clone();
        // Turned and mirrored as it is shown.
        let transform = self.transforms.get(&video.id).copied().unwrap_or_default();
        let settings = self.options.convert.video;
        let what = clip.describe(&settings);
        let output = crate::transcode::clip_output(&path, a, b, &clip);
        if self.dry(|_| vec![format!("save {} – {} as {what}: {}", clock(a), clock(b), output.display())], "would save the section") {
            return;
        }
        self.toast(format!("Saving {what}…"));
        // A section worth saving is worth coming back to.
        self.keep_quietly(&path, a, b);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let name = output.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let (mut said, started) = (None, Instant::now());
            let result = crate::transcode::cut(&path, a, b, &clip, &settings, &knobs, transform, |share| {
                let percent = (share * 100.0) as u32;
                if said != Some(percent) {
                    said = Some(percent);
                    let _ = proxy.send_event(Event::VideoProgress(format!("{name}  ·  {}", progress_words(share, started))));
                }
            });
            let mut done = Converted { created: Vec::new(), failed: 0, what, before: 0, after: 0, how: None, error: None, saving: true };
            match result {
                Ok(made) => {
                    println!("[section] {} {:.3}-{:.3} -> {}", path.display(), a, b, made.display());
                    done.created.push(made);
                }
                Err(error) => {
                    eprintln!("jog: cannot save the section of {}: {error}", path.display());
                    done.failed = 1;
                    done.error = Some(short_error(&path, &error));
                }
            }
            let _ = proxy.send_event(Event::Converted(done));
        });
    }

    /// Keeps the section marked, to come back to in the Sections tab; or,
    /// kept already, forgets it.
    pub(super) fn keep_section(&mut self) {
        let Some(video) = &self.video else { return self.toast("Sections are for videos") };
        let Some((a, b)) = self.section().and_then(|section| section.span()) else {
            return self.toast("No section · Shift+A, Shift+B");
        };
        let path = self.files[video.id].clone();
        let kept = self.kept_sections.get_or_insert_with(read_kept);
        let list = kept.entry(path).or_default();
        let forgetting = list.iter().any(|kept| kept.is(a, b));
        match forgetting {
            true => list.retain(|kept| !kept.is(a, b)),
            false => {
                list.push(Kept { a, b, name: None });
                list.sort_by(|x, y| x.a.total_cmp(&y.a));
            }
        }
        let result = write_kept(kept);
        match (result, forgetting) {
            (Err(error), _) => self.toast(format!("Not kept · {error}")),
            (Ok(()), true) => self.toast(format!("Removed {} – {}", clock(a), clock(b))),
            (Ok(()), false) => self.toast(format!("Kept {} – {}", clock(a), clock(b))),
        }
        self.redraw();
    }

    /// Keeps the section without a word, as saving it as a file does.
    fn keep_quietly(&mut self, path: &Path, a: f64, b: f64) {
        let kept = self.kept_sections.get_or_insert_with(read_kept);
        let list = kept.entry(path.to_path_buf()).or_default();
        if !list.iter().any(|kept| kept.is(a, b)) {
            list.push(Kept { a, b, name: None });
            list.sort_by(|x, y| x.a.total_cmp(&y.a));
            let _ = write_kept(kept);
        }
    }

    /// The kept section a menu or command is about: the one right-clicked
    /// in the Sections tab, else the one marked. A chapter is not one.
    fn target_entry(&mut self) -> Option<Entry> {
        let entries = self.entries();
        let marked = self.section().and_then(|section| section.span());
        let found = match self.entry_target {
            Some(index) => entries.get(index).cloned(),
            None => marked.and_then(|(a, b)| entries.into_iter().find(|entry| (entry.start - a).abs() < 0.05 && (entry.end - b).abs() < 0.05)),
        };
        found.filter(|entry| entry.kept)
    }

    /// Removes a kept section from the Sections tab.
    pub(super) fn forget_section(&mut self) {
        let (Some(entry), Some(video)) = (self.target_entry(), &self.video) else { return self.toast("No kept section here") };
        let path = self.files[video.id].clone();
        let kept = self.kept_sections.get_or_insert_with(read_kept);
        if let Some(list) = kept.get_mut(&path) {
            list.retain(|kept| !kept.is(entry.start, entry.end));
        }
        match write_kept(kept) {
            Ok(()) => self.toast(format!("Removed {} – {}", clock(entry.start), clock(entry.end))),
            Err(error) => self.toast(format!("Not removed · {error}")),
        }
        self.entry_target = None;
        // The tab goes with the last of them.
        if self.entries().is_empty() {
            self.strip_tab = StripTab::Scenes;
        }
        self.redraw();
    }

    /// Opens the box to name a kept section.
    pub(super) fn ask_section_name(&mut self) {
        let (Some(entry), Some(video)) = (self.target_entry(), &self.video) else { return self.toast("No kept section here") };
        let id = video.id;
        let text: Vec<char> = entry.title.unwrap_or_default().chars().collect();
        let kind = PromptKind::SectionName { start: entry.start, end: entry.end };
        self.prompt = Some(Prompt { id, kind, field: TextField::new(text.clone(), text.len()) });
        self.menu = None;
        self.redraw();
    }

    /// Gives a kept section the name typed; nothing typed takes it away.
    pub(super) fn name_section(&mut self, start: f64, end: f64, typed: &str) {
        let Some(video) = &self.video else { return };
        let path = self.files[video.id].clone();
        let kept = self.kept_sections.get_or_insert_with(read_kept);
        if let Some(found) = kept.get_mut(&path).and_then(|list| list.iter_mut().find(|kept| kept.is(start, end))) {
            found.name = Some(typed.trim().to_string()).filter(|name| !name.is_empty());
        }
        if let Err(error) = write_kept(kept) {
            self.toast(format!("Not saved · {error}"));
        }
        self.entry_target = None;
        self.redraw();
    }

    /// The sections kept and the chapters of the video on view, in order.
    pub(super) fn entries(&mut self) -> Vec<Entry> {
        let Some(video) = &self.video else { return Vec::new() };
        let id = video.id;
        let Some((_, length)) = self.video_time() else { return Vec::new() };
        let path = self.files[id].clone();
        let kept = self.kept_sections.get_or_insert_with(read_kept).get(&path).into_iter().flatten();
        let mut entries: Vec<Entry> = kept.map(|kept| Entry { start: kept.a, end: kept.b, title: kept.name.clone(), kept: true }).collect();
        // The file's chapters, read from the player once a video, when it
        // has the file open: each is a wait on it, not to be made a frame
        // until then.
        if video.showing && !self.chapters.contains_key(&id) {
            let found = self.player.as_ref().map(|player| {
                let count = player.number("chapter-list/count").unwrap_or(0.0) as usize;
                (0..count.min(200))
                    .filter_map(|n| {
                        let time = player.number(&format!("chapter-list/{n}/time"))?;
                        let title = player.text(&format!("chapter-list/{n}/title")).filter(|title| !title.is_empty()).unwrap_or_else(|| format!("Chapter {}", n + 1));
                        Some((time, title))
                    })
                    .collect::<Vec<_>>()
            });
            if let Some(found) = found {
                self.chapters.insert(id, found);
            }
        }
        let chapters = self.chapters.get(&id).cloned().unwrap_or_default();
        for (index, (time, title)) in chapters.iter().enumerate() {
            let end = chapters.get(index + 1).map_or(length, |(next, _)| *next);
            entries.push(Entry { start: *time, end, title: Some(title.clone()), kept: false });
        }
        entries.sort_by(|x, y| x.start.total_cmp(&y.start));
        entries
    }

    /// Asks for a frame from where each entry starts, unless they have
    /// been asked for already.
    fn want_entry_frames(&mut self, entries: &[Entry]) {
        let Some(video) = self.video.as_ref().filter(|video| video.showing) else { return };
        let id = video.id;
        let starts: Vec<f64> = entries.iter().map(|entry| entry.start).collect();
        if self.entry_frames.get(&id).is_some_and(|had| had.starts == starts) || starts.is_empty() {
            return;
        }
        self.entry_frames.insert(id, EntryFrames { frames: vec![None; starts.len()], starts: starts.clone() });
        let proxy = self.proxy.clone();
        let finder = self.section_finder.get_or_insert_with(|| scenes::Finder::new(move |found| proxy.send_event(Event::EntryFrame(found)).is_ok()));
        finder.want(id, self.files[id].clone(), starts);
    }

    pub(super) fn entry_frame(&mut self, found: scenes::Found) {
        let scenes::Found(id, index, frame) = found;
        if let Some(place) = self.entry_frames.get_mut(&id).and_then(|had| had.frames.get_mut(index)) {
            *place = Some(Arc::new(frame));
        }
        self.redraw();
    }

    /// T: the scenes; again, the sections and chapters, when there are
    /// any; again, nothing.
    pub(super) fn cycle_strip(&mut self) {
        if self.video.is_none() {
            return self.toast("No scenes here");
        }
        let has_entries = !self.entries().is_empty();
        (self.scenes_on, self.strip_tab) = match (self.scenes_on, self.strip_tab) {
            (false, _) => (true, StripTab::Scenes),
            (true, StripTab::Scenes) if has_entries => (true, StripTab::Sections),
            _ => (false, StripTab::Scenes),
        };
        self.redraw();
    }

    /// How many cells fit across, clear of the tabs at the left.
    fn entries_across(strip: &Strip, window_width: i32) -> usize {
        ((window_width - strip.pad * 24 + strip.pad) / strip.pitch()).max(1) as usize
    }

    /// Where the cell of an entry stands: the row centered as the scenes'
    /// when it fits, else from the first one scrolled to.
    fn entry_left(&self, strip: &Strip, index: usize, count: usize, window_width: i32) -> i32 {
        if count <= Self::entries_across(strip, window_width) {
            let row = count as i32 * strip.pitch() - strip.pad;
            return (window_width - row).max(strip.pad * 12) / 2 + index as i32 * strip.pitch();
        }
        strip.pad * 12 + (index as i32 - self.entry_first as i32) * strip.pitch()
    }

    /// Whether a cell standing at `left` is drawn: clear of the tabs and
    /// whole within the window.
    fn entry_in_view(&self, strip: &Strip, left: i32, window_width: i32) -> bool {
        left >= strip.pad * 12 && left + strip.cell_w <= window_width
    }

    /// Scrolls the Sections tab by cells, as far as there are more.
    pub(super) fn scroll_entries(&mut self, by: i64) {
        let (width, height) = self.window_size();
        let count = self.entries().len();
        let furthest = count.saturating_sub(Self::entries_across(&Strip::for_height(height), width as i32));
        self.entry_first = (self.entry_first as i64 + by).clamp(0, furthest as i64) as usize;
        self.redraw();
    }

    /// Scrolls so that an entry's cell is in view.
    fn show_entry(&mut self, index: usize) {
        let (width, height) = self.window_size();
        let across = Self::entries_across(&Strip::for_height(height), width as i32);
        if index < self.entry_first {
            self.entry_first = index;
        } else if index >= self.entry_first + across {
            self.entry_first = index + 1 - across;
        }
    }

    /// Whether the pointer is over the Sections tab's cells.
    pub(super) fn over_entries(&self, y: f64) -> bool {
        let (_, height) = self.window_size();
        self.strip_tab == StripTab::Sections && self.scenes_shown() && y >= f64::from(Strip::for_height(height).top)
    }

    /// The entry whose cell a window position is on.
    pub(super) fn entry_at(&mut self, x: f64, y: f64) -> Option<usize> {
        let (width, height) = self.window_size();
        let strip = Strip::for_height(height);
        if !self.over_entries(y) {
            return None;
        }
        let count = self.entries().len();
        (0..count).find(|&index| {
            let left = self.entry_left(&strip, index, count, width as i32);
            self.entry_in_view(&strip, left, width as i32) && (left..left + strip.cell_w).contains(&(x as i32))
        })
    }

    /// The two tabs, at the strip's left end: their words and places.
    fn strip_tabs(&self, strip: &Strip, px: u32, count: usize) -> [(StripTab, String, [i32; 4]); 2] {
        let small = (px * 3 / 4).max(10);
        let line = Text::line_height(small);
        let h = line + strip.pad / 2;
        let w = small as i32 * 8;
        let left = strip.pad;
        let middle = strip.top + strip.height / 2;
        [
            (StripTab::Scenes, "Scenes".to_string(), [left, middle - h - strip.pad / 4, w, h]),
            (StripTab::Sections, format!("Sections {count}"), [left, middle + strip.pad / 4, w, h]),
        ]
    }

    /// A click on the strip: on a tab, that tab; on an entry, that
    /// stretch marked as the section, looping. Whether it was either.
    pub(super) fn strip_click(&mut self, x: f64, y: f64, px: u32) -> bool {
        let (_, height) = self.window_size();
        let strip = Strip::for_height(height);
        let count = self.entries().len();
        let hit = self.strip_tabs(&strip, px, count).into_iter().find(|(_, _, [l, t, w, h])| (*l..l + w).contains(&(x as i32)) && (*t..t + h).contains(&(y as i32)));
        if let Some((tab, ..)) = hit {
            if tab == StripTab::Sections && count == 0 {
                self.toast("No sections yet");
            } else {
                self.strip_tab = tab;
            }
            self.redraw();
            return true;
        }
        if self.strip_tab != StripTab::Sections {
            return false;
        }
        if let Some(index) = self.entry_at(x, y) {
            self.choose_entry(index);
        }
        true
    }

    /// Marks an entry as the section, and plays it from its start.
    fn choose_entry(&mut self, index: usize) {
        let entries = self.entries();
        let (Some(entry), Some(video)) = (entries.get(index), &self.video) else { return };
        let Some((_, length)) = self.video_time() else { return };
        self.sections.insert(video.id, Section { a: Some(entry.start), b: Some(entry.end) });
        self.show_entry(index);
        self.apply_loop();
        self.seek_to(entry.start / length, true);
        self.say_section();
    }

    /// Left and Right in the Sections tab: the entry after or before the
    /// one marked. Whether there were entries to go by.
    pub(super) fn entry_jump(&mut self, forward: bool) -> bool {
        let entries = self.entries();
        if entries.is_empty() {
            return false;
        }
        let at = self.section().and_then(|section| section.a);
        let now = at.and_then(|a| entries.iter().position(|entry| (entry.start - a).abs() < 0.05));
        let to = match (now, forward) {
            (None, _) => 0,
            (Some(index), true) => (index + 1).min(entries.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
        };
        self.choose_entry(to);
        true
    }

    /// The cells of the Sections tab: where each starts, the one marked
    /// ringed.
    pub(super) fn draw_entries(&mut self, canvas: &mut Canvas, strip: Strip) {
        canvas.shade(0, strip.top, canvas.width, strip.height, 64);
        let entries = self.entries();
        self.want_entry_frames(&entries);
        // Scrolled no further than the last cells fill the row.
        let furthest = entries.len().saturating_sub(Self::entries_across(&strip, canvas.width));
        self.entry_first = self.entry_first.min(furthest);
        let (mx, my) = self.mouse;
        let over = self.entry_at(mx, my);
        let id = self.video.as_ref().map(|video| video.id);
        let frames = id.and_then(|id| self.entry_frames.get(&id));
        let marked = self.section().and_then(|section| section.span());
        let top = strip.top + strip.pad;
        let (round, edge) = ((strip.pad * 2 / 3).max(3), (strip.pad / 4).max(2));
        for (index, entry) in entries.iter().enumerate() {
            let left = self.entry_left(&strip, index, entries.len(), canvas.width);
            if !self.entry_in_view(&strip, left, canvas.width) {
                continue;
            }
            let frame = frames.filter(|had| had.starts.get(index) == Some(&entry.start)).and_then(|had| had.frames.get(index)).and_then(Option::as_ref);
            let [x, y, w, h] = match frame {
                Some(frame) => {
                    let (w, h) = fit_size((frame.width, frame.height), (strip.cell_w as u32, strip.cell_h as u32), false);
                    let (w, h) = (w as i32, h as i32);
                    let place = [left + (strip.cell_w - w) / 2, top + (strip.cell_h - h) / 2, w, h];
                    canvas.image_rounded(frame, place[0], place[1], w, h, round);
                    place
                }
                None => {
                    canvas.round_fill(left, top, strip.cell_w, strip.cell_h, round, PLACEHOLDER_COLOR, 256);
                    [left, top, strip.cell_w, strip.cell_h]
                }
            };
            let is_marked = marked.is_some_and(|(a, b)| (a - entry.start).abs() < 0.05 && (b - entry.end).abs() < 0.05);
            let ring = if is_marked { Some(accent()) } else { (over == Some(index)).then_some(accent_soft()) };
            if let Some(color) = ring {
                canvas.round_outline(x - edge, y - edge, w + 2 * edge, h + 2 * edge, round + edge, edge, color);
            }
        }
    }

    /// The words of the strip: the tabs, and on each entry its stretch
    /// and, for a chapter, its title.
    pub(super) fn draw_strip_words(&mut self, canvas: &mut Canvas, text: &mut Text, strip: &Strip, px: u32) {
        let entries = self.entries();
        for (tab, words, [l, t, w, h]) in self.strip_tabs(strip, px, entries.len()) {
            let chosen = tab == self.strip_tab;
            let small = (px * 3 / 4).max(10);
            canvas.round_fill(l, t, w, h, h / 2, if chosen { accent() } else { 0x000000 }, if chosen { 256 } else { 150 });
            let color = if chosen { on_accent() } else if tab == StripTab::Sections && entries.is_empty() { DIM_TEXT_COLOR } else { TEXT_COLOR };
            let tw = text.width(&words, small);
            text.draw(canvas, l + (w - tw) / 2, t + (h - Text::line_height(small)) / 2, small, color, &words);
        }
        if self.strip_tab != StripTab::Sections {
            return;
        }
        let small = (px * 3 / 4).max(10);
        let (top, pad, line) = (strip.top + strip.pad, (small as i32 / 4).max(2), Text::line_height(small));
        // Arrows at the ends of a row that runs on past them.
        let middle = top + (strip.cell_h - Text::line_height(px)) / 2;
        let across = Self::entries_across(strip, canvas.width);
        if entries.len() > across {
            if self.entry_first > 0 {
                text.draw(canvas, strip.pad * 12 - strip.pad * 3 / 2, middle, px, TEXT_COLOR, "‹");
            }
            if self.entry_first + across < entries.len() {
                text.draw(canvas, canvas.width - strip.pad * 2, middle, px, TEXT_COLOR, "›");
            }
        }
        for (index, entry) in entries.iter().enumerate() {
            let left = self.entry_left(strip, index, entries.len(), canvas.width);
            if !self.entry_in_view(strip, left, canvas.width) {
                continue;
            }
            let span = format!("{} – {}", clock(entry.start), clock(entry.end));
            let bottom = top + strip.cell_h;
            let badge_w = text.width(&span, small) + 2 * pad;
            canvas.round_fill(left + strip.cell_w - badge_w - pad, bottom - line - pad, badge_w, line, pad, 0x000000, 190);
            text.draw(canvas, left + strip.cell_w - badge_w, bottom - line - pad, small, TEXT_COLOR, &span);
            if let Some(title) = &entry.title {
                let title_w = (text.width(title, small) + 2 * pad).min(strip.cell_w - 2 * pad);
                canvas.round_fill(left + pad, top + pad, title_w, line, pad, 0x000000, 190);
                text.draw(canvas, left + 2 * pad, top + pad, small, TEXT_COLOR, title);
            }
        }
    }

    /// Draws the section over the seek bar's track: the stretch between
    /// the ends in the accent, and each end as a flag to take hold of.
    /// Gives back where the flags' letters stand across, for the time under
    /// the pointer to stay clear of.
    pub(super) fn draw_section(&self, canvas: &mut Canvas, bar: &SeekBar, top: i32, thickness: i32, length: f64, labels: &mut Vec<(i32, i32, Ordering, String)>, px: u32) -> Vec<i32> {
        let Some(section) = self.section() else { return Vec::new() };
        let x_of = |at: f64| bar.x + (f64::from(bar.width) * at / length) as i32;
        if let Some((a, b)) = section.span() {
            let (left, right) = (x_of(a), x_of(b));
            canvas.round_fill(left, top - thickness / 3, (right - left).max(2), thickness + thickness * 2 / 3, thickness / 2, accent(), 200);
        }
        let line = Text::line_height(px);
        // Ends close together put their letters back to back, A to the
        // left of its flag and B to the right, rather than one on the other.
        let close = match (section.a, section.b) {
            (Some(a), Some(b)) => (x_of(b) - x_of(a)).abs() < line,
            _ => false,
        };
        let mut flags = Vec::new();
        for (edge, at) in [(Edge::A, section.a), (Edge::B, section.b)] {
            let Some(at) = at else { continue };
            let x = x_of(at);
            let tall = thickness * 3;
            canvas.fill(x - 1, bar.middle - tall / 2, 3, tall, accent_soft());
            let (word, side) = match edge {
                Edge::A => ("A", if close { Ordering::Less } else { Ordering::Equal }),
                Edge::B => ("B", if close { Ordering::Greater } else { Ordering::Equal }),
            };
            labels.push((x, bar.middle - tall / 2 - line, side, word.to_string()));
            flags.push(x);
        }
        flags
    }
}
