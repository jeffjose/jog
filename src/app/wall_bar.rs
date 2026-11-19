//! The wall's bar: a row of controls floating at its foot, up while the
//! pointer is near the bottom edge. The page before and after, the pages
//! turning by themselves and how long each stays, how many across, the
//! sound, and the ways out.

use super::*;
use super::wall::HOLDS;

/// What a button of the bar does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum WallAction {
    Back,
    Auto,
    Forward,
    Shorter,
    Longer,
    Larger,
    Smaller,
    Sound,
    Grid,
    Close,
}

/// How far up from the bottom edge the pointer brings the bar, in bar
/// heights.
const REACH: i32 = 3;

impl App {
    /// Whether the pointer is near enough the foot of the wall for its bar.
    pub(super) fn wall_bar_reach(&self, y: f64) -> bool {
        let (_, height) = self.window_size();
        let px = ui_px(height) as i32;
        let bar = Text::line_height(px as u32) + px * 3 / 2;
        self.wall().is_some() && self.menu.is_none() && y >= f64::from(height as i32 - bar * REACH) && y < f64::from(height)
    }

    /// The button of the bar under a window position, while it is up.
    pub(super) fn wall_bar_at(&self, x: f64, y: f64) -> Option<WallAction> {
        let inside = |[bx, by, bw, bh]: [i32; 4]| (bx..bx + bw).contains(&(x as i32)) && (by..by + bh).contains(&(y as i32));
        self.wall_bar_up.then(|| self.wall_bar.iter().find(|(place, _)| inside(*place)).map(|(_, action)| *action)).flatten()
    }

    /// Whether a window position is on the bar, buttons or not.
    pub(super) fn on_wall_bar(&self, x: f64, y: f64) -> bool {
        let [bx, by, bw, bh] = self.wall_bar_place;
        self.wall_bar_up && (bx..bx + bw).contains(&(x as i32)) && (by..by + bh).contains(&(y as i32))
    }

    /// Follows the pointer: the bar comes up near the bottom edge and goes
    /// away from it, and its buttons light up under it.
    pub(super) fn wall_bar_hover(&mut self, x: f64, y: f64) {
        let up = self.wall_bar_reach(y);
        let over = if up { self.wall_bar_at(x, y) } else { None };
        if up != self.wall_bar_up || over != self.wall_bar_over {
            (self.wall_bar_up, self.wall_bar_over) = (up, over);
            self.redraw();
        }
    }

    pub(super) fn wall_bar_press(&mut self, action: WallAction, event_loop: &ActiveEventLoop) {
        match action {
            WallAction::Back => self.turn_wall(false),
            WallAction::Forward => self.turn_wall(true),
            WallAction::Auto => self.wall_autoplay(),
            WallAction::Shorter => self.wall_hold_by(false),
            WallAction::Longer => self.wall_hold_by(true),
            WallAction::Larger => self.zoom_wall(true),
            WallAction::Smaller => self.zoom_wall(false),
            WallAction::Sound => self.execute(Command::ToggleMute, event_loop),
            WallAction::Grid => {
                self.wall_bar_up = false;
                self.wall_to_grid();
            }
            WallAction::Close => {
                self.wall_bar_up = false;
                self.toggle_wall();
            }
        }
        self.redraw();
    }

