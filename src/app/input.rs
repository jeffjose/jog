//! Input: keys, the mouse and the wheel, sent where they belong.

use super::*;

/// The pointer since it came into the window: about to be told where it
/// is, found somewhere and not moved from there, or moved.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Arrival {
    Coming,
    At(f64, f64),
    Moved,
}

/// How far the pointer has to go from where it came into the window to
/// count as moved, in pixels.
const POINTER_MOVED: f64 = 3.0;

impl App {

    pub(super) fn key_pressed(&mut self, key: Key<&str>, repeat: bool, numpad: bool, event_loop: &ActiveEventLoop) {
        if self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() {
            self.row_target = None;
        }
        if self.help {
            // What is typed looks for a shortcut; Escape empties that, and
            // then, or Enter, or `?` before anything is typed, closes it.
            let typing = !self.modifiers.control_key() && !self.modifiers.alt_key();
            match key {
                Key::Named(NamedKey::Escape) if !self.help_search.is_empty() => self.help_search.clear(),
                Key::Named(NamedKey::Backspace) => drop(self.help_search.pop()),
                Key::Named(NamedKey::Space) if !self.help_search.is_empty() => self.help_search.push(' '),
                Key::Character("?") if self.help_search.is_empty() => self.help = false,
                Key::Character(typed) if typing => self.help_search.push_str(typed),
                Key::Named(NamedKey::Escape | NamedKey::Enter) => self.help = false,
                _ => {}
            }
            return self.redraw();
        }
        if self.confirm.is_some() {
            return self.confirm_key(key, event_loop);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key, event_loop);
        }
        if self.editor.is_some() {
            return self.editor_key(key);
        }
        // The track picker takes the keys it knows; the rest work as ever.
        if self.track_picker.is_some() && self.picker_key(&key) {
            return;
        }
        // Esc closes the Activity panel first.
        if self.activity_key(&key) {
            return;
        }
        if self.convert_panel.is_some() {
            return self.panel_key(key, event_loop);
        }
        if self.video_panel.is_some() {
            return self.video_panel_key(key);
        }
        if self.compare.is_some() {
            return self.compare_key(key);
        }
        if self.track_editor.is_some() {
            return self.track_editor_key(key);
        }
        // The file list clicked on last has the arrow keys: up and down go
        // through it, opening each file as they come to it.
        if self.list_keys && self.modifiers.is_empty() && self.palette.is_none() && self.menu.is_none() && self.list_search.is_none() && self.list_key(key.clone()) {
            return;
        }
        if self.inspector.is_some() {
            // What the inspector does not take goes on to the video.
            if self.inspector_key(key.clone()) {
                return;
            }
        }
        if self.overview.is_some() && self.palette.is_none() && self.overview_key(key.clone()) {
            return;
        }
        if self.palette.is_some() {
            self.palette_key(key);
            // A command picked in the box is run once the box has closed.
            if let Some(command) = self.run_next.take() {
                self.execute(command, event_loop);
            }
            return;
        }
        // Going to another workspace works from anywhere but a text box.
        let to_space = commands::for_key(key.clone(), self.modifiers, numpad);
        if let Some(command @ (Command::WorkspaceKey(_) | Command::NextWorkspace | Command::PrevWorkspace | Command::HomeWorkspace)) = to_space {
            self.menu = None;
            return self.execute(command, event_loop);
        }
        if self.menu.is_some() {
            return self.menu_key(key, event_loop);
        }
        // What is typed goes into the file list's search while that has
        // the keyboard, and with the list up `/` is what gives it to it.
        if self.list_search.is_some() && self.grid.is_none() {
            return self.list_search_key(key);
        }
        if matches!(key, Key::Character("/")) && self.modifiers.is_empty() && self.list_panel().is_some() {
            return self.focus_list_search();
        }
        if self.grid.is_some() {
            return self.grid_key(key, event_loop);
        }
        // Escape leaves a kept search before it leaves the program.
        if self.filter.is_some() && key == Key::Named(NamedKey::Escape) {
            return self.clear_filter(false);
        }
        // While triaging, the space bar keeps the file on view and `x`
        // rejects it; everything else is as it always is.
        if self.triage_on && self.modifiers.is_empty() {
            let verdict = match key {
                Key::Named(NamedKey::Space) => Some(Verdict::Kept),
                Key::Character("x") => Some(Verdict::Rejected),
                _ => None,
            };
            if let Some(verdict) = verdict {
                if !repeat {
                    self.decide(verdict);
                }
                return;
            }
        }
        // `v` held is the loupe, there until the key is let go.
        if matches!(key, Key::Character("v")) && self.modifiers.is_empty() {
            if !repeat {
                self.loupe = Some(1.0);
                self.request();
                self.redraw();
            }
            return;
        }
        if self.video.is_some() && self.video_key(key.clone(), repeat) {
            return;
        }
        if let Some(command) = commands::for_key(key, self.modifiers, numpad) {
            if !repeat || command.repeats() {
                self.execute(command, event_loop);
            }
        }
    }

    pub(super) fn mouse_moved(&mut self, x: f64, y: f64) {
        let (dx, dy) = (x - self.mouse.0, y - self.mouse.1);
        self.mouse = (x, y);
        if self.editor.is_some() {
            return self.editor_motion();
        }
        // An edge held follows the pointer, and nothing else sees it move.
        if let Some(Drag::Resize(edge)) = self.drag {
            self.drag_edge_to(edge, x, y);
            return;
        }
        if let Some(Drag::Move { which, from, start }) = self.drag {
            return self.move_dialog(which, from, start, x, y);
        }
        self.edge_cursor(x, y);
        // The button held down and then moved is a drag, not a hold; and
        // the loupe goes where the pointer does.
        if self.hold.as_ref().is_some_and(|hold| !hold.by_key && !hold.fast && (x - hold.at.0).hypot(y - hold.at.1) > 6.0) {
            self.hold = None;
        }
        if self.loupe.is_some() {
            self.redraw();
        }
        // Over the Convert panel the pointer moves the divider and lights
        // up what it is on; nothing behind the panel sees it.
        if self.convert_panel.is_some() || self.video_panel.is_some() || self.compare.is_some() {
            return self.redraw();
        }
        // The scene under the pointer is picked out, and plays.
        let over = self.scene_at(x, y);
        if over != self.scene_over {
            self.scene_over = over;
            self.redraw();
        }

        // In the delete confirmation the button under the pointer is the
        // one picked out, Delete turning red as it is.
        if self.confirm.is_some() {
            let over = self.confirm_button_at(x, y);
            if let (Some(confirm), Some(yes)) = (&mut self.confirm, over) {
                if confirm.yes != yes {
                    confirm.yes = yes;
                    self.redraw();
                }
            }
            return;
        }

        // In the find box the match under the pointer is picked out, as
        // Up and Down would pick it. Only when the pointer has really
        // moved: a list scrolling under a still pointer picks nothing.
        // In the overview the card under the pointer is the one picked
        // out, and so the one running through its pictures.
        if self.overview.is_some() && self.palette.is_none() {
            let over = self.overview_card_at(x, y).filter(|_| (dx, dy) != (0.0, 0.0));
            if let (Some(overview), Some(which)) = (&mut self.overview, over) {
                if overview.cursor != which {
                    (overview.cursor, overview.tick) = (which, 0);
                    self.overview_due = None;
                    self.request();
                    self.redraw();
                }
            }
            return;
        }
        if self.palette.is_some() {
            let row = self.palette_row_at(x, y).filter(|_| (dx, dy) != (0.0, 0.0));
            if let (Some(palette), Some(row)) = (&mut self.palette, row) {
                if palette.selected != row {
                    (palette.selected, palette.focus) = (row, 0);
                    self.request();
                    self.redraw();
                }
            }
            return;
        }

        if let Some(menu) = &mut self.menu {
            // Moving the pointer elsewhere leaves a keyboard selection alone.
            if let Some(sub) = &mut menu.sub {
                let hovered = sub.row_at(x, y);
                if hovered.is_some() {
                    if hovered != sub.selected {
                        sub.selected = hovered;
                        self.redraw();
                    }
                    return;
                }
            }
            let hovered = menu.row_at(x, y);
            if hovered.is_some() && hovered != menu.selected {
                menu.selected = hovered;
                // Resting on an item that has a menu of its own opens it.
                menu.sub = None;
                self.open_submenu(false);
                self.redraw();
            }
            return;
        }
        if self.grid.is_some() {
            // Dragging along the page bar goes through the pages, and
            // picks nothing out on the way.
            if matches!(self.drag, Some(Drag::Pages)) {
                return self.drag_pages(x);
            }
            self.point_cursor();
            // The buttons light up under the pointer.
            let inside = |b: &Button| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32));
            let arrows = self.grid_arrows();
            let close = self.grid_close();
            let over = self.grid_buttons.iter().chain(arrows.iter().map(|(place, _)| place)).chain(close.iter()).position(inside);
            if over != self.grid_over {
                self.grid_over = over;
                self.redraw();
            }
            return;
        }
        let hover = self.list_row_at(x, y);
        // The buttons at the head of the file list light up as well.
        let on_button = self.list_buttons().iter().position(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32)));
        if hover != self.list_hover || on_button != self.list_button_over {
            (self.list_hover, self.list_button_over) = (hover, on_button);
            self.redraw();
        }
        let over = self.strip_cell_at(x, y);
        // The buttons of a triage light up under the pointer.
        let on_triage = self.triage_button_at(x, y);
        if on_triage != self.triage_over {
            self.triage_over = on_triage;
            self.redraw();
        }
        // The buttons on the strip light up under the pointer too.
        let on_button = self.strip_button_at(x, y);
        if over != self.strip_over || on_button != self.strip_button_over {
            (self.strip_over, self.strip_button_over) = (over, on_button);
            self.redraw();
        }
        let on_video = self.video.is_some() || self.list.get(self.focus()).is_some_and(|id| files::is_video(&self.files[*id]));
        // Where the pointer was when it came into the window is no move of
        // it: a window opening under a pointer at its foot would otherwise
        // bring the filmstrip, or the seek bar's preview, up for a moment.
        // What is along the bottom waits for the pointer to move.
        let moved = match self.pointer_found {
            Arrival::Coming => {
                self.pointer_found = Arrival::At(x, y);
                false
            }
            Arrival::At(fx, fy) if (x - fx).hypot(y - fy) < POINTER_MOVED => false,
            Arrival::At(..) => {
                self.pointer_found = Arrival::Moved;
                true
            }
            Arrival::Moved => true,
        };
        match &mut self.drag {
            Some(Drag::Pan) => self.pan(dx, dy),
            // Only in the grid, which is dealt with above.
            Some(Drag::Pages | Drag::Resize(_) | Drag::Move { .. }) => {}
            Some(Drag::Zoom { start_x, start_scale, anchor, moved }) => {
                *moved = true;
                let (scale, anchor) = (*start_scale * ((x - *start_x) / 300.0).exp(), *anchor);
                if let Some(native) = self.native() {
                    let (left, stage) = self.stage();
                    self.view.zoom_to(scale, (anchor.0 - f64::from(left), anchor.1), native, stage);
                    self.view_changed();
                }
            }
            Some(Drag::Scrub) => {
                let bar = self.seek_bar();
                let fraction = ((x - f64::from(bar.x)) / f64::from(bar.width)).clamp(0.0, 1.0);
                self.hover = Some(fraction);
                self.request_preview(fraction);
                self.seek_to(fraction, false);
            }
            Some(Drag::Section(edge)) => {
                let edge = *edge;
                let bar = self.seek_bar();
                self.drag_edge(edge, (x - f64::from(bar.x)) / f64::from(bar.width));
            }
            // Over a video the bottom of the window belongs to the seek bar,
            // so the filmstrip does not also open there; `t` still pins it.
            // A video by what the file is, not by its player having begun:
            // as the window opens the pointer may be at its foot before the
            // first video is playing, and the filmstrip would come up for a
            // moment and go.
            None if on_video => {
                self.set_strip_hover(false);
                if self.video.is_some() && moved {
                    self.video_pointer(x, y);
                }
            }
            None if !moved => {}
            None => {
                let height = self.window_size().1;
                // A thin strip at the bottom edge opens the filmstrip; once
                // open, it stays while the pointer is anywhere over it.
                // The button that closes it sits just above it.
                let reach = ui_px(height) as i32 * 2;
                let zone = if self.band_shown() { Strip::for_height(height).height + reach } else { (height as i32 / 50).max(6) };
                self.set_strip_hover(y >= f64::from(height as i32 - zone));
            }
        }
    }

    pub(super) fn set_strip_hover(&mut self, hover: bool) {
        if hover == self.strip_hover {
            return;
        }
        self.strip_hover = hover;
        if hover && !self.strip_used {
            self.strip_used = true;
            self.request();
        }
        self.redraw();
    }

    /// A key while the file list has the arrow keys; whether it took it.
    /// Up and down go to the file before and after in the list, as far as
    /// its ends; Page Up and Down a page of rows; Home and End its first
    /// and last; Escape gives the keys back to the picture.
    fn list_key(&mut self, key: Key<&str>) -> bool {
        let Some(panel) = self.list_panel() else {
            self.list_keys = false;
            return false;
        };
        let last = self.list.len().saturating_sub(1);
        let page = panel.rows.saturating_sub(1).max(1);
        let to = match key {
            Key::Named(NamedKey::ArrowUp) => self.pos.saturating_sub(1),
            Key::Named(NamedKey::ArrowDown) => (self.pos + 1).min(last),
            Key::Named(NamedKey::PageUp) => self.pos.saturating_sub(page),
            Key::Named(NamedKey::PageDown) => (self.pos + page).min(last),
            Key::Named(NamedKey::Home) => 0,
            Key::Named(NamedKey::End) => last,
            Key::Named(NamedKey::Escape) => {
                self.list_keys = false;
                self.redraw();
                return true;
            }
            _ => return false,
        };
        if to != self.pos {
            self.go_to(to, to > self.pos);
        }
        true
    }

    pub(super) fn mouse_button(&mut self, button: MouseButton, pressed: bool, event_loop: &ActiveEventLoop) {
        // Edit has the whole window; while its copy is being named, the
        // box has the keys and the mouse waits.
        if self.editor.is_some() {
            if self.prompt.is_none() {
                match button {
                    MouseButton::Left => self.editor_mouse(pressed),
                    MouseButton::Right if pressed => self.editor_right_click(),
                    MouseButton::Middle => self.editor_pan(pressed),
                    _ => {}
                }
            }
            return;
        }
        let (x, y) = self.mouse;
        // The arrow keys go where the last click was: to the file list after
        // a click on it, to the picture after a click anywhere else.
        if button == MouseButton::Left && pressed {
            let on_list = self.list_panel().is_some_and(|panel| panel.holds(x, y));
            if on_list != self.list_keys {
                self.list_keys = on_list;
                self.redraw();
            }
        }
        // A panel's edge taken hold of, or let go of.
        if button == MouseButton::Left {
            if !pressed && matches!(self.drag, Some(Drag::Resize(_))) {
                return self.let_go_of_edge();
            }
            if !pressed && matches!(self.drag, Some(Drag::Move { .. })) {
                self.drag = None;
                return;
            }
            if pressed && (self.grab_edge(x, y) || self.grab_dialog(x, y)) {
                return;
            }
        }
        // The row a menu was brought up on is done with once the menu and
        // whatever it asked are.
        if pressed && self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() {
            self.row_target = None;
            self.entry_target = None;
        }
        // A right click on a kept section in the Sections tab: what can be
        // done with it.
        if button == MouseButton::Right && pressed && self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() {
            if let Some(index) = self.entry_at(x, y).filter(|index| self.entries().get(*index).is_some_and(|entry| entry.kept)) {
                self.entry_target = Some(index);
                self.open_menu(commands::SECTION_MENU, None);
                return self.redraw();
            }
        }
        // The mouse's own Back and Forward, as in a browser.
        let quiet = self.menu.is_none() && self.prompt.is_none() && self.confirm.is_none() && self.palette.is_none();
        if pressed && quiet && matches!(button, MouseButton::Back | MouseButton::Forward) {
            return self.travel(button == MouseButton::Back);
        }
        if self.help {
            self.help = !pressed;
            return self.redraw();
        }
        // The Activity panel and the pill of what runs take a click on them.
        if button == MouseButton::Left && pressed && self.confirm.is_none() && self.menu.is_none() && self.activity_click(x, y) {
            return self.redraw();
        }
        if button == MouseButton::Left && pressed && self.confirm.is_none() && self.toast_click(x, y, event_loop) {
            return self.redraw();
        }
        if self.confirm.is_some() {
            // A click on one of its buttons presses it; elsewhere, nothing.
            if button == MouseButton::Left && pressed {
                match self.confirm_button_at(x, y) {
                    Some(true) => {
                        if let Some(confirm) = self.confirm.take() {
                            self.delete(&confirm.ids, event_loop);
                        }
                    }
                    Some(false) => self.confirm = None,
                    None => {}
                }
                self.redraw();
            }
            return;
        }
        // The track picker takes a click on it, and closes on one elsewhere.
        if self.track_picker.is_some() && button == MouseButton::Left && pressed && self.picker_click(x, y) {
            return;
        }
        if self.convert_panel.is_some() {
            if button == MouseButton::Left && pressed {
                self.panel_click(x, y);
            }
            return;
        }
        if self.video_panel.is_some() {
            if button == MouseButton::Left && pressed {
                self.video_panel_click(x, y);
            }
            return;
        }
        if self.compare.is_some() {
            if button == MouseButton::Left && pressed {
                self.compare_click(x, y);
            }
            return;
        }
        if self.track_editor.is_some() {
            if button == MouseButton::Left && pressed {
                self.track_editor_click(x, y);
            }
            return;
        }
        // On the inspector's panel, its own; beside it, the video's.
        if self.over_inspector(x, y) {
            if button == MouseButton::Left && pressed {
                self.inspector_click(x, y, event_loop);
            }
            return;
        }
        if self.overview.is_some() && self.palette.is_none() {
            // A click on a card goes to that workspace.
            if button == MouseButton::Left && pressed {
                if let Some(which) = self.overview_card_at(x, y) {
                    self.leave_overview(Some(which));
                }
            }
            return;
        }
        if self.palette.is_some() {
            // A click on a match goes to it; one anywhere else closes the box.
            if button == MouseButton::Left && pressed {
                let on = |[left, top, width, height]: [i32; 4]| {
                    (left..left + width).contains(&(x as i32)) && (top..top + height).contains(&(y as i32))
                };
                let drawn = self.palette.as_ref().map(|palette| palette.drawn.clone()).unwrap_or_default();
                let button = drawn.into_iter().find(|(_, place)| on(*place)).map(|(end, _)| end);
                let row = self.palette_row_at(x, y);
                match (&mut self.palette, button, row) {
                    (Some(_), Some(end), _) => self.finish_palette(end),
                    (Some(palette), None, Some(row)) => {
                        palette.selected = row;
                        self.finish_palette(PaletteEnd::Go);
                        if let Some(command) = self.run_next.take() {
                            self.execute(command, event_loop);
                        }
                    }
                    _ => self.palette = None,
                }
                self.redraw();
            }
            return;
        }
        // A click on one of the buttons of a triage presses it.
        let on_triage = self.triage_button_at(x, y).map(|at| self.triage_buttons.borrow()[at].0);
        if let Some(command) = on_triage.filter(|_| self.triage_on && self.grid.is_none() && self.menu.is_none() && self.prompt.is_none()) {
            if button == MouseButton::Left && pressed {
                match command {
                    Command::Reload => self.triage_refresh(event_loop),
                    command => self.execute(command, event_loop),
                }
            }
            return;
        }
        if self.grid.is_some() && self.menu.is_none() {
            let hit = |b: &&Button| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32));
            match (button, pressed) {
                (MouseButton::Left, true) => {
                    // The buttons under the grid, then those at its sides
                    // for the pages either way, then the pictures.
                    let arrow = self.grid_arrows().into_iter().find(|(place, _)| hit(&place)).map(|(_, forward)| forward);
                    if self.grid_close().as_ref().is_some_and(|place| hit(&place)) {
                        return self.toggle_grid();
                    }
                    if let Some(command) = self.grid_buttons.iter().find(hit).map(|button| button.command) {
                        self.execute(command, event_loop);
                    } else if let Some(forward) = arrow {
                        let per_page = self.grid_layout(self.window_size()).per_page() as i64;
                        self.move_cursor(if forward { per_page } else { -per_page });
                    } else if self.grid_bar().is_some_and(|bar| bar.holds((x, y))) {
                        // A click on the bar goes to the page at that spot;
                        // keeping the button down drags through them.
                        self.drag = Some(Drag::Pages);
                        self.drag_pages(x);
                    } else if self.point_cursor() {
                        // Shift marks the run from the last one marked to
                        // here; Ctrl marks this one; a plain click opens it.
                        let cursor = self.focus();
                        if self.modifiers.shift_key() {
                            self.mark_range(cursor);
                        } else if self.modifiers.control_key() {
                            self.mark_one(cursor);
                        } else {
                            self.open_from_grid();
                        }
                    }
                }
                (MouseButton::Left, false) => self.drag = None,
                // A right click on a picture brings up what can be done
                // with it.
                (MouseButton::Right, true) => match self.point_cursor() {
                    true => self.open_menu(commands::GRID_MENU, None),
                    // Off the pictures, it is the grid as a whole.
                    false => self.open_menu(commands::GRID_AROUND_MENU, None),
                },
                _ => {}
            }
            return;
        }
        match (button, pressed) {
            (MouseButton::Left, true) if self.menu.is_some() => {
                if let Some(menu) = &mut self.menu {
                    // A click outside both menus selects nothing: it closes them.
                    let inner = menu.sub.as_mut().map(|sub| (sub.row_at(x, y), sub));
                    match inner {
                        Some((Some(row), sub)) => sub.selected = Some(row),
                        _ => {
                            menu.sub = None;
                            menu.selected = menu.row_at(x, y);
                        }
                    }
                }
                self.activate_menu(event_loop);
            }
            (MouseButton::Left, true) if self.hover.is_some() => {
                let bar = self.seek_bar();
                if let Some(tool) = self.section_tool_at(x, y) {
                    self.section_tool(tool);
                } else if let Some(command) = self.bar_hit(x, y) {
                    // The title, the time or a detail: copied.
                    self.execute(command, event_loop);
                } else if bar.on_control(0, x, y) {
                    self.step(false);
                } else if bar.on_control(2, x, y) {
                    self.step(true);
                } else if let Some(command) = (0..).zip(self.bar_buttons()).find(|(which, _)| bar.on_button(*which, x, y)).map(|(_, command)| command) {
                    self.execute(command, event_loop);
                } else if bar.on_symbol(x, y) {
                    self.video_action(VideoAction::Pause);
                } else if let Some(edge) = self.section_edge_at(&bar, x) {
                    // An end of the section is taken hold of, to move it.
                    self.drag = Some(Drag::Section(edge));
                } else if bar.on_track(x) && y >= f64::from(bar.middle - bar.unit * 4) {
                    // Not from as high up as the section's buttons: a click that
                    // misses one of them is no seek.
                    // Click to jump there; keep the button down to scrub.
                    // On another section, that one is in use from then on.
                    if let Some(n) = self.other_section_at(x, y) {
                        self.activate_section(n);
                    }
                    self.seek_to(self.hover.unwrap_or(0.0), self.precise);
                    self.drag = Some(Drag::Scrub);
                }
            }
            // The buttons on the debug overlay copy its figures.
            // The choices of rendition in the info panel.
            (MouseButton::Left, true) if self.info_buttons.iter().any(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32))) => {
                let hit = self.info_buttons.iter().find(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32)));
                if let Some(command) = hit.map(|button| button.command) {
                    self.execute(command, event_loop);
                }
            }
            (MouseButton::Left, true) if self.debug_buttons.iter().any(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32))) => {
                let hit = self.debug_buttons.iter().find(|b| (b.x..b.x + b.width).contains(&(x as i32)) && (b.y..b.y + b.height).contains(&(y as i32)));
                if let Some(command) = hit.map(|button| button.command) {
                    self.execute(command, event_loop);
                }
            }
            (MouseButton::Left, true) => {
                let height = self.window_size().1;
                let strip = Strip::for_height(height);
                let panel = self.list_panel().filter(|panel| panel.holds(x, y));
                // The file list's own parts are measured from where it stands.
                let within = x as i32 - self.list_panel().map_or(0, |panel| panel.left);
                // A click in the search box of the file list puts the
                // keyboard there, and one anywhere else takes it away.
                let in_field = self.list_field().is_some_and(|[fx, fy, w, h]| (fx..fx + w).contains(&within) && (fy..fy + h).contains(&(y as i32)));
                if !in_field {
                    self.list_search = None;
                }
                if self.list_click(x, y, in_field, panel.is_some(), event_loop) {
                    // The file list's, done.
                } else if let Some(which) = self.strip_button_at(x, y) {
                    self.strip_button(which);
                } else if self.scenes_shown() && y >= f64::from(strip.top) && self.strip_click(x, y, ui_px(height)) {
                    // A tab, or a section marked from the Sections tab.
                } else if self.scenes_shown() && y >= f64::from(strip.top) {
                    // A click on a scene plays from where it starts.
                    let start = self.scene_at(x, y).and_then(|index| Some(self.scenes_now()?.1[index]));
                    if let (Some(start), Some((_, length))) = (start, self.video_time()) {
                        self.seek_to(start / length, true);
                        // The seek bar comes up to show how far in that is.
                        self.time_until = Some(Instant::now() + TOAST);
                    }
                } else if self.strip_visible() && y >= f64::from(strip.top) {
                    if let Some(pos) = self.strip_cell_at(x, y) {
                        // The list and the strip stay as they are: the one
                        // clicked is not brought to the middle.
                        let first = self.list_panel().map(|panel| panel.first);
                        let center = self.strip_center();
                        self.go_to(pos, pos > self.pos);
                        self.list_scroll = first.map(|first| (self.pos, self.list.len(), first));
                        self.strip_scroll = center - self.pos as i64;
                    }
                } else {
                    // A second click in the same spot does what `f` does.
                    let near = |(at, (cx, cy)): (Instant, (f64, f64))| {
                        at.elapsed() < DOUBLE_CLICK && (x - cx).abs() < 8.0 && (y - cy).abs() < 8.0
                    };
                    if self.last_click.take().is_some_and(near) {
                        self.toggle_fullscreen();
                    } else {
                        self.last_click = Some((Instant::now(), (x, y)));
                        self.drag = Some(Drag::Pan);
                        // Kept down on a video, it hurries it along.
                        self.begin_hold(false);
                    }
                }
            }
            (MouseButton::Middle, true) => {
                if let Some(native) = self.native() {
                    let start_scale = self.view.scale(native, self.stage().1);
                    self.drag = Some(Drag::Zoom { start_x: x, start_scale, anchor: (x, y), moved: false });
                }
            }
            (MouseButton::Left | MouseButton::Middle, false) => {
                self.end_hold(false);
                match self.drag.take() {
                    Some(Drag::Zoom { moved: false, .. }) => self.execute(Command::ZoomActual, event_loop),
                    Some(Drag::Section(edge)) => {
                        self.drop_edge(edge);
                        // Let go: the hand open again.
                        self.edge_cursor(x, y);
                    }
                    _ => {}
                }
            }
            // On the section on the seek bar, or its buttons: what can be
            // done with it.
            (MouseButton::Right, true) if self.on_section(x, y) => {
                self.open_menu(commands::SECTION_BAR_MENU, None);
                self.redraw();
            }
            // On the bar's title, time or details: what of the file to copy.
            (MouseButton::Right, true) if self.bar_hit(x, y).is_some() => {
                self.open_menu(commands::BAR_INFO_MENU, None);
                self.redraw();
            }
            // On another section, that one in use, and what can be done with it.
            (MouseButton::Right, true) if self.other_section_at(x, y).is_some() => {
                if let Some(n) = self.other_section_at(x, y) {
                    self.activate_section(n);
                }
                self.open_menu(commands::SECTION_BAR_MENU, None);
                self.redraw();
            }
            // Elsewhere along the seek bar: sections begun and ended there.
            (MouseButton::Right, true) if self.on_seek_track(x, y) => {
                self.bar_menu_at = self.hover.zip(self.video_time()).map(|(fraction, (_, length))| fraction * length);
                self.open_menu(commands::SEEK_BAR_MENU, None);
                self.redraw();
            }
            // On a row of the file list, the menu is of that file.
            (MouseButton::Right, true) => match self.list_row_at(x, y) {
                Some(at) => {
                    self.row_target = Some(at);
                    self.open_menu(commands::LIST_MENU, None);
                    self.redraw();
                }
                None => self.execute(Command::ToggleMenu, event_loop),
            },
            _ => {}
        }
    }

    pub(super) fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            MouseScrollDelta::PixelDelta(position) => position.y / 50.0,
        };
        // Over the Activity panel, it scrolls that.
        if self.activity_wheel(lines) {
            return;
        }
        if self.editor.is_some() {
            return self.editor_wheel(lines);
        }
        // With the loupe up, the wheel makes it stronger or weaker.
        if let Some(strength) = &mut self.loupe {
            *strength = (*strength * 1.25f64.powf(lines)).clamp(0.5, 8.0);
            return self.redraw();
        }
        if lines == 0.0 || self.menu.is_some() {
            return;
        }
        if self.convert_panel.is_some() {
            return self.panel_wheel(lines);
        }
        if self.video_panel.is_some() {
            return;
        }
        if self.compare.is_some() {
            return self.compare_wheel(lines);
        }
        // Over Inspect, its Files tab's list scrolls as the list does.
        let on_list = self.list_panel().is_some_and(|panel| panel.embedded && panel.holds(self.mouse.0, self.mouse.1));
        if self.over_inspector(self.mouse.0, self.mouse.1) && !on_list {
            return self.inspector_wheel(lines);
        }
        // In the find box it scrolls the list of matches, and what then
        // lies under the pointer is the one picked out. It does not step
        // through them one at a time: the pointer and the arrows do that.
        if self.overview.is_some() && self.palette.is_none() {
            return;
        }
        if self.palette.is_some() {
            let rows = self.palette_layout().map_or(1, |layout| layout.rows.max(1));
            if let Some(palette) = &mut self.palette {
                let furthest = palette.rows().saturating_sub(rows) as i64;
                palette.first = (palette.first as i64 - (lines * 3.0).round() as i64).clamp(0, furthest) as usize;
                // A pick that has gone out of view is brought to its edge.
                palette.selected = palette.selected.clamp(palette.first, palette.first + rows - 1);
            }
            let under = self.palette_row_at(self.mouse.0, self.mouse.1);
            if let (Some(palette), Some(row)) = (&mut self.palette, under) {
                palette.selected = row;
            }
            self.request();
            return self.redraw();
        }
        // Over the Sections tab it scrolls a row too long for the window.
        if self.over_entries(self.mouse.1) {
            return self.scroll_entries(-(lines.round() as i64).clamp(-3, 3));
        }
        // Over the filmstrip it moves the strip along, to look at what is
        // further off without leaving the file on view. A click goes there.
        if self.strip_cell_at(self.mouse.0, self.mouse.1).is_some() {
            let last = self.list.len() as i64 - 1;
            let center = (self.strip_center() - (lines * 2.0).round() as i64).clamp(0, last);
            self.strip_scroll = center - self.pos as i64;
            self.strip_over = self.strip_cell_at(self.mouse.0, self.mouse.1);
            self.request();
            return self.redraw();
        }
        // Over the file list the wheel moves the list, not the file on view.
        let (x, y) = self.mouse;
        if let Some(panel) = self.list_panel().filter(|panel| panel.holds(x, y)) {
            let rows = (lines * 3.0).round() as i64;
            let last = (self.list.len() - panel.rows) as i64;
            let first = (panel.first as i64 - rows).clamp(0, last) as usize;
            self.list_scroll = Some((self.pos, self.list.len(), first));
            self.list_hover = self.list_row_at(x, y);
            return self.redraw();
        }
        if self.modifiers.control_key() && self.grid.is_none() {
            self.scrolled = 0.0;
            return self.zoom_by(ZOOM_STEP.powf(lines), Some(self.mouse));
        }
        // A touchpad reports many small movements per gesture; stepping on
        // each would race through the list. Step once per whole line.
        if (self.scrolled < 0.0) != (lines < 0.0) {
            self.scrolled = 0.0;
        }
        self.scrolled += lines;
        let steps = self.scrolled.trunc();
        self.scrolled -= steps;
        for _ in 0..steps.abs() as u32 {
            if self.grid.is_some() {
                // A page at a time.
                let per_page = self.grid_layout(self.window_size()).per_page() as i64;
                self.move_cursor(if steps < 0.0 { per_page } else { -per_page });
            } else {
                self.step(steps < 0.0);
            }
        }
    }

}

