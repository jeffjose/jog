//! The seek bar's own controls of how a video plays. The volume, as a
//! slider over its button: up while the pointer is on the button or the
//! slider, or the slider is held; a click or a drag along it sets the
//! volume, the wheel over it moves it, and a click on the button itself
//! mutes, as it always has. And the cog's menu, of speeds and tracks,
//! standing above the cog.

use super::*;

/// The loudest the slider goes, as mpv counts: what the file has, no more.
const LOUDEST: f64 = 100.0;

impl App {
    /// Where the slider would stand on `bar`: over the volume button. And
    /// whether the pointer is on the button, the slider or between them.
    fn volume_place(&self, bar: &SeekBar) -> Option<([i32; 4], bool)> {
        let which = self.bar_buttons().iter().position(|command| *command == Command::ToggleMute)? as i32;
        let (left, top, side) = bar.button(which);
        let unit = bar.unit;
        let (w, h) = (side + unit, unit * 9);
        let place = [left + side / 2 - w / 2, top - unit / 2 - h, w, h];
        let (x, y) = (self.mouse.0 as i32, self.mouse.1 as i32);
        let on_button = (left - unit / 2..left + side + unit / 2).contains(&x) && (top - unit / 2..top + side + unit / 2).contains(&y);
        let on_slider = (place[0]..place[0] + w).contains(&x) && (place[1]..top).contains(&y);
        Some((place, on_button || on_slider || matches!(self.drag, Some(Drag::Volume))))
    }

    /// The slider, while it is up.
    pub(super) fn volume_slider(&self, bar: &SeekBar) -> Option<[i32; 4]> {
        self.hover?;
        self.volume_place(bar).filter(|(_, up)| *up).map(|(place, _)| place)
    }

    /// The top of the slider, while the pointer is on it or its button:
    /// the bar stays up that far above it.
    pub(super) fn volume_reach(&self, bar: &SeekBar) -> Option<i32> {
        self.volume_place(bar).filter(|(_, up)| *up).map(|(place, _)| place[1])
    }

    /// The track the handle runs along: its middle across, its top and
    /// its length down.
    fn volume_track(&self, [x, y, w, h]: [i32; 4], unit: i32) -> (i32, i32, i32) {
        (x + w / 2, y + unit * 3, h - unit * 4)
    }

    /// Whether a point is on the slider, to take hold of it.
    pub(super) fn on_volume(&self, x: f64, y: f64) -> bool {
        let bar = self.seek_bar();
        self.volume_slider(&bar).is_some_and(|[left, top, w, h]| (left..left + w).contains(&(x as i32)) && (top..top + h).contains(&(y as i32)))
    }

    /// The volume set from where along the slider the pointer is; turned
    /// up from silence, the sound comes back on.
    pub(super) fn volume_to(&mut self, y: f64) {
        let bar = self.seek_bar();
        let Some(place) = self.volume_place(&bar).map(|(place, _)| place) else { return };
        let (_, track_top, track_h) = self.volume_track(place, bar.unit);
        let share = (1.0 - (y - f64::from(track_top)) / f64::from(track_h.max(1))).clamp(0.0, 1.0);
        let Some(player) = &self.player else { return };
        player.set("volume", &format!("{:.0}", share * LOUDEST));
        if self.muted && share > 0.0 {
            self.muted = false;
            for player in self.players() {
                player.set("mute", "no");
            }
        }
        self.redraw();
    }

    /// The slider: on a panel, the track filled up to the volume, its
    /// handle, and the volume in numbers at the top, for the text pass.
    pub(super) fn draw_volume(&self, canvas: &mut Canvas, bar: &SeekBar, labels: &mut Vec<(i32, i32, Ordering, String)>) {
        let Some(place) = self.volume_slider(bar) else { return };
        let [x, y, w, h] = place;
        draw::panel(canvas, x, y, w, h, w / 2);
        let unit = bar.unit;
        let (middle, track_top, track_h) = self.volume_track(place, unit);
        let volume = self.player.as_ref().and_then(|player| player.number("volume")).unwrap_or(LOUDEST);
        let share = if self.muted { 0.0 } else { (volume / LOUDEST).clamp(0.0, 1.0) };
        let thickness = (unit / 3).max(3);
        canvas.round_fill(middle - thickness / 2, track_top, thickness, track_h, thickness / 2, TEXT_COLOR, 72);
        let filled = (f64::from(track_h) * share) as i32;
        canvas.round_fill(middle - thickness / 2, track_top + track_h - filled, thickness, filled.max(thickness), thickness / 2, TEXT_COLOR, 256);
        let handle = unit * 5 / 4;
        canvas.round_fill(middle - handle / 2, track_top + track_h - filled - handle / 2, handle, handle, handle / 2, TEXT_COLOR, 256);
        let said = if self.muted { commands::icons::VOLUME_X.to_string() } else { format!("{volume:.0}") };
        labels.push((middle, y + unit / 2, Ordering::Equal, said));
    }
}

impl App {
    /// The playback menu, from the cog at the right of the bar.
    pub(super) fn open_playback_menu(&mut self) {
        self.open_bar_menu(Command::PlaybackSettings, commands::PLAYBACK_MENU, Some("Speed".to_string()));
    }

    /// A menu from one of the bar's buttons: opened above it, its right
    /// edge in line with the button's, and running upward, so it is
    /// clear of the button and of the seek bar.
    pub(super) fn open_bar_menu(&mut self, button: Command, items: &'static [Option<MenuItem>], title: Option<String>) {
        let Some((right, bottom)) = self.bar_anchor(button) else {
            return self.open_menu(items, title);
        };
        let mut menu = self.build_menu(items, title, (0, 0));
        let (width, _) = self.window_size();
        menu.x = (right - menu.width).clamp(0, (width as i32 - menu.width).max(0));
        menu.y = (bottom - menu.height).max(0);
        menu.upward = true;
        self.menu = Some(menu);
    }

    /// Where a popover from one of the bar's buttons goes: the right edge
    /// of the button, and a little above it. None with the button not on
    /// the bar.
    pub(super) fn bar_anchor(&self, button: Command) -> Option<(i32, i32)> {
        let bar = self.seek_bar();
        let which = self.bar_buttons().iter().position(|command| *command == button)?;
        let (left, top, side) = bar.button(which as i32);
        Some((left + side, top - bar.unit / 2))
    }
}