    /// Draws the bar, when it is up, and keeps where its buttons are.
    pub(super) fn draw_wall_bar(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        self.wall_bar.clear();
        self.wall_bar_place = [0; 4];
        let Some(wall) = self.wall().filter(|_| self.wall_bar_up) else { return };
        let unit = px as i32;
        let pad = unit / 2;
        let line = Text::line_height(px);
        let side = line + pad;
        let height = side + unit;
        let small = (px * 5 / 6).max(11);

        let len = self.list.len();
        let tiles = self.wall_tiles(self.window_size());
        let (first, last) = (tiles.first().map_or(wall.start, |tile| tile.pos), tiles.last().map_or(wall.start, |tile| tile.pos));
        let at_start = wall.start == 0;
        let at_end = last + 1 >= len && !wall.auto;
        let columns = self.wall_columns(wall, self.window_size());
        let hold = wall.hold.as_secs();
        let (auto, muted) = (wall.auto, self.muted);
        let shown = if len == 0 { "0".to_string() } else if first == last { format!("{} / {len}", first + 1) } else { format!("{}–{} / {len}", first + 1, last + 1) };
        let every = format!("{hold} s");
        let across = format!("{columns} across");

        // The parts in a row: a button (its icon, or the symbol for the
        // pages turning), words, or the rule between groups.
        enum Part<'a> {
            Button(WallAction, &'a str, bool),
            Words(&'a str, u32),
            Rule,
        }
        let sound = if muted { commands::icons::VOLUME_X } else { commands::icons::VOLUME_2 };
        let parts = [
            Part::Button(WallAction::Back, commands::icons::SKIP_BACK, !at_start),
            Part::Button(WallAction::Auto, "", true),
            Part::Button(WallAction::Forward, commands::icons::SKIP_FORWARD, !at_end),
            Part::Words(&shown, small),
            Part::Rule,
            Part::Button(WallAction::Shorter, commands::icons::MINUS, hold > HOLDS[0]),
            Part::Words(&every, px),
            Part::Button(WallAction::Longer, commands::icons::PLUS, hold < HOLDS[HOLDS.len() - 1]),
            Part::Rule,
            Part::Button(WallAction::Smaller, commands::icons::ZOOM_OUT, columns < 16),
            Part::Words(&across, px),
            Part::Button(WallAction::Larger, commands::icons::ZOOM_IN, columns > 2),
            Part::Rule,
            Part::Button(WallAction::Sound, sound, true),
            Part::Button(WallAction::Grid, commands::icons::LAYOUT_GRID, true),
            Part::Button(WallAction::Close, commands::icons::X, true),
        ];
        let rule = unit;
        let width_of = |text: &mut Text, part: &Part| match part {
            Part::Button(..) => side,
            Part::Words(words, size) => text.width(words, *size) + pad * 2,
            Part::Rule => rule,
        };
        let inner: i32 = parts.iter().map(|part| width_of(text, part)).sum();
        let width = (inner + unit * 2).min(canvas.width - unit * 2);
        let x0 = (canvas.width - width) / 2;
        let y0 = canvas.height - height - unit;
        let radius = height / 2;
        canvas.soft_shadow(x0, y0, width, height, radius, unit);
        canvas.round_fill(x0, y0, width, height, radius, MENU_COLOR, 236);
        canvas.round_outline(x0, y0, width, height, radius, 1, MENU_EDGE_COLOR);
        self.wall_bar_place = [x0, y0, width, height];

        let mut x = x0 + unit;
        let top = y0 + (height - side) / 2;
        let middle = y0 + height / 2;
        for part in &parts {
            let w = width_of(text, part);
            match *part {
                Part::Button(action, icon, live) => {
                    let over = live && self.wall_bar_over == Some(action);
                    if over {
                        canvas.round_fill(x, top, side, side, side / 2, accent(), 256);
                    }
                    let color = match () {
                        _ if over => on_accent(),
                        _ if !live => FAINT_TEXT_COLOR,
                        _ if action == WallAction::Sound && muted => DIM_TEXT_COLOR,
                        _ if action == WallAction::Auto && auto => accent_soft(),
                        _ => TEXT_COLOR,
                    };
                    if action == WallAction::Auto {
                        // What a press does, as on the seek bar: pause the
                        // pages turning, or start them.
                        let size = side * 2 / 5;
                        let (sx, sy) = (x + (side - size) / 2, middle - size / 2);
                        match auto {
                            true => canvas.pause_symbol(sx, sy, size, color),
                            false => canvas.play_symbol(sx + size / 8, sy, size, color),
                        }
                    } else {
                        let icon_left = x + (side - text.width(icon, px)) / 2;
                        text.draw(canvas, icon_left, middle - line / 2, px, color, icon);
                    }
                    if live {
                        self.wall_bar.push(([x, top, side, side], action));
                    }
                }
                Part::Words(words, size) => {
                    let color = if size == px { TEXT_COLOR } else { DIM_TEXT_COLOR };
                    text.draw(canvas, x + pad, middle - Text::line_height(size) / 2, size, color, words);
                }
                Part::Rule => canvas.fill(x + rule / 2, top + pad / 2, 1, side - pad, MENU_EDGE_COLOR),
            }
            x += w;
        }
    }
}
