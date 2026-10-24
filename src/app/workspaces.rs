//! Workspaces: named folders, the overview of them, and the sessions shown as they are.

use super::*;

impl App {

    /// Goes to another workspace. What this one was showing is kept for
    /// coming back to: its place, its order, a kept search. The other is
    /// listed the first time it is gone to, and picked up where it was
    /// left after that.
    pub(super) fn switch_space(&mut self, to: usize) {
        let Some(name) = self.spaces.get(to).map(|space| space.name.clone()) else {
            return self.toast(format!("No workspace {}", to + 1));
        };
        if to == self.space {
            return self.toast(format!("@{name}"));
        }
        // Only one that resumes is taken up as it was left; any other is
        // listed anew, to show what is there now.
        let start = self.spaces[to].start;
        let incoming = match self.spaces[to].kept.take().filter(|kept| !kept.list.is_empty() && start == Start::Resume) {
            Some(kept) => kept,
            None => {
                let space = &self.spaces[to];
                let sort = space.sort;
                let listing = files::Listing { recursive: space.recursive, sort, reverse: space.reverse, filelist: space.filelist.clone(), ..files::Listing::default() };
                let found = files::collect(&space.paths, &listing).files;
                if found.is_empty() {
                    return self.toast(format!("@{name} is empty"));
                }
                // Files seen before, in whatever workspace, keep their ids.
                let mut list = self.ids_for(found);
                let reversed = self.spaces[to].reverse;
                // Shuffled anew each time, for one that starts at random.
                if start == Start::Random {
                    self.rng.shuffle(&mut list);
                }
                Kept { list, pos: 0, cursor: 0, filter: None, filter_from: Vec::new(), sort: sort.filter(|_| start != Start::Random), reversed, shuffled: start == Start::Random }
            }
        };
        let leaving = Kept {
            cursor: self.focus(),
            list: std::mem::take(&mut self.list),
            pos: self.pos,
            filter: self.filter.take(),
            filter_from: std::mem::take(&mut self.filter_from),
            sort: self.sort,
            reversed: self.reversed,
            shuffled: self.shuffled,
        };
        self.spaces[self.space].kept = Some(leaving);
        self.space = to;

        // Refreshing and watching go by where the options point.
        let space = &self.spaces[to];
        self.options.paths = space.paths.clone();
        (self.options.listing.recursive, self.options.listing.sort, self.options.listing.reverse) = (space.recursive, space.sort, space.reverse);
        self.options.listing.filelist = space.filelist.clone();

        let last = incoming.list.len() - 1;
        let count = incoming.list.len();
        (self.filter, self.sort, self.reversed, self.shuffled) = (incoming.filter, incoming.sort, incoming.reversed, incoming.shuffled);
        self.filter_from = incoming.filter_from;
        (self.list, self.pos) = (incoming.list, incoming.pos.min(last));
        // A session gone to for the first time is landed in where it was left.
        if let Some(spot) = self.spaces[to].spot.take() {
            self.take_up(spot);
        }
        (self.palette, self.menu, self.prompt, self.overview) = (None, None, None, None);
        (self.strip_scroll, self.strip_over, self.list_scroll, self.list_hover) = (0, None, None, None);
        let layout = self.grid_layout(self.window_size());
        if let Some(grid) = &mut self.grid {
            *grid = Grid::at(incoming.cursor.min(last), &layout);
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
        self.sync_video();
        // A watch goes on, now on this workspace's folders.
        if self.watch.is_some() {
            self.set_watch(true);
        }
        self.toast(format!("@{name}  ·  {count} files"));
        if self.options.verbose {
            eprintln!("jog: workspace @{name}: {count} files");
        }
        self.redraw();
    }

    /// The key that goes to a workspace, as a list shows it: Ctrl with
    /// its number for one of the config file's, Ctrl+` for what was opened
    /// by its path.
    pub(super) fn space_key(&self, which: usize) -> &'static str {
        match self.spaces[which].saved {
            true => self.spaces[which].key.and_then(|key| SPACE_KEYS.get(key - 1)).copied().unwrap_or(""),
            false if which == self.home => "Ctrl+`",
            false => "",
        }
    }

