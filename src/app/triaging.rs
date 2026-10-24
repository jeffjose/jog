//! Triage: keeping and rejecting files one after another, and running a command on a lot of them.

use super::*;

impl App {

    /// What a triage has decided about a file, as far as is remembered.
    pub(super) fn verdict(&self, id: usize) -> Option<Verdict> {
        *self.verdicts.borrow_mut().entry(id).or_insert_with(|| self.triage.borrow_mut().get_or_insert_with(Triage::load).of(&self.files[id]))
    }

    /// Decides about the file on view and goes on to the next. Deciding
    /// the same again takes the decision back, and stays. Nothing is done
    /// to the file either way.
    pub(super) fn decide(&mut self, verdict: Verdict) {
        // The one a menu was brought up on, or the grid's cursor is on,
        // where it is not the file on view.
        let id = self.list[self.focus()];
        let on_view = self.grid.is_none() && id == self.id();
        let to = Some(verdict).filter(|_| self.verdict(id) != Some(verdict));
        self.triage.borrow_mut().get_or_insert_with(Triage::load).decide(&self.files[id], to);
        self.verdicts.borrow_mut().insert(id, to);
        if on_view && self.triage_on {
            if to.is_some() && self.pos + 1 < self.list.len() {
                self.step(true);
            }
        } else {
            // With no tally in sight to show it, it is said.
            let name = self.name_of(id);
            self.toast(match to {
                Some(Verdict::Kept) => format!("{}  Kept {name}", commands::icons::CHECK_FILLED),
                Some(Verdict::Rejected) => format!("{}  Rejected {name}", commands::icons::X_FILLED),
                None => format!("Undecided  ·  {name}"),
            });
        }
        self.redraw();
    }

    /// How many of all the files are kept, rejected and still undecided.
    pub(super) fn tally(&self) -> (usize, usize, usize) {
        let all = self.everything();
        let count = |wanted: Option<Verdict>| all.iter().filter(|id| self.verdict(**id) == wanted).count();
        (count(Some(Verdict::Kept)), count(Some(Verdict::Rejected)), count(None))
    }

    /// The files a triage has put in one of its three lots, in list order.
    pub(super) fn lot(&self, wanted: Option<Verdict>) -> Vec<usize> {
        self.everything().into_iter().filter(|id| self.verdict(*id) == wanted).collect()
    }

    /// Which button of a triage is at a place in the window.
    pub(super) fn triage_button_at(&self, x: f64, y: f64) -> Option<usize> {
        let on = |[left, top, width, height]: [i32; 4]| (left..left + width).contains(&(x as i32)) && (top..top + height).contains(&(y as i32));
        self.triage_buttons.borrow().iter().position(|(_, place)| on(*place))
    }

    /// Looks for files that have come or gone, and says how many did.
    pub(super) fn triage_refresh(&mut self, event_loop: &ActiveEventLoop) {
        let before = self.everything().len();
        self.execute(Command::Reload, event_loop);
        let now = self.everything().len();
        self.toast(match now.cmp(&before) {
            Ordering::Greater => format!("{}  {} more files", commands::icons::REFRESH, now - before),
            Ordering::Less => format!("{}  {} fewer files", commands::icons::REFRESH, before - now),
            Ordering::Equal => format!("{}  No files have come or gone", commands::icons::REFRESH),
        });
    }

