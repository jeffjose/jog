//! The find box (/ and Ctrl+K): files, commands, places, workspaces and
//! messages; and the search a list can be narrowed to.

use super::*;

impl App {

    /// Opens the box for finding files, on `/` and Ctrl+K. A search kept
    /// earlier comes back as it was typed, ready to be changed.
    /// Opens the box with something already typed: `>sort `, say, to
    /// pick among the ways of sorting by typing a few letters.
    pub(super) fn open_palette_with(&mut self, typed: &str) {
        self.open_palette();
        if let Some(palette) = &mut self.palette {
            palette.field = TextField::new(typed.chars().collect(), typed.chars().count());
        }
        self.palette_changed();
    }

    pub(super) fn open_palette(&mut self) {
        let text: Vec<char> = self.filter.as_ref().map(|(typed, _)| typed.chars().collect()).unwrap_or_default();
        let field = TextField::new(text.clone(), text.len());
        self.menu = None;
        self.palette = Some(Palette { field, matches: Vec::new(), narrowed: false, selected: 0, focus: 0, drawn: Vec::new(), commands: Vec::new(), first: 0 });
        self.palette_changed();
        // Start on the file on view, where it is among the matches.
        let current = self.list[self.focus()];
        if let Some(palette) = &mut self.palette {
            palette.selected = palette.matches.iter().position(|id| *id == current).unwrap_or(0);
        }
        let rows = self.palette_layout().map_or(1, |layout| layout.rows.max(1));
        if let Some(palette) = &mut self.palette {
            palette.show_selected(rows);
        }
    }

    /// Finds the matches for what the box now says, out of every file:
    /// all of them, in order, with nothing typed, otherwise the closest
    /// match first.
    pub(super) fn palette_changed(&mut self) {
        let Some(palette) = &self.palette else { return };
        let typed = palette.field.string();
        // Nothing typed: the files viewed lately, then the commands.
        let recent = typed.is_empty().then(|| [self.recent_files(), self.recent_commands()].concat()).filter(|recent| !recent.is_empty());
        let (matches, narrowed, commands) = match typed.chars().next() {
            // Nothing typed yet: the commands run from here lately, the
            // latest first, to run again at once. A letter finds files.
            None if recent.is_some() => (Vec::new(), false, recent.unwrap_or_default()),
            // `>` and then a number is a size to find files by.
            Some('>') if !typed[1..].starts_with(|c: char| c.is_ascii_digit()) => (Vec::new(), false, self.commands_matching(&typed[1..])),
            Some(':') => self.place_named(&typed[1..]),
            Some('@') => (Vec::new(), false, self.spaces_matching(&typed[1..])),
            Some('~') => (Vec::new(), false, self.messages_matching(&typed[1..])),
            Some('!') => (Vec::new(), false, self.shell_matching(&typed[1..])),
            _ => {
                let (matches, narrowed) = self.matching(&typed);
                (matches, narrowed, Vec::new())
            }
        };
        if let Some(palette) = &mut self.palette {
            (palette.matches, palette.narrowed, palette.selected, palette.focus) = (matches, narrowed, 0, 0);
            palette.first = 0;
            palette.commands = commands;
        }
        self.request();
        self.redraw();
    }