impl App {
    /// A click on the file list: in its search box, on a button at its
    /// head, a row's loop or star, or a row. Whether it was any of them.
    /// The main window's and the list's own, popped out, alike.
    pub(super) fn list_click(&mut self, x: f64, y: f64, in_field: bool, on_panel: bool, event_loop: &ActiveEventLoop) -> bool {
        let within = x as i32 - self.list_panel().map_or(0, |panel| panel.left);
        let inside = |b: &Button| (b.x..b.x + b.width).contains(&within) && (b.y..b.y + b.height).contains(&(y as i32));
        if in_field {
            self.focus_list_search();
        } else if let Some(command) = self.list_buttons().iter().find(|button| inside(button)).map(|button| button.command) {
            // One of the buttons at the head of the file list.
            self.execute(command, event_loop);
        } else if let Some(pos) = self.list_loop_at(x, y) {
            // The loop before it has that video play again when it
            // ends, whatever the list's loop.
            self.toggle_file_loop(self.list[pos]);
        } else if let Some(pos) = self.list_star_at(x, y) {
            // The star at the end of a row marks that file, or
            // takes its mark away.
            let id = self.list[pos];
            if !self.marks.remove(&id) {
                self.marks.insert(id);
            }
            self.redraw();
        } else if on_panel {
            // A click on a name goes to that file; with Shift it
            // marks the run to it, with Ctrl it marks it.
            let row = self.list_row_at(x, y);
            if let Some(pos) = row.filter(|_| self.modifiers.shift_key()) {
                self.mark_range(pos);
            } else if let Some(pos) = row.filter(|_| self.modifiers.control_key()) {
                self.mark_one(pos);
            } else if let Some(pos) = row {
                // The list and the strip stay as they are: the one
                // clicked is not brought to the middle.
                let first = self.list_panel().map(|panel| panel.first);
                let center = self.strip_center();
                self.go_to(pos, pos > self.pos);
                self.list_scroll = first.map(|first| (self.pos, self.list.len(), first));
                self.strip_scroll = center - self.pos as i64;
            }
        } else {
            return false;
        }
        true
    }
}