    /// What a triage has at the top of the picture, in a box as the find
    /// box and the menus have. At its ends a button to reject and one to
    /// keep, each with its key, and edged in its color when it is what
    /// was decided about the file on view. Between them how many are
    /// rejected, beside the one, and kept, beside the other, with a bar
    /// that fills from each end and how many are still to go written over
    /// it. Last, two round ones: to look for files that have come or gone,
    /// and to finish.
    pub(super) fn draw_triage(&self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        self.triage_buttons.borrow_mut().clear();
        if !self.triage_on || self.grid.is_some() || self.overview.is_some() {
            return;
        }
        let (kept, rejected, undecided) = self.tally();
        let total = (kept + rejected + undecided).max(1);
        let decided = self.verdict(self.id());
        let small = (px * 5 / 6).max(11);
        let (pad, line) = (px as i32 * 2 / 3, Text::line_height(px));
        // How tall a button is, and how far the box stands off them.
        let (side, inset) = (line + pad, pad * 2 / 3);

        let choices = [
            (commands::icons::X_FILLED, "Reject", "x", DANGER_COLOR, Verdict::Rejected, Command::Reject),
            (commands::icons::CHECK_FILLED, "Keep", "Space", KEPT_COLOR, Verdict::Kept, Command::Keep),
        ];
        let widths: [i32; 2] = std::array::from_fn(|which| {
            let (icon, word, key, ..) = choices[which];
            pad + text.width(icon, px) + pad / 2 + text.width(word, px) + pad * 2 / 3 + text.width(key, small) + pad
        });
        let counts = [rejected.to_string(), kept.to_string()];
        let count_widths = [text.width(&counts[0], px), text.width(&counts[1], px)];
        let left_words = format!("{undecided} to go");
        let track = (px as i32 * 9).max(text.width(&left_words, small) + pad);
        let middle = pad + count_widths[0] + pad + track + pad + count_widths[1] + pad;
        let w = inset + widths[0] + middle + widths[1] + pad + side + pad / 2 + side + inset;
        let h = side + 2 * inset;
        let (left, stage) = self.stage();
        let (x, y) = (left + (stage.0 as i32 - w) / 2, px as i32);
        panel(canvas, x, y, w, h, pad);

        let over = self.triage_over;
        let mut buttons = Vec::new();
        let top = y + inset;
        let text_top = top + (side - line) / 2;
        let mut choice = |canvas: &mut Canvas, text: &mut Text, at: i32, which: usize| {
            let (icon, word, key, color, verdict, command) = choices[which];
            let (chosen, hovered) = (decided == Some(verdict), over == Some(buttons.len()));
            let width = widths[which];
            // As the buttons of the find box are: filled in under the
            // pointer, and edged in its own color once it is chosen.
            let round = pad / 2;
            canvas.round_fill(at, top, width, side, round, if hovered { accent() } else { PLACEHOLDER_COLOR }, 256);
            let (edge, edge_color) = if chosen { (2, color) } else if hovered { (1, accent()) } else { (1, MENU_EDGE_COLOR) };
            canvas.round_outline(at, top, width, side, round, edge, edge_color);
            let (main, dim) = row_text(hovered);
            let mut across = at + pad;
            text.draw(canvas, across, text_top, px, if hovered { main } else { color }, icon);
            across += text.width(icon, px) + pad / 2;
            text.draw(canvas, across, text_top, px, main, word);
            across += text.width(word, px) + pad * 2 / 3;
            text.draw(canvas, across, top + (side - Text::line_height(small)) / 2, small, dim, key);
            buttons.push((command, [at, top, width, side]));
        };
        let mut at = x + inset;
        choice(canvas, text, at, 0);
        at += widths[0];

        // Between them: the count of the rejected, the bar with what is
        // left over it, the count of the kept.
        let mut across = at + pad;
        text.draw(canvas, across, text_top, px, DANGER_COLOR, &counts[0]);
        across += count_widths[0] + pad;
        let thick = (px as i32 / 5).max(3);
        let small_line = Text::line_height(small);
        let words_top = top + (side - small_line - thick - pad / 4) / 2;
        let words_left = across + (track - text.width(&left_words, small)) / 2;
        text.draw(canvas, words_left, words_top, small, DIM_TEXT_COLOR, &left_words);
        let bar_top = words_top + small_line + pad / 4;
        canvas.round_fill(across, bar_top, track, thick, thick / 2, TEXT_COLOR, 40);
        let share = |count: usize| (i64::from(track) * count as i64 / total as i64) as i32;
        if rejected > 0 {
            canvas.round_fill(across, bar_top, share(rejected).max(thick), thick, thick / 2, DANGER_COLOR, 256);
        }
        if kept > 0 {
            let kept_w = share(kept).max(thick);
            canvas.round_fill(across + track - kept_w, bar_top, kept_w, thick, thick / 2, KEPT_COLOR, 256);
        }
        across += track + pad;
        text.draw(canvas, across, text_top, px, KEPT_COLOR, &counts[1]);
        at += middle;
        choice(canvas, text, at, 1);
        at += widths[1] + pad;

        // Last, two round ones, as the file list has: look for files
        // again, and finish.
        for (icon, command) in [(commands::icons::REFRESH, Command::Reload), (commands::icons::X, Command::Triage)] {
            let hovered = over == Some(buttons.len());
            canvas.round_fill(at, top, side, side, side / 2, if hovered { accent() } else { PLACEHOLDER_COLOR }, 256);
            let icon_left = at + (side - text.width(icon, small)) / 2;
            text.draw(canvas, icon_left, top + (side - small_line) / 2, small, if hovered { on_accent() } else { TEXT_COLOR }, icon);
            buttons.push((command, [at, top, side, side]));
            at += side + pad / 2;
        }
        *self.triage_buttons.borrow_mut() = buttons;
    }