    /// The commands whose names match what follows a `>` in the box, the
    /// closest first: everything in the menu that applies to the file on
    /// view, those in a submenu named with it, as in "Sort: Newest first".
    pub(super) fn commands_matching(&mut self, typed: &str) -> Vec<(String, &'static str, Command)> {
        let video = files::is_video(&self.files[self.list[self.focus()]]);
        let mut all = Vec::new();
        let mut groups: Vec<(&str, &'static [Option<MenuItem>])> = vec![("", commands::MENU)];
        while let Some((group, items)) = groups.pop() {
            for item in items.iter().flatten() {
                let applies = match item.applies {
                    Applies::Always => true,
                    Applies::Images => !video,
                    Applies::Videos => video,
                    Applies::Filtered => self.filter.is_some(),
                    Applies::Mixed => self.mixed(),
                };
                match item.command {
                    _ if !applies => {}
                    _ if !item.sub.is_empty() => groups.push((item.label, item.sub)),
                    // The box itself is not something to run from it.
                    Command::Search | Command::Palette | Command::CommandPalette => {}
                    command if group.is_empty() => all.push((self.menu_label(item), item.keys, command)),
                    command => all.push((format!("{group}: {}", self.menu_label(item)), item.keys, command)),
                }
            }
        }
        all.sort_by(|a, b| a.0.cmp(&b.0));
        // Those run lately come first, the latest first.
        let recent = self.recent_labels().to_vec();
        let lately = |label: &str| recent.iter().position(|had| had == label).unwrap_or(usize::MAX);
        all.sort_by_key(|entry| lately(&entry.0));
        // The Debug commands after the rest, unless run lately.
        all.sort_by_key(|entry| (lately(&entry.0), entry.0.starts_with("Debug:")));
        let query = Query::parse(typed);
        if query.is_empty() {
            return all;
        }
        let score = |name: &str, loose: bool| query.score(&Facts { name, video: false, marked: false, seen: None, verdict: None, folder: None, size: None }, loose);
        let pass = |loose: bool| -> Vec<(i32, (String, &'static str, Command))> {
            all.iter().filter_map(|entry| Some((score(&entry.0, loose)?, entry.clone()))).collect()
        };
        let mut scored = pass(false);
        if scored.is_empty() {
            scored = pass(true);
        }
        // Those run lately that match come first, the latest first; then
        // the rest, the closest match first.
        scored.sort_by_key(|(score, entry)| (lately(&entry.0), -score));
        scored.into_iter().map(|(_, entry)| entry).collect()
    }

    /// The names of the commands run from the box lately, the latest
    /// first, read from the state folder the first time.
    pub(super) fn recent_labels(&mut self) -> &[String] {
        self.palette_recent.get_or_insert_with(|| {
            let path = crate::positions::state_dir().map(|dir| dir.join("palette-recent"));
            let text = path.and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
            text.lines().filter(|line| !line.is_empty()).map(str::to_string).collect()
        })
    }

    /// The commands run lately that still apply here, as the box lists
    /// commands, the latest first.
    pub(super) fn recent_commands(&mut self) -> Vec<(String, &'static str, Command)> {
        let recent = self.recent_labels().to_vec();
        if recent.is_empty() {
            return Vec::new();
        }
        let all = self.commands_matching("");
        recent.iter().filter_map(|label| all.iter().find(|entry| entry.0 == *label).cloned()).collect()
    }

    /// The files viewed lately, the one on view aside, as the box lists
    /// commands: a few, the latest first.
    pub(super) fn recent_files(&mut self) -> Vec<(String, &'static str, Command)> {
        let current = self.list.get(self.pos).map(|&id| self.full_path(id));
        // Only those of this run's list: opening one file is not to show what
        // was viewed elsewhere, or before.
        let paths = self.ours(self.recent_paths());
        let others = paths.into_iter().filter(|path| Some(path) != current.as_ref() && path.exists());
        self.recent_shown = others.take(6).collect();
        // Named by the file, and by its folder too when it is another's.
        let here = current.as_ref().and_then(|path| path.parent().map(Path::to_path_buf));
        let said = |path: &PathBuf| {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match path.parent().filter(|dir| Some(dir.to_path_buf()) != here) {
                Some(dir) => format!("{name}  ·  {}", short_dir(dir)),
                None => name,
            }
        };
        self.recent_shown.iter().enumerate().map(|(at, path)| (said(path), "", Command::OpenRecent(at))).collect()
    }

    /// Of `paths`, in the order given, those this run has in its list:
    /// what was viewed, or went wrong, in other folders or other runs is
    /// not for whoever is looking at this one to see.
    pub(super) fn ours(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let listed: HashSet<PathBuf> = {
            // By name first, so that few of a long list are made absolute.
            let names: HashSet<&std::ffi::OsStr> = paths.iter().filter_map(|path| path.file_name()).collect();
            let named = self.files.iter().filter(|file| file.file_name().is_some_and(|name| names.contains(name)));
            named.map(|file| std::path::absolute(file).unwrap_or_else(|_| file.clone())).collect()
        };
        paths.into_iter().filter(|path| listed.contains(path)).collect()
    }

    pub(super) fn full_path(&self, id: usize) -> PathBuf {
        std::path::absolute(&self.files[id]).unwrap_or_else(|_| self.files[id].clone())
    }

    /// The files viewed lately, the latest first: in this run, then those
    /// kept from runs before, whatever folder they are in.
    pub(super) fn recent_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.visited.borrow().iter().map(|&id| self.full_path(id)).collect();
        let kept = crate::positions::state_dir().and_then(|dir| std::fs::read_to_string(dir.join("recent-files")).ok()).unwrap_or_default();
        for line in kept.lines().filter(|line| !line.is_empty()) {
            let path = PathBuf::from(line);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    /// Keeps the files viewed lately for the next run.
    pub(super) fn save_recent(&self) {
        let Some(dir) = crate::positions::state_dir() else { return };
        let text: String = self.recent_paths().iter().take(30).map(|path| format!("{}\n", path.display())).collect();
        let _ = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("recent-files"), text));
    }

    /// Puts a command first among those run lately, and keeps the list.
    pub(super) fn remember_command(&mut self, label: &str) {
        const KEPT: usize = 12;
        self.recent_labels();
        let recent = self.palette_recent.get_or_insert_with(Vec::new);
        recent.retain(|had| had != label);
        recent.insert(0, label.to_string());
        recent.truncate(KEPT);
        if let Some(dir) = crate::positions::state_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("palette-recent"), recent.join("\n") + "\n");
        }
    }

    /// What follows a `:` in the box names a place: `42` is the 42nd file
    /// of the list, `1:30` or `90s` a time in the video on view, and `50%`
    /// that far through the video, or through the list when no video is
    /// on view. Given back as the box's matches, whether they narrow the
    /// list (never), and its commands.
    pub(super) fn place_named(&mut self, typed: &str) -> (Vec<usize>, bool, Vec<(String, &'static str, Command)>) {
        let typed = typed.trim();
        let length = self.video_time().map(|(_, length)| length);
        let seek = |seconds: f64| vec![(format!("Go to {} in this video", clock(seconds)), "", Command::SeekTo(seconds))];
        let file = |app: &App, number: usize| vec![app.list[number.clamp(1, app.list.len()) - 1]];
        let number = |text: &str| text.parse::<f64>().ok().filter(|number| *number >= 0.0);
        if let Some(percent) = typed.strip_suffix('%').and_then(number) {
            let part = (percent / 100.0).min(1.0);
            return match length {
                Some(length) => (Vec::new(), false, seek(length * part)),
                None => (file(self, (part * self.list.len() as f64).ceil() as usize), false, Vec::new()),
            };
        }
        // Hours, minutes and seconds, or minutes and seconds, or seconds.
        let parts: Option<Vec<f64>> = typed.trim_end_matches('s').split(':').map(number).collect();
        let seconds = parts.filter(|parts| parts.len() <= 3).map(|parts| parts.iter().fold(0.0, |total, part| total * 60.0 + part));
        let timed = typed.contains(':') || typed.ends_with('s');
        match (seconds, length) {
            (Some(seconds), Some(length)) if timed => (Vec::new(), false, seek(seconds.min(length))),
            (Some(place), _) if !timed => (file(self, place as usize), false, Vec::new()),
            _ => (Vec::new(), false, Vec::new()),
        }
    }

    /// The buttons at the right of the box, in order: carry on with only
    /// the matches, when what is typed narrows the list, and go back to
    /// every file, when a search was kept before.
    pub(super) fn palette_buttons(&self) -> Vec<(PaletteEnd, String)> {
        let Some(palette) = &self.palette else { return Vec::new() };
        let mut buttons = Vec::new();
        // For a command, once a triage has decided about anything: the
        // lots it can be run on in place of the file on view.
        if palette.field.text.first() == Some(&'!') {
            let (kept, rejected, undecided) = self.tally();
            if kept + rejected > 0 {
                buttons.push((PaletteEnd::OnKept, format!("On {kept} kept")));
                buttons.push((PaletteEnd::OnRejected, format!("On {rejected} rejected")));
                buttons.push((PaletteEnd::OnEvery, format!("On all {}", kept + rejected + undecided)));
            }
            return buttons;
        }
        if palette.narrowed && !palette.matches.is_empty() {
            buttons.push((PaletteEnd::Keep, format!("Show only {}", palette.matches.len())));
        }
        if let Some((_, all)) = &self.filter {
            let back = self.back_label();
            buttons.push((PaletteEnd::All, format!("{}{}", back[..1].to_uppercase(), &back[1..])));
            buttons.push((PaletteEnd::Here, format!("Show all {}, stay here", all.len())));
        }
        buttons
    }

    pub(super) fn palette_key(&mut self, key: Key<&str>) {
        let rows = self.palette_layout().map_or(1, |layout| layout.rows.max(1));
        let buttons = self.palette_buttons();
        let Some(palette) = &mut self.palette else { return };
        palette.focus = palette.focus.min(buttons.len());
        let last = palette.rows().saturating_sub(1);
        let at_end = palette.field.cursor == palette.field.text.len();
        // Moving among the matches takes the keyboard back from a button.
        let pick = |palette: &mut Palette, to: usize| (palette.selected, palette.focus) = (to.min(last), 0);
        match key {
            Key::Named(NamedKey::Escape) => self.palette = None,
            Key::Named(NamedKey::Enter) => {
                let end = palette.focus.checked_sub(1).map_or(PaletteEnd::Go, |button| buttons[button].0);
                self.finish_palette(end);
            }
            // Tab goes round: from the text to each button in turn and
            // back, Shift+Tab the other way. Enter presses the one it is on.
            Key::Named(NamedKey::Tab) => {
                let stops = buttons.len() + 1;
                let back = self.modifiers.shift_key();
                palette.focus = (palette.focus + if back { stops - 1 } else { 1 }) % stops;
            }
            // Right from the end of the text goes on to the buttons, and
            // left comes back.
            Key::Named(NamedKey::ArrowRight) if at_end || palette.focus > 0 => {
                palette.focus = (palette.focus + 1).min(buttons.len());
            }
            Key::Named(NamedKey::ArrowLeft) if palette.focus > 0 => palette.focus -= 1,
            Key::Named(NamedKey::ArrowDown) => pick(palette, palette.selected + 1),
            Key::Named(NamedKey::ArrowUp) => pick(palette, palette.selected.saturating_sub(1)),
            Key::Named(NamedKey::PageDown) => pick(palette, palette.selected + rows),
            Key::Named(NamedKey::PageUp) => pick(palette, palette.selected.saturating_sub(rows)),
            key => {
                // Typing goes to the text wherever the keyboard was.
                palette.focus = 0;
                if palette.field.key(key, self.modifiers) {
                    return self.palette_changed();
                }
            }
        }
        // The list moves only as far as it must to keep the pick in view.
        if let Some(palette) = &mut self.palette {
            palette.show_selected(rows);
        }
        self.request();
        self.redraw();
    }

    /// Closes the box: going to the match picked out in it, carrying on
    /// with only its matches, or going back to every file.
    pub(super) fn finish_palette(&mut self, end: PaletteEnd) {
        let Some(palette) = self.palette.take() else { return };
        // A command is left for whoever closed the box to run, with what
        // running one needs.
        if let (PaletteEnd::Go, Some((label, _, command))) = (end, palette.commands.get(palette.selected)) {
            // What was typed is kept for it, the `!` aside.
            if *command == Command::RunTyped {
                self.shell_typed = Some(palette.field.string().chars().skip(1).collect());
            }
            // A command from the menu's, remembered to be offered first
            // next time.
            if matches!(palette.field.text.first(), None | Some('>')) && !matches!(command, Command::OpenRecent(_)) {
                self.remember_command(label);
            }
            self.run_next = Some(*command);
            return self.redraw();
        }
        // A command for a lot of a triage: the one typed, or the one picked
        // out from those run before, on each file of the lot in turn.
        let lot = match end {
            PaletteEnd::OnKept => Some(self.lot(Some(Verdict::Kept))),
            PaletteEnd::OnRejected => Some(self.lot(Some(Verdict::Rejected))),
            PaletteEnd::OnEvery => Some(self.everything()),
            _ => None,
        };
        if let Some(ids) = lot {
            let template = match palette.commands.get(palette.selected) {
                Some((_, _, Command::RunShell(at))) => self.shell_history.get_or_insert_with(history::load).get(*at).cloned().unwrap_or_default(),
                _ => palette.field.string().chars().skip(1).collect(),
            };
            match ids.is_empty() {
                true => self.toast("None to run it on"),
                false => self.run_shell_on(&template, ids, true),
            }
            return self.redraw();
        }
        let picked = palette.matches.get(palette.selected).copied();
        match end {
            PaletteEnd::Keep => {
                let typed = palette.field.string().trim().to_string();
                self.keep_matches(typed, palette.matches, picked);
            }
            PaletteEnd::All => self.clear_filter(false),
            PaletteEnd::Here => self.clear_filter(true),
            PaletteEnd::OnKept | PaletteEnd::OnRejected | PaletteEnd::OnEvery => {}
            PaletteEnd::Go => {
                let Some(id) = picked else { return self.redraw() };
                // One that a kept search has hidden brings every file back.
                if !self.list.contains(&id) {
                    self.clear_filter(true);
                }
                let pos = self.list.iter().position(|other| *other == id).unwrap_or(self.pos);
                // From the grid, the file is opened as Enter on its cell would.
                if let Some(grid) = &mut self.grid {
                    grid.cursor = pos;
                    return self.open_from_grid();
                }
                self.go_to(pos, pos > self.pos);
            }
        }
        self.redraw();
    }

    /// The go-to box sits in the upper middle of the window, with as many
    /// matches under it as fit, the one picked out among them.
    pub(super) fn palette_layout(&self) -> Option<PaletteLayout> {
        let palette = self.palette.as_ref()?;
        let (width, height) = self.window_size();
        let px = ui_px(height);
        let line = Text::line_height(px);
        let (head, row_height) = (line + 2 * px as i32, line * 2);
        // Above it there is room for a row of frames from the video picked
        // out, as wide as the window allows.
        let frame_width = (width as i32 * 9 / 10 / story::SEGMENTS as i32).min(px as i32 * 14);
        let frame = (frame_width, frame_width * 9 / 16);
        // The whole of it, with the frames above and as many matches as fit
        // below, sits in the middle; what is typed then stays where it is
        // however many matches there are.
        let above = frame.1 + px as i32;
        let fit = ((height as i32 * 3 / 4 - above - head) / row_height).clamp(1, 10) as usize;
        let top = (height as i32 - above - head - fit as i32 * row_height) / 2 + above;
        let rows = fit.min(palette.rows());
        let first = palette.first.min(palette.rows().saturating_sub(rows));
        let box_width = (px as i32 * 48).min(width as i32);
        Some(PaletteLayout { left: (width as i32 - box_width) / 2, top, width: box_width, head, row_height, first, rows, px, frame })
    }

    /// The files whose matches the go-to box is showing.
    pub(super) fn palette_shown(&self) -> Vec<usize> {
        let (Some(palette), Some(layout)) = (&self.palette, self.palette_layout()) else { return Vec::new() };
        palette.matches.iter().skip(layout.first).take(layout.rows).copied().collect()
    }

    /// The row of the go-to box at a window position, counted through all
    /// of its matches.
    pub(super) fn palette_row_at(&self, x: f64, y: f64) -> Option<usize> {
        let layout = self.palette_layout()?;
        let (x, y) = (x as i32 - layout.left, y as i32 - layout.top - layout.head);
        let row = y.div_euclid(layout.row_height);
        ((0..layout.width).contains(&x) && (0..layout.rows as i32).contains(&row)).then(|| layout.first + row as usize)
    }

    pub(super) fn draw_palette(&self, canvas: &mut Canvas, text: &mut Text, layout: &PaletteLayout) -> Vec<(PaletteEnd, [i32; 4])> {
        let Some(palette) = &self.palette else { return Vec::new() };
        let (px, pad) = (layout.px, layout.px as i32);
        let line = Text::line_height(px);
        let height = layout.head + layout.rows as i32 * layout.row_height + if layout.rows > 0 { pad / 2 } else { 0 };
        panel(canvas, layout.left, layout.top, layout.width, height, pad * 2 / 3);

        // Above the box, for a video: frames from along it, side by side.
        if let Some(frames) = self.story_subject().and_then(|id| self.story_frames.get(&id)) {
            let gap = (pad / 4).max(2);
            let (cell_w, cell_h) = layout.frame;
            let strip_width = (cell_w + gap) * frames.len() as i32 + gap;
            let (strip_left, strip_top) = ((canvas.width - strip_width) / 2, layout.top - cell_h - 2 * gap - pad / 2);
            panel(canvas, strip_left, strip_top, strip_width, cell_h + 2 * gap, pad / 2);
            for (index, frame) in frames.iter().enumerate() {
                let (cell_left, cell_top) = (strip_left + gap + index as i32 * (cell_w + gap), strip_top + gap);
                // One frame stands for each stretch: the middle one.
                match frame.as_ref().and_then(|segment| segment.get(segment.len() / 2)) {
                    Some(frame) => {
                        let (w, h) = fit_size((frame.width, frame.height), (cell_w as u32, cell_h as u32), true);
                        let (w, h) = (w as i32, h as i32);
                        canvas.image(frame, cell_left + (cell_w - w) / 2, cell_top + (cell_h - h) / 2, w, h);
                    }
                    None => canvas.fill(cell_left, cell_top, cell_w, cell_h, PLACEHOLDER_COLOR),
                }
                canvas.round_off(cell_left, cell_top, cell_w, cell_h, pad / 3, MENU_COLOR);
            }
        }

        let label = match palette.field.text.first() {
            None if !palette.commands.is_empty() => "Recent:   ",
            _ if !palette.commands.is_empty() && palette.field.text.first() == Some(&':') => "Go to:   ",
            Some('>') if palette.matches.is_empty() => "Run:   ",
            Some(':') => "Go to:   ",
            Some('@') => "Workspace:   ",
            Some('~') => "Messages:   ",
            Some('!') => "Run command:   ",
            _ => "Find:   ",
        };
        let typed = palette.field.string();
        let before_cursor: String = palette.field.text[..palette.field.cursor].iter().collect();
        let note = match palette.rows() {
            0 => "no match".to_string(),
            count => format!("{} of {count}", palette.selected + 1),
        };
        let (x, y) = (layout.left + pad, layout.top + pad);
        let typed_left = x + text.width(label, px);
        text.draw(canvas, x, y, px, DIM_TEXT_COLOR, label);
        draw_selection(canvas, text, &palette.field, typed_left, y, px);
        text.draw(canvas, typed_left, y, px, TEXT_COLOR, &typed);
        if palette.focus == 0 {
            canvas.fill(typed_left + text.width(&before_cursor, px), y, (pad / 10).max(2), line, TEXT_COLOR);
        }
        // At the right, from the edge inwards: the buttons, then the count.
        let mut right = layout.left + layout.width - pad;
        let mut drawn = Vec::new();
        let buttons = self.palette_buttons();
        for (at, (end, label)) in buttons.iter().enumerate().rev() {
            let focused = palette.focus == at + 1;
            let (button_w, button_h) = (text.width(label, px) + pad, line + pad / 2);
            let (button_x, button_y) = (right - button_w, y - pad / 4);
            let fill = if focused { accent() } else { PLACEHOLDER_COLOR };
            canvas.round_fill(button_x, button_y, button_w, button_h, pad / 3, fill, 256);
            canvas.round_outline(button_x, button_y, button_w, button_h, pad / 3, 1, if focused { TEXT_COLOR } else { MENU_EDGE_COLOR });
            text.draw(canvas, button_x + pad / 2, y, px, if focused { on_accent() } else { TEXT_COLOR }, label);
            drawn.push((*end, [button_x, button_y, button_w, button_h]));
            right = button_x - pad / 2;
        }
        let note_left = right - pad / 2 - text.width(&note, px);
        text.draw(canvas, note_left, y, px, DIM_TEXT_COLOR, &note);

        // Each match: a small picture, the name, and a video's length.
        let thumb = ((layout.row_height - pad / 4) * 16 / 9, layout.row_height - pad / 4);
        for row in 0..layout.rows {
            let at = layout.first + row;
            let top = layout.top + layout.head + row as i32 * layout.row_height;
            if at == palette.selected {
                canvas.round_fill(layout.left + pad / 3, top, layout.width - 2 * (pad / 3), layout.row_height, pad / 2, accent(), 256);
            }
            // Words on the row picked out are in colors that read on it.
            let (main, dim) = row_text(at == palette.selected);
            // A command: what it is called, a mark if it is on, and its key.
            if let Some((name, keys, command)) = palette.commands.get(at) {
                let text_top = top + (layout.row_height - line) / 2;
                if self.is_checked(*command) {
                    let mark = pad / 2;
                    canvas.round_fill(layout.left + pad, top + (layout.row_height - mark) / 2, mark, mark, mark / 4, main, 256);
                }
                text.draw(canvas, layout.left + 2 * pad, text_top, px, main, name);
                let keys_left = layout.left + layout.width - pad - text.width(keys, px);
                text.draw(canvas, keys_left, text_top, px, dim, keys);
                continue;
            }
            let id = palette.matches[at];
            let (cell_left, cell_top) = (layout.left + pad, top + pad / 8);
            match self.loader.thumb(id) {
                Some(frame) => {
                    let (w, h) = fit_size((frame.width, frame.height), (thumb.0 as u32, thumb.1 as u32), true);
                    canvas.image(&frame, cell_left + (thumb.0 - w as i32) / 2, cell_top + (thumb.1 - h as i32) / 2, w as i32, h as i32);
                }
                None => canvas.fill(cell_left, cell_top, thumb.0, thumb.1, PLACEHOLDER_COLOR),
            }
            let text_top = top + (layout.row_height - line) / 2;
            let length = self.durations.get(&id).copied().flatten().map(clock).unwrap_or_default();
            let length_width = text.width(&length, px);
            let right = layout.left + layout.width - pad;
            text.draw(canvas, right - length_width, text_top, px, dim, &length);
            let name_left = cell_left + thumb.0 + pad;
            let room = right - length_width - pad - name_left;
            let name = clipped(text, &self.name_of(id), room, px);
            let color = if self.marks.contains(&id) && at != palette.selected { MARK_COLOR } else { main };
            text.draw(canvas, name_left, text_top, px, color, &name);
            // The folder it is in, below the one opened, if it is in one:
            // among many alike names, which one is which.
            let folder = self.folder_within(self.files[id].parent().unwrap_or(Path::new("")));
            let name_w = text.width(&name, px);
            if !folder.is_empty() && room - name_w > pad * 4 {
                let folder = clipped(text, &folder, room - name_w - pad, px);
                text.draw(canvas, name_left + name_w + pad, text_top, px, dim, &folder);
            }
        }
        drawn
    }

    /// How well file `id` matches a search, or `None` if it does not. With
    /// `loose`, letters scattered through the name count too.
    pub(super) fn score(&mut self, query: &Query, id: usize, loose: bool) -> Option<i32> {
        let path = &self.files[id];
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
        let size = match query.wants_size() {
            true => *self.sizes.entry(id).or_insert_with(|| std::fs::metadata(path).ok().map(|meta| meta.len())),
            false => None,
        };
        let seen = (query.wants_seen() && files::is_video(path)).then(|| self.watched(id));
        // The folder it is in from the folder opened down, not from the
        // root: in /mnt/nas/photos, "nas" is in every path and tells none
        // apart. Made once for each folder in turn, as the files come.
        let (roots, memo) = (&self.options.paths, &self.folder_memo);
        let folder = path.parent().unwrap_or(Path::new(""));
        let within = || {
            if let Some((_, said)) = memo.borrow().as_ref().filter(|(had, _)| had == folder) {
                return said.clone();
            }
            let said: std::rc::Rc<str> = within_roots(roots, folder).to_lowercase().into();
            *memo.borrow_mut() = Some((folder.to_path_buf(), said.clone()));
            said
        };
        let facts = Facts { name: &name, video: files::is_video(path), marked: self.marks.contains(&id), seen, verdict: self.verdict(id), folder: Some(&within), size };
        query.score(&facts, loose)
    }

    pub(super) fn sort_by(&mut self, sort: Sort) {
        (self.sort, self.shuffled) = (Some(sort), false);
        let reversed = self.reversed;
        self.reorder(|ids, files| {
            files::sort_ids(ids, files, sort);
            if reversed {
                ids.reverse();
            }
        });
        let label = commands::SORT.iter().flatten().find(|item| item.command == Command::SortBy(sort)).map_or("", |item| item.label);
        self.toast(format!("Sorted  ·  {}{}", label.to_lowercase(), if reversed { ", reversed" } else { "" }));
    }

    pub(super) fn grid_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let layout = self.grid_layout(self.window_size());
        match key {
            Key::Named(NamedKey::Escape) => return self.toggle_grid(),
            Key::Named(NamedKey::Tab) => return self.execute(Command::GridMenu, event_loop),
            // With a video playing on, the space bar pauses and plays it,
            // as it does with the video on view.
            Key::Named(NamedKey::Space) if self.pip && self.video.is_some() => return self.video_action(VideoAction::Pause),
            Key::Named(NamedKey::ArrowUp) => return self.step_cursor(0, -1),
            Key::Named(NamedKey::ArrowDown) => return self.step_cursor(0, 1),
            // Ctrl with + or - makes the pictures larger or smaller.
            Key::Character("+" | "=") if self.modifiers.control_key() => return self.zoom_grid(true),
            Key::Character("-" | "_") if self.modifiers.control_key() => return self.zoom_grid(false),
            // Here `i` is for the names under the pictures.
            Key::Character("i") => {
                self.grid_titles = !self.grid_titles;
                return self.redraw();
            }
            _ => {}
        }
        // Ctrl+K is the one key here that a modifier changes the meaning of.
        if commands::for_key(key.clone(), self.modifiers, false) == Some(Command::Palette) {
            return self.execute(Command::Palette, event_loop);
        }
        match commands::for_key(key, ModifiersState::empty(), false) {
            Some(Command::Next) => self.step_cursor(1, 0),
            Some(Command::Previous) => self.step_cursor(-1, 0),
            Some(Command::Shuffle) => self.execute(Command::Shuffle, event_loop),
            Some(Command::JumpForward) => self.move_cursor(layout.per_page() as i64),
            Some(Command::JumpBack) => self.move_cursor(-(layout.per_page() as i64)),
            Some(Command::First) => self.move_cursor(i64::MIN),
            Some(Command::Last) => self.move_cursor(i64::MAX),
            Some(Command::Action(0)) => self.open_from_grid(),
            Some(
                command @ (Command::ToggleGrid
                | Command::ToggleHelp
                | Command::ToggleFullscreen
                | Command::Mark
                | Command::CopyName
                | Command::CopyPath
                | Command::CopyTo
                | Command::MoveTo
                | Command::Undo
                | Command::Redo
                | Command::Rename
                | Command::Delete
                | Command::Trash
                | Command::MarkMenu
                | Command::SaveFilelist
                | Command::Search
                | Command::ClearFilter
                | Command::ShowAllHere
                | Command::SortMenu
                | Command::Quit),
            ) => self.execute(command, event_loop),
            _ => {}
        }
    }

}