    /// The workspaces whose names match what follows an `@` in the find
    /// box, as rows to pick from: each with its folder and its key, the
    /// one on show ticked. And, for a folder that was opened by its path,
    /// a row to save it as one.
    pub(super) fn spaces_matching(&mut self, typed: &str) -> Vec<(String, &'static str, Command)> {
        let query = Query::parse(typed);
        let wanted = |name: &str, loose: bool| query.is_empty() || query.score(&Facts { name, video: false, marked: false, seen: None, verdict: None, folder: None, size: None }, loose).is_some();
        let row = |(at, space): (usize, &Space)| {
            let folder = space.paths.first().map(|path| short_dir(path)).unwrap_or_default();
            let unsaved = if space.saved { "" } else { "   (not saved)" };
            (format!("{}   {folder}{unsaved}", space.name), self.space_key(at), Command::Workspace(at))
        };
        let pass = |loose: bool| -> Vec<(String, &'static str, Command)> {
            self.spaces.iter().enumerate().filter(|(_, space)| wanted(&space.name, loose)).map(row).collect()
        };
        let mut rows = pass(false);
        if rows.is_empty() {
            rows = pass(true);
        }
        let here = &self.spaces[self.space];
        if !here.saved && query.is_empty() {
            rows.push((format!("Save {} as workspace…", here.name), "", Command::SaveWorkspace));
        }
        rows
    }

    /// Opens a box for the name to save the folder on view under, as a
    /// workspace. Only a single folder can be one.
    pub(super) fn ask_workspace_name(&mut self) {
        let here = &self.spaces[self.space];
        if here.saved {
            return self.toast(format!("Already @{}", here.name));
        }
        if !matches!(here.paths.as_slice(), [only] if only.is_dir()) {
            return self.toast("Only one folder can be a workspace");
        }
        let text: Vec<char> = here.name.chars().collect();
        let id = self.list[self.focus()];
        self.prompt = Some(Prompt { id, kind: PromptKind::SaveWorkspace, field: TextField::new(text.clone(), text.len()) });
        (self.menu, self.palette) = (None, None);
        self.redraw();
    }

    /// Writes the folder on view into the config file as a workspace.
    pub(super) fn save_space(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        if self.spaces.iter().any(|space| space.saved && space.name == name) {
            return self.toast(format!("@{name} exists"));
        }
        if self.dry(|_| vec![format!("add the workspace {name} to the config file")], "would save the workspace") {
            return;
        }
        let Some(folder) = self.spaces[self.space].paths.first().and_then(|path| path.canonicalize().ok()) else {
            return self.toast("Folder not found");
        };
        match crate::options::save_workspace(name, &folder) {
            Ok(file) => {
                let space = &mut self.spaces[self.space];
                (space.name, space.saved) = (name.to_string(), true);
                // It takes the first digit that no other has.
                let taken: Vec<usize> = self.spaces.iter().filter_map(|space| space.key).collect();
                self.spaces[self.space].key = (1..=9).find(|key| !taken.contains(key));
                self.toast(format!("Saved @{name}  ·  {}", short_dir(&file)));
            }
            Err(error) => self.toast(format!("Not saved · {error}")),
        }
    }

    /// The ids of some files, those not seen before being taken on. Files
    /// seen before, in whatever workspace, keep the ids they have.
    pub(super) fn ids_for(&mut self, found: Vec<PathBuf>) -> Vec<usize> {
        let known: HashMap<PathBuf, usize> = self.files.iter().cloned().enumerate().map(|(id, path)| (path, id)).collect();
        let mut list = Vec::with_capacity(found.len());
        for path in found {
            list.push(known.get(&path).copied().unwrap_or_else(|| {
                self.loader.add_file(path.clone());
                self.files.push(path);
                self.files.len() - 1
            }));
        }
        list
    }