    /// The graphs of the picture's colors, on a panel under the debug
    /// overlay: a row of buttons to choose them and how they count, then
    /// each one chosen under its name, then how much of the picture is
    /// at pure black and at pure white. Gives back where the buttons went.
    pub(super) fn draw_scopes(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32, at: (i32, i32), pointer: (i32, i32)) -> (Vec<Button>, i32) {
        let small = (px * 5 / 6).max(11);
        let (pad, row) = (px as i32, Text::line_height(small) + px as i32 / 6);
        let button_height = row + pad / 3;
        let mut choices: Vec<(&str, Command, bool)> = scopes::KINDS.iter().enumerate().map(|(kind, (name, _))| (*name, Command::Scope(kind), self.scopes[kind])).collect();
        choices.push(("Log", Command::ScopeLog, self.scope_log));
        let widths: Vec<i32> = choices.iter().map(|(label, ..)| text.width(label, small) + pad).collect();
        let buttons_width = widths.iter().sum::<i32>() + pad / 3 * (widths.len() as i32 - 1);
        let graph_width = (px as i32 * 22).max(buttons_width);
        // How tall each graph is: the vectorscope wants a square.
        let tall = |kind: usize| if kind == scopes::VECTOR { graph_width * 3 / 5 } else { graph_width * 2 / 5 };
        let (x, y) = at;
        // Each graph chosen, under its name: where its name and it go.
        let mut graphs = Vec::new();
        let mut top = y + pad + button_height + pad / 2;
        for kind in (0..scopes::KINDS.len()).filter(|kind| self.scopes[*kind]) {
            graphs.push((kind, top, top + row));
            top += row + tall(kind) + pad / 2;
        }
        let (width, height) = (graph_width + 2 * pad, top + row + pad - y);

        // Solid, unlike the panel above, and not filled where a graph will
        // cover it: blending or filling so large a panel each frame would
        // cost more than the graphs do. Only its two ends are rounded.
        let (round, ground) = (pad / 2, 0x0D0D0D);
        canvas.round_fill(x, y, width, 2 * round, round, ground, 256);
        let mut filled = y + round;
        for (kind, _, graph_top) in &graphs {
            canvas.fill(x, filled, width, graph_top - filled, ground);
            canvas.fill(x, *graph_top, pad, tall(*kind), ground);
            canvas.fill(x + pad + graph_width, *graph_top, width - pad - graph_width, tall(*kind), ground);
            filled = graph_top + tall(*kind);
        }
        canvas.fill(x, filled, width, y + height - round - filled, ground);
        canvas.round_fill(x, y + height - 2 * round, width, 2 * round, round, ground, 256);
        canvas.round_outline(x, y, width, height, round, 1, MENU_EDGE_COLOR);

        // The buttons, as those that copy the figures above: filled in
        // while their graph is up, and under the pointer.
        let mut drawn = Vec::new();
        let (mut left, buttons_top) = (x + pad, y + pad);
        for ((label, command, on), button_width) in choices.into_iter().zip(widths) {
            let over = (left..left + button_width).contains(&pointer.0) && (buttons_top..buttons_top + button_height).contains(&pointer.1);
            let lit = on || over;
            canvas.round_fill(left, buttons_top, button_width, button_height, pad / 3, if lit { accent() } else { PLACEHOLDER_COLOR }, 256);
            canvas.round_outline(left, buttons_top, button_width, button_height, pad / 3, 1, if lit { accent() } else { KEY_EDGE_COLOR });
            let words_top = buttons_top + (button_height - Text::line_height(small)) / 2;
            text.draw(canvas, left + pad / 2, words_top, small, if lit { on_accent() } else { TEXT_COLOR }, label);
            drawn.push(Button { x: left, y: buttons_top, width: button_width, height: button_height, command });
            left += button_width + pad / 3;
        }

        for (kind, name_top, graph_top) in graphs {
            text.draw(canvas, x + pad, name_top, small, DIM_TEXT_COLOR, scopes::KINDS[kind].1);
            let stride = canvas.width;
            let mut place = scopes::Place { pixels: canvas.all(), stride, x: x + pad, y: graph_top, w: graph_width, h: tall(kind) };
            match kind {
                scopes::OVERLAID => scopes::histogram(&mut place, &self.scope_counts, self.scope_log, false),
                scopes::ROWS => scopes::histogram(&mut place, &self.scope_counts, self.scope_log, true),
                scopes::WAVEFORM => scopes::waveform(&mut place, &self.scope_counts),
                scopes::PARADE => scopes::parade(&mut place, &self.scope_counts),
                _ => scopes::vector(&mut place, &self.scope_counts),
            }
        }
        // How much has lost its detail at either end: a warning color
        // past a fiftieth of the picture.
        let (black, white) = self.scope_counts.clipped();
        let mut words_left = x + pad;
        for (words, warn) in [("clipped   black ".to_string(), false), (format!("{black:.1}%"), black > 2.0), ("   white ".to_string(), false), (format!("{white:.1}%"), white > 2.0)] {
            text.draw(canvas, words_left, top, small, if warn { MARK_COLOR } else if words.ends_with('%') { TEXT_COLOR } else { DIM_TEXT_COLOR }, &words);
            words_left += text.width(&words, small);
        }
        (drawn, y + height + pad / 2)
    }