impl App {

    /// How many files there are in all, whatever the list is narrowed to.
    pub(super) fn total(&self) -> usize {
        self.filter.as_ref().map_or(self.list.len(), |(_, all)| all.len())
    }

    /// Whether the list on view holds both pictures and videos, so that
    /// narrowing it to one kind would leave something out.
    pub(super) fn mixed(&self) -> bool {
        let mut kinds = self.list.iter().map(|id| files::is_video(&self.files[*id]));
        kinds.next().is_some_and(|first| kinds.any(|kind| kind != first))
    }

    /// Every file, whatever the list is narrowed to, in the list's order.
    pub(super) fn everything(&self) -> Vec<usize> {
        self.filter.as_ref().map_or_else(|| self.list.clone(), |(_, all)| all.clone())
    }

    /// The files, out of all of them, that match what was typed, closest
    /// first, and whether anything was typed that narrows them. Names with
    /// a word as typed come before any that merely have its letters
    /// scattered through them, which count only when there are none of
    /// the first kind: in a folder of long names nearly any word is in
    /// most of them that way.
    pub(super) fn matching(&mut self, typed: &str) -> (Vec<usize>, bool) {
        let query = Query::parse(typed);
        let everything = self.everything();
        if query.is_empty() {
            return (everything, false);
        }
        let mut pass = |loose: bool| -> Vec<(usize, i32)> {
            everything.iter().filter_map(|&id| Some((id, self.score(&query, id, loose)?))).collect()
        };
        let mut scored = pass(false);
        if scored.is_empty() {
            scored = pass(true);
        }
        scored.sort_by_key(|(_, score)| -score);
        (scored.into_iter().map(|(id, _)| id).collect(), true)
    }