    /// The files of a workspace, so far as they are known: of the one on
    /// show, the list; of another, what it was showing when it was left
    /// or, not yet gone to, what the overview has had listed.
    pub(super) fn space_ids(&self, which: usize) -> &[usize] {
        match which == self.space {
            true => &self.list,
            false => self.spaces[which].kept.as_ref().map_or(&[], |kept| &kept.list),
        }
    }

    /// Brings up the overview: every workspace as a card. Those not yet
    /// gone to are listed on other threads, so that their cards have
    /// pictures to show; a large folder may take a moment.
    pub(super) fn open_overview(&mut self) {
        self.overview = Some(Overview { cursor: self.space, tick: 0 });
        (self.menu, self.palette, self.overview_due) = (None, None, None);
        for which in 0..self.spaces.len() {
            let space = &mut self.spaces[which];
            if which == self.space || space.kept.is_some() || std::mem::replace(&mut space.asked, true) {
                continue;
            }
            let listing = files::Listing { recursive: space.recursive, sort: space.sort, reverse: space.reverse, filelist: space.filelist.clone(), ..files::Listing::default() };
            let (paths, proxy) = (space.paths.clone(), self.proxy.clone());
            let key = paths.clone();
            std::thread::spawn(move || drop(proxy.send_event(Event::SpaceListed(key, files::collect(&paths, &listing).files))));
        }
        self.sync_video();
        self.request();
        self.redraw();
    }

    /// Closes the overview, going to a workspace if one was picked.
    pub(super) fn leave_overview(&mut self, to: Option<usize>) {
        (self.overview, self.overview_due) = (None, None);
        match to.filter(|to| *to != self.space) {
            Some(to) => self.switch_space(to),
            None => {
                self.sync_video();
                self.request();
                self.redraw();
            }
        }
    }

    /// Forgets a session in the overview, and takes its card away. The one
    /// open stays: it would be kept again on quitting.
    pub(super) fn forget_session(&mut self, which: usize) {
        if which == self.space {
            return self.toast("This one is open");
        }
        let Some(id) = self.spaces.get(which).and_then(|space| space.spot.as_ref()).map(|spot| spot.id.clone()) else {
            return self.toast("Not a session");
        };
        if let Err(error) = sessions::forget(&id) {
            return self.toast(format!("Not forgotten · {error}"));
        }
        self.spaces.remove(which);
        if self.space > which {
            self.space -= 1;
        }
        if let Some(overview) = &mut self.overview {
            overview.cursor = overview.cursor.min(self.spaces.len() - 1);
            overview.tick = 0;
        }
        self.toast(format!("Forgot {id}"));
        self.request();
        self.redraw();
    }

    /// Handles a key in the overview. Returns whether it was one of its.
    pub(super) fn overview_key(&mut self, key: Key<&str>) -> bool {
        let columns = self.overview_layout().0;
        let last = self.spaces.len() - 1;
        let Some(overview) = &mut self.overview else { return false };
        let to = match key {
            Key::Named(NamedKey::Escape) | Key::Character("G" | "g" | "q") => {
                self.leave_overview(None);
                return true;
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                let picked = overview.cursor;
                self.leave_overview(Some(picked));
                return true;
            }
            // Delete forgets the session picked out.
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                let picked = overview.cursor;
                self.forget_session(picked);
                return true;
            }
            // A digit goes straight to that workspace.
            Key::Character(digit) if !self.modifiers.control_key() && digit.parse::<usize>().is_ok_and(|n| self.spaces.iter().any(|space| space.key == Some(n))) => {
                let wanted = digit.parse::<usize>().ok();
                let picked = self.spaces.iter().position(|space| space.key == wanted).unwrap_or(0);
                self.leave_overview(Some(picked));
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => (overview.cursor + 1).min(last),
            Key::Named(NamedKey::ArrowLeft) => overview.cursor.saturating_sub(1),
            Key::Named(NamedKey::ArrowDown) => (overview.cursor + columns).min(last),
            Key::Named(NamedKey::ArrowUp) => overview.cursor.checked_sub(columns).unwrap_or(overview.cursor),
            Key::Named(NamedKey::Home) => 0,
            Key::Named(NamedKey::End) => last,
            _ => return false,
        };
        if to != overview.cursor {
            (overview.cursor, overview.tick) = (to, 0);
            self.overview_due = None;
            self.request();
        }
        self.redraw();
        true
    }

