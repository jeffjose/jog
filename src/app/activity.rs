//! Activity: what jog has been doing that takes a while, and what it has
//! said. A conversion or a section saved is a job: while it runs a pill at
//! the top right says how far along it is, for as long as it runs, rather
//! than a toast that comes and goes; done, it is kept, with the files it
//! made, to be shown, copied, renamed or found in their folder. The panel,
//! on the right over the picture, lists the jobs, the latest first, and
//! under them the toasts of late; Clear lets go of what is over.

use std::path::{Path, PathBuf};

use winit::keyboard::{Key, NamedKey};

use super::draw::{rich_block, run_width, CHOICE};
use super::*;

/// How a job stands.
pub(super) enum JobState {
    Running { share: f64, words: String },
    /// Done: the files it made, and what the toast said of them.
    Done { outputs: Vec<PathBuf>, said: String },
    Failed(String),
    Canceled,
}

pub(super) struct Job {
    id: u64,
    /// What it is, as it began: "Converting 3 → AVIF", say.
    title: String,
    ended: Option<Instant>,
    state: JobState,
}

/// A click on the panel or the pill.
#[derive(Clone, PartialEq, Debug)]
enum Hit {
    Close,
    Clear,
    Cancel,
    Open,
    Show(PathBuf),
    Copy(PathBuf),
    Rename(PathBuf),
    Folder(PathBuf),
    Message(usize),
}

#[derive(Default)]
pub struct Activity {
    jobs: Vec<Job>,
    next: u64,
    /// The panel is up.
    pub(super) open: bool,
    /// How far down the panel is scrolled, in pixels.
    scroll: i32,
    /// Where the panel's parts were drawn, and what a click there does; and
    /// the pill's.
    pill: Vec<(Hit, [i32; 4])>,
    drawn: Vec<(Hit, [i32; 4])>,
    /// Where the panel is, for clicks and the wheel over it.
    place: Option<[i32; 4]>,
}

impl Activity {
    fn running(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|job| matches!(job.state, JobState::Running { .. }))
    }
}

/// How long ago, in a word or two.
fn ago(at: Instant) -> String {
    match at.elapsed().as_secs() {
        0..=59 => "just now".to_string(),
        seconds @ 60..=3599 => format!("{} min ago", seconds / 60),
        seconds => format!("{} h ago", seconds / 3600),
    }
}

impl App {
    /// A job begun, said so in the pill; its number, for what it says next.
    pub(super) fn job_begin(&mut self, title: String) -> u64 {
        let activity = &mut self.activity;
        activity.next += 1;
        let id = activity.next;
        activity.jobs.insert(0, Job { id, title, ended: None, state: JobState::Running { share: 0.0, words: String::new() } });
        self.redraw();
        id
    }

    /// How far along a job is, `share` of it, and in words.
    pub(super) fn job_progress(&mut self, id: u64, share: f64, words: String) {
        if let Some(job) = self.activity.jobs.iter_mut().find(|job| job.id == id) {
            job.state = JobState::Running { share, words };
            self.redraw();
        }
    }

    /// A job over, however it went.
    pub(super) fn job_end(&mut self, id: u64, state: JobState) {
        if let Some(job) = self.activity.jobs.iter_mut().find(|job| job.id == id) {
            job.state = state;
            job.ended = Some(Instant::now());
        }
        self.redraw();
    }

    /// A file made at once, a frame or a copy: kept as a job done, for
    /// finding again.
    pub(super) fn job_made(&mut self, path: &Path, said: &str) {
        let id = self.job_begin(said.to_string());
        self.job_end(id, JobState::Done { outputs: vec![path.to_path_buf()], said: said.to_string() });
    }

    /// A file renamed: the jobs that made it name it as it is now.
    pub(super) fn job_renamed(&mut self, from: &Path, to: &Path) {
        for job in &mut self.activity.jobs {
            if let JobState::Done { outputs, .. } = &mut job.state {
                for output in outputs.iter_mut().filter(|output| *output == from) {
                    *output = to.to_path_buf();
                }
            }
        }
    }

    pub(super) fn toggle_activity(&mut self) {
        self.activity.open = !self.activity.open;
        self.activity.scroll = 0;
        self.redraw();
    }