    /// Narrows the list from the start to what `typed` would find in the
    /// find box, for `--filter`. Returns whether anything is left to show.
    pub fn start_filter(&mut self, typed: &str) -> bool {
        let (matches, narrowed) = self.matching(typed);
        if narrowed && !matches.is_empty() {
            // The file it would have started on, if that is among them.
            let start = Some(self.id()).filter(|id| matches.contains(id));
            self.keep_matches(typed.trim().to_string(), matches, start);
        }
        !narrowed || !self.list.is_empty() && self.filter.is_some()
    }

    /// Narrows the list to `matches`, in the order the list had them, and
    /// goes to `picked` among them.
    pub(super) fn keep_matches(&mut self, typed: String, matches: Vec<usize>, picked: Option<usize>) {
        if matches.is_empty() {
            return;
        }
        // What is on view now is what leaving this search goes back to:
        // every file, or the matches of the search this one was made in.
        let within = self.filter.as_ref().map(|(typed, _)| typed.clone());
        if within.as_deref() != Some(typed.as_str()) {
            let (viewed, focus) = (self.id(), self.list[self.focus()]);
            let order = self.everything();
            self.filter_from.push(FilterFrom { typed: within, viewed, focus, order, sort: self.sort, reversed: self.reversed, shuffled: self.shuffled });
        }
        let total = self.total();
        self.narrow(typed, matches, picked);
        self.toast(format!("{} of {total}  ·  Esc {}", self.list.len(), self.back_label()));
    }