    /// How the overview's cards are laid out: how many across, and for
    /// each workspace where its picture goes: left, top, width, height.
    /// Under each picture there is room for its name and a line about it.
    pub(super) fn overview_layout(&self) -> (usize, Vec<[i32; 4]>) {
        let (width, height) = self.window_size();
        let (width, height, px) = (width as i32, height as i32, ui_px(height) as i32);
        let count = self.spaces.len() as i32;
        let columns = if count <= 3 { count } else { (f64::from(count).sqrt().ceil() as i32).max(1) };
        let rows = (count + columns - 1) / columns;
        let (gap, words) = (px * 3, Text::line_height(px as u32) * 3);
        // As large as the window allows, both ways, up to a size that
        // still reads as a card.
        let across = (width * 85 / 100 - gap * (columns - 1)) / columns;
        let down = ((height * 80 / 100 - gap * (rows - 1)) / rows - words) * 16 / 10;
        let card_w = across.min(down).min(px * 40).max(px * 6);
        let card_h = card_w * 10 / 16;
        let left = (width - columns * card_w - (columns - 1) * gap) / 2;
        let top = (height - rows * (card_h + words) - (rows - 1) * gap) / 2 + px;
        let place = |which: i32| {
            // A last row that is not full sits in the middle.
            let (column, row) = (which % columns, which / columns);
            let in_row = if row == rows - 1 { count - row * columns } else { columns };
            let indent = (columns - in_row) * (card_w + gap) / 2;
            [left + indent + column * (card_w + gap), top + row * (card_h + words + gap), card_w, card_h]
        };
        (columns as usize, (0..count).map(place).collect())
    }

    /// Which workspace's card is at a window position.
    pub(super) fn overview_card_at(&self, x: f64, y: f64) -> Option<usize> {
        let words = Text::line_height(ui_px(self.window_size().1)) * 3;
        let (_, cards) = self.overview_layout();
        cards.iter().position(|[left, top, width, height]| (*left..left + width).contains(&(x as i32)) && (*top..top + height + words).contains(&(y as i32)))
    }

