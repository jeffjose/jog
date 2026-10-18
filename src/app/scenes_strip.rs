//! A video's scenes along the bottom: found, shown, gone between.

use super::*;

impl App {

    /// Whether the strip of the video's scenes is up, in place of the
    /// filmstrip of files.
    pub(super) fn scenes_shown(&self) -> bool {
        self.scenes_on && self.grid.is_none() && self.overview.is_none() && self.video.is_some()
    }

    /// Whether anything stands along the bottom of the window.
    pub(super) fn band_shown(&self) -> bool {
        self.strip_visible() || self.scenes_shown()
    }

    /// Asks for a frame from each of the equal parts of the video on view,
    /// once it is showing and unless they have been asked for before.
    /// What the video on view says about its HDR, read on another thread
    /// the first time it is asked for: `None` until then, and for a file
    /// that says nothing.
    pub(super) fn hdr_info(&mut self) -> Option<Arc<hdrmeta::Info>> {
        let id = self.video.as_ref().map(|video| video.id)?;
        if let Some(known) = self.hdr_infos.get(&id) {
            return known.clone();
        }
        self.hdr_infos.insert(id, None);
        let (path, proxy) = (self.files[id].clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let info = hdrmeta::read(&path).map(Arc::new);
            let _ = proxy.send_event(Event::HdrInfo(id, info));
        });
        None
    }

    /// Badges for what a video is: from the file, and from the player for
    /// what the file does not say, as an HEVC one keeps its color inside
    /// the stream.
    pub(super) fn hdr_badges(&mut self) -> Vec<String> {
        let info = self.hdr_info();
        let mut badges = info.as_ref().map(|info| info.badges()).unwrap_or_default();
        let known = info.as_ref().is_some_and(|info| info.transfer.is_some());
        if !known {
            let gamma = self.player.as_ref().and_then(|player| player.text("video-params/gamma"));
            let named = match gamma.as_deref() {
                Some("pq") => Some("PQ"),
                Some("hlg") => Some("HLG"),
                _ => None,
            };
            let plus = info.as_ref().is_some_and(|info| !info.hdr10plus.is_empty());
            match named {
                Some("PQ") if plus => badges.insert(0, "HDR10+".to_string()),
                Some(name) => badges.insert(0, name.to_string()),
                None => {}
            }
            let bt2020 = self.player.as_ref().and_then(|player| player.text("video-params/primaries")).as_deref() == Some("bt.2020");
            if bt2020 && !badges.iter().any(|badge| badge == "BT.2020") {
                badges.push("BT.2020".to_string());
            }
        }
        badges
    }

    pub(super) fn want_scenes(&mut self) {
        let Some(id) = self.video.as_ref().filter(|video| video.showing).map(|video| video.id) else { return };
        if self.scenes.contains_key(&id) {
            return;
        }
        let Some((_, length)) = self.video_time() else { return };
        let starts = scenes::starts(length);
        self.scenes.insert(id, SceneList { frames: vec![None; starts.len()], starts: Some(starts.clone()) });
        let proxy = self.proxy.clone();
        let finder = self.scene_finder.get_or_insert_with(|| scenes::Finder::new(move |found| proxy.send_event(Event::Scenes(found)).is_ok()));
        finder.want(id, self.files[id].clone(), starts);
    }

    /// The scenes of the video on view, once they are known, and which of
    /// them is playing.
    pub(super) fn scenes_now(&self) -> Option<(&SceneList, &[f64], usize)> {
        let list = self.scenes.get(&self.video.as_ref()?.id)?;
        let starts = list.starts.as_deref()?;
        let (position, _) = self.video_time()?;
        // A hair early, so that the scene just jumped to is the one playing.
        let playing = starts.iter().rposition(|start| *start <= position + 0.05).unwrap_or(0);
        Some((list, starts, playing))
    }

    /// Goes to the scene after the one playing, or back to the start of
    /// this one and from there to the one before. Past the last scene is
    /// the next file. Returns whether there were scenes to go by.
    pub(super) fn scene_jump(&mut self, forward: bool) -> bool {
        let Some((_, starts, playing)) = self.scenes_now() else { return false };
        let Some((position, length)) = self.video_time() else { return false };
        let to = match forward {
            true => starts.get(playing + 1).copied(),
            // A moment into a scene, back is to where it began.
            false if position - starts[playing] > SCENE_RESTART || playing == 0 => Some(starts[playing]),
            false => Some(starts[playing - 1]),
        };
        match to {
            Some(start) => {
                self.seek_to(start / length, true);
                self.time_until = Some(Instant::now() + TOAST);
            }
            None => self.step(true),
        }
        true
    }

    /// Where the cell of a scene is along the strip: they stand in a row
    /// in the middle of the window, and stay where they are.
    pub(super) fn scene_left(&self, strip: &Strip, index: usize, window_width: i32) -> i32 {
        let row = scenes::PARTS as i32 * strip.pitch() - strip.pad;
        (window_width - row) / 2 + index as i32 * strip.pitch()
    }

    /// The scene whose cell a window position is on.
    pub(super) fn scene_at(&self, x: f64, y: f64) -> Option<usize> {
        if self.strip_tab != StripTab::Scenes {
            return None;
        }
        let (width, height) = self.window_size();
        let strip = Strip::for_height(height);
        let (_, starts, _) = self.scenes_now().filter(|_| self.scenes_shown() && y >= f64::from(strip.top))?;
        (0..starts.len()).find(|&index| {
            let left = self.scene_left(&strip, index, width as i32);
            (left..left + strip.cell_w).contains(&(x as i32))
        })
    }

    pub(super) fn draw_scenes(&self, canvas: &mut Canvas, strip: Strip) {
        canvas.shade(0, strip.top, canvas.width, strip.height, 64);
        let Some((list, starts, playing)) = self.scenes_now() else { return };
        let top = strip.top + strip.pad;
        let (round, edge) = ((strip.pad * 2 / 3).max(3), (strip.pad / 4).max(2));
        let over = self.scene_at(self.mouse.0, self.mouse.1);
        for index in 0..starts.len() {
            let left = self.scene_left(&strip, index, canvas.width);
            if left + strip.cell_w < 0 || left > canvas.width {
                continue;
            }
            // Under the pointer the scene plays, once the player has it.
            let playing_here = self.trailer.as_ref().filter(|trailer| trailer.id == usize::MAX - index);
            let live = playing_here.and_then(|trailer| trailer.frame.as_ref()).filter(|frame| !frame.pixels.is_empty());
            let still = list.frames.get(index).and_then(Option::as_ref).map(|frame| &**frame);
            let [x, y, w, h] = match live.or(still) {
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
            // The scene playing is ringed brightly, the one under the
            // pointer quietly.
            let ring = if index == playing { Some(TEXT_COLOR) } else { (over == Some(index)).then_some(accent_soft()) };
            if let Some(color) = ring {
                canvas.round_outline(x - edge, y - edge, w + 2 * edge, h + 2 * edge, round + edge, edge, color);
            }
        }
    }

    /// The words on the strip of scenes: in the corner of each when it
    /// starts.
    /// Until the video is showing, that they are on their way.
    pub(super) fn draw_scene_labels(&self, canvas: &mut Canvas, text: &mut Text, strip: &Strip, px: u32) {
        let small = (px * 3 / 4).max(10);
        let (top, pad, line) = (strip.top + strip.pad, (small as i32 / 4).max(2), Text::line_height(small));
        let Some((list, starts, _)) = self.scenes_now() else {
            let words = "Scenes…";
            let left = (canvas.width - text.width(words, px)) / 2;
            return text.draw(canvas, left, strip.top + (strip.height - Text::line_height(px)) / 2, px, DIM_TEXT_COLOR, words);
        };
        for (index, start) in starts.iter().enumerate() {
            let left = self.scene_left(strip, index, canvas.width);
            if left + strip.cell_w < 0 || left > canvas.width {
                continue;
            }
            let at = clock(*start);
            // In the bottom right corner of the picture, which may be
            // narrower than its cell, as a video's length is in the filmstrip.
            let size = list.frames.get(index).and_then(Option::as_ref).map_or((strip.cell_w, strip.cell_h), |frame| {
                let (w, h) = fit_size((frame.width, frame.height), (strip.cell_w as u32, strip.cell_h as u32), false);
                (w as i32, h as i32)
            });
            let (right, bottom) = (left + (strip.cell_w + size.0) / 2, top + (strip.cell_h + size.1) / 2);
            let badge_w = text.width(&at, small) + 2 * pad;
            let (badge_x, badge_y) = (right - badge_w - pad, bottom - line - pad);
            canvas.round_fill(badge_x, badge_y, badge_w, line, pad, 0x000000, 190);
            text.draw(canvas, badge_x + pad, badge_y, small, TEXT_COLOR, &at);
        }
    }

}