    /// Puts only `matches` in the list, in the order the list had them,
    /// and goes to `picked` among them.
    pub(super) fn narrow(&mut self, typed: String, matches: Vec<usize>, picked: Option<usize>) {
        let everything = self.everything();
        let wanted: HashSet<usize> = matches.into_iter().collect();
        let list: Vec<usize> = everything.iter().copied().filter(|id| wanted.contains(id)).collect();
        let at = picked.and_then(|picked| list.iter().position(|id| *id == picked)).unwrap_or(0);
        if self.options.verbose {
            eprintln!("jog: search \"{typed}\" kept: {} of {} files", list.len(), everything.len());
            for (pos, id) in list.iter().enumerate() {
                eprintln!("jog:     [{}] {}", pos + 1, self.files[*id].display());
            }
        }
        self.filter = Some((typed, everything));
        self.set_list(list, at, at);
        self.sync_video();
        self.redraw();
    }

    /// Where leaving the search on view goes back to, in a few words: the
    /// search it was made in, by what was typed for that, or else the file
    /// that was on view, by the start of its name.
    pub(super) fn back_label(&self) -> String {
        let short = |text: &str| match text.chars().count() {
            0..=18 => text.to_string(),
            _ => format!("{}…", text.chars().take(16).collect::<String>().trim_end()),
        };
        match self.filter_from.last() {
            Some(FilterFrom { typed: Some(typed), .. }) => format!("back to \"{}\"", short(typed)),
            Some(from) => format!("back to {}", short(&self.name_of(if self.grid.is_some() { from.focus } else { from.viewed }))),
            None => "back".to_string(),
        }
    }

