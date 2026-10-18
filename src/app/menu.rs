//! The menu: laid out, and driven from the keyboard and the mouse.

use super::*;

impl App {

    /// Opens a menu of `entries`. One with a title is keyboard-first: it
    /// sits in the middle of the window with its first item selected. One
    /// without opens at the pointer.
    pub(super) fn open_menu(&mut self, entries: &'static [Option<MenuItem>], title: Option<String>) {
        let at = (self.mouse.0 as i32, self.mouse.1 as i32);
        self.menu = Some(self.build_menu(entries, title, at));
    }

    /// Opens the menu belonging to the selected item, beside it. From the
    /// keyboard its first item is selected, ready for the arrow keys.
    pub(super) fn open_submenu(&mut self, select_first: bool) {
        let Some(menu) = &self.menu else { return };
        let Some(row) = menu.selected.map(|row| &menu.rows[row]) else { return };
        let Some(item) = row.item.filter(|item| !item.sub.is_empty()) else { return };
        let (left, right, top) = (menu.x, menu.x + menu.width - 1, menu.y + row.top - menu.px as i32 / 2);
        let mut sub = self.build_menu(item.sub, None, (right, top));
        // Where there is no room to the right, it opens to the left.
        if sub.x < right {
            sub.x = (left - sub.width + 1).max(0);
        }
        if select_first {
            sub.selected = sub.rows.iter().position(|row| row.item.is_some());
        }
        if let Some(menu) = &mut self.menu {
            menu.sub = Some(Box::new(sub));
        }
    }

    /// Lays out a menu of `entries` with its corner at `at`, or as near as
    /// the window allows; one with a title goes in the middle instead.
    pub(super) fn build_menu(&mut self, all_entries: &'static [Option<MenuItem>], title: Option<String>, at: (i32, i32)) -> Menu {
        let (width, height) = self.window_size();
        let px = ui_px(height);

        // Leave out what does not apply to the files in question: zooming a
        // video, say, or pausing a photo. A marked set can hold both kinds.
        let ids = if title.is_some() { self.targets() } else { vec![self.list[self.focus()]] };
        let videos = ids.iter().filter(|id| files::is_video(&self.files[**id])).count();
        let applies = |item: &MenuItem| match item.applies {
            Applies::Always => true,
            Applies::Images => videos < ids.len(),
            Applies::Videos => videos > 0,
            Applies::Filtered => self.filter.is_some(),
        };
        let mut entries: Vec<Option<&'static MenuItem>> = Vec::new();
        for entry in all_entries {
            match entry {
                Some(item) if applies(item) => entries.push(Some(item)),
                // A separator only after an item, so none double up.
                None if matches!(entries.last(), Some(Some(_))) => entries.push(None),
                _ => {}
            }
        }
        if matches!(entries.last(), Some(None)) {
            entries.pop();
        }

        let output = self.options.output_dir.as_deref().map(short_dir);
        let text = self.text.get_or_insert_with(Text::load);
        let (row_height, separator_height, pad) = (px as i32 * 9 / 5, px as i32 * 3 / 5, px as i32 / 2);

        let items = entries.iter().flatten();
        let label_width = items.clone().map(|i| text.width(&Self::label_for(i, output.as_deref()), px)).max().unwrap_or(0);
        let keys_width = items.map(|i| text.width(i.keys, px)).max().unwrap_or(0);
        let title_width = title.as_ref().map_or(0, |title| text.width(title, px));
        // Padding, a gutter for check marks, the label, a gap and the keys.
        let content_width = (px as i32 * 2 + label_width + px as i32 * 2 + keys_width).max(title_width);
        let menu_width = content_width + 2 * pad;

        let mut top = pad + if title.is_some() { row_height } else { 0 };
        let mut rows = Vec::new();
        for &item in &entries {
            let row_height = if item.is_some() { row_height } else { separator_height };
            rows.push(MenuRow { top, height: row_height, item });
            top += row_height;
        }
        let menu_height = top + pad;

        let centered = ((width as i32 - menu_width) / 2, (height as i32 - menu_height) / 2);
        let (x, y) = if title.is_some() { centered } else { at };
        Menu {
            x: x.clamp(0, (width as i32 - menu_width).max(0)),
            y: y.clamp(0, (height as i32 - menu_height).max(0)),
            width: menu_width,
            height: menu_height,
            px,
            selected: title.as_ref().and(rows.iter().position(|row| row.item.is_some())),
            rows,
            title,
            sub: None,
        }
    }