    fn activity_hit(&mut self, hit: Hit) {
        match hit {
            Hit::Close => self.activity.open = false,
            Hit::Open => self.activity.open = !self.activity.open,
            Hit::Clear => {
                self.activity.jobs.retain(|job| matches!(job.state, JobState::Running { .. }));
                self.messages.clear();
                self.activity.scroll = 0;
            }
            Hit::Cancel => {
                if crate::transcode::cancel() {
                    self.toast("Canceling…");
                }
            }
            Hit::Show(path) => self.show_path(&path),
            Hit::Copy(path) => {
                let full = std::path::absolute(&path).unwrap_or(path);
                let copied = self.snapshot.is_some() || crate::ops::set_clipboard(&full.to_string_lossy());
                self.toast(if copied { format!("Copied  ·  {}", full.display()) } else { "No clipboard tool (xclip, xsel, wl-copy)".to_string() });
            }
            Hit::Rename(path) => {
                self.show_path(&path);
                let on_view = self.list.get(self.focus()).is_some_and(|id| self.files[*id] == path);
                if on_view {
                    self.start_rename();
                }
            }
            Hit::Folder(path) => {
                let folder = path.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
                let opened = std::process::Command::new("xdg-open").arg(&folder).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
                if opened.is_err() {
                    self.toast("No xdg-open to open the folder with");
                }
            }
            Hit::Message(at) => {
                if let Some((_, said)) = self.messages.get(at) {
                    let said = said.clone();
                    let copied = self.snapshot.is_some() || crate::ops::set_clipboard(&said);
                    self.toast(if copied { "Copied" } else { "No clipboard tool (xclip, xsel, wl-copy)" });
                }
            }
        }
        self.redraw();
    }

    /// A click on the panel or the pill: whether it was one, done.
    pub(super) fn activity_click(&mut self, x: f64, y: f64) -> bool {
        let inside = |[left, top, w, h]: [i32; 4]| x >= f64::from(left) && x < f64::from(left + w) && y >= f64::from(top) && y < f64::from(top + h);
        let open = self.activity.open;
        let hits = self.activity.pill.iter().chain(self.activity.drawn.iter().filter(|_| open));
        if let Some(hit) = hits.rev().find(|(_, place)| inside(*place)).map(|(hit, _)| hit.clone()) {
            self.activity_hit(hit);
            return true;
        }
        // Elsewhere on the panel: nothing, but not the picture under it.
        self.activity.open && self.activity.place.is_some_and(inside)
    }

    /// The wheel over the panel scrolls it: whether it was over it.
    pub(super) fn activity_wheel(&mut self, lines: f64) -> bool {
        let (x, y) = self.mouse;
        let over = self.activity.open && self.activity.place.is_some_and(|[left, top, w, h]| x >= f64::from(left) && x < f64::from(left + w) && y >= f64::from(top) && y < f64::from(top + h));
        if over {
            let step = Text::line_height(ui_px(self.window_size().1)) * 2;
            self.activity.scroll = (self.activity.scroll - (lines * f64::from(step)) as i32).max(0);
            self.redraw();
        }
        over
    }

    /// Esc closes the panel: whether the key was its.
    pub(super) fn activity_key(&mut self, key: &Key<&str>) -> bool {
        if self.activity.open && *key == Key::Named(NamedKey::Escape) {
            self.activity.open = false;
            self.redraw();
            return true;
        }
        false
    }

    /// While anything runs, at the top right below where a toast goes: how
    /// far along, for as long as it runs, with Cancel and the panel a click
    /// away.
    pub(super) fn draw_activity_pill(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32, right: i32, top: i32) {
        self.activity.pill.clear();
        let said = {
            let running: Vec<&Job> = self.activity.running().collect();
            let Some(first) = running.first() else { return };
            let JobState::Running { words, .. } = &first.state else { return };
            let said = if words.is_empty() { format!("{}…", first.title) } else { words.clone() };
            match running.len() {
                1 => said,
                more => format!("{said}  ·  {} more", more - 1),
            }
        };
        let line: Vec<(String, u32)> = vec![(format!("{}  {said}", commands::icons::REPLACE), TEXT_COLOR), ("  ".to_string(), TEXT_COLOR), (format!("{CHOICE}Cancel"), TEXT_COLOR), (format!("{CHOICE}Activity"), TEXT_COLOR)];
        let pad = px as i32 / 2;
        let w = line.iter().map(|(run, _)| run_width(text, run, px)).sum::<i32>() + 3 * pad;
        let (_, choices) = rich_block(canvas, text, &[line], px, (right - w, top));
        for (name, place) in choices {
            let hit = if name == "Cancel" { Hit::Cancel } else { Hit::Open };
            self.activity.pill.push((hit, place));
        }
    }