    /// Draws the overview: each workspace as a card of four of its
    /// pictures, with its name, its key and how many files it has. The
    /// card picked out shows one picture at a time instead, going through
    /// what is in it.
    pub(super) fn draw_overview(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let Some((cursor, tick)) = self.overview.as_ref().map(|overview| (overview.cursor, overview.tick)) else { return };
        let (_, cards) = self.overview_layout();
        let (pad, line) = (px as i32, Text::line_height(px));
        let round = pad;

        // Sessions to choose from are shown the same way, and named so.
        let what = if self.spaces.iter().any(|space| space.caption.is_some()) { "Sessions" } else { "Workspaces" };
        let heading = format!("{}  {what}", commands::icons::FOLDERS);
        let big = px * 3 / 2;
        let first_top = cards.iter().map(|card| card[1]).min().unwrap_or(0);
        let heading_left = (canvas.width - text.width_strong(&heading, big)) / 2;
        text.draw_strong(canvas, heading_left, (first_top - Text::line_height(big) - 2 * pad).max(pad), big, TEXT_COLOR, &heading);

        for (which, &[left, top, width, height]) in cards.iter().enumerate() {
            let ids = self.space_ids(which).to_vec();
            canvas.round_fill(left, top, width, height, round, PLACEHOLDER_COLOR, 256);
            // Scaled to fit its place, in the middle of it.
            let picture = |canvas: &mut Canvas, id: usize, [x, y, w, h]: [i32; 4]| {
                let Some(thumb) = self.loader.thumb(id) else { return false };
                let (fit_w, fit_h) = fit_size((thumb.width, thumb.height), (w as u32, h as u32), true);
                canvas.image(&thumb, x + (w - fit_w as i32) / 2, y + (h - fit_h as i32) / 2, fit_w as i32, fit_h as i32);
                true
            };
            if which == cursor && !ids.is_empty() {
                // The one picked out runs through its pictures; one not
                // yet decoded leaves the one before it up.
                let shown = (0..ids.len().min(6)).map(|back| ids[(tick + ids.len() - back) % ids.len()]).any(|id| picture(canvas, id, [left, top, width, height]));
                if !shown {
                    picture(canvas, ids[0], [left, top, width, height]);
                }
            } else {
                // Four of them, two by two.
                let gap = (pad / 3).max(2);
                let (cell_w, cell_h) = ((width - gap) / 2, (height - gap) / 2);
                for (at, id) in ids.iter().take(4).enumerate() {
                    let (column, row) = (at as i32 % 2, at as i32 / 2);
                    picture(canvas, *id, [left + column * (cell_w + gap), top + row * (cell_h + gap), cell_w, cell_h]);
                }
            }
            canvas.round_off(left, top, width, height, round, GRID_COLOR);
            // Ringed brightly when picked out; quietly, in the accent,
            // when it is the workspace that was on show.
            if which == cursor || which == self.space {
                let edge = (pad / 5).max(2);
                let color = if which == cursor { TEXT_COLOR } else { accent() };
                canvas.round_outline(left - 2 * edge, top - 2 * edge, width + 4 * edge, height + 4 * edge, round + 2 * edge, edge, color);
            }

            let space = &self.spaces[which];
            let name = clipped(text, &space.name, width, px * 5 / 4);
            let name_top = top + height + pad / 2;
            let name_left = left + (width - text.width_strong(&name, px * 5 / 4)) / 2;
            text.draw_strong(canvas, name_left, name_top, px * 5 / 4, if which == cursor { TEXT_COLOR } else { DIM_TEXT_COLOR }, &name);
            let files = match ids.len() {
                0 if which != self.space => "listing\u{2026}".to_string(),
                1 => "1 file".to_string(),
                count => format!("{count} files"),
            };
            let key = Some(self.space_key(which)).filter(|key| !key.is_empty()).map(|key| format!("{key}   ")).unwrap_or_default();
            let about = space.caption.clone().unwrap_or_else(|| format!("{key}{files}{}", if space.saved { "" } else { "   not saved" }));
            let about = clipped(text, &about, width, px);
            let about_left = left + (width - text.width(&about, px)) / 2;
            text.draw(canvas, about_left, name_top + line * 5 / 4, px, DIM_TEXT_COLOR, &about);
        }
        let hint = match what {
            "Sessions" => "Enter to open  ·  Delete to forget  ·  Esc to close",
            _ => "Enter to open  ·  Esc to close",
        };
        let small = (px * 5 / 6).max(11);
        let hint_left = (canvas.width - text.width(hint, small)) / 2;
        text.draw(canvas, hint_left, canvas.height - Text::line_height(small) - pad, small, DIM_TEXT_COLOR, hint);

        // The slideshow moves on while there is more than one to show.
        if self.space_ids(cursor).len() > 1 && self.overview_due.is_none() {
            self.overview_due = Some(Instant::now() + OVERVIEW_STEP);
        }
    }

}