    /// The `X` menu: what can be done with the marked images.
    pub(super) fn open_mark_menu(&mut self) {
        let title = match self.marks.len() {
            0 => format!("Nothing marked  ·  {}", self.name_of(self.list[self.focus()])),
            1 => "1 marked image".to_string(),
            count => format!("{count} marked images"),
        };
        self.open_menu(commands::MARK_MENU, Some(title));
    }

    /// Moves the target files to the trash.
    pub(super) fn trash(&mut self, event_loop: &ActiveEventLoop) {
        if self.dry(|app| app.targets().iter().map(|id| format!("move to trash: {}", app.files[*id].display())).collect(), "would move to trash") {
            return;
        }
        self.trash_ids(self.targets(), false, event_loop);
    }

    /// Moves these files to the trash. Redo does too, without forgetting
    /// what else can be redone.
    pub(super) fn trash_ids(&mut self, ids: Vec<usize>, redoing: bool, event_loop: &ActiveEventLoop) {
        let (mut removed, mut failed, mut undoable) = (Vec::new(), 0, Vec::new());
        for id in ids {
            let Some(pos) = self.list.iter().position(|&other| other == id) else { continue };
            // Into the home trash, noted so that Undo can bring it back;
            // from another filesystem, by gio, for good.
            let done = match ops::trash(&self.files[id]) {
                Ok(Some(trashed)) => {
                    undoable.push((Moved { id, pos, from: self.files[id].clone(), to: trashed.file.clone() }, trashed));
                    true
                }
                Ok(None) => std::process::Command::new("gio").arg("trash").arg("--").arg(&self.files[id]).status().is_ok_and(|s| s.success()),
                Err(error) => {
                    eprintln!("jog: cannot trash {}: {error}", self.files[id].display());
                    false
                }
            };
            if done {
                println!("[trash] {}", self.files[id].display());
                removed.push((id, pos));
            } else {
                failed += 1;
            }
        }
        let mut summary = format!("Trashed {}", removed.len());
        if failed > 0 {
            summary.push_str(&format!("  ·  {failed} failed"));
        }
        if undoable.is_empty() {
            self.toast(summary);
        } else {
            self.toast_with(summary, vec![("Undo".into(), ToastLink::Undo)]);
            match redoing {
                true => self.undo.push(Undo::Trashed(undoable)),
                false => self.did(Undo::Trashed(undoable)),
            }
        }
        if !removed.is_empty() {
            self.drop_from_list(&removed, event_loop);
        }
    }

    pub(super) fn menu_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let Some(menu) = &mut self.menu else { return };
        let step = |menu: &mut Menu, forward: bool| {
            let len = menu.rows.len();
            let mut at = menu.selected.unwrap_or(if forward { len - 1 } else { 0 });
            loop {
                at = if forward { (at + 1) % len } else { (at + len - 1) % len };
                if menu.rows[at].item.is_some() {
                    break menu.selected = Some(at);
                }
            }
        };
        // The keys go to the menu opened last.
        let opens = menu.sub.is_none() && menu.selected.is_some_and(|row| menu.rows[row].item.is_some_and(|item| !item.sub.is_empty()));
        let inner = menu.sub.is_some();
        let active = match &mut menu.sub {
            Some(sub) => &mut **sub,
            None => menu,
        };
        match key {
            Key::Named(NamedKey::ArrowDown) => step(active, true),
            Key::Named(NamedKey::ArrowUp) => step(active, false),
            Key::Named(NamedKey::ArrowRight) if opens => self.open_submenu(true),
            Key::Named(NamedKey::ArrowLeft | NamedKey::Escape) if inner => menu.sub = None,
            Key::Named(NamedKey::Enter) => self.activate_menu(event_loop),
            Key::Named(NamedKey::Escape | NamedKey::Tab) => self.menu = None,
            _ => {}
        }
        self.redraw();
    }

    /// Runs the selected item and closes the menu, or opens the menu that
    /// the item stands for. With nothing selected it only closes.
    pub(super) fn activate_menu(&mut self, event_loop: &ActiveEventLoop) {
        let Some(menu) = &self.menu else { return };
        let active = menu.sub.as_deref().unwrap_or(menu);
        match active.selected.and_then(|row| active.rows[row].item) {
            Some(item) if !item.sub.is_empty() => self.open_submenu(true),
            Some(item) => {
                self.menu = None;
                self.execute(item.command, event_loop);
            }
            None => self.menu = None,
        }
        self.redraw();
    }

}