    /// Puts a different set of files in the list, with the one at `viewed`
    /// on view and, in the grid, the cursor on the one at `focus`.
    pub(super) fn set_list(&mut self, list: Vec<usize>, viewed: usize, focus: usize) {
        self.list = list;
        self.pos = viewed.min(self.list.len() - 1);
        if let Some(grid) = &mut self.grid {
            grid.cursor = focus.min(self.list.len() - 1);
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
    }

    /// Leaves the search on view for what it was made in: every file, or
    /// the matches of another search. The files are in the order they had
    /// then, whatever has been done to the matches since, and the one that
    /// was on view then is on view again, to carry on from there. With
    /// `stay` it instead shows every file and stays on the one on view now.
    pub(super) fn clear_filter(&mut self, stay: bool) {
        let Some((_, mut all)) = self.filter.take() else { return };
        let (mut viewed, mut focus) = (self.id(), self.list[self.focus()]);
        // Staying drops every search there is, back to how it all began.
        let from = match stay {
            true => self.filter_from.drain(..).next(),
            false => self.filter_from.pop(),
        };
        let mut within = None;
        if let Some(from) = from {
            // Files that have turned up since go after the rest.
            let place: HashMap<usize, usize> = from.order.iter().enumerate().map(|(place, id)| (*id, place)).collect();
            all.sort_by_key(|id| place.get(id).copied().unwrap_or(usize::MAX));
            (self.sort, self.reversed, self.shuffled) = (from.sort, from.reversed, from.shuffled);
            // It may have been deleted or moved away in the meantime.
            if !stay && all.contains(&from.viewed) {
                (viewed, focus) = (from.viewed, if all.contains(&from.focus) { from.focus } else { from.viewed });
                within = from.typed;
            }
        }
        let at = |id: usize| all.iter().position(|&other| other == id).unwrap_or(0);
        let (viewed, focus) = (at(viewed), at(focus));
        self.set_list(all, viewed, focus);
        self.sync_video();
        // Back in the search this one was made in, at the same file.
        if let Some(typed) = within {
            let picked = self.list[self.focus()];
            let (matches, narrowed) = self.matching(&typed);
            if narrowed && matches.contains(&picked) {
                self.narrow(typed.clone(), matches, Some(picked));
                return self.toast(format!("“{typed}”  ·  {} of {}", self.focus() + 1, self.list.len()));
            }
            self.filter_from.clear();
        }
        if self.options.verbose {
            eprintln!("jog: search cleared: all {} files, at {}", self.list.len(), self.pos + 1);
        }
        self.toast(format!("All files  ·  {} of {}", self.focus() + 1, self.list.len()));
    }

    pub(super) fn finish_rename(&mut self) {
        let Some(prompt) = self.prompt.take() else { return };
        let typed = prompt.field.string();
        let name = typed.trim();
        let from = self.files[prompt.id].clone();
        let old_name = self.name_of(prompt.id);
        if name == old_name {
            return;
        }
        // A path moves it as well: to another folder, made if it is not
        // there, under the name at the end.
        let Some(to) = ops::rename_target(&from, name).filter(|_| name != "." && name != "..") else {
            self.toast("Not a usable name");
            return;
        };
        let here = from.parent().and_then(|dir| dir.canonicalize().ok());
        let moved = to.parent() != here.as_deref();
        if !moved && to.file_name() == from.file_name() {
            return;
        }
        let said = if moved { short_dir(&to) } else { to.file_name().unwrap_or_default().to_string_lossy().into_owned() };
        if self.dry(|_| vec![format!("rename: {} -> {}", from.display(), to.display())], "would rename") {
            return;
        }
        if let Some(dir) = to.parent().filter(|_| moved) {
            if let Err(error) = std::fs::create_dir_all(dir) {
                return self.toast(format!("Not usable · {error}"));
            }
        }
        match ops::move_file(&from, &to) {
            Ok(()) => {
                match moved {
                    true => println!("[rename] {} -> {}", from.display(), to.display()),
                    false => println!("[rename] {old_name} -> {said}"),
                }
                self.set_path(prompt.id, to.clone());
                self.did(Undo::Renamed { id: prompt.id, from, to });
                self.toast_with(format!("{} {said}", if moved { "Moved to" } else { "Renamed to" }), vec![("Undo".into(), ToastLink::Undo)]);
                self.refresh();
            }
            Err(error) => self.toast(format!("Not renamed · {error}")),
        }
    }

}

/// A folder as it is below the one opened that holds it, not from the
/// root; empty for the folder opened itself, or one outside it.
fn within_roots(roots: &[PathBuf], folder: &Path) -> String {
    let here = [PathBuf::from(".")];
    let roots = if roots.is_empty() { &here[..] } else { roots };
    // A file named itself stands for the folder it is in.
    let folder_of = |root: &PathBuf| if root.is_file() { root.parent().unwrap_or(Path::new("")).to_path_buf() } else { root.clone() };
    let rest = roots.iter().map(folder_of).filter_map(|root| folder.strip_prefix(&root).ok().map(Path::to_path_buf)).min_by_key(|rest| rest.as_os_str().len());
    // Outside all of them, as from a file list: nothing, rather than the
    // whole way from the root.
    rest.map(|rest| rest.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    /// The folder a file is in, below the one opened.
    pub(super) fn folder_within(&self, folder: &Path) -> String {
        within_roots(&self.options.paths, folder)
    }
}