    /// The panel: on the right, over the picture, as tall as the window.
    pub(super) fn draw_activity(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        if !self.activity.open {
            self.activity.place = None;
            return;
        }
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let width = draw::panel_width(canvas.width, px, 30.0);
        let right = canvas.width - self.list_right_inset();
        let (left, top, height) = (right - width, 0, canvas.height);
        canvas.fill(left, top, width, height, GRID_COLOR);
        canvas.fill(left, top, 1, height, MENU_EDGE_COLOR);
        self.activity.place = Some([left, top, width, height]);
        let mut drawn: Vec<(Hit, [i32; 4])> = Vec::new();
        let inner = left + pad;
        let inner_w = width - 2 * pad;
        let mut y = top + pad;

        // A small button: its words on a ground lit under the pointer.
        let pill = |canvas: &mut Canvas, text: &mut Text, x: i32, y: i32, words: &str| -> [i32; 4] {
            let (w, h) = (text.width(words, small) + pad, small_line + pad / 3);
            let place = [x, y, w, h];
            let over = (x..x + w).contains(&mouse.0) && (y..y + h).contains(&mouse.1);
            canvas.round_fill(x, y, w, h, h / 2, if over { PLACEHOLDER_COLOR } else { 0x1C1C1C }, 256);
            text.draw(canvas, x + pad / 2, y + pad / 6, small, TEXT_COLOR, words);
            place
        };

        // The heading, Clear and the way out.
        text.draw_strong(canvas, inner, y, px, TEXT_COLOR, "Activity");
        let close = pad * 3 / 2;
        let (close_x, close_y) = (inner + inner_w - close, y + (line - close) / 2);
        let button = Button { x: close_x, y: close_y, width: close, height: close, command: Command::Activity };
        draw::round_button(canvas, text, &button, commands::icons::X, px, (close_x..close_x + close).contains(&mouse.0) && (close_y..close_y + close).contains(&mouse.1), false, false);
        drawn.push((Hit::Close, [close_x, close_y, close, close]));
        let over = self.activity.jobs.iter().any(|job| !matches!(job.state, JobState::Running { .. })) || !self.messages.is_empty();
        if over {
            let w = text.width("Clear", small) + pad;
            let place = pill(canvas, text, close_x - pad / 2 - w, y + (line - small_line - pad / 3) / 2, "Clear");
            drawn.push((Hit::Clear, place));
        }
        y += line + pad;

        // What follows scrolls; the heading stays.
        let content_top = y;
        let bottom = top + height - pad;
        let mut y = content_top - self.activity.scroll;
        let fits = |y: i32, h: i32| y >= content_top && y + h <= bottom;
        let heading = |canvas: &mut Canvas, text: &mut Text, y: &mut i32, words: &str| {
            if fits(*y, small_line) {
                text.draw_strong(canvas, inner, *y, small, DIM_TEXT_COLOR, words);
            }
            *y += small_line + pad / 3;
        };

        if self.activity.jobs.is_empty() && self.messages.is_empty() {
            // Nothing at all: said in the middle, quietly.
            let middle = top + height / 2 - line;
            for (at, (words, color)) in [("Nothing yet", TEXT_COLOR), ("Conversions and saves show here", DIM_TEXT_COLOR)].into_iter().enumerate() {
                let w = text.width(words, small);
                text.draw(canvas, inner + (inner_w - w) / 2, middle + at as i32 * small_line, small, color, words);
            }
        } else if self.activity.jobs.is_empty() {
            if fits(y, line) {
                text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Nothing yet  ·  conversions and saves show here");
            }
            y += line + pad;
        }
        let running: Vec<usize> = (0..self.activity.jobs.len()).filter(|at| matches!(self.activity.jobs[*at].state, JobState::Running { .. })).collect();
        if !running.is_empty() {
            heading(canvas, text, &mut y, "Now");
            for at in running {
                let job = &self.activity.jobs[at];
                let JobState::Running { share, words } = &job.state else { continue };
                if fits(y, line) {
                    draw_clipped(canvas, text, inner, y, px, TEXT_COLOR, &job.title, inner_w);
                }
                y += line;
                if fits(y, pad) {
                    let thick = (pad / 3).max(3);
                    canvas.round_fill(inner, y + pad / 3, inner_w, thick, thick / 2, 0x2C2C2C, 256);
                    canvas.round_fill(inner, y + pad / 3, ((f64::from(inner_w) * share.clamp(0.0, 1.0)) as i32).max(thick), thick, thick / 2, accent(), 256);
                }
                y += pad;
                if fits(y, small_line + pad / 3) {
                    let said = if words.is_empty() { "starting…" } else { words.as_str() };
                    draw_clipped(canvas, text, inner, y, small, DIM_TEXT_COLOR, said, inner_w - pad * 5);
                    let w = text.width("Cancel", small) + pad;
                    let place = pill(canvas, text, inner + inner_w - w, y, "Cancel");
                    drawn.push((Hit::Cancel, place));
                }
                y += small_line + pad;
            }
        }
        let finished: Vec<usize> = (0..self.activity.jobs.len()).filter(|at| !matches!(self.activity.jobs[*at].state, JobState::Running { .. })).collect();
        if !finished.is_empty() {
            heading(canvas, text, &mut y, "Done");
            for at in finished {
                let job = &self.activity.jobs[at];
                let when = job.ended.map(ago).unwrap_or_default();
                let when_w = text.width(&when, small);
                match &job.state {
                    JobState::Done { outputs, said } => {
                        // What the toast said of it, then each file it made,
                        // with what can be done with it.
                        if fits(y, line) {
                            draw_clipped(canvas, text, inner, y, small, DIM_TEXT_COLOR, said, inner_w - when_w - pad);
                            text.draw(canvas, inner + inner_w - when_w, y, small, DIM_TEXT_COLOR, &when);
                        }
                        y += small_line + pad / 3;
                        for output in outputs.iter().take(20) {
                            let name = output.file_name().unwrap_or_default().to_string_lossy().into_owned();
                            let gone = !output.exists();
                            if fits(y, line) {
                                let color = if gone { DIM_TEXT_COLOR } else { TEXT_COLOR };
                                let name = if gone { format!("{name}  ·  gone") } else { name };
                                draw_clipped(canvas, text, inner, y, px, color, &name, inner_w);
                            }
                            y += line;
                            if !gone && fits(y, small_line + pad / 3) {
                                let mut x = inner;
                                for (words, hit) in [("Show", Hit::Show(output.clone())), ("Copy path", Hit::Copy(output.clone())), ("Rename", Hit::Rename(output.clone())), ("Folder", Hit::Folder(output.clone()))] {
                                    let place = pill(canvas, text, x, y, words);
                                    x += place[2] + pad / 3;
                                    drawn.push((hit, place));
                                }
                            }
                            if !gone {
                                y += small_line + pad / 3;
                            }
                            y += pad / 3;
                        }
                        if outputs.len() > 20 && fits(y, small_line) {
                            text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, &format!("and {} more", outputs.len() - 20));
                            y += small_line;
                        }
                    }
                    JobState::Failed(error) => {
                        if fits(y, line) {
                            draw_clipped(canvas, text, inner, y, px, TEXT_COLOR, &job.title, inner_w - when_w - pad);
                            text.draw(canvas, inner + inner_w - when_w, y, small, DIM_TEXT_COLOR, &when);
                        }
                        y += line;
                        if fits(y, small_line) {
                            draw_clipped(canvas, text, inner, y, small, DANGER_COLOR, error, inner_w);
                        }
                        y += small_line;
                    }
                    JobState::Canceled => {
                        if fits(y, line) {
                            draw_clipped(canvas, text, inner, y, px, DIM_TEXT_COLOR, &format!("{}  ·  canceled", job.title), inner_w - when_w - pad);
                            text.draw(canvas, inner + inner_w - when_w, y, small, DIM_TEXT_COLOR, &when);
                        }
                        y += line;
                    }
                    JobState::Running { .. } => {}
                }
                y += pad / 2;
            }
        }
        // What jog has said of late, the latest first: a click copies it.
        if !self.messages.is_empty() {
            y += pad / 2;
            heading(canvas, text, &mut y, "Messages");
            for at in (0..self.messages.len()).rev() {
                let (when, said) = &self.messages[at];
                let when = ago(*when);
                let when_w = text.width(&when, small);
                if fits(y, small_line) {
                    let place = [inner, y, inner_w, small_line];
                    if (inner..inner + inner_w).contains(&mouse.0) && (y..y + small_line).contains(&mouse.1) {
                        canvas.round_fill(inner - pad / 3, y, inner_w + pad * 2 / 3, small_line, pad / 3, 0x1C1C1C, 256);
                    }
                    draw_clipped(canvas, text, inner, y, small, TEXT_COLOR, said, inner_w - when_w - pad);
                    text.draw(canvas, inner + inner_w - when_w, y, small, DIM_TEXT_COLOR, &when);
                    drawn.push((Hit::Message(at), place));
                }
                y += small_line + pad / 4;
            }
        }
        // Not past the end.
        let length = y + self.activity.scroll - content_top;
        let room = bottom - content_top;
        if self.activity.scroll > (length - room).max(0) {
            self.activity.scroll = (length - room).max(0);
        }
        self.activity.drawn = drawn;
    }
}

/// Words cut to `room`, drawn.
#[allow(clippy::too_many_arguments)]
fn draw_clipped(canvas: &mut Canvas, text: &mut Text, x: i32, y: i32, px: u32, color: u32, words: &str, room: i32) {
    let words = clipped(text, words, room, px);
    text.draw(canvas, x, y, px, color, &words);
}

impl App {
    /// Whether any job is running.
    pub(super) fn activity_running(&self) -> bool {
        self.activity.running().next().is_some()
    }
}