    /// A video's HDR, on a panel under the other debug panels: what it is
    /// and how its metadata is carried, the figures of the frame on view,
    /// and its curves. The 2094-50 ones as hdr-explorer draws them: input
    /// and output as multiples of the reference white, the stored curves
    /// in gray, and in green the curve for the headroom chosen with the
    /// buttons, a dashed line at its peak. Beside them the HDR10+ curve,
    /// as fractions of the scene's brightest and of the screen's peak.
    pub(super) fn draw_hdr(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32, at: (i32, i32), pointer: (i32, i32)) -> Vec<Button> {
        let Some(info) = self.hdr_info() else { return Vec::new() };
        let badges = self.hdr_badges();
        if badges.is_empty() && !info.is_hdr() {
            return Vec::new();
        }
        let seconds = self.video_time().map_or(0.0, |(position, _)| position);
        let (agtm, plus) = (info.agtm_at(seconds), info.hdr10plus_at(seconds));
        let small = (px * 5 / 6).max(11);
        let (pad, row) = (px as i32, Text::line_height(small) + px as i32 / 6);
        let side = px as i32 * 13;

        // The words: what it is, how it is carried, the figures.
        let mut lines: Vec<(String, u32)> = Vec::new();
        for (what, how) in &info.carried {
            lines.push((format!("{what} in {how}"), DIM_TEXT_COLOR));
        }
        if let Some((peak, floor)) = info.mastering {
            lines.push((format!("Mastering display {peak:.0} nits, black {floor:.4}"), DIM_TEXT_COLOR));
        }
        if let Some((cll, fall)) = info.light {
            lines.push((format!("MaxCLL {cll} nits, MaxFALL {fall} nits"), DIM_TEXT_COLOR));
        }
        if let Some(agtm) = agtm {
            let heads: Vec<String> = agtm.altr.iter().map(|altr| format!("{:.2}", altr.headroom)).collect();
            lines.push((format!("2094-50  graded for {:.2} stops, white {:.0} nits, curves at {} stops", agtm.baseline, agtm.reference_white, heads.join(", ")), TEXT_COLOR));
        }
        if let Some(plus) = plus {
            let [r, g, b] = plus.maxscl;
            lines.push((format!("HDR10+  for a {:.0}-nit screen, brightest {r:.0} / {g:.0} / {b:.0} nits, mean {:.0}", plus.target, plus.average), TEXT_COLOR));
        }
        let graphs = usize::from(agtm.is_some()) + usize::from(plus.is_some());
        let words = lines.iter().map(|(line, _)| text.width(line, small)).max().unwrap_or(0);
        let width = words.max(graphs as i32 * (side + pad) - pad).max(px as i32 * 16) + 2 * pad;
        let graph_rows = if graphs > 0 { row + side + row + pad / 2 } else { 0 };
        let controls = if agtm.is_some() { row + pad / 3 + pad / 2 } else { 0 };
        let height = pad + row + lines.len() as i32 * row + pad / 2 + controls + graph_rows + pad / 2;
        let (x, y) = at;
        canvas.round_fill(x, y, width, height, pad / 2, 0x000000, 190);
        canvas.round_outline(x, y, width, height, pad / 2, 1, MENU_EDGE_COLOR);

        // A heading with the badges.
        let mut left = x + pad;
        text.draw(canvas, left, y + pad, small, accent_soft(), "HDR");
        left += text.width("HDR", small);
        for name in &badges {
            let (run, color) = badge(name);
            left += draw_run(canvas, text, left, y + pad, small, color, &run).0;
        }
        let mut top = y + pad + row;
        for (line, color) in &lines {
            text.draw(canvas, x + pad, top, small, *color, line);
            top += row;
        }
        top += pad / 2;

        // The headroom the 2094-50 curve is for, with buttons to move it.
        let mut drawn = Vec::new();
        if agtm.is_some() {
            let button_height = row + pad / 3;
            let mut left = x + pad;
            for (label, up) in [("−", false), ("+", true)] {
                let button_width = text.width(label, small) + pad;
                let over = (left..left + button_width).contains(&pointer.0) && (top..top + button_height).contains(&pointer.1);
                canvas.round_fill(left, top, button_width, button_height, pad / 3, if over { accent() } else { PLACEHOLDER_COLOR }, 256);
                canvas.round_outline(left, top, button_width, button_height, pad / 3, 1, if over { accent() } else { KEY_EDGE_COLOR });
                text.draw(canvas, left + pad / 2, top + (button_height - Text::line_height(small)) / 2, small, if over { on_accent() } else { TEXT_COLOR }, label);
                drawn.push(Button { x: left, y: top, width: button_width, height: button_height, command: Command::HdrHeadroom(up) });
                left += button_width + pad / 3;
            }
            let what = if self.hdr_headroom == 0.0 { " (SDR, as this screen)".to_string() } else { format!(" ({:.0} times SDR white)", self.hdr_headroom.exp2()) };
            text.draw(canvas, left + pad / 2, top + (button_height - Text::line_height(small)) / 2, small, TEXT_COLOR, &format!("Headroom {:.2} stops{what}", self.hdr_headroom));
            top += button_height + pad / 2;
        }

        let mut left = x + pad;
        let place = |left: i32| [left, top + row, side, side];
        if let Some(agtm) = agtm {
            let headroom = self.hdr_headroom;
            let most = agtm.altr.iter().map(|altr| altr.headroom).fold(agtm.baseline, f64::max).max(headroom);
            let range = most.exp2().ceil().max(2.0);
            text.draw(canvas, left, top, small, DIM_TEXT_COLOR, &format!("2094-50, 0 to {range:.0} times white"));
            let mut lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(|x| x), 0x6A6A6A, true)];
            for altr in &agtm.altr {
                lines.push((Box::new(move |x| hdrmeta::Agtm::output(altr, x)), 0x9A9A9A, false));
            }
            lines.push((Box::new(move |_| headroom.exp2()), 0x3DD68C, true));
            lines.push((Box::new(move |x| agtm.output_at(headroom, x)), 0x3DD68C, false));
            plot(canvas, place(left), (range, range), 1.0, &lines);
            left += side + pad;
        }
        if let Some(plus) = plus {
            text.draw(canvas, left, top, small, DIM_TEXT_COLOR, &format!("HDR10+, for {:.0} nits", plus.target));
            let lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(|x| x), 0x6A6A6A, true), (Box::new(move |x| plus.tone(x)), 0xB48CF0, false)];
            plot(canvas, place(left), (1.0, 1.0), 0.25, &lines);
        }
        drawn
    }

    /// Writes the files of this session out in the three lots of a triage,
    /// if any of them has been decided about, and says where.
    pub(super) fn write_triage(&self, session: &str) {
        let files: Vec<(PathBuf, Option<Verdict>)> = self.everything().into_iter().map(|id| (self.files[id].clone(), self.verdict(id))).collect();
        if files.iter().all(|(_, verdict)| verdict.is_none()) {
            return;
        }
        if let Some(list) = triage::write_out(session, &files) {
            println!("[triage] {}", list.display());
        }
    }

}
