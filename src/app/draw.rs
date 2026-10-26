//! Drawing: the picture or video, and everything over it.

use super::*;

impl App {

    pub(super) fn draw(&mut self) {
        // While the first image is the window's background and the
        // window manager has not yet put the window up, a frame handed
        // over would be thrown away, and the handing over only delays
        // the server in putting the window up.
        if let Some((_, since)) = &self.backdrop {
            if !self.focused_once && since.elapsed() < BACKDROP_AT_MOST {
                return;
            }
        }
        let (Some(window), Some(mut surface)) = (self.window.clone(), self.surface.take()) else { return };
        let size = PhysicalSize::from(self.window_size());
        // A frame nobody here asked for is the system's: the window has
        // been uncovered or mapped, and wants all of itself drawn.
        let ours = self.asked.take();
        if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            let entered = Instant::now();
            if surface.resize(width, height).is_ok() {
                // This waits for the display to have taken the frame before.
                if let Ok(mut buffer) = surface.buffer_mut() {
                    let started = Instant::now();
                    let waited = started.duration_since(entered);
                    // Over a video, what was drawn last time is known, and
                    // only that need be gone over again, in this buffer
                    // when it is the one shown last.
                    self.video_layer.reused = Some(buffer.age() == 1);
                    self.paint(&mut buffer, (size.width, size.height));
                    // Not over a video, or the loupe over it too: nothing known.
                    if self.video_layer.reused.take().is_some() || self.loupe.is_some() {
                        self.video_layer.forget_drawn();
                    }
                    self.draw_loupe(&mut buffer, (size.width, size.height));
                    let only = self.video_layer.changed.take();
                    // Whether the window still shows what was last handed
                    // it, and so only what differs need be told of; not
                    // when the whole picture was just painted afresh.
                    let kept = ours && buffer.age() == 1 && self.presented.len() == buffer.len() && !self.repainted.take();
                    // Something is still on its way somewhere: draw again soon.
                    let moving = self.animating.take();
                    if moving {
                        // No oftener than the display shows a frame.
                        let every = *self.frame_every.get_or_insert_with(|| {
                            let rate = window.current_monitor().and_then(|monitor| monitor.refresh_rate_millihertz());
                            Duration::from_secs_f64(1000.0 / f64::from(rate.filter(|rate| *rate > 0).unwrap_or(60_000)))
                        });
                        self.anim_due = Some((started + every).max(Instant::now() + Duration::from_millis(1)));
                    }
                    let painted = started.elapsed();
                    // The display is handed only what has changed: taking
                    // the whole of a large window is what takes it longest,
                    // several times as long as drawing the frame did.
                    let parts = match kept {
                        true => Some(ui::changed(&buffer, &mut self.presented, size.width as usize, only.as_deref())),
                        false => {
                            ui::copy_all(&mut self.presented, &buffer);
                            None
                        }
                    };
                    let told = started.elapsed().saturating_sub(painted);
                    window.pre_present_notify();
                    let presenting = Instant::now();
                    let share = match &parts {
                        Some(parts) => {
                            let rects: Vec<softbuffer::Rect> = parts
                                .iter()
                                .filter_map(|[x, y, w, h]| Some(softbuffer::Rect { x: *x, y: *y, width: NonZeroU32::new(*w)?, height: NonZeroU32::new(*h)? }))
                                .collect();
                            let _ = buffer.present_with_damage(&rects);
                            (parts.iter().map(|part| f64::from(part[2]) * f64::from(part[3])).sum::<f64>() + 0.0) / (f64::from(size.width) * f64::from(size.height))
                        }
                        None => {
                            let _ = buffer.present();
                            1.0
                        }
                    };
                    let presented = presenting.elapsed();
                    // What a frame really cost: waiting for the display to
                    // be done with the last, drawing this one, telling what
                    // changed in it, handing that over and how much of the
                    // window it was, and how long after the last it began.
                    if self.options.debug {
                        let gap = self.frame_began.map(|before| started.duration_since(before)).unwrap_or_default();
                        let what = if moving { "moving" } else { "still " };
                        eprintln!(
                            "jog: draw  {what}  wait {waited:>7.1?}  paint {painted:>7.1?}  compare {told:>7.1?}  present {presented:>7.1?}  sent {:>5.1}%  since last {gap:>7.1?}",
                            share * 100.0
                        );
                    }
                    self.frame_began = Some(started);
                    self.painted(started, painted);
                    if let Some(scene) = self.bench_scene().filter(|scene| !scene.is_empty()) {
                        self.bench_note(scene, "redraw", painted);
                    }
                    // A frame has reached the window the ordinary way, and
                    // the window is up: the background is no longer needed.
                    if self.focused_once {
                        self.backdrop = None;
                    }
                }
            }
        }
        self.surface = Some(surface);
    }

    /// Notes how long a redraw took, and whether it was the first to show
    /// the file on view.
    pub(super) fn painted(&mut self, started: Instant, painted: Duration) {
        let now = Instant::now();
        let perf = &mut self.perf;
        perf.paint = painted;
        perf.present = now.duration_since(started).saturating_sub(painted);
        perf.paints.push_back(now);
        while perf.paints.front().is_some_and(|at| now.duration_since(*at) > Duration::from_secs(1)) {
            perf.paints.pop_front();
        }
        let up = self.grid.is_none() && self.shown.as_ref().is_some_and(|(id, _)| *id == self.id());
        if up && self.perf.shown_in.is_none() {
            let took = now.duration_since(self.perf.arrived);
            self.perf.shown_in = Some(took);
            if self.options.verbose {
                self.report_shown(took);
            }
            // For the benchmark: how long the picture was in coming, and
            // how long its decode was. The first picture starts it off.
            if let Some(bench) = &self.bench {
                let id = self.id();
                let made = self.loader.stats(id).fitted;
                match bench.begun {
                    false => {
                        self.bench_note("start", "launch to first picture", bench.launched.elapsed());
                        self.bench_step_due = true;
                    }
                    true => {
                        let scene = bench.scene;
                        if matches!(scene, "step" | "fast" | "back" | "video") {
                            self.bench_note(scene, "key to picture", took);
                        }
                        // Going back is over pictures decoded already.
                        if matches!(scene, "step" | "fast" | "video") {
                            if let Some(made) = made.filter(|made| made.how.is_empty()) {
                                self.bench_note(scene, "decode, read and fit", made.read + made.fit);
                            }
                        }
                    }
                }
            }
            // The overlay was drawn before these figures were known.
            if self.show_debug {
                self.redraw();
            }
        }
    }

    /// For `--verbose`: a line about the file that has just come up.
    pub(super) fn report_shown(&self, took: Duration) {
        let id = self.id();
        let mut line = format!("jog: [{}/{}] {}", self.pos + 1, self.list.len(), self.files[id].display());
        if let Some((width, height)) = self.native() {
            line.push_str(&format!("  {width}x{height}"));
        }
        if let Some(size) = std::fs::metadata(&self.files[id]).ok().map(|meta| meta.len()) {
            line.push_str(&format!("  {}", meta::human_size(size)));
        }
        line.push_str(&format!("  shown in {}", ms(took)));
        // A frame decoded ahead of time cost nothing on arrival.
        if let Some(made) = self.loader.stats(id).fitted {
            let how = if made.how.is_empty() { String::new() } else { format!(", {}", made.how) };
            line.push_str(&format!(" (read {}, fit {}{how})", ms(made.read), ms(made.fit)));
        }
        eprintln!("{line}");
    }

    /// The frame to draw from: the full-resolution one when it is loaded.
    pub(super) fn source(&self) -> Option<Arc<Frame>> {
        let (id, frame) = self.shown.as_ref()?;
        let full = if *id == self.id() { self.loader.full(*id) } else { None };
        Some(full.unwrap_or_else(|| frame.clone()))
    }

    pub(super) fn paint(&mut self, pixels: &mut [u32], window: (u32, u32)) {
        // Drawing waits on nothing: see `mpv::drawing`.
        let _drawing = crate::mpv::drawing();
        let playing = self.grid.is_none() && self.video.as_ref().is_some_and(|video| video.showing);
        // Drawn on the GPU, the video is in its own window over this one,
        // shown while a video plays and over where it goes.
        let (left, (stage_w, stage_h)) = self.stage();
        // Not while a frame from before a pause is shown: jog draws those.
        let stepped_back = self.back_shown().is_some();
        // With the grid up, a video playing on in its cell, or in the small
        // picture under the grid, has its window there.
        let showing = self.video.as_ref().is_some_and(|video| video.showing);
        let pip = (self.grid.is_some() && showing).then(|| self.pip_place(window).or_else(|| self.pip_small(window))).flatten();
        let full = (playing && !stepped_back).then_some([left, 0, stage_w as i32, stage_h as i32]);
        // Over the workspaces or in Edit, nothing of a video is shown.
        let elsewhere = self.overview.is_some() || self.editor.is_some();
        self.show_video_windows(full.or(pip).filter(|_| !elsewhere));
        // The overview of workspaces takes the whole window.
        if self.overview.is_some() {
            self.scene_live = false;
            ui::fill_all(pixels, GRID_COLOR);
            return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
        }
        // So does Edit: the picture with its marks, and the tools.
        if self.editor.is_some() {
            self.scene_live = false;
            return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
        }
        // Fresh out of the grid, the picture grows from the cell it had
        // there before anything else of it is drawn.
        if self.grid.is_none() {
            if let Some((picture, [x, y, w, h])) = self.growing() {
                ui::fill_all(pixels, self.options.background);
                Canvas::new(pixels, window.0, window.1).image_fast(&picture, x, y, w, h);
                self.scene_live = false;
                return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
            }
        }
        let playing = self.grid.is_none() && self.video.as_ref().is_some_and(|video| video.showing);
        if self.video_window_shown() && full.is_some() {
            return self.paint_over_video(pixels, window);
        }
        self.clear_video_layer();
        // A frame kept from before a pause, stepped back to: drawn as it
        // was kept, at this very size, in place of the main player's.
        if playing {
            let len = window.0 as usize * window.1 as usize;
            if let Some((_, kept)) = self.back_shown().filter(|(_, kept)| kept.len() == len) {
                ui::copy_into(pixels, kept);
                self.video_kept.set(None);
                self.repainted.set(true);
                self.scene_live = false;
                return self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
            }
        }
        let player = self.player.as_ref().filter(|_| playing);
        let started = Instant::now();
        if let Some(player) = player {
            // Asked to draw at a size it has not drawn at, the player sets
            // itself up afresh, which for a large video is ten times as long
            // as the frame: while the window is being resized, every frame. So
            // it draws at the size it had, and the frame is scaled here, until
            // the window has kept one size for a moment.
            let resizing = self.resized_at.is_some_and(|at| at.elapsed() < RESIZE_SETTLE);
            let size = self.video_render.filter(|size| resizing && *size != window && size.0 > 0 && size.1 > 0).unwrap_or(window);
            // The player draws a new frame straight into the window. A
            // frame drawn over again, for the pointer moving over the bar
            // or a word coming up, is copied from a copy kept of it: a
            // fraction of what the player takes to scale it to a large
            // window, and the display is then told only of what moved
            // over it. The copy is made the first time it is wanted,
            // so that playback with nothing over it pays nothing for it.
            let len = window.0 as usize * window.1 as usize;
            let showing = self.video.as_ref().map(|video| video.id);
            let kept = self.video_kept.get() == Some((window, showing)) && self.video_copy.len() == len;
            let fresh = size == window && player.frame_waiting();
            let drawn = if size != window {
                self.video_kept.set(None);
                self.repainted.set(true);
                ui::size_for(&mut self.video_scratch, size.0 as usize * size.1 as usize);
                let drawn = player.render_latest(&mut self.video_scratch, size.0, size.1);
                if drawn {
                    let (x, y, scale) = Self::video_place(size, window);
                    let at = [x as i32, y as i32, (f64::from(size.0) * scale) as i32, (f64::from(size.1) * scale) as i32];
                    ui::fill_all(pixels, 0);
                    let whole = [0.0, 0.0, f64::from(size.0), f64::from(size.1)];
                    let _ = render::draw_crop(pixels, window.0 as usize, &self.video_scratch, size, whole, at);
                }
                drawn
            } else if fresh {
                self.video_render = Some(window);
                self.video_kept.set(None);
                self.repainted.set(true);
                player.render_latest(pixels, window.0, window.1)
            } else if !kept {
                self.video_render = Some(window);
                ui::size_for(&mut self.video_copy, len);
                let drawn = player.render_latest(&mut self.video_copy, window.0, window.1);
                self.video_kept.set(drawn.then_some((window, showing)));
                if drawn {
                    ui::copy_into(pixels, &self.video_copy);
                }
                drawn
            } else {
                ui::copy_into(pixels, &self.video_copy);
                true
            };
            if drawn {
                self.perf.render = started.elapsed();
                if self.options.debug && self.options.verbose {
                    eprintln!("jog: {:>7.1?} video render", self.perf.render);
                }
                self.scene_live = false;
                let overlays = Instant::now();
                // What of the frame paused on is outside BT.709, if asked.
                let _ = self.draw_gamut(pixels, window);
                self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
                if self.options.debug {
                    eprintln!("jog: {:>7.1?} draw  video  overlays {:>7.1?}", self.perf.render, overlays.elapsed());
                }
                if fresh {
                    self.hitch_frame(self.perf.render, overlays.elapsed());
                }
                return;
            }
        }
        let source = if self.grid.is_some() { None } else { self.source() };
        match source {
            None if self.grid.is_some() => {
                let started = Instant::now();
                // The grid slides after its cursor; while it does, it is
                // drawn again and again.
                let layout = self.grid_layout(window);
                let mut moved = false;
                if let Some(grid) = &mut self.grid {
                    let was = grid.target;
                    grid.follow(&layout);
                    moved = grid.target != was;
                    if grid.scrolling() {
                        self.grid_due = Some(Instant::now() + Duration::from_millis(8));
                    }
                }
                // Other rows are coming into view: their pictures are wanted.
                if moved {
                    self.request();
                }
                ui::fill_all(pixels, GRID_COLOR);
                self.scene_live = false;
                self.render_pip(window);
                self.draw_grid(&mut Canvas::new(pixels, window.0, window.1));
                self.draw_shrinking(&mut Canvas::new(pixels, window.0, window.1), &layout);
                if self.options.debug {
                    eprintln!("jog: {:>7.1?} draw  grid", started.elapsed());
                }
                if self.bench_scene() == Some("grid") {
                    self.bench_note("grid", "draw the page", started.elapsed());
                }
            }
            Some(frame) => {
                let layout = self.layout_of(frame.native);
                let shown = self.shown.as_ref().map(|(id, _)| *id);
                // Big sources are scaled crudely while zooming or panning,
                // then smoothly once the view has settled.
                let settled = self.settle_at.is_none() || frame.pixels.len() <= SMOOTH_ALWAYS;
                // Past a few times magnification, show the actual pixels.
                let magnified = layout.width >= frame.width * SHOW_PIXELS_AT;
                let smooth = self.antialias && settled && !magnified;
                // An animation's clock starts when its first picture is drawn.
                if frame.animation.is_some() && self.picture_due.is_none() {
                    self.picture = 0;
                    self.picture_due = Some(Instant::now() + picture_delay(&frame, 0));
                }
                let picture = if frame.animation.is_some() { self.picture } else { 0 };
                let key = SceneKey { frame, picture, layout, smooth, window };
                let same = self.scene_key.as_ref().is_some_and(|current| current.matches(&key));
                // A picture shown at the size it was made, which most are,
                // is copied straight into the window: scaling it would be
                // a copy anyway, and a copy kept to copy from is only the
                // same copy twice over. A scaled picture is kept, so that
                // a frame drawn over it again does not scale it again.
                let direct = (key.frame.width, key.frame.height) == (layout.width, layout.height);
                let background = self.options.background;
                let started = Instant::now();
                if direct {
                    render::draw_image(pixels, window, &key.frame, picture, layout, smooth, background);
                } else if !same {
                    ui::size_for(&mut self.scene, pixels.len());
                    render::draw_image(&mut self.scene, window, &key.frame, picture, layout, smooth, background);
                }
                if !same {
                    self.scene_id = shown;
                    let took = started.elapsed();
                    if self.options.debug {
                        let how = if smooth { "smooth" } else { "nearest" };
                        eprintln!("jog: {took:>7.1?} draw  {how} {}x{} -> {}x{}", key.frame.width, key.frame.height, layout.width, layout.height);
                    }
                    if self.bench_scene() == Some("zoom") {
                        self.bench_note("zoom", if smooth { "scale the picture, smooth" } else { "scale the picture, rough" }, took);
                    }
                    let (from, to) = ((key.frame.width, key.frame.height), (layout.width, layout.height));
                    self.perf.scaled = Some(Scaled { took, smooth, from, to });
                    self.scene_key = Some(key);
                    self.repainted.set(true);
                }
                if !direct {
                    ui::copy_into(pixels, &self.scene);
                }
                self.scene_live = true;
            }
            None => {
                self.scene_live = false;
                ui::fill_all(pixels, self.options.background);
            }
        }
        self.draw_overlays(&mut Canvas::new(pixels, window.0, window.1));
    }

    pub(super) fn draw_overlays(&mut self, canvas: &mut Canvas) {
        // The dialog drawn this frame, if any, says where its heading is.
        self.dialog_header.set(None);
        let (width, height) = (canvas.width, canvas.height);
        let px = ui_px(height as u32);
        let margin = px as i32;
        self.keep_video_clear();
        self.tend_story();
        self.tend_trailer();
        // The grid replaces the image and everything describing it.
        let viewing = self.grid.is_none() && self.overview.is_none() && self.editor.is_none();
        // The graphs of the picture's colors are counted from it as it is
        // on the screen, before anything is drawn over it: only the part of
        // it in view, which zoomed in is the part looked at.
        let graphing = viewing && self.show_debug && self.scopes.contains(&true);
        let mut graph_cost = Duration::ZERO;
        if graphing {
            let shown = self.size_of(self.id());
            let (left, stage) = self.stage();
            let [x, y, w, h] = match shown.map(|native| self.layout_of(native)) {
                Some(layout) => [layout.x, layout.y, layout.width as i32, layout.height as i32],
                None => [left, 0, stage.0 as i32, stage.1 as i32],
            };
            let (x0, y0) = (x.max(0), y.max(0));
            let (x1, y1) = ((x + w).min(width), (y + h).min(height));
            let rect = [x0 as usize, y0 as usize, (x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize];
            let counting = Instant::now();
            self.scope_counts.count(canvas.all(), width as usize, rect, self.scopes);
            graph_cost = counting.elapsed();
        }
        let strip = (viewing && self.strip_visible()).then(|| Strip::for_height(height as u32)).filter(Strip::has_room);
        if let Some(strip) = strip {
            self.draw_strip(canvas, strip);
        }
        // The scenes of the video on view stand there instead, when asked for.
        let scenes_level = self.glide(&self.scenes_rise, if self.scenes_shown() { 1.0 } else { 0.0 }, EASE);
        let scene_strip = (scenes_level > 0.01 && viewing && self.video.is_some()).then(|| {
            // It rises from the bottom edge and sinks back into it.
            let strip = Strip::for_height(height as u32);
            Strip { top: strip.top + ((1.0 - scenes_level) * f64::from(strip.height)) as i32, ..strip }
        })
        .filter(Strip::has_room);
        if let Some(strip) = scene_strip {
            match self.strip_tab {
                StripTab::Scenes => {
                    self.want_scenes();
                    self.draw_scenes(canvas, strip);
                }
                StripTab::Sections => self.draw_entries(canvas, strip),
            }
        }
        let bottom = strip.or(scene_strip).map_or(height, |s| s.top) - margin;

        let marked = viewing && self.marks.contains(&self.id());
        if marked {
            // A bar along the top edge flags a marked image.
            canvas.fill(0, 0, width, (px as i32 / 3).max(4), MARK_COLOR);
        }
        // The lines at the top left, each a run of words in their colors:
        // what matters bright, what describes it muted.
        let (soft, dim) = (accent_soft(), DIM_TEXT_COLOR);
        let mut top_left: Vec<Vec<(String, u32)>> = Vec::new();
        // Said all the while, so that nothing done is taken for done.
        if self.options.dry_run {
            top_left.push(vec![("Dry run  ·  nothing changes".to_string(), MARK_COLOR)]);
        }
        if let Some((typed, all)) = self.filter.as_ref().filter(|_| self.show_info) {
            let found = format!("{} {typed}", commands::icons::SEARCH);
            top_left.push(vec![(found, soft), (format!("   {} of {}", self.list.len(), all.len()), TEXT_COLOR), ("   Esc to go back".to_string(), dim)]);
        }
        if viewing && !self.marks.is_empty() {
            let count = format!("{} {} marked", commands::icons::STAR, self.marks.len());
            let line = if marked { vec![(count, MARK_COLOR), ("   this one too".to_string(), dim)] } else { vec![(count, dim)] };
            top_left.push(line);
        }
        if viewing && self.show_info {
            // The name is there to be clicked, which copies it; the folder
            // before it, dimmer, copies the whole path.
            let folder = self.files[self.id()].parent().filter(|dir| !dir.as_os_str().is_empty()).map(short_dir);
            let mut line = vec![(format!("{LINK}{STRONG}{}", self.name()), TEXT_COLOR), (format!("   {} / {}", self.pos + 1, self.list.len()), soft)];
            if let Some(folder) = folder {
                line.insert(0, (format!("{LINK}{}/", folder.trim_end_matches('/')), dim));
            }
            // A video not watched yet says so, until it has been.
            if self.is_new(self.id()) {
                line.push(("   new".to_string(), accent_soft()));
            }
            // Which workspace this is, when there are several to be in.
            if self.spaces.len() > 1 {
                line.insert(0, (format!("@{}   ", self.spaces[self.space].name), soft));
            }
            // Beside the name, badges for what the file is: an image's
            // format and whether it is an Ultra HDR photo; a video's codec
            // and HDR formats.
            let ultra = self.video.is_none() && self.is_ultra_hdr(self.id());
            let mut badges: Vec<String> = Vec::new();
            match self.video.is_some() {
                true => {
                    badges.extend(self.codec_name());
                    badges.extend(self.hdr_badges());
                    // How many tracks there are to pick from, when more than one.
                    let (subs, audio) = self.video.as_ref().and_then(|video| video.tracks).unwrap_or((0, 0));
                    if audio > 1 {
                        badges.push(format!("{audio} audio"));
                    }
                    if subs > 0 {
                        badges.push(format!("{subs} subs"));
                    }
                }
                false => {
                    badges.extend(self.format_of(self.id()));
                    if ultra {
                        badges.push("Ultra HDR".to_string());
                    }
                }
            }
            let before_badges = line.len();
            for name in &badges {
                line.push((if line.len() > before_badges { "" } else { " " }.to_string(), dim));
                line.push(badge(name));
            }
            top_left.push(line);
            // The dimensions, to be clicked, which copies them.
            let (dims, rest) = self.info_line();
            let mut details = vec![(rest, dim)];
            // Which audio and subtitles are playing.
            if let Some(words) = self.video.as_ref().and_then(|video| video.track_words.clone()) {
                details.push((format!("   {words}"), dim));
            }
            if let Some(dims) = dims {
                details.insert(0, (format!("{LINK}{dims}"), dim));
            }
            if ultra {
                // The three renditions, to be clicked: the one on view filled.
                details.push(("   ".to_string(), soft));
                for mode in [Hdr::Sdr, Hdr::Full, Hdr::GainMap] {
                    let marker = if mode == self.hdr { CHOSEN } else { CHOICE };
                    details.push((format!("{marker}{}", mode.name()), TEXT_COLOR));
                }
            }
            top_left.push(details);
            if let Some((position, length)) = self.video_time() {
                let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
                let sign = if paused { commands::icons::PAUSE } else { commands::icons::PLAY };
                let mut line = vec![(format!("{sign} {}", clock(position)), TEXT_COLOR), (format!(" / {}", clock(length)), dim)];
                if self.speed != 1.0 {
                    line.push((format!("   {}x", self.speed), soft));
                }
                // Looped on its own, from its row in the file list, it plays
                // again whatever the list's loop.
                let own = self.video.as_ref().is_some_and(|video| self.looped.contains(&video.id));
                match self.looping {
                    Looping::Off | Looping::List if own => line.push((format!("   {} this video", commands::icons::REPEAT_1), soft)),
                    Looping::Off => {}
                    Looping::File => line.push((format!("   {} this video", commands::icons::REPEAT_1), soft)),
                    Looping::List => line.push((format!("   {} the list", commands::icons::REPEAT), soft)),
                }
                if self.muted {
                    line.push((format!("   {} muted", commands::icons::VOLUME_X), MARK_COLOR));
                }
                top_left.push(line);
            }
        }
        let debug = if viewing && self.show_debug { self.debug_groups() } else { Vec::new() };
        // The seek bar: a rounded track lifted off the bottom edge, with a
        // handle and the times at either end. It is out of the way during
        // playback, and appears while the pointer is at the bottom of the
        // window and for a moment after a seek, a frame step or a pause.
        let mut time = Vec::new();
        let mut time_bottom = bottom;
        // Text placed by an anchor: ending at it, centered on it, or starting at it.
        let mut labels: Vec<(i32, i32, Ordering, String)> = Vec::new();
        // The buttons at the right of the bar: where, which icon, how bright.
        let mut buttons: Vec<(i32, i32, &str, u32)> = Vec::new();
        // What a button over a section does, said over it as a tooltip.
        let mut bar_tip: Option<(i32, i32, String)> = None;
        // The title, time and details along the bar, written with the text.
        let mut bar_info: Option<bar_info::BarInfo> = None;
        let mut subtitle_margin = SUBTITLE_MARGIN;
        let bar_wanted = viewing && (self.hover.is_some() || self.time_until.is_some());
        // It rises from the bottom edge and sinks back into it.
        let bar_level = self.glide(&self.bar_rise, if bar_wanted { 1.0 } else { 0.0 }, EASE);
        // Wanted, whether it is drawn, and if not, why not: told once each time it changes.
        if self.options.debug {
            let state = match (bar_wanted, self.video_time()) {
                (false, _) => None,
                (true, None) => Some(match self.player.as_ref() {
                    None => "x no player".to_string(),
                    // Where it is, as last told of and as asked now: the one
                    // missing and the other there, what it says goes untold.
                    Some(player) => format!(
                        "x open={:?} dur={:?} file={:?} pos={:?}/{:?} pause={:?} eof={:?}",
                        player.text("path"),
                        player.number("duration"),
                        self.video.as_ref().map(|video| self.files[video.id].display().to_string()),
                        player.number("time-pos"),
                        player.number_now("time-pos"),
                        player.flag("pause"),
                        player.flag("eof-reached"),
                    ),
                }),
                (true, Some(_)) if bar_level >= 0.999 => Some(format!("drawn y{} {width}x{height}", self.seek_bar().middle)),
                (true, Some(_)) => Some("rising".to_string()),
            };
            if *self.bar_said.borrow() != state {
                if let Some(state) = &state {
                    eprintln!("jog: bar {state}");
                }
                *self.bar_said.borrow_mut() = state;
                self.bar_frames.set(0);
            }
            // The first frames drawn while it is wanted, each: whether frames
            // come at all, and what each found.
            if bar_wanted && self.bar_frames.get() < 8 {
                self.bar_frames.set(self.bar_frames.get() + 1);
                eprintln!("jog: bar f{} t={:?} why={} lvl={bar_level:.2} {width}x{height}", self.bar_frames.get(), self.video_time(), self.video_time_why());
            }
        }
        // Where the bar stands on a strip it comes up from behind it. What
        // the strip shows is kept while the bar is on its way, and put back
        // over whatever of the bar was drawn across it.
        let onto = ((self.strip_pinned || self.triage_on) && strip.is_some()) || scene_strip.is_some();
        let band_top = Strip::for_height(height as u32).top;
        let behind = (onto && bar_level > 0.01 && bar_level < 0.999).then(|| canvas.copy(0, band_top, width, height - band_top));
        if let Some((position, length)) = (bar_level > 0.01).then(|| self.video_time()).flatten() {
            let mut bar = self.seek_bar();
            // Up from the bottom edge, or from behind the strip it stands
            // on: the same way either way.
            let travel = if onto { bar.unit * 5 } else { bar.unit * 6 };
            bar.middle += ((1.0 - bar_level) * f64::from(travel)) as i32;
            let unit = bar.unit;
            let line = Text::line_height(px);
            let thickness = unit * 2 / 3;
            let top = bar.middle - thickness / 2;
            // Darken the picture behind it, more and more towards the
            // bottom edge.
            let shade_top = bar.middle - unit * 6;
            // Down to what the bar stands on: the window's edge, or a strip
            // along it, which is not to be darkened with the picture.
            canvas.fade_down(bar.left, shade_top, width - bar.left, (bar.middle + unit * 2).min(height) - shade_top, 70);
            // The handle is where the video is, and jumps with it.
            let done = (f64::from(bar.width) * position / length) as i32;
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 72, bar.width);
            canvas.pill(bar.x, top, bar.width, thickness, TEXT_COLOR, 256, done.max(thickness / 2));
            // A notch in the bar where each scene starts, while they are up.
            if let Some((_, starts, _)) = self.scenes_now().filter(|_| self.scenes_shown()) {
                for start in &starts[1..] {
                    let at = bar.x + (f64::from(bar.width) * start / length) as i32;
                    canvas.fill(at - 1, top, 2, thickness, 0x000000);
                }
            }
            let flags = self.draw_section(canvas, &bar, top, thickness, length, &mut labels, px);
            // What can be done with the section, in a row above it.
            let tools = self.section_tools(&bar);
            let kept = !tools.is_empty() && self.section_kept();
            self.draw_section_tools(canvas, &tools, kept, &mut buttons, &mut bar_tip, px);
            // The span of that row, which the time and the frame under the
            // pointer stand clear of; on it, they are not shown at all.
            let tools_span = tools.first().zip(tools.last()).map(|(first, last)| (first.1[0], last.1[0] + last.1[2], first.1[1]));
            let on_tools = tools_span.is_some_and(|(left, right, top)| {
                let (x, y) = self.mouse;
                x >= f64::from(left) && x < f64::from(right) && y >= f64::from(top - unit / 2) && y < f64::from(bar.middle - unit)
            });
            let handle = unit * 5 / 4;
            canvas.pill(bar.x + done - handle / 2, bar.middle - handle / 2, handle, handle, TEXT_COLOR, 256, handle);
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
            // Either side of it: the file before and the file after.
            for (control, icon) in [(0, commands::icons::SKIP_BACK), (2, commands::icons::SKIP_FORWARD)] {
                let (left, top, side) = bar.control(control);
                labels.push((left + side / 2, top + (side - line) / 2, Ordering::Equal, icon.to_string()));
            }
            let (symbol_left, symbol_top, side) = bar.symbol();
            // The symbol is what a press on it does: pause while playing,
            // and play while paused.
            if !paused {
                canvas.pause_symbol(symbol_left, symbol_top, side, TEXT_COLOR);
            } else {
                canvas.play_symbol(symbol_left, symbol_top, side, TEXT_COLOR);
            }
            if bar.title_w > 0 {
                // The title over the time at the left, the time left at the
                // right, and what the file is over the track's right end.
                let small_line = Text::line_height((px * 5 / 6).max(11));
                let left = bar.control(2).0 + bar.control(2).2 + unit;
                bar_info = Some(bar_info::BarInfo {
                    left,
                    top: bar.middle - (line + small_line) / 2,
                    room: bar.title_w,
                    title: self.bar_title(),
                    time: format!("{} / {}", clock(position), clock(length)),
                    right: bar.x + bar.width,
                    details_top: bar.middle - unit - small_line,
                    details: self.bar_details(),
                    flags: flags.clone(),
                    letters_top: bar.middle - thickness * 3 / 2 - line,
                    tools: tools_span.map(|(left, right, _)| (left, right)),
                });
                labels.push((bar.x + bar.width + unit, bar.middle - line / 2, Ordering::Greater, format!("−{}", clock((length - position).max(0.0)))));
            } else {
                labels.push((bar.x - unit, bar.middle - line / 2, Ordering::Less, clock(position)));
                labels.push((bar.x + bar.width + unit, bar.middle - line / 2, Ordering::Greater, clock(length)));
            }
            for (which, command) in (0..).zip(self.bar_buttons()) {
                let (left, top, side) = bar.button(which);
                let (icon, on) = match command {
                    Command::CycleLoop => match self.looping {
                        Looping::Off => (commands::icons::REPEAT, false),
                        Looping::File => (commands::icons::REPEAT_1, true),
                        Looping::List => (commands::icons::REPEAT, true),
                    },
                    Command::ToggleMute if self.muted => (commands::icons::VOLUME_X, false),
                    Command::ToggleMute => (commands::icons::VOLUME_2, true),
                    Command::CycleSubtitles if self.subtitles => (commands::icons::CAPTIONS, true),
                    Command::CycleSubtitles => (commands::icons::CAPTIONS_OFF, false),
                    // Fullscreen: what pressing it does, out or in.
                    Command::ToggleFullscreen if self.window.as_ref().is_some_and(|window| window.fullscreen().is_some()) => (commands::icons::SHRINK, true),
                    Command::ToggleFullscreen => (commands::icons::EXPAND, true),
                    Command::SmallWindow => (commands::icons::PIP, true),
                    Command::PlaybackSettings => (commands::icons::SETTINGS, true),
                    _ => (commands::icons::AUDIO_LINES, true),
                };
                buttons.push((left + side / 2, top + (side - line) / 2, icon, if on { TEXT_COLOR } else { DIM_TEXT_COLOR }));
            }
            // Over the volume button, while the pointer is on it, its slider.
            self.draw_volume(canvas, &bar, &mut labels);
            // Subtitles stand clear of the bar while it is up.
            subtitle_margin = (height - (bar.middle - unit * 3)) * 720 / height.max(1);
            time_bottom = bar.middle - unit * 4;

            // Not over the section's buttons, nor over the bar's own words,
            // nor while a menu or a window is open over it.
            if let Some(fraction) = self.hover.filter(|_| bar.on_track(self.mouse.0) && !on_tools && !self.bar_busy() && self.bar_hit(self.mouse.0, self.mouse.1).is_none()) {
                // A tick where the pointer is, the time there, and above
                // that the frame from that part of the video.
                let at = bar.x + (f64::from(bar.width) * fraction) as i32;
                canvas.fill(at - 1, top - unit / 3, 2, thickness + unit * 2 / 3, MARK_COLOR);
                // Over a section's flag, the time stands a line higher than
                // the flag's letter.
                let over_flag = flags.iter().any(|flag| (flag - at).abs() < line * 2);
                let mut label_top = bar.middle - unit * 2 - line / 2 - if over_flag { line } else { 0 };
                // Under the section's buttons, above them.
                // The time is a few letters wide either side of the pointer.
                if let Some((_, _, tools_top)) = tools_span.filter(|(left, right, _)| (left - unit * 6..right + unit * 6).contains(&at)) {
                    label_top = label_top.min(tools_top - unit / 2 - line);
                }
                // What playing from there does with a section looping: it
                // loops, or past its end it plays on.
                let seconds = fraction * length;
                let said = match self.section().and_then(|section| section.looping()) {
                    Some((a, b)) if seconds >= a && seconds <= b => format!("{}  {}", commands::icons::REPEAT, clock(seconds)),
                    Some((_, b)) if seconds > b => format!("{}  ·  past {}", clock(seconds), self.active_letters().1),
                    _ => clock(seconds),
                };
                labels.push((at.clamp(unit * 4, width - unit * 4), label_top, Ordering::Equal, said));
                // The video has changed under a pointer that has not moved:
                // the frame kept is of the one before, and is not shown;
                // the one for this video is asked for.
                if self.preview_id != self.video.as_ref().map(|video| video.id) {
                    self.preview_frame = None;
                    self.request_preview(fraction);
                }
                if let Some(frame) = &self.preview_frame {
                    let (w, h) = (frame.width as i32, frame.height as i32);
                    let left = (at - w / 2).clamp(unit, (width - unit - w).max(unit));
                    // Clear of the time under it, not crowding it.
                    let picture_top = label_top - unit - h;
                    let round = unit / 2;
                    canvas.image_rounded(frame, left, picture_top, w, h, round);
                    canvas.round_outline(left - 2, picture_top - 2, w + 4, h + 4, round + 2, 2, MENU_EDGE_COLOR);
                }
            } else if self.time_until.is_some() && !paused {
                time.push(format!("{} / {}", clock(position), clock(length)));
            }
            // Paused, to the frame: the time in hundredths and the frame's
            // number, for stepping one at a time.
            // Not over a preview, which says its own time.
            if paused && !self.hover.is_some_and(|_| bar.on_track(self.mouse.0)) {
                let fps = self.player.as_ref().and_then(|player| player.number("container-fps")).filter(|fps| *fps > 1.0);
                let frame = fps.map_or(String::new(), |fps| format!("  ·  frame {}", (position * fps).round() as u64));
                time.push(format!("{}.{:02}{frame}", clock(position), ((position.fract() * 100.0) as u64).min(99)));
            }
        }
        let exif = if viewing && self.show_exif {
            self.current_meta().map(|meta| meta.exif.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let footer: Vec<String> = self.grid_footer().into_iter().collect();
        let mut actions = Vec::new();
        if viewing && self.show_actions {
            let defined = self.options.actions.iter().enumerate();
            actions.extend(defined.filter_map(|(slot, a)| Some(format!("{slot}: {}", a.as_ref()?))));
            if actions.is_empty() {
                actions.push("No actions defined (see --action)".to_string());
            }
        }
        let toast: Vec<String> = self.toast.iter().map(|(message, _)| message.clone()).collect();
        let error: Vec<String> = match (&self.error, &self.shown) {
            (Some(reason), None) if viewing => vec![format!("Not shown · {}", self.name()), reason.clone()],
            _ => Vec::new(),
        };

        let blocks = [&exif, &actions, &toast, &error, &footer, &time];
        let nothing_to_write = blocks.iter().all(|lines| lines.is_empty()) && labels.is_empty() && top_left.is_empty() && debug.is_empty();
        // In Inspect's Files tab, it is drawn there instead.
        let panel = if viewing { self.list_panel().filter(|panel| !panel.embedded) } else { None };
        let boxes = self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some() || panel.is_some() || self.convert_panel.is_some() || self.video_panel.is_some() || self.compare.is_some() || self.track_picker.is_some() || self.inspector.is_some() || self.track_editor.is_some() || self.editor.is_some();
        // The filmstrip has words of its own: the lengths of its videos.
        let boxes = boxes || self.palette.is_some() || strip.is_some() || scene_strip.is_some() || self.overview.is_some() || self.triage_on;
        let boxes = boxes || self.activity.open || self.activity_running();
        if nothing_to_write && !boxes && !self.help {
            // Nothing drawn means no debug overlay, and none of its buttons.
            self.debug_buttons.clear();
            self.bar_hits.clear();
            return;
        }
        let mut text = self.text.take().unwrap_or_else(Text::load);
        // Under all else: it fills the window, and toasts and boxes come
        // up over it.
        if self.editor.is_some() {
            self.draw_editor(canvas, &mut text, px);
        }
        self.draw_overview(canvas, &mut text, px);
        self.draw_grid_labels(canvas, &mut text, px);
        self.draw_grid_footer(canvas, &mut text, &footer, px);
        // What sits on the left moves over to make room for the file list.
        let left = margin + panel.as_ref().filter(|_| !self.list_right).map_or(0, |panel| panel.width);
        // What sits on the right moves over likewise when the list is there.
        let width = width - self.right_inset();
        if let Some(strip) = &scene_strip {
            if self.strip_tab == StripTab::Scenes {
                self.draw_scene_labels(canvas, &mut text, strip, px);
            }
            self.draw_strip_words(canvas, &mut text, strip, px);
        }
        if let Some(strip) = &strip {
            self.draw_strip_lengths(canvas, &mut text, strip, px);
            self.draw_strip_tip(canvas, &mut text, strip, px);
        }
        if strip.is_some() || scene_strip.is_some() {
            self.draw_strip_buttons(canvas, &mut text, px);
        }
        if let Some(panel) = &panel {
            for pos in panel.first..panel.first + panel.rows {
                self.want_duration(self.list[pos]);
            }
            // Drawn as though it stood at the left edge, on a sheet of its
            // own that starts as what is under it, and laid where it stands.
            let (across, down) = (panel.width, canvas.height);
            let mut sheet = canvas.copy(panel.left, 0, across, down);
            if sheet.len() == (across * down) as usize {
                self.draw_list(&mut Canvas::new(&mut sheet, across as u32, down as u32), &mut text, panel);
                canvas.dissolve(panel.left, 0, across, down, &sheet, 256);
            }
        }
        let (below, choices) = rich_block(canvas, &mut text, &top_left, px, (left, margin));
        // The renditions of an Ultra HDR photo, to be clicked.
        self.info_buttons = choices
            .into_iter()
            .filter_map(|(name, [x, y, width, height])| {
                // The file's name, which a click copies.
                if name == self.name() {
                    return Some(Button { x, y, width, height, command: Command::CopyViewedName });
                }
                // The folder before it, which copies the whole path, and
                // the dimensions.
                if name.ends_with('/') {
                    return Some(Button { x, y, width, height, command: Command::CopyViewedPath });
                }
                if name.contains(" x ") && name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Some(Button { x, y, width, height, command: Command::CopyViewedSize });
                }
                let mode = [Hdr::Sdr, Hdr::Full, Hdr::GainMap].into_iter().find(|mode| mode.name() == name)?;
                Some(Button { x, y, width, height, command: Command::SetHdr(mode) })
            })
            .collect();
        let pointer = (self.mouse.0 as i32, self.mouse.1 as i32);
        let (debug_buttons, debug_bottom) = draw_debug(canvas, &mut text, &debug, px, (left, below), &mut self.debug_keep, pointer);
        self.debug_buttons = debug_buttons;
        self.debug_last = debug;
        let mut below_debug = debug_bottom;
        if graphing {
            let drawing = Instant::now();
            let (graph_buttons, graphs_bottom) = self.draw_scopes(canvas, &mut text, px, (left, debug_bottom), pointer);
            self.debug_buttons.extend(graph_buttons);
            self.scope_cost = graph_cost + drawing.elapsed();
            below_debug = graphs_bottom;
        }
        // Under them, a video's HDR: its formats and its curves.
        if viewing && self.show_debug && self.video.is_some() {
            let hdr_buttons = self.draw_hdr(canvas, &mut text, px, (left, below_debug), pointer);
            self.debug_buttons.extend(hdr_buttons);
        }
        text_block(canvas, &mut text, &exif, px, |_, h| (left, bottom - h));
        text_block(canvas, &mut text, &actions, px, |w, h| (width - margin - w, bottom - h));
        // The Activity panel, under the toast and what runs, which stand
        // to its left.
        self.draw_activity(canvas, &mut text, px);
        // A toast slides in from the right edge, and off that way again
        // as its time runs out.
        let toast_in = self.glide(&self.toast_slide, 1.0, EASE);
        let toast_left = self.toast.as_ref().map_or(Duration::ZERO, |(_, until)| until.saturating_duration_since(Instant::now()));
        let toast_out = match self.toast.is_some() && self.options.animations && toast_left < EASE {
            true => {
                self.animating.set(true);
                (1.0 - toast_left.as_secs_f64() / EASE.as_secs_f64()).powi(2)
            }
            false => 0.0,
        };
        let toast_away = (1.0 - toast_in) + toast_out;
        self.toast_buttons.clear();
        match (&self.toast, self.toast_links.is_empty()) {
            (Some((message, _)), false) => {
                // One line: the words, the file's name in them there to be
                // clicked when it offers Show, then its links as buttons.
                let shows = self.toast_links.iter().any(|(_, link)| matches!(link, ToastLink::Show(_)));
                let mut line: Vec<(String, u32)> = vec![(if shows { format!("{LINK}{message}") } else { message.clone() }, TEXT_COLOR), ("  ".to_string(), TEXT_COLOR)];
                line.extend(self.toast_links.iter().map(|(label, _)| (format!("{CHOICE}{label}"), TEXT_COLOR)));
                let pad = px as i32 / 2;
                let w = line.iter().map(|(run, _)| run_width(&mut text, run, px)).sum::<i32>() + 3 * pad;
                let x = width - margin - w + (toast_away * f64::from(w + margin)) as i32;
                let (_, choices) = rich_block(canvas, &mut text, &[line], px, (x, margin));
                for (name, place) in choices {
                    let link = match self.toast_links.iter().find(|(label, _)| *label == name) {
                        Some((_, link)) => link.clone(),
                        // The words themselves: Show.
                        None => match self.toast_links.iter().find(|(_, link)| matches!(link, ToastLink::Show(_))) {
                            Some((_, link)) => link.clone(),
                            None => continue,
                        },
                    };
                    self.toast_buttons.push((link, place));
                }
            }
            _ => text_block(canvas, &mut text, &toast, px, |w, _| (width - margin - w + (toast_away * f64::from(w + margin)) as i32, margin)),
        }
        // While anything runs, how far along, below where a toast goes.
        let pill_top = margin + if self.toast.is_some() { Text::line_height(px) + px as i32 } else { 0 };
        self.draw_activity_pill(canvas, &mut text, px, width - margin, pill_top);
        self.draw_triage(canvas, &mut text, px);
        // Twice the usual size, centered just above the seek bar.
        text_block(canvas, &mut text, &time, px, |w, h| ((width - w) / 2, time_bottom - h));
        for (center, top, icon, color) in buttons {
            let left = center - text.width(icon, px) / 2;
            text.draw(canvas, left, top, px, color, icon);
        }
        if subtitle_margin != self.subtitle_margin {
            self.subtitle_margin = subtitle_margin;
            for player in self.players() {
                player.set("sub-margin-y", &subtitle_margin.to_string());
            }
        }
        for (anchor, top, side, label) in &labels {
            let label_width = text.width(label, px);
            let left = match side {
                Ordering::Less => anchor - label_width,
                Ordering::Equal => anchor - label_width / 2,
                Ordering::Greater => *anchor,
            };
            text.draw(canvas, left, *top, px, TEXT_COLOR, label);
        }
        match &bar_info {
            Some(info) => self.draw_bar_info(canvas, &mut text, info, px),
            None => self.bar_hits.clear(),
        }
        // A button's tooltip, as the others look: a box of its own, a
        // little shadow, standing clear above the buttons.
        if let Some((center, bottom, words)) = &bar_tip {
            let small = (px * 5 / 6).max(11);
            let pad = px as i32 / 2;
            let (w, h) = (text.width(words, small) + pad * 5 / 2, Text::line_height(small) + pad);
            let x = (center - w / 2).clamp(pad, (width - w - pad).max(pad));
            let y = bottom - h;
            draw::panel(canvas, x, y, w, h, pad);
            text.draw(canvas, x + pad * 5 / 4, y + pad / 2, small, TEXT_COLOR, words);
        }
        if let Some(behind) = &behind {
            canvas.dissolve(0, band_top, width, height - band_top, behind, 256);
            // Its words went with it, and are written again.
            if let Some(strip) = &scene_strip {
                if self.strip_tab == StripTab::Scenes {
                    self.draw_scene_labels(canvas, &mut text, strip, px);
                }
                self.draw_strip_words(canvas, &mut text, strip, px);
            }
            if let Some(strip) = &strip {
                self.draw_strip_lengths(canvas, &mut text, strip, px);
            }
            if strip.is_some() || scene_strip.is_some() {
                self.draw_strip_buttons(canvas, &mut text, px);
            }
        }
        text_block(canvas, &mut text, &error, px, |w, h| ((width - w) / 2, (height - h) / 2));
        // A menu or a box comes up over a moment: what was there is kept
        // and laid back over it, less and less. It is well on its way from
        // the first frame, so that nothing seems to wait.
        let boxed = self.menu.is_some() || self.prompt.is_some() || self.palette.is_some() || self.confirm.is_some() || self.help || self.convert_panel.is_some() || self.video_panel.is_some();
        let box_level = self.glide(&self.box_fade, if boxed { 1.0 } else { 0.0 }, EASE);
        let mut behind = std::mem::take(&mut self.fade_buf);
        let fading = boxed && box_level < 0.99;
        if fading {
            ui::copy_all(&mut behind, canvas.all());
        }
        if let Some(menu) = &self.menu {
            self.draw_menu(canvas, &mut text, menu);
        }
        if self.convert_panel.is_some() {
            self.draw_convert_panel(canvas, &mut text, px);
        }
        if self.video_panel.is_some() {
            self.draw_video_panel(canvas, &mut text, px);
        }
        if self.compare.is_some() {
            self.draw_compare(canvas, &mut text, px);
        }
        if self.track_picker.is_some() {
            self.draw_track_picker(canvas, &mut text, px);
        }
        if self.track_editor.is_some() {
            self.draw_track_editor(canvas, &mut text, px);
        }
        // Popped out, it is drawn in its own window instead.
        if self.inspector.is_some() && !self.popped(Popped::Inspector) {
            self.draw_inspector(canvas, &mut text, px);
        }
        // Over the panels, which a box can be opened from.
        if let Some(prompt) = &self.prompt {
            let count = self.targets().len();
            let files = if count == 1 { self.name_of(prompt.id) } else { format!("{count} files") };
            let (icon, title, hint) = match prompt.kind {
                PromptKind::Rename => (commands::icons::PENCIL, "Rename".to_string(), "Enter to rename  ·  a path moves it  ·  Tab completes"),
                PromptKind::SaveWorkspace => (commands::icons::FOLDER_PLUS, "Save as workspace".to_string(), "Enter to save to the config"),
                PromptKind::SectionName { start, end } => (commands::icons::PENCIL, format!("Name {} – {}", clock(start), clock(end)), "Enter to save  ·  empty clears it"),
                PromptKind::FitSize => (commands::icons::REPLACE, "Fit under".to_string(), "500 KB, 1.2 MB  ·  Enter to fit"),
                PromptKind::EditCopy => (commands::icons::COPY, "Save as copy".to_string(), "Enter to save  ·  beside the original"),
                PromptKind::TrackLanguage(_) => (commands::icons::PENCIL, "Language".to_string(), "eng, kor, fre…  ·  Enter to set"),
                PromptKind::TrackTitle(_) => (commands::icons::PENCIL, "Title".to_string(), "Enter to set  ·  empty clears it"),
                PromptKind::TrackFile => (commands::icons::FOLDER_PLUS, "Add a subtitle or audio file".to_string(), "a path, then +0.5 to move it later  ·  Tab completes"),
                PromptKind::SendTo { moving: true } => (commands::icons::FOLDER_INPUT, format!("Move {files} to"), "Enter to move  ·  Tab completes"),
                PromptKind::SendTo { moving: false } => (commands::icons::FOLDER_PLUS, format!("Copy {files} to"), "Enter to copy  ·  Tab completes"),
            };
            // What Enter would do, as it is typed.
            let hint = self.prompt_hint(prompt).unwrap_or_else(|| hint.to_string());
            draw_prompt(canvas, &mut text, icon, &title, &prompt.field, &hint, px);
        }
        if let Some(layout) = self.palette_layout() {
            for id in self.palette_shown() {
                self.want_duration(id);
            }
            let drawn = self.draw_palette(canvas, &mut text, &layout);
            if let Some(palette) = &mut self.palette {
                palette.drawn = drawn;
            }
        }
        if let Some(confirm) = &self.confirm {
            let drawn = draw_confirm(canvas, &mut text, confirm, px);
            if let Some(confirm) = &mut self.confirm {
                confirm.drawn = drawn;
            }
        }
        if self.help {
            // Drawn once for what is typed in it, and laid in after: its
            // words take longer than a 4K frame can spare.
            let key = (self.help_search.clone(), canvas.width, canvas.height);
            let radius = px as i32;
            match self.help_kept.as_ref().filter(|kept| kept.0 == key) {
                Some((_, [x, y, w, h], inside)) => {
                    // The straight middle laid in whole; the curved top and
                    // bottom drawn over the video as it is now.
                    canvas.paste(*x, y + radius, *w, h - 2 * radius, inside);
                    for rows in [*y..y + radius, y + h - radius..y + h] {
                        canvas.band = Some(rows);
                        draw::panel(canvas, *x, *y, *w, *h, radius);
                    }
                    canvas.band = None;
                }
                None => {
                    let [x, y, w, h] = draw_help(canvas, &mut text, px, &self.help_search);
                    let inside = canvas.copy(x, y + radius, w, h - 2 * radius);
                    self.help_kept = Some((key, [x, y, w, h], inside));
                }
            }
        }
        if fading {
            ui::dissolve_all(canvas.all(), &behind, ((1.0 - box_level) * 0.6 * 256.0) as u32);
        }
        self.fade_buf = behind;
        // Letters whose font is still loading show as boxes for now.
        if text.waiting() {
            self.font_due = Some(Instant::now() + Duration::from_millis(150));
        }
        self.text = Some(text);
    }

    /// The list positions whose cells are in view, or partly so while the
    /// grid slides: the page on show and the one coming in beside it.
    pub(super) fn grid_shown(&self, layout: &GridLayout) -> std::ops::Range<usize> {
        let scroll = self.grid.as_ref().map_or(0.0, Grid::scroll);
        let first = scroll.floor().max(0.0) as usize * layout.per_page();
        first.min(self.list.len())..(first + 2 * layout.per_page()).min(self.list.len())
    }

    /// The space a grid cell gives its picture: left, top, width and
    /// height. With titles on, a line under it is kept for the name.
    pub(super) fn grid_area(&self, layout: &GridLayout, pos: usize, window_height: u32) -> [i32; 4] {
        let scroll = self.grid.as_ref().map_or(0.0, Grid::scroll);
        let cell = (pos % layout.per_page()) as i32;
        let (column, row) = (cell % layout.columns, cell / layout.columns);
        let title = if self.grid_titles { Text::line_height(ui_px(window_height)) + layout.pad / 2 } else { 0 };
        // Pages lie side by side; the one on show is at the window's left.
        let slid = ((layout.page_of(pos) - scroll) * f64::from(layout.width)).round() as i32;
        let left = layout.left + column * layout.cell_w + layout.pad + slid;
        let top = layout.top + row * layout.cell_h + layout.pad;
        [left, top, layout.cell_w - 2 * layout.pad, (layout.cell_h - 2 * layout.pad - title).max(1)]
    }

    /// Where in its space a picture of this size is drawn: scaled to fit,
    /// in the middle.
    pub(super) fn grid_picture(area: [i32; 4], size: (u32, u32), enlarge: bool) -> [i32; 4] {
        let [left, top, width, height] = area;
        let (w, h) = fit_size(size, (width as u32, height as u32), enlarge);
        let (w, h) = (w as i32, h as i32);
        [left + (width - w) / 2, top + (height - h) / 2, w, h]
    }

    pub(super) fn draw_grid(&self, canvas: &mut Canvas) {
        let Some(grid) = &self.grid else { return };
        let layout = self.grid_layout((canvas.width as u32, canvas.height as u32));
        let edge = (layout.pad / 4).max(2);
        let radius = (layout.cell_w / 36).max(4);

        // The plain thumbnails go down first, all in one pass over the
        // rows of the window on every core; the cells with more to them
        // are drawn over that one by one.
        let shown: Vec<(usize, usize, Option<Arc<Frame>>)> =
            self.grid_shown(&layout).map(|pos| (pos, self.list[pos], self.loader.thumb(self.list[pos]))).collect();
        let plain = |pos: usize, id: usize| {
            let trailer = self.trailer.as_ref().is_some_and(|trailer| trailer.id == id && pos == grid.cursor && trailer.frame.as_ref().is_some_and(|frame| !frame.pixels.is_empty()));
            let live = self.pip_live.is_some() && self.pip && self.pip_in_cell && pos == self.pos;
            !trailer && !live
        };
        let blits: Vec<(&Frame, [i32; 4])> = shown
            .iter()
            .filter(|(pos, id, thumb)| thumb.is_some() && plain(*pos, *id))
            .map(|(pos, _, thumb)| {
                let thumb = thumb.as_deref().expect("filtered to the thumbnails there are");
                let area = self.grid_area(&layout, *pos, canvas.height as u32);
                (thumb, Self::grid_picture(area, (thumb.width, thumb.height), false))
            })
            .collect();
        canvas.images(&blits);

        let mut cursor_place = None;
        for (pos, id, thumb) in &shown {
            let (pos, id) = (*pos, *id);
            let area = self.grid_area(&layout, pos, canvas.height as u32);
            // A video under the cursor plays as a trailer.
            let trailer = self.trailer.as_ref().filter(|trailer| trailer.id == id && pos == grid.cursor);
            // The video that was on view plays on in its cell, sound and all.
            let live = self.pip_live.as_ref().filter(|_| self.pip && self.pip_in_cell && pos == self.pos);
            let since = trailer.filter(|_| live.is_none()).and_then(|trailer| trailer.since);
            let trailer = live.or(trailer.and_then(|trailer| trailer.frame.as_ref()).filter(|frame| !frame.pixels.is_empty()));
            let [x, y, w, h] = match (trailer, thumb) {
                (Some(frame), _) => {
                    let place = Self::grid_picture(area, (frame.width, frame.height), true);
                    // The thumbnail gives way to the trailer's first frames
                    // gradually, rather than being cut off by them.
                    let left = since.map_or(0.0, |since| 1.0 - since.elapsed().as_secs_f64() / TRAILER_FADE.as_secs_f64());
                    let fading = thumb.as_ref().filter(|_| left > 0.0).map(|thumb| {
                        canvas.image(thumb, place[0], place[1], place[2], place[3]);
                        canvas.copy(place[0], place[1], place[2], place[3])
                    });
                    canvas.image(frame, place[0], place[1], place[2], place[3]);
                    if let Some(was) = fading {
                        canvas.dissolve(place[0], place[1], place[2], place[3], &was, (left * 256.0) as u32);
                    }
                    place
                }
                // Drawn already, with the others.
                (None, Some(thumb)) => Self::grid_picture(area, (thumb.width, thumb.height), false),
                (None, None) => {
                    canvas.fill(area[0], area[1], area[2], area[3], PLACEHOLDER_COLOR);
                    area
                }
            };
            canvas.round_off(x, y, w, h, radius, GRID_COLOR);
            if self.marks.contains(&id) {
                let dot = layout.pad;
                canvas.round_fill(x + dot / 2, y + dot / 2, dot, dot, dot / 2, MARK_COLOR, 256);
            }
            // The cursor is ringed brightly, the image being viewed quietly.
            if pos == grid.cursor {
                // Drawn last, on its way from the cell it was on.
                cursor_place = Some([x, y, w, h]);
            } else if pos == self.pos {
                let color = accent();
                let gap = edge * 2;
                canvas.round_outline(x - gap, y - gap, w + 2 * gap, h + 2 * gap, radius + gap, edge, color);
            }
        }
        // The cursor's ring glides over from the cell it was on. While a
        // page slides, and when the grid has just come up, it is simply
        // where it belongs.
        if let Some(place) = cursor_place {
            let snap = grid.scrolling() || self.ring_snap.take();
            let [x, y, w, h] = std::array::from_fn(|side| {
                let to = f64::from(place[side]);
                if snap {
                    self.ring[side].set(Glide::at(to));
                }
                self.glide(&self.ring[side], to, EASE).round() as i32
            });
            let gap = edge * 2;
            canvas.round_outline(x - gap, y - gap, w + 2 * gap, h + 2 * gap, radius + gap, edge, accent());
        }
    }

    /// The bar above the grid's footer that shows where among the pages the
    /// one on show is, when there is more than one page.
    pub(super) fn grid_bar(&self) -> Option<PageBar> {
        let (width, height) = self.window_size();
        let layout = self.grid_layout((width, height));
        let pages = self.list.len().div_ceil(layout.per_page());
        let thick = (ui_px(height) as i32 / 3).max(4);
        let track = width as i32 / 3;
        let y = height as i32 - GridLayout::footer_height(height) - thick;
        (self.grid.is_some() && pages > 1).then_some(PageBar { x: (width as i32 - track) / 2, y, width: track, height: thick, pages })
    }

    /// Goes to the page at a spot along the page bar, keeping the cursor in
    /// the cell it was in.
    pub(super) fn drag_pages(&mut self, x: f64) {
        let (Some(bar), Some(grid)) = (self.grid_bar(), &self.grid) else { return };
        let per_page = self.grid_layout(self.window_size()).per_page();
        let along = ((x - f64::from(bar.x)) / f64::from(bar.width)).clamp(0.0, 1.0);
        let page = ((along * bar.pages as f64) as usize).min(bar.pages - 1);
        let target = (page * per_page + grid.cursor % per_page).min(self.list.len() - 1);
        let by = target as i64 - grid.cursor as i64;
        if by != 0 {
            self.move_cursor(by);
        }
    }

    /// The round button at the grid's top right that leaves it.
    pub(super) fn grid_close(&self) -> Option<Button> {
        self.grid.as_ref()?;
        let (width, height) = self.window_size();
        let size = ui_px(height) as i32 * 2;
        // In from the corner a little, both ways.
        Some(Button { x: width as i32 - size - size, y: size, width: size, height: size, command: Command::ToggleGrid })
    }


    /// The round buttons at the grid's left and right edges, for the page
    /// before and the page after: where each is, and whether it goes
    /// forward. Only those with a page to go to.
    pub(super) fn grid_arrows(&self) -> Vec<(Button, bool)> {
        let Some(grid) = &self.grid else { return Vec::new() };
        let (width, height) = self.window_size();
        let layout = self.grid_layout((width, height));
        let pages = self.list.len().div_ceil(layout.per_page());
        let size = ui_px(height) as i32 * 3;
        let y = (height as i32 - GridLayout::footer_height(height) - size) / 2;
        let button = |x: i32| Button { x, y, width: size, height: size, command: Command::Next };
        let mut arrows = Vec::new();
        if grid.target >= 1.0 {
            arrows.push((button(size / 4), false));
        }
        if (grid.target as usize) + 1 < pages {
            arrows.push((button(width as i32 - size - size / 4), true));
        }
        arrows
    }

    /// The words on the grid: under each picture its title, when those are
    /// on, and in the corner of each video how long it is.
    pub(super) fn draw_grid_labels(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let Some(cursor) = self.grid.as_ref().filter(|_| self.overview.is_none()).map(|grid| grid.cursor) else { return };
        let layout = self.grid_layout((canvas.width as u32, canvas.height as u32));
        let line = Text::line_height(px);
        for pos in self.grid_shown(&layout) {
            let id = self.list[pos];
            let area = self.grid_area(&layout, pos, canvas.height as u32);
            let size = self.loader.thumb(id).map(|thumb| (thumb.width, thumb.height));
            let [x, y, w, h] = size.map_or(area, |size| Self::grid_picture(area, size, false));
            // A dot in the corner of a video not watched yet.
            let new = self.new_level(id);
            if new > 0.0 {
                let (dot, inset) = ((px as i32 * 2 / 3).max(6), (px as i32 / 2).max(4));
                canvas.round_fill(x + inset - 2, y + inset - 2, dot + 4, dot + 4, dot / 2 + 2, 0x000000, (150.0 * new) as u32);
                canvas.round_fill(x + inset, y + inset, dot, dot, dot / 2, accent_soft(), (256.0 * new) as u32);
            }
            // What a triage has decided about it, in the other corner: a
            // tick on green for kept, a cross on red for rejected.
            if let Some(verdict) = self.verdict(id) {
                let (big, inset) = (px * 5 / 4, (px as i32 / 2).max(4));
                let (icon, color) = if verdict == Verdict::Kept { (commands::icons::CHECK_FILLED, KEPT_COLOR) } else { (commands::icons::X_FILLED, DANGER_COLOR) };
                let side = text.width(icon, big);
                let (left, top) = (x + w - side - inset, y + inset);
                // Dark behind it, so that the tick reads on any picture.
                canvas.round_fill(left + side / 8, top + side / 8, side - side / 4, side - side / 4, side / 2, 0x000000, 256);
                text.draw(canvas, left, top + (side - Text::line_height(big)) / 2, big, color, icon);
            }
            self.want_duration(id);
            if let Some(length) = self.durations.get(&id).copied().flatten().map(clock) {
                let pad = px as i32 / 3;
                let (badge_w, badge_h) = (text.width(&length, px) + 2 * pad, line + pad / 2);
                let (badge_x, badge_y) = (x + w - badge_w - pad, y + h - badge_h - pad);
                canvas.round_fill(badge_x, badge_y, badge_w, badge_h, pad, 0x000000, 190);
                text.draw(canvas, badge_x + pad, badge_y + pad / 4, px, TEXT_COLOR, &length);
            }
            if self.grid_titles {
                let title = clipped(text, &title_of(&self.files[id]), area[2], px);
                let left = area[0] + (area[2] - text.width(&title, px)) / 2;
                let color = if pos == cursor { TEXT_COLOR } else { DIM_TEXT_COLOR };
                text.draw(canvas, left, area[1] + area[3] + layout.pad / 2, px, color, &title);
            }
        }
        // A bar above the footer shows where among the pages this one is.
        if let (Some(grid), Some(bar)) = (&self.grid, self.grid_bar()) {
            let thumb = (bar.width / bar.pages as i32).max(px as i32 * 2).min(bar.width);
            let along = (grid.scroll() / (bar.pages - 1) as f64).clamp(0.0, 1.0);
            let thumb_left = bar.x + (f64::from(bar.width - thumb) * along).round() as i32;
            // Brighter with the pointer on it, or while it is dragged.
            let held = self.drag.as_ref().is_some_and(|drag| matches!(drag, Drag::Pages)) || bar.holds(self.mouse);
            canvas.round_fill(bar.x, bar.y, bar.width, bar.height, bar.height / 2, TEXT_COLOR, if held { 70 } else { 40 });
            canvas.round_fill(thumb_left, bar.y, thumb, bar.height, bar.height / 2, if held { accent_soft() } else { TEXT_COLOR }, if held { 256 } else { 200 });
        }
        // A cross at the top right leaves the grid.
        if let Some(place) = self.grid_close() {
            let over = (place.x..place.x + place.width).contains(&(self.mouse.0 as i32)) && (place.y..place.y + place.height).contains(&(self.mouse.1 as i32));
            canvas.round_fill(place.x, place.y, place.width, place.height, place.width / 2, if over { accent() } else { 0x000000 }, if over { 256 } else { 170 });
            canvas.round_outline(place.x, place.y, place.width, place.height, place.width / 2, 1, MENU_EDGE_COLOR);
            let cross = commands::icons::X;
            let inset = ((place.width - text.width(cross, px)) / 2, (place.height - Text::line_height(px)) / 2);
            text.draw(canvas, place.x + inset.0, place.y + inset.1, px, if over { on_accent() } else { TEXT_COLOR }, cross);
        }
        // The buttons for the pages either way, brighter under the pointer.
        let (mouse_x, mouse_y) = (self.mouse.0 as i32, self.mouse.1 as i32);
        for (place, forward) in self.grid_arrows() {
            let over = (place.x..place.x + place.width).contains(&mouse_x) && (place.y..place.y + place.height).contains(&mouse_y);
            canvas.round_fill(place.x, place.y, place.width, place.height, place.width / 2, if over { accent() } else { 0x000000 }, if over { 256 } else { 170 });
            canvas.round_outline(place.x, place.y, place.width, place.height, place.width / 2, 1, MENU_EDGE_COLOR);
            let (arrow, big) = (if forward { commands::icons::CHEVRON_RIGHT } else { commands::icons::CHEVRON_LEFT }, px * 3 / 2);
            let inset = ((place.width - text.width(arrow, big)) / 2, (place.height - Text::line_height(big)) / 2);
            text.draw(canvas, place.x + inset.0, place.y + inset.1, big, if over { on_accent() } else { TEXT_COLOR }, arrow);
        }
    }

    /// Draws the line under the grid. While images are marked it also has
    /// the count and a row of buttons for acting on them: each an icon,
    /// what it does, and its key.
    pub(super) fn draw_grid_footer(&mut self, canvas: &mut Canvas, text: &mut Text, footer: &[String], px: u32) {
        self.grid_buttons.clear();
        let Some(label) = footer.first() else { return };
        let (pad, gap) = (px as i32 / 2, px as i32 * 2);
        let height = Text::line_height(px) + pad;
        let actions = if self.marks.is_empty() { &[][..] } else { commands::GRID_ACTIONS };
        let count = format!("{} marked", self.marks.len());
        let small = (px * 5 / 6).max(11);

        // A button's parts: the icon, what it does, and the key for it.
        let parts = |text: &mut Text, label: &'static str, command: Command| {
            let (name, key) = label.split_once("  ").unwrap_or((label, ""));
            let icon = commands::command_icon(command);
            let widths = (text.width(icon, px), text.width(name, px), text.width(key, small));
            (icon, name, key, widths, 2 * pad + widths.0 + pad / 2 + widths.1 + pad + widths.2)
        };
        let mut width = text.width(label, px);
        if !actions.is_empty() {
            width += gap + text.width(&count, px) + gap - pad;
            width += actions.iter().map(|&(label, command)| parts(text, label, command).4 + pad).sum::<i32>();
        }
        let footer_height = GridLayout::footer_height(canvas.height as u32);
        // Nothing of the pictures shows behind this line.
        canvas.fill(0, canvas.height - footer_height, canvas.width, footer_height, GRID_COLOR);
        // At its left, the video that plays on while the grid is up.
        let taken = self.draw_now_playing(canvas, text, px, (canvas.width - width) / 2 - px as i32);
        // In a narrow window that would run under it: then it starts beside
        // it, and is cut short at the window's edge.
        let mut x = ((canvas.width - width) / 2).max(taken);
        let label = &clipped(text, label, canvas.width - x - pad, px);
        let y = canvas.height - footer_height + (footer_height - height) / 2;

        text.draw(canvas, x, y + pad / 2, px, TEXT_COLOR, label);
        if actions.is_empty() {
            return;
        }
        x += text.width(label, px) + gap;
        text.draw(canvas, x, y + pad / 2, px, MARK_COLOR, &count);
        x += text.width(&count, px) + gap;
        let (mouse_x, mouse_y) = (self.mouse.0 as i32, self.mouse.1 as i32);
        for &(label, command) in actions {
            let (icon, name, key, widths, width) = parts(text, label, command);
            let over = (x..x + width).contains(&mouse_x) && (y..y + height).contains(&mouse_y);
            // The one that cannot be taken back is marked as such.
            let danger = command == Command::Delete;
            let fill = if over && danger { DANGER_COLOR } else if over { accent() } else { PLACEHOLDER_COLOR };
            canvas.round_fill(x, y, width, height, height / 2, fill, 256);
            canvas.round_outline(x, y, width, height, height / 2, 1, if over { fill } else { MENU_EDGE_COLOR });
            let mut at = x + pad;
            let (main, dim) = row_text(over && !danger);
            text.draw(canvas, at, y + pad / 2, px, if danger && !over { DANGER_COLOR } else { main }, icon);
            at += widths.0 + pad / 2;
            text.draw(canvas, at, y + pad / 2, px, main, name);
            at += widths.1 + pad;
            let key_top = y + (height - Text::line_height(small)) / 2;
            text.draw(canvas, at, key_top, small, if over && danger { TEXT_COLOR } else { dim }, key);
            self.grid_buttons.push(Button { x, y, width, height, command });
            x += width + pad;
        }
    }

    /// The line under the grid: the image under the cursor and the page.
    pub(super) fn grid_footer(&self) -> Option<String> {
        let grid = self.grid.as_ref().filter(|_| self.overview.is_none())?;
        let path = &self.files[self.list[grid.cursor]];
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
        let layout = self.grid_layout(self.window_size());
        let pages = self.list.len().div_ceil(layout.per_page()).max(1);
        let mut footer = format!("{name}   [{}/{}]", grid.cursor + 1, self.list.len());
        if pages > 1 {
            footer.push_str(&format!("      Page {} of {pages}", grid.cursor / layout.per_page() + 1));
        }
        Some(footer)
    }

    /// Whether a file is an Ultra HDR photo, found from its first bytes
    /// once and remembered.
    pub(super) fn is_ultra_hdr(&self, id: usize) -> bool {
        if let Some(known) = self.ultra_hdr.borrow().get(&id) {
            return *known;
        }
        let path = &self.files[id];
        // The thread that decoded the file looked, and reading the file
        // here would hold up the frame; only a file not decoded yet is read.
        let found = match self.loader.sniffed(id) {
            Some((_, ultra_hdr)) => ultra_hdr,
            None => {
                files::is_jpeg(path) && {
                    use std::io::Read;
                    let mut head = Vec::new();
                    std::fs::File::open(path).and_then(|file| file.take(256 << 10).read_to_end(&mut head)).is_ok() && ultrahdr::is_ultra_hdr(&head)
                }
            }
        };
        self.ultra_hdr.borrow_mut().insert(id, found);
        found
    }

    /// An image's format, found from its first bytes once and remembered.
    pub(super) fn format_of(&self, id: usize) -> Option<String> {
        if let Some(known) = self.formats.borrow().get(&id) {
            return known.clone();
        }
        let path = &self.files[id];
        // As the thread that decoded the file found it, when it has; a
        // RAW file's name is from its ending, with nothing read.
        let found = match self.loader.sniffed(id).filter(|_| !files::is_raw(path)) {
            Some((format, _)) => format.map(str::to_string),
            None => files::format_name(path),
        };
        self.formats.borrow_mut().insert(id, found.clone());
        found
    }

    /// The codec of the video on view, as it is usually written.
    pub(super) fn codec_name(&self) -> Option<String> {
        let codec = self.player.as_ref()?.text("video-format")?;
        Some(match codec.as_str() {
            "hevc" => "HEVC".to_string(),
            "h264" => "H.264".to_string(),
            "av1" => "AV1".to_string(),
            "vp9" => "VP9".to_string(),
            "vp8" => "VP8".to_string(),
            "mpeg2video" => "MPEG-2".to_string(),
            other => other.to_uppercase(),
        })
    }

    /// The info overlay's second line: the dimensions, apart so that they
    /// can be clicked, then the zoom and the size on disk.
    pub(super) fn info_line(&mut self) -> (Option<String>, String) {
        let size = self.current_meta().and_then(|meta| meta.size).map(meta::human_size);
        let Some(native) = self.native() else { return (None, size.unwrap_or_default()) };
        // A video's frame rate, then how large it is drawn against its own
        // pixels: "at 141%".
        let mut line = String::new();
        let fps = self.video.as_ref().and(self.player.as_ref()).and_then(|player| player.number("container-fps")).filter(|fps| *fps > 0.0);
        if let Some(fps) = fps {
            line.push_str(&format!("   {} fps", fps_words(fps)));
        }
        let percent = self.view.scale(native, self.stage().1) * 100.0;
        line.push_str(&format!("   at {percent:.0}%"));
        if let Some(size) = size {
            line.push_str(&format!("   {size}"));
        }
        (Some(format!("{} x {}", native.0, native.1)), line)
    }

    /// The debug overlay, as groups of figures each with a name: what the
    /// player reports while a video is showing, otherwise what decoding
    /// and drawing the image cost.
    pub(super) fn debug_groups(&mut self) -> Vec<Group> {
        let perf = &self.perf;
        let mut drawing = vec![("paint", ms(perf.paint)), ("present", ms(perf.present))];
        // What the graphs under the overlay cost, counting and drawing.
        if self.scopes.contains(&true) {
            drawing.push(("graphs", ms(self.scope_cost)));
        }
        if self.video.as_ref().is_some_and(|video| video.showing) {
            // Asking the player on every frame would get in its way, and
            // the figures would change too fast to read.
            let fresh = perf.video_lines.as_ref().is_some_and(|(at, _)| at.elapsed() < VIDEO_STATS_EVERY);
            if !fresh {
                let groups = self.video_debug_groups();
                self.perf.video_lines = Some((Instant::now(), groups));
            }
            let perf = &self.perf;
            let mut groups = perf.video_lines.as_ref().map(|(_, groups)| groups.clone()).unwrap_or_default();
            // The player holds a frame back until it is due, so only a
            // paused video shows what drawing one costs by itself.
            let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
            drawing.insert(0, (if paused { "render" } else { "render + wait" }, ms(perf.render)));
            if let Some(rate) = perf.rate() {
                drawing.push(("drawn", format!("{rate:.1} fps")));
            }
            groups.push(("Drawing", drawing));
            return groups;
        }

        let stats = self.loader.stats(self.id());
        let mut groups = Vec::new();
        if let (Some(made), Some(native)) = (stats.fitted, self.native()) {
            let megapixels = f64::from(native.0) * f64::from(native.1) / 1e6;
            let mut decode = Vec::new();
            if made.how.is_empty() {
                let rate = megapixels / made.read.as_secs_f64().max(1e-6);
                decode.push(("total", ms(made.read + made.fit)));
                decode.push(("read", ms(made.read)));
                decode.push(("fit", ms(made.fit)));
                decode.push(("speed", format!("{rate:.0} MP/s")));
            } else {
                decode.push(("total", format!("{} ({})", ms(made.fit), made.how)));
            }
            decode.push(("size", format!("{megapixels:.1} MP")));
            // Decoded ahead as a neighbor, or only once it was gone to.
            decode.push(("when", if made.at <= perf.arrived { "prefetched" } else { "on demand" }.to_string()));
            if let Some(full) = stats.full {
                let how = if full.how.is_empty() { String::new() } else { format!(" ({})", full.how) };
                decode.push(("full size", format!("{}{how}", ms(full.read + full.fit))));
            }
            groups.push(("Decode", decode));
        }
        if let Some(scaled) = &perf.scaled {
            let (from, to) = (scaled.from, scaled.to);
            drawing.insert(0, ("scale", format!("{} {}", ms(scaled.took), if scaled.smooth { "smooth" } else { "nearest" })));
            drawing.insert(1, ("from", format!("{} x {}", from.0, from.1)));
            drawing.insert(2, ("to", format!("{} x {}", to.0, to.1)));
        }
        if let Some(took) = perf.shown_in {
            drawing.push(("shown in", ms(took)));
        }
        groups.push(("Drawing", drawing));
        let kept = |(count, bytes): (usize, usize)| format!("{count} ({})", meta::human_size(bytes as u64));
        let memory = vec![("frames", kept(stats.fitted_kept)), ("thumbnails", kept(stats.thumbs_kept)), ("decoding", stats.busy.to_string())];
        groups.push(("Memory", memory));
        groups
    }

    /// What the player says about the video: the file, then how playback
    /// is keeping up.
    pub(super) fn video_debug_groups(&self) -> Vec<Group> {
        let (Some(player), Some(video)) = (&self.player, &self.video) else { return Vec::new() };
        let number = |property| player.number(property);
        let mut file = Vec::new();
        file.push(("codec", player.text("video-format").unwrap_or_default()));
        if let Some((w, h)) = number("width").zip(number("height")) {
            file.push(("size", format!("{w:.0} x {h:.0}")));
        }
        if let Some(format) = player.text("video-params/pixelformat") {
            file.push(("pixels", format));
        }
        if let Some(rate) = number("container-fps") {
            file.push(("rate", format!("{rate:.2} fps")));
        }
        if let Some(bits) = number("video-bitrate").filter(|bits| *bits > 0.0) {
            file.push(("bitrate", format!("{:.1} Mbps", bits / 1e6)));
        }
        let decoder = match player.text("hwdec-current").as_deref() {
            None | Some("no" | "") => "CPU".to_string(),
            Some(decoder) => decoder.to_string(),
        };
        // The player names every format its reader for this one takes, as
        // `mov,mp4,m4a`: the file's own ending says which, when it is one.
        if let Some(formats) = player.text("file-format") {
            let ending = self.files[video.id].extension().and_then(|ending| ending.to_str()).map(str::to_lowercase);
            let named = formats.split(',').find(|format| Some(*format) == ending.as_deref());
            let container = named.or(formats.split(',').next()).unwrap_or_default().to_string();
            file.push(("container", container));
        }
        if let Some(length) = number("duration") {
            file.push(("length", clock(length)));
        }
        file.push(("decoder", decoder));
        if let Some(took) = video.start_took {
            file.push(("start", ms(took)));
        }

        let mut audio = Vec::new();
        match player.text("audio-codec-name") {
            Some(codec) => audio.push(("codec", codec)),
            None => audio.push(("codec", "none".to_string())),
        }
        if let Some(channels) = number("audio-params/channel-count") {
            audio.push(("channels", format!("{channels:.0}")));
        }
        if let Some(rate) = number("audio-params/samplerate") {
            audio.push(("rate", format!("{:.1} kHz", rate / 1000.0)));
        }
        if let Some(bits) = number("audio-bitrate").filter(|bits| *bits > 0.0) {
            audio.push(("bitrate", format!("{:.0} kbps", bits / 1000.0)));
        }

        let mut playback = Vec::new();
        let count = |property| number(property).unwrap_or(0.0);
        playback.push(("late", format!("{:.0}", count("frame-drop-count"))));
        playback.push(("dropped", format!("{:.0}", count("vo-drop-frame-count"))));
        if let Some(sync) = number("avsync") {
            playback.push(("A-V", format!("{:+.0} ms", sync * 1000.0)));
        }
        if let Some(ahead) = number("demuxer-cache-duration") {
            playback.push(("read ahead", format!("{ahead:.1} s")));
        }
        if let Some(took) = video.seek_took {
            playback.push(("seek", ms(took)));
        }
        if self.speed != 1.0 {
            playback.push(("speed", format!("{}x", self.speed)));
        }
        vec![("Video", file), ("Audio", audio), ("Playback", playback)]
    }

    /// Details of the current file, or `None` while they are being read.
    /// Reading them means opening the file, which on a slow disk would
    /// hold up drawing, so it happens on another thread.
    pub(super) fn current_meta(&mut self) -> Option<&Meta> {
        let id = self.id();
        if self.meta.as_ref().is_some_and(|(meta_id, _)| *meta_id == id) {
            return self.meta.as_ref().map(|(_, meta)| meta);
        }
        if self.meta_asked != Some(id) {
            self.meta_asked = Some(id);
            let (path, proxy) = (self.files[id].clone(), self.proxy.clone());
            std::thread::spawn(move || drop(proxy.send_event(Event::Meta(id, meta::read(&path)))));
        }
        None
    }

    /// Where in the window the filmstrip's cell for a list position is:
    /// its left edge. The file on view sits in the middle.
    /// The list position in the middle of the filmstrip: the file on view,
    /// unless the wheel has moved the strip along to look further afield.
    pub(super) fn strip_center(&self) -> i64 {
        (self.pos as i64 + self.strip_scroll).clamp(0, self.list.len() as i64 - 1)
    }

    /// The round buttons in the top right corner of the strip along the
    /// bottom, the filmstrip or the scenes, as the panels have them: its
    /// close, and before it its pin, which has the picture make room for
    /// it rather than lie under it.
    pub(super) fn strip_buttons(&self) -> Vec<(StripButton, Button)> {
        let (width, height) = self.window_size();
        let strip = Strip::for_height(height);
        let size = ui_px(height) as i32 * 3 / 2;
        // Not while a triage has it up: it is part of that.
        let shown = self.grid.is_none() && (self.strip_visible() || self.scenes_shown()) && !self.triage_on;
        if !shown {
            return Vec::new();
        }
        // In the corner of the strip itself: over a video the seek bar and
        // its buttons stand just above it.
        let place = |at: i32| Button { x: width as i32 - (at + 1) * size - size / 2 - at * size / 3, y: strip.top + size / 3, width: size, height: size, command: Command::ToggleFilmstrip };
        vec![(StripButton::Close, place(0)), (StripButton::Pin, place(1))]
    }

    /// Which of the strip's buttons is at a window position.
    pub(super) fn strip_button_at(&self, x: f64, y: f64) -> Option<StripButton> {
        let (x, y) = (x as i32, y as i32);
        self.strip_buttons().into_iter().find(|(_, b)| (b.x..b.x + b.width).contains(&x) && (b.y..b.y + b.height).contains(&y)).map(|(which, _)| which)
    }

    pub(super) fn strip_left(&self, strip: &Strip, pos: usize, window_width: i32) -> i32 {
        let middle = window_width / 2 - strip.cell_w / 2;
        // The strip slides along to a new middle rather than jumping to it.
        let center = self.glide(&self.strip_slide, self.strip_center() as f64, EASE);
        middle + ((pos as f64 - center) * f64::from(strip.pitch())).round() as i32
    }

    /// The list position of the filmstrip cell at a window position.
    pub(super) fn strip_cell_at(&self, x: f64, y: f64) -> Option<usize> {
        let (width, height) = self.window_size();
        let strip = Strip::for_height(height);
        if self.grid.is_some() || !self.strip_visible() || y < f64::from(strip.top) {
            return None;
        }
        let first_cell = f64::from(width as i32 / 2 - strip.cell_w / 2 - strip.pad / 2);
        let pos = self.strip_center() + ((x - first_cell) / f64::from(strip.pitch())).floor() as i64;
        (0..self.list.len() as i64).contains(&pos).then_some(pos as usize)
    }

    /// A thumbnail at the size a filmstrip cell takes it, made once and
    /// kept; the thumbnail as it is when the cell is no smaller.
    pub(super) fn strip_thumb(&self, id: usize, thumb: &Arc<Frame>, (w, h): (u32, u32)) -> Arc<Frame> {
        if w >= thumb.width && h >= thumb.height {
            return thumb.clone();
        }
        let mut kept = self.strip_thumbs.borrow_mut();
        // The one kept is for this very thumbnail, not one since redone.
        if let Some((_, small)) = kept.get(&(id, w, h)).filter(|(from, _)| Arc::ptr_eq(from, thumb)) {
            return small.clone();
        }
        // A couple of hundred cells' worth is some megabytes; past that it
        // is started afresh rather than grown without end.
        if kept.len() >= 200 {
            kept.clear();
        }
        let small = Arc::new(crate::decode::shrink(thumb, (w, h)).unwrap_or_else(|_| Frame::still(thumb.width, thumb.height, thumb.native, thumb.pixels.clone())));
        kept.insert((id, w, h), (thumb.clone(), small.clone()));
        small
    }

    pub(super) fn draw_strip(&self, canvas: &mut Canvas, strip: Strip) {
        canvas.shade(0, strip.top, canvas.width, strip.height, 64);
        let reach = i64::from(canvas.width / 2 / strip.pitch() + 1);
        let top = strip.top + strip.pad;
        let round = (strip.pad * 2 / 3).max(3);
        let edge = (strip.pad / 4).max(2);

        for offset in -reach..=reach {
            let pos = self.strip_center() + offset;
            if !(0..self.list.len() as i64).contains(&pos) {
                continue;
            }
            let pos = pos as usize;
            let left = self.strip_left(&strip, pos, canvas.width);
            // Where the picture goes in its cell, for what is drawn round it.
            let [x, y, w, h] = match self.loader.thumb(self.list[pos]) {
                Some(thumb) => {
                    let bounds = (strip.cell_w as u32, strip.cell_h as u32);
                    let (w, h) = fit_size((thumb.width, thumb.height), bounds, false);
                    let small = self.strip_thumb(self.list[pos], &thumb, (w, h));
                    let (w, h) = (w as i32, h as i32);
                    let place = [left + (strip.cell_w - w) / 2, top + (strip.cell_h - h) / 2, w, h];
                    canvas.image_rounded(&small, place[0], place[1], w, h, round);
                    place
                }
                None => {
                    canvas.round_fill(left, top, strip.cell_w, strip.cell_h, round, PLACEHOLDER_COLOR, 256);
                    [left, top, strip.cell_w, strip.cell_h]
                }
            };
            if self.marks.contains(&self.list[pos]) {
                let dot = strip.pad;
                canvas.round_fill(x + dot / 3, y + dot / 3, dot, dot, dot / 2, MARK_COLOR, 256);
            }
            // The file on view is ringed brightly, the one under the
            // pointer quietly.
            let ring = if pos == self.pos { Some(accent()) } else { (self.strip_over == Some(pos)).then_some(accent_soft()) };
            if let Some(color) = ring {
                // With a little gap between it and the picture, as the grid's.
                let gap = edge * 2;
                canvas.round_outline(x - gap, y - gap, w + 2 * gap, h + 2 * gap, round + gap, edge, color);
            }
        }
    }

    /// The cross in the corner of the filmstrip, which closes it.
    pub(super) fn draw_strip_buttons(&self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let over = self.strip_button_at(self.mouse.0, self.mouse.1);
        for (which, place) in self.strip_buttons() {
            let (icon, on) = match which {
                StripButton::Close => (commands::icons::X, false),
                StripButton::Pin => (commands::icons::PIN, self.strip_docked),
            };
            // Over the picture, unless pinned below it.
            round_button(canvas, text, &place, icon, px, over == Some(which), on, !self.band_docked());
        }
    }

    /// In the corner of each video in the filmstrip, how long it is, as the
    /// grid has it.
    pub(super) fn draw_strip_lengths(&mut self, canvas: &mut Canvas, text: &mut Text, strip: &Strip, px: u32) {
        let small = (px * 3 / 4).max(10);
        let reach = i64::from(canvas.width / 2 / strip.pitch() + 1);
        let (top, pad) = (strip.top + strip.pad, (small as i32 / 4).max(2));
        for offset in -reach..=reach {
            let pos = self.strip_center() + offset;
            if !(0..self.list.len() as i64).contains(&pos) {
                continue;
            }
            let id = self.list[pos as usize];
            // A dot in the corner of a video not watched yet.
            let new = self.new_level(id);
            if new > 0.0 {
                let (dot, inset) = ((small as i32 / 2).max(5), (small as i32 / 3).max(3));
                let left = self.strip_left(strip, pos as usize, canvas.width) + (strip.cell_w - self.loader.thumb(id).map_or(strip.cell_w, |thumb| {
                    fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false).0 as i32
                })) / 2;
                canvas.round_fill(left + inset - 1, top + inset - 1, dot + 2, dot + 2, dot / 2 + 1, 0x000000, (150.0 * new) as u32);
                canvas.round_fill(left + inset, top + inset, dot, dot, dot / 2, accent_soft(), (256.0 * new) as u32);
            }
            // What a triage has decided about it, in its top right corner:
            // a tick or a cross on a filled circle, as in the grid.
            if let Some(verdict) = self.verdict(id) {
                let (icon, color) = if verdict == Verdict::Kept { (commands::icons::CHECK_FILLED, KEPT_COLOR) } else { (commands::icons::X_FILLED, DANGER_COLOR) };
                let across = self.loader.thumb(id).map_or(strip.cell_w, |thumb| fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false).0 as i32);
                let down = self.loader.thumb(id).map_or(strip.cell_h, |thumb| fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false).1 as i32);
                let (side, inset) = (text.width(icon, px), (small as i32 / 3).max(3));
                let left = self.strip_left(strip, pos as usize, canvas.width) + (strip.cell_w + across) / 2 - side - inset;
                let badge_top = top + (strip.cell_h - down) / 2 + inset;
                // Dark behind it, so that the tick reads on any picture.
                canvas.round_fill(left + side / 8, badge_top + side / 8, side - side / 4, side - side / 4, side / 2, 0x000000, 256);
                text.draw(canvas, left, badge_top + (side - Text::line_height(px)) / 2, px, color, icon);
            }
            self.want_duration(id);
            let Some(length) = self.durations.get(&id).copied().flatten().map(clock) else { continue };
            // The corner of the picture, which may be narrower than its cell.
            let left = self.strip_left(strip, pos as usize, canvas.width);
            let size = self.loader.thumb(id).map_or((strip.cell_w as u32, strip.cell_h as u32), |thumb| {
                fit_size((thumb.width, thumb.height), (strip.cell_w as u32, strip.cell_h as u32), false)
            });
            let (w, h) = (size.0 as i32, size.1 as i32);
            let (right, bottom) = (left + (strip.cell_w + w) / 2, top + (strip.cell_h + h) / 2);
            let (badge_w, badge_h) = (text.width(&length, small) + 2 * pad, Text::line_height(small));
            let (badge_x, badge_y) = (right - badge_w - pad, bottom - badge_h - pad);
            canvas.round_fill(badge_x, badge_y, badge_w, badge_h, pad, 0x000000, 190);
            text.draw(canvas, badge_x + pad, badge_y, small, TEXT_COLOR, &length);
        }
    }

    /// Over the filmstrip cell under the pointer: what the file is called
    /// and where in the list it is.
    pub(super) fn draw_strip_tip(&mut self, canvas: &mut Canvas, text: &mut Text, strip: &Strip, px: u32) {
        let Some(pos) = self.strip_over else { return };
        let id = self.list[pos];
        let tip = format!("{}   {}/{}", title_of(&self.files[id]), pos + 1, self.list.len());
        let pad = px as i32 / 2;
        let tip = clipped(text, &tip, canvas.width - 4 * pad, px);
        let (width, height) = (text.width(&tip, px) + 2 * pad, Text::line_height(px) + pad);
        let center = self.strip_left(strip, pos, canvas.width) + strip.cell_w / 2;
        let left = (center - width / 2).clamp(pad, (canvas.width - width - pad).max(pad));
        let top = strip.top - height - pad / 2;
        panel(canvas, left, top, width, height, pad);
        text.draw(canvas, left + pad, top + pad / 2, px, TEXT_COLOR, &tip);
    }

    /// Where the file list goes, when it is open: down the left side, above
    /// the filmstrip, with the file on view kept in the middle.
    pub(super) fn list_panel(&self) -> Option<ListPanel> {
        // In Inspect's Files tab, in the part of it that tab has.
        if let Some([left, head, width, bottom]) = self.inspector_list.get().filter(|_| self.files_tab_up()) {
            let px = ui_px(self.window_size().1);
            let row_height = Text::line_height(px) + px as i32 / 2;
            let top = head + Text::line_height(px) * 2 + px as i32 * 3;
            let rows = (((bottom - top) / row_height).max(1) as usize).min(self.list.len());
            let scrolled = self.list_scroll.filter(|(pos, len, _)| (*pos, *len) == (self.pos, self.list.len()));
            let first = scrolled.map_or(self.pos.saturating_sub(rows / 2), |(_, _, first)| first).min(self.list.len() - rows);
            return Some(ListPanel { left, width, bottom, top, row_height, first, rows, px, head, embedded: true });
        }
        if !self.show_list || self.grid.is_some() {
            return None;
        }
        // Popped out, it is in its own window, across the whole of it, and
        // not in the main one at all.
        let out = self.drawing_popout == Some(Popped::List);
        if out != self.popped(Popped::List) {
            return None;
        }
        let main = self.window_size();
        let (width, height) = if out { self.popout_size().unwrap_or(main) } else { main };
        let px = ui_px(main.1);
        let bottom = if !out && self.band_shown() { Strip::for_height(height).top } else { height as i32 };
        let row_height = Text::line_height(px) + px as i32 / 2;
        // Above the rows: a heading, and under it the buttons for the order.
        let top = Text::line_height(px) * 2 + px as i32 * 3;
        let rows = (((bottom - top) / row_height).max(1) as usize).min(self.list.len());
        let scrolled = self.list_scroll.filter(|(pos, len, _)| (*pos, *len) == (self.pos, self.list.len()));
        let first = scrolled.map_or(self.pos.saturating_sub(rows / 2), |(_, _, first)| first).min(self.list.len() - rows);
        let panel_width = if out { width as i32 } else { panel_width(width as i32, px, self.list_units) };
        let left = if self.list_right { width as i32 - panel_width } else { 0 };
        Some(ListPanel { left, width: panel_width, bottom, top, row_height, first, rows, px, head: 0, embedded: false })
    }

    /// Asks for the length of a video, once; `Event::Duration` brings it.
    /// One thread asks ffprobe about each in turn.
    pub(super) fn want_duration(&mut self, id: usize) {
        if !files::is_video(&self.files[id]) || self.durations.contains_key(&id) {
            return;
        }
        self.durations.insert(id, None);
        let sender = self.prober.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel::<(usize, PathBuf)>();
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                for (id, path) in receiver {
                    let arguments = ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"];
                    let output = std::process::Command::new("ffprobe").args(arguments).arg(&path).output();
                    let seconds = output.ok().and_then(|output| String::from_utf8_lossy(&output.stdout).trim().parse().ok());
                    if proxy.send_event(Event::Duration(id, seconds)).is_err() {
                        return;
                    }
                }
            });
            sender
        });
        let _ = sender.send((id, self.files[id].clone()));
    }

    /// The video whose frames the go-to box lays out: the match picked out.
    pub(super) fn story_subject(&self) -> Option<usize> {
        let palette = self.palette.as_ref()?;
        let id = *palette.matches.get(palette.selected).filter(|_| palette.commands.is_empty())?;
        files::is_video(&self.files[id]).then_some(id)
    }

    /// What a trailer is played of: in the grid, the video under its cursor.
    pub(super) fn trailer_subject(&self) -> Option<usize> {
        // With the strip of scenes up, the scene under the pointer: it goes
        // by a number no file has, counted down from the largest there is.
        if self.scenes_shown() {
            return self.scene_at(self.mouse.0, self.mouse.1).map(|index| usize::MAX - index);
        }
        let grid = self.grid.as_ref().filter(|grid| grid.moved && self.palette.is_none() && self.overview.is_none())?;
        let id = self.list[grid.cursor];
        // Not of the one that is playing in its cell as it is.
        let playing = self.pip && self.video.as_ref().is_some_and(|video| video.id == id);
        (files::is_video(&self.files[id]) && !playing).then_some(id)
    }

    /// Has the trailer thread play the video under the grid's cursor, and
    /// stop when the cursor leaves it. Called on each redraw. Nothing here
    /// waits on the player: the thread does all of that.
    /// With `--verbose` or `--debug`, says on the terminal what has become
    /// of a trailer: waited for, skipped, started, showing or stopped.
    pub(super) fn trailer_note(&self, what: &str, id: usize, more: &str) {
        if self.options.verbose || self.options.debug {
            // A scene of the video on view goes by its place in the strip.
            let name = if id < self.files.len() { self.name_of(id) } else { format!("scene {}", usize::MAX - id + 1) };
            eprintln!("jog: [trailer] {what:<8} {name}{more}");
        }
    }

    pub(super) fn tend_trailer(&mut self) {
        let subject = self.trailer_subject();
        if self.trailer.as_ref().map(|trailer| trailer.id) == subject {
            self.trailer_due = None;
            return;
        }
        if let Some(was) = self.trailer.take() {
            if let Some(trailers) = &self.trailers {
                trailers.stop();
            }
            self.trailer_note("stopped", was.id, "");
        }
        // The cursor left a video it had not rested on long enough.
        if let Some((due, _)) = self.trailer_due.filter(|(due, _)| Some(*due) != subject) {
            self.trailer_note("skipped", due, " (the cursor moved on)");
        }
        let Some(id) = subject else { return self.trailer_due = None };
        // Not for a video the cursor is only passing over: opening one
        // is work, and a trailer flashing up for a moment is a nuisance.
        match self.trailer_due {
            Some((due, at)) if due == id && at <= Instant::now() => self.trailer_due = None,
            Some((due, _)) if due == id => return,
            _ => {
                self.trailer_note("waiting", id, &format!(" {} for the cursor to rest", ms(TRAILER_DELAY)));
                return self.trailer_due = Some((id, Instant::now() + TRAILER_DELAY));
            }
        }
        let proxy = self.proxy.clone();
        let window = self.window_size();
        // A scene of the video on view plays on from where it starts, in
        // its cell of the strip; a video in the grid as a trailer of the
        // whole of it, in its cell there.
        let scene = self.scenes_now().and_then(|(_, starts, _)| starts.get(usize::MAX - id).copied()).zip(self.video.as_ref().map(|video| video.id));
        let (file, bounds, from) = match scene {
            Some((start, file)) => {
                let strip = Strip::for_height(window.1);
                (file, (strip.cell_w as u32, strip.cell_h as u32), Some(start))
            }
            None if id < self.files.len() => (id, self.grid_layout(window).thumb_size(), None),
            None => return,
        };
        let trailers = self.trailers.get_or_insert_with(|| Trailers::new(move |note| proxy.send_event(Event::Trailer(note)).is_ok()));
        trailers.play(id, self.files[file].clone(), bounds, from);
        self.trailer = Some(Trailer { id, frame: None, since: None, asked: Instant::now() });
        self.trailer_note("started", id, "");
    }

    /// Sees to it that the frames of the video picked out in the go-to box
    /// are on their way. Called on each redraw.
    pub(super) fn tend_story(&mut self) {
        let Some(id) = self.story_subject() else { return };
        if !self.story_frames.contains_key(&id) {
            // Frames are kept for the last few videos only.
            self.story_order.push_back(id);
            if self.story_order.len() > STORIES_KEPT {
                let oldest = self.story_order.pop_front();
                oldest.map(|oldest| self.story_frames.remove(&oldest));
            }
            self.story_frames.insert(id, vec![None; story::SEGMENTS]);
        }
        let frames = &self.story_frames[&id];
        let missing: Vec<usize> = (0..story::SEGMENTS).filter(|index| frames[*index].is_none()).collect();
        if !missing.is_empty() && self.story_asked != Some(id) {
            self.story_asked = Some(id);
            let proxy = self.proxy.clone();
            let stories = self.stories.get_or_insert_with(|| Stories::new(move |found| proxy.send_event(Event::Story(found)).is_ok()));
            stories.want(id, self.files[id].clone(), missing);
        }
    }

    /// The list position of the file-list row at a window position.
    pub(super) fn list_row_at(&self, x: f64, y: f64) -> Option<usize> {
        let panel = self.list_panel().filter(|panel| panel.holds(x, y))?;
        let row = (y as i32 - panel.top).div_euclid(panel.row_height);
        (0..panel.rows as i32).contains(&row).then(|| panel.first + row as usize)
    }


    /// The buttons at the right of the file list's heading: sort, shuffle,
    /// and close.
    pub(super) fn list_buttons(&self) -> Vec<Button> {
        let Some(panel) = self.list_panel() else { return Vec::new() };
        let px = panel.px as i32;
        // The cross, at the right of the heading.
        let side = px * 3 / 2;
        let mut buttons = vec![Button { x: panel.width - side - px / 2, y: panel.head + px / 2, width: side, height: side, command: Command::ToggleList }];
        // Beside it, the search: a button that opens the box to type in,
        // or, with the list narrowed, the one that widens it again.
        let beside = if self.list_searching() { Command::ShowAllHere } else { Command::ListSearch };
        buttons.push(Button { x: panel.width - 2 * side - px / 2 - px / 3, y: panel.head + px / 2, width: side, height: side, command: beside });
        // Then the pin, which stands the list beside the picture and not
        // over it, and the one that moves it to the other side.
        // And the loop, for the whole list: off, this video, or all of them.
        for (at, command) in [(2, Command::ListPin), (3, Command::ListFlip), (4, Command::CycleLoop), (5, Command::ListPopOut)] {
            buttons.push(Button { x: panel.width - (at + 1) * side - px / 2 - at * px / 3, y: panel.head + px / 2, width: side, height: side, command });
        }
        // The orders, sharing the width of the row under the heading.
        let orders = [Command::SortBy(Sort::Name), Command::SortBy(Sort::Newest), Command::SortBy(Sort::Smallest), Command::Shuffle, Command::ReverseOrder];
        let (gap, height) = (px / 3, Text::line_height(panel.px) + px / 4);
        let width = (panel.width - px - gap * (orders.len() as i32 - 1)) / orders.len() as i32;
        let top = panel.top - px / 2 - px / 2 - height;
        buttons.extend(orders.into_iter().enumerate().map(|(at, command)| Button { x: px / 2 + at as i32 * (width + gap), y: top, width, height, command }));
        // In Inspect's Files tab, the list is a tab: what closes, pins,
        // flips or pops it out is Inspect's to do. Search and loop stay, in
        // the corner.
        if panel.embedded {
            let side = px * 3 / 2;
            let mut round = 0;
            buttons.retain(|button| !matches!(button.command, Command::ToggleList | Command::ListPin | Command::ListFlip | Command::ListPopOut));
            for button in &mut buttons {
                if matches!(button.command, Command::ListSearch | Command::ShowAllHere | Command::CycleLoop) {
                    button.x = panel.width - (round + 1) * side - px / 2 - round * px / 3;
                    round += 1;
                }
            }
        }
        buttons
    }

    /// The list position whose star, at the right end of its row, is at a
    /// window position.
    pub(super) fn list_star_at(&self, x: f64, y: f64) -> Option<usize> {
        let panel = self.list_panel()?;
        let star = (panel.width - panel.px as i32 * 2..panel.width).contains(&(x as i32 - panel.left));
        self.list_row_at(x, y).filter(|_| star)
    }

    /// The list position whose menu chevron, shown on the row under the
    /// pointer before its star (and a video's loop), is at a window position.
    pub(super) fn list_more_at(&self, x: f64, y: f64) -> Option<usize> {
        let panel = self.list_panel()?;
        let room = panel.px as i32 * 2;
        let pos = self.list_row_at(x, y)?;
        let before = if files::is_video(&self.files[self.list[pos]]) { 2 } else { 1 };
        let right = panel.width - room * before;
        (right - room..right).contains(&(x as i32 - panel.left)).then_some(pos)
    }

    /// The list position of the video whose loop, just before the star, is
    /// at a window position.
    pub(super) fn list_loop_at(&self, x: f64, y: f64) -> Option<usize> {
        let panel = self.list_panel()?;
        let room = panel.px as i32 * 2;
        let on = (panel.width - room * 2..panel.width - room).contains(&(x as i32 - panel.left));
        self.list_row_at(x, y).filter(|pos| on && files::is_video(&self.files[self.list[*pos]]))
    }

    pub(super) fn draw_list(&self, canvas: &mut Canvas, text: &mut Text, panel: &ListPanel) {
        let px = panel.px;
        let pad = px as i32 / 2;
        // The pointer, as measured from where the list stands.
        let mouse = (self.mouse.0 as i32 - panel.left, self.mouse.1 as i32);
        // Pinned, it is beside the picture and has a ground of its own;
        // otherwise it lies over the picture, which shows through darkly.
        match self.list_pinned {
            _ if panel.embedded => {}
            true => canvas.fill(0, 0, panel.width, panel.bottom, GRID_COLOR),
            false => canvas.shade(0, 0, panel.width, panel.bottom, 36),
        }
        // A line down the side that faces the picture: in the accent, and
        // wider, while the list has the arrow keys.
        if !panel.embedded {
            let (width, color) = if self.list_keys { (2, accent()) } else { (1, MENU_EDGE_COLOR) };
            canvas.fill(if self.list_right { 0 } else { panel.width - width }, 0, width, panel.bottom, color);
        } else if self.list_keys {
            canvas.fill(0, panel.top, 2, panel.bottom - panel.top, accent());
        }

        // The heading: how many files, and what they are narrowed to, with
        // a cross at its right to close the list. Under it a row of
        // buttons for the order: the one in force filled in, like the
        // buttons under the grid in look, and lighting up under the pointer.
        let buttons = self.list_buttons();
        let small = (px * 5 / 6).max(11);
        match self.list_field() {
            // The search box: a magnifying glass, what is typed, and at its
            // right how many files that leaves of how many.
            Some([x, y, w, h]) => {
                let typing = self.list_search.as_ref();
                canvas.round_fill(x, y, w, h, h / 2, 0x111111, 256);
                let (edge, color) = if typing.is_some() { (2, accent()) } else { (1, MENU_EDGE_COLOR) };
                canvas.round_outline(x, y, w, h, h / 2, edge, color);
                let glass = commands::icons::SEARCH;
                let line = y + (h - Text::line_height(px)) / 2;
                text.draw(canvas, x + pad, line, px, DIM_TEXT_COLOR, glass);
                let left = x + pad + text.width(glass, px) + pad;
                let count = self.filter.as_ref().map(|(_, all)| format!("{} of {}", self.list.len(), all.len())).unwrap_or_default();
                let count_left = x + w - pad - text.width(&count, small);
                text.draw(canvas, count_left, y + (h - Text::line_height(small)) / 2, small, DIM_TEXT_COLOR, &count);
                let room = count_left - left - pad;
                match typing {
                    Some(field) => {
                        draw_selection(canvas, text, field, left, line, px);
                        let typed = clipped(text, &field.string(), room, px);
                        text.draw(canvas, left, line, px, TEXT_COLOR, &typed);
                        let before: String = field.text[..field.cursor].iter().collect();
                        let cursor = left + text.width(&before, px).min(room);
                        canvas.fill(cursor, line, (px as i32 / 10).max(2), Text::line_height(px), TEXT_COLOR);
                    }
                    None => {
                        let typed = self.filter.as_ref().map(|(typed, _)| typed.clone()).unwrap_or_default();
                        let typed = clipped(text, &typed, room, px);
                        text.draw(canvas, left, line, px, TEXT_COLOR, &typed);
                    }
                }
            }
            None => {
                let heading = format!("{} files", self.list.len());
                let heading = clipped(text, &heading, panel.width - px as i32 * 10, px);
                text.draw_strong(canvas, pad * 2, panel.head + px as i32 * 3 / 4, px, TEXT_COLOR, &heading);
            }
        }
        canvas.fill(pad, panel.top - pad / 2, panel.width - 2 * pad, 1, MENU_EDGE_COLOR);
        for button in &buttons {
            let over = (button.x..button.x + button.width).contains(&mouse.0) && (button.y..button.y + button.height).contains(&mouse.1);
            let on = self.is_checked(button.command);
            if matches!(button.command, Command::ToggleList | Command::ListSearch | Command::ShowAllHere | Command::ListPin | Command::ListFlip | Command::CycleLoop | Command::ListPopOut) {
                // The round buttons at the head of every panel.
                let icon = match button.command {
                    Command::ListSearch => commands::icons::SEARCH,
                    Command::ShowAllHere => commands::icons::LIST_X,
                    Command::ListPin => commands::icons::PIN,
                    Command::ListFlip => commands::icons::REPLACE,
                    Command::CycleLoop if self.looping == Looping::File => commands::icons::REPEAT_1,
                    Command::CycleLoop => commands::icons::REPEAT,
                    Command::ListPopOut if self.popped(Popped::List) => commands::icons::POP_IN,
                    Command::ListPopOut => commands::icons::EXTERNAL_LINK,
                    _ => commands::icons::X,
                };
                // Out, its button is one to bring it in, not one lit.
                let on = on && button.command != Command::ListPopOut;
                round_button(canvas, text, button, icon, px, over, on, false);
                continue;
            }
            let lit = over || on;
            canvas.round_fill(button.x, button.y, button.width, button.height, button.height / 2, if lit { accent() } else { PLACEHOLDER_COLOR }, if over && !on { 200 } else { 256 });
            canvas.round_outline(button.x, button.y, button.width, button.height, button.height / 2, 1, if lit { accent() } else { KEY_EDGE_COLOR });
            let (icon, word) = match button.command {
                Command::SortBy(Sort::Newest) => (commands::icons::CLOCK, "Newest"),
                Command::SortBy(Sort::Smallest) => (commands::icons::WEIGHT, "Size"),
                Command::SortBy(_) => (commands::icons::A_TO_Z, "Name"),
                Command::Shuffle => (commands::icons::SHUFFLE, "Shuffle"),
                _ => (commands::icons::ARROW_UP_DOWN, "Reverse"),
            };
            // The word goes when there is no room for it beside the icon.
            let icon_width = text.width(icon, small);
            let word = if icon_width + pad / 2 + text.width(word, small) + pad <= button.width { word } else { "" };
            let content = icon_width + if word.is_empty() { 0 } else { pad / 2 + text.width(word, small) };
            let (left, top) = (button.x + (button.width - content) / 2, button.y + (button.height - Text::line_height(small)) / 2);
            let color = if lit { on_accent() } else { TEXT_COLOR };
            text.draw(canvas, left, top, small, color, icon);
            text.draw(canvas, left + icon_width + pad / 2, top, small, color, word);
        }


        let numbers = text.width(&self.list.len().to_string(), px);
        let star_room = px as i32 * 2;
        for row in 0..panel.rows {
            let pos = panel.first + row;
            let id = self.list[pos];
            let top = panel.top + row as i32 * panel.row_height;
            let (current, hovered) = (pos == self.pos, self.list_hover == Some(pos));
            // The row the arrows have picked out in a search is ringed and
            // lit, plainly enough to be found at a glance.
            let picked = self.list_cursor == Some(pos);
            // The row a menu is open for, filled softly.
            let targeted = self.row_target == Some(pos) && !picked;
            if current {
                canvas.round_fill(pad * 3 / 4, top + 1, panel.width - pad * 3 / 2, panel.row_height - 2, pad * 3 / 4, accent(), 256);
            } else if hovered {
                // It lights up over a moment rather than at once.
                if self.row_faded.replace(Some(pos)) != Some(pos) {
                    self.row_fade.set(Glide::at(0.0));
                }
                let lit = self.glide(&self.row_fade, 1.0, EASE);
                canvas.round_fill(pad * 3 / 4, top + 1, panel.width - pad * 3 / 2, panel.row_height - 2, pad * 3 / 4, TEXT_COLOR, (28.0 * lit) as u32);
            }
            if targeted && !current {
                canvas.round_fill(pad * 3 / 4, top + 1, panel.width - pad * 3 / 2, panel.row_height - 2, pad * 3 / 4, TEXT_COLOR, 34);
            }
            if picked {
                if !current {
                    canvas.round_fill(pad * 3 / 4, top + 1, panel.width - pad * 3 / 2, panel.row_height - 2, pad * 3 / 4, TEXT_COLOR, 56);
                }
                canvas.round_outline(pad * 3 / 4, top + 1, panel.width - pad * 3 / 2, panel.row_height - 2, pad * 3 / 4, 2, if current { TEXT_COLOR } else { accent_soft() });
            }
            let line = top + (panel.row_height - Text::line_height(px)) / 2;
            let (main, dim) = row_text(current);
            // At the right end a star: lit on a marked file, and offered,
            // faintly, on the row under the pointer, where a click marks it.
            let marked = self.marks.contains(&id);
            if marked || hovered {
                let star = commands::icons::STAR;
                let color = if marked && !current { MARK_COLOR } else if marked { main } else { dim };
                let star_left = panel.width - star_room + (star_room - pad - text.width(star, px)) / 2;
                text.draw(canvas, star_left, line, px, color, star);
            }
            // Before the star, on a video, its loop: lit on one that plays
            // again when it ends, and offered on the row under the pointer.
            let video = files::is_video(&self.files[id]);
            let loop_room = if video { star_room } else { 0 };
            // With every video looping, each says so.
            let looped = self.looped.contains(&id) || self.looping == Looping::File;
            if video && (looped || hovered) {
                let icon = commands::icons::REPEAT_1;
                let color = if looped && !current { accent_soft() } else if looped { main } else { dim };
                let icon_left = panel.width - star_room - loop_room + (loop_room - pad - text.width(icon, px)) / 2;
                text.draw(canvas, icon_left, line, px, color, icon);
            }
            let star_room = star_room + loop_room;
            // On the row under the pointer, a chevron to its menu: what can
            // be done with the file, without a right click.
            let more_room = if hovered { px as i32 * 2 } else { 0 };
            if hovered {
                let half = (px as i32 / 4).max(3);
                let cx = panel.width - star_room - more_room + (more_room - pad) / 2 + half / 2;
                let cy = top + panel.row_height / 2 - half / 2;
                let over = self.list_more_at(self.mouse.0, self.mouse.1) == Some(pos);
                if over {
                    canvas.round_fill(cx - half * 2, top + 2, half * 4, panel.row_height - 4, pad / 2, TEXT_COLOR, 40);
                }
                for row in 0..half {
                    canvas.fill(cx - half + row, cy + row, (half - row) * 2, 1, if over { main } else { dim });
                }
            }
            let star_room = star_room + more_room;
            // The video playing shows how far along it is: the time gone
            // by with its length, and a thin bar along the foot of its row.
            let along = self.video_time().filter(|_| current && self.video.as_ref().is_some_and(|video| video.id == id));
            if let Some((position, total)) = along {
                let (x, w) = (pad * 3 / 2, panel.width - pad * 3);
                let (bar_y, bar_h) = (top + panel.row_height - (px as i32 / 6).max(3) - 2, (px as i32 / 8).max(2));
                canvas.round_fill(x, bar_y, w, bar_h, bar_h / 2, main, 70);
                let done = (f64::from(w) * (position / total).clamp(0.0, 1.0)) as i32;
                canvas.round_fill(x, bar_y, done.max(bar_h), bar_h, bar_h / 2, main, 256);
            }
            // A video's length sits before it, once it is known.
            let length = self.durations.get(&id).copied().flatten().map(clock).unwrap_or_default();
            let length = match along {
                Some((position, total)) => format!("{} / {}", clock(position), clock(total)),
                None => length,
            };
            let length_width = if length.is_empty() { 0 } else { text.width(&length, px) + pad };
            text.draw(canvas, panel.width - star_room - length_width, line, px, dim, &length);
            // Before that, what a triage has decided about it: a tick for
            // kept, a cross for rejected.
            let verdict = self.verdict(id);
            let verdict_width = if verdict.is_some() { text.width(commands::icons::CHECK_FILLED, px) + pad } else { 0 };
            if let Some(verdict) = verdict {
                let (icon, color) = if verdict == Verdict::Kept { (commands::icons::CHECK_FILLED, KEPT_COLOR) } else { (commands::icons::X_FILLED, DANGER_COLOR) };
                text.draw(canvas, panel.width - star_room - length_width - verdict_width, line, px, if current { main } else { color }, icon);
            }
            let length_width = length_width + verdict_width;
            // A dot at the left edge for a video not watched yet.
            let new = self.new_level(id);
            if new > 0.0 {
                let dot = (px as i32 / 3).max(4);
                canvas.round_fill(pad * 5 / 4 - dot / 2, top + (panel.row_height - dot) / 2, dot, dot, dot / 2, if current { main } else { accent_soft() }, (256.0 * new) as u32);
            }
            // Its number in the list, set against the right of a column
            // wide enough for the largest, and then its name.
            let number = (pos + 1).to_string();
            let number_left = pad * 2 + numbers - text.width(&number, px);
            text.draw(canvas, number_left, line, px, dim, &number);
            let name_left = pad * 2 + numbers + pad;
            let name = clipped(text, &self.name_of(id), panel.width - star_room - length_width - name_left - pad, px);
            match current {
                true => text.draw_strong(canvas, name_left, line, px, main, &name),
                false => text.draw(canvas, name_left, line, px, main, &name),
            }
        }
    }

    pub(super) fn draw_menu(&self, canvas: &mut Canvas, text: &mut Text, menu: &Menu) {
        let px = menu.px;
        let pad = px as i32 / 2;
        panel(canvas, menu.x, menu.y, menu.width, menu.height, px as i32 * 2 / 3);
        if let Some(title) = &menu.title {
            text.draw(canvas, menu.x + pad, menu.y + pad, px, DIM_TEXT_COLOR, title);
        }

        for (index, row) in menu.rows.iter().enumerate() {
            let top = menu.y + row.top;
            let Some(item) = row.item else {
                canvas.fill(menu.x + pad, top + row.height / 2, menu.width - 2 * pad, 1, MENU_EDGE_COLOR);
                continue;
            };
            if menu.selected == Some(index) {
                canvas.round_fill(menu.x + pad / 2, top, menu.width - pad, row.height, pad / 2, accent(), 256);
            }
            let line = top + (row.height - Text::line_height(px)) / 2;
            // In the gutter: a tick on what is switched on, otherwise the
            // item's own small picture.
            let (main, dim) = row_text(menu.selected == Some(index));
            let (sign, color) = if self.is_checked(item.command) { (commands::icons::CHECK, main) } else { (commands::icon(item), dim) };
            let gutter = px as i32 * 2;
            let sign_left = menu.x + pad + (gutter - pad - text.width(sign, px)) / 2;
            text.draw(canvas, sign_left, line, px, color, sign);
            // The row picked, a weight up.
            match menu.selected == Some(index) {
                true => text.draw_strong(canvas, menu.x + pad + gutter, line, px, main, &self.menu_label(item)),
                false => text.draw(canvas, menu.x + pad + gutter, line, px, main, &self.menu_label(item)),
            }
            let keys_left = menu.x + menu.width - pad - text.width(item.keys, px);
            text.draw(canvas, keys_left, line, px, dim, item.keys);
        }
        if let Some(sub) = &menu.sub {
            self.draw_menu(canvas, text, sub);
        }
    }
}

/// A line of hints, as `↑↓ pick  ·  Esc close`, broken between them into
/// as many lines as it takes to fit `width`.
pub(super) fn wrap_hints(text: &mut Text, hints: &str, px: u32, width: i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for hint in hints.split("  ·  ") {
        match lines.last_mut() {
            Some(last) if text.width(&format!("{last}  ·  {hint}"), px) <= width => *last = format!("{last}  ·  {hint}"),
            _ => lines.push(hint.to_string()),
        }
    }
    lines
}

/// `line`, cut short with an ellipsis if it is wider than `width`.
pub(super) fn clipped(text: &mut Text, line: &str, width: i32, px: u32) -> String {
    if text.width(line, px) <= width {
        return line.to_string();
    }
    let mut kept: Vec<char> = line.chars().collect();
    loop {
        kept.pop();
        let shorter: String = kept.iter().chain(&['\u{2026}']).collect();
        if kept.is_empty() || text.width(&shorter, px) <= width {
            return shorter;
        }
    }
}

/// How wide a panel down the side is: the file list and the inspector
/// alike, `units` lines of text (thirty unless dragged), at most half the
/// window.
pub(super) fn panel_width(window_width: i32, px: u32, units: f64) -> i32 {
    ((f64::from(px) * units) as i32).min(window_width / 2)
}

/// The round button at the head of a panel or strip: pin, close and the
/// like, alike everywhere. Filled with the accent under the pointer or
/// while what it stands for is on; `glass` for one over a picture, dark
/// and see-through.
#[allow(clippy::too_many_arguments)]
pub(super) fn round_button(canvas: &mut Canvas, text: &mut Text, place: &Button, icon: &str, px: u32, over: bool, on: bool, glass: bool) {
    let lit = over || on;
    let (ground, alpha) = match (lit, glass) {
        (true, _) => (accent(), 256),
        (false, true) => (0x000000, 170),
        (false, false) => (PLACEHOLDER_COLOR, 256),
    };
    canvas.round_fill(place.x, place.y, place.width, place.height, place.width / 2, ground, alpha);
    let small = (px * 5 / 6).max(11);
    let inset = ((place.width - text.width(icon, small)) / 2, (place.height - Text::line_height(small)) / 2);
    text.draw(canvas, place.x + inset.0, place.y + inset.1, small, if lit { on_accent() } else { TEXT_COLOR }, icon);
}

/// The ground of a menu, a box or a window: a soft shadow under it, a
/// fine edge, and a light along the top, as though lit from above.
pub(super) fn panel(canvas: &mut Canvas, x: i32, y: i32, width: i32, height: i32, radius: i32) {
    canvas.soft_shadow(x, y, width, height, radius, (radius * 3 / 2).clamp(6, 16));
    canvas.round_fill(x, y, width, height, radius, MENU_COLOR, 256);
    canvas.round_outline(x, y, width, height, radius, 1, MENU_EDGE_COLOR);
    canvas.round_fill(x + radius, y + 1, (width - 2 * radius).max(0), 1, 0, 0xFFFFFF, 18);
}

/// Begins a run of words that is drawn as a badge: small, on a rounded
/// ground of the run's color.
pub(super) const BADGE: char = '\u{1}';

/// Begin a run that is a choice to be clicked, drawn as the buttons of
/// the debug overlay are: the second for the one chosen, filled in.
pub(super) const CHOICE: char = '\u{2}';
pub(super) const CHOSEN: char = '\u{3}';
/// Begins a run drawn as it is, but there to be clicked: the file's name
/// in the info overlay, which copies it.
pub(super) const LINK: char = '\u{4}';
/// Begins a run a weight up; after `LINK`, one there to be clicked too.
pub(super) const STRONG: char = '\u{5}';

/// The color of a badge for what a picture or video is.
pub(super) fn badge_color(name: &str) -> u32 {
    match name {
        "HDR10+" => 0x8E5BD6,
        "2094-50" => 0x2FA36B,
        "HLG" => 0xD9822B,
        "HDR10" | "PQ" => 0x3E7BD6,
        "Ultra HDR" => 0xC9A227,
        "HEVC" | "AV1" | "VP9" | "H.264" | "VP8" | "MPEG-2" => 0x505A66,
        "JPEG XL" | "AVIF" | "HEIC" | "HEIF" => 0x4C6A8C,
        _ => 0x6A6A6A,
    }
}

/// A run for a badge with this name.
pub(super) fn badge(name: &str) -> (String, u32) {
    (format!("{BADGE}{name}"), badge_color(name))
}

/// How wide a run is drawn: a badge or a choice with its ground and the
/// gap after it.
pub(super) fn run_width(text: &mut Text, run: &str, px: u32) -> i32 {
    let run = run.strip_prefix(LINK).map_or(run, |words| words);
    if let Some(words) = run.strip_prefix(STRONG) {
        return text.width_strong(words, px);
    }
    if let Some(words) = run.strip_prefix(LINK) {
        return text.width(words, px);
    }
    match run.strip_prefix([BADGE, CHOICE, CHOSEN]) {
        Some(name) => text.width(name, (px * 4 / 5).max(10)) + px as i32 * 3 / 2,
        None => text.width(run, px),
    }
}

/// Draws a run, a badge or a choice as one; gives back how wide it was
/// and, for a choice, where its button is.
pub(super) fn draw_run(canvas: &mut Canvas, text: &mut Text, x: i32, y: i32, px: u32, color: u32, run: &str) -> (i32, Option<[i32; 4]>) {
    if let Some(words) = run.strip_prefix(LINK).and_then(|words| words.strip_prefix(STRONG)) {
        text.draw_strong(canvas, x, y, px, color, words);
        let width = text.width_strong(words, px);
        return (width, Some([x, y, width, Text::line_height(px)]));
    }
    if let Some(words) = run.strip_prefix(STRONG) {
        text.draw_strong(canvas, x, y, px, color, words);
        return (text.width_strong(words, px), None);
    }
    if let Some(words) = run.strip_prefix(LINK) {
        text.draw(canvas, x, y, px, color, words);
        let width = text.width(words, px);
        return (width, Some([x, y, width, Text::line_height(px)]));
    }
    let Some(name) = run.strip_prefix([BADGE, CHOICE, CHOSEN]) else {
        text.draw(canvas, x, y, px, color, run);
        return (text.width(run, px), None);
    };
    let small = (px * 4 / 5).max(10);
    let (pad, line) = (px as i32 / 2, Text::line_height(px));
    let (w, h) = (text.width(name, small) + pad, Text::line_height(small) + pad / 3);
    let (left, top) = (x + pad / 2, y + (line - h) / 2);
    let place = match run.chars().next() {
        Some(BADGE) => {
            canvas.round_fill(left, top, w, h, h / 3, color, 200);
            None
        }
        // A choice, as the debug overlay's buttons: the one chosen filled.
        first => {
            let chosen = first == Some(CHOSEN);
            canvas.round_fill(left, top, w, h, h / 3, if chosen { accent() } else { PLACEHOLDER_COLOR }, 256);
            canvas.round_outline(left, top, w, h, h / 3, 1, if chosen { accent() } else { MENU_EDGE_COLOR });
            Some([left, top, w, h])
        }
    };
    let words = if run.starts_with(CHOSEN) { on_accent() } else { TEXT_COLOR };
    text.draw(canvas, left + pad / 2, top + (h - Text::line_height(small)) / 2, small, words, name);
    (run_width(text, run, px), place)
}

/// Draws lines of a function each on a small graph: a dark square with a
/// faint grid every `tick`, the axes running from 0 to `max`. A line is a
/// function from x to y, its color, and whether it is dashed.
#[allow(clippy::type_complexity)]
pub(super) fn plot(canvas: &mut Canvas, [x, y, w, h]: [i32; 4], max: (f64, f64), tick: f64, lines: &[(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)]) {
    canvas.fill(x, y, w, h, 0x0B0B0B);
    let mut at = tick;
    while at < max.0.max(max.1) - 1e-9 {
        if at < max.0 {
            canvas.fill(x + (at / max.0 * f64::from(w)) as i32, y, 1, h, 0x262626);
        }
        if at < max.1 {
            canvas.fill(x, y + h - 1 - (at / max.1 * f64::from(h)) as i32, w, 1, 0x262626);
        }
        at += tick;
    }
    let thick = (w / 150).max(2);
    for (line, color, dashed) in lines {
        let mut last: Option<i32> = None;
        for column in 0..w {
            let value = line((f64::from(column) + 0.5) / f64::from(w) * max.0);
            if !value.is_finite() || (*dashed && (column / (thick * 4)) % 2 == 1) {
                last = None;
                continue;
            }
            let row = h - 1 - (value / max.1 * f64::from(h)).round() as i32;
            // Joined to the last column's, so that a steep part has no gaps.
            let (from, to) = match last {
                Some(last) => (last.min(row), last.max(row)),
                None => (row, row),
            };
            last = Some(row);
            let (from, to) = (from.max(0), to.min(h - 1));
            if from <= to {
                canvas.fill(x + column, y + from, thick, to - from + thick.min(h - to), *color);
            }
        }
    }
}

/// Draws lines made of runs of words, each run in its own color, on a
/// darkened panel with its top-left corner at `at`. Returns where the
/// next thing down should start, and where each choice in it went, by
/// its name.
pub(super) fn rich_block(canvas: &mut Canvas, text: &mut Text, lines: &[Vec<(String, u32)>], px: u32, at: (i32, i32)) -> (i32, Vec<(String, [i32; 4])>) {
    if lines.is_empty() {
        return (at.1, Vec::new());
    }
    let pad = px as i32 / 2;
    let line_height = Text::line_height(px);
    let widths: Vec<i32> = lines.iter().map(|line| line.iter().map(|(run, _)| run_width(text, run, px)).sum()).collect();
    let width = widths.into_iter().max().unwrap_or(0) + 3 * pad;
    let height = line_height * lines.len() as i32 + 2 * pad;
    // See-through over the picture, with the panels' fine edge and light.
    canvas.round_fill(at.0, at.1, width, height, pad, 0x000000, 176);
    canvas.round_ring(at.0, at.1, width, height, pad, 1, 0xFFFFFF, 22);
    canvas.round_fill(at.0 + pad, at.1 + 1, (width - 2 * pad).max(0), 1, 0, 0xFFFFFF, 18);
    let mut choices = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let mut x = at.0 + 3 * pad / 2;
        for (run, color) in line {
            let (wide, place) = draw_run(canvas, text, x, at.1 + pad + index as i32 * line_height, px, *color, run);
            if let Some(place) = place {
                choices.push((run.chars().skip(1).filter(|c| *c != STRONG).collect(), place));
            }
            x += wide;
        }
    }
    (at.1 + height + pad, choices)
}

/// The sizes the debug overlay has grown to: its figures change all the
/// time, and a panel that changed width with each of them would jitter.
/// It only ever gets wider or taller, until it shows other clusters.
#[derive(Default)]
pub(super) struct DebugKeep {
    titles: Vec<&'static str>,
    columns: Vec<(i32, i32)>,
    rows: i32,
}

/// Draws the debug overlay: a panel of clusters side by side, each a
/// heading over its figures, the names muted and the values bright, and
/// under them buttons to copy the lot. Returns where the buttons went.
pub(super) fn draw_debug(canvas: &mut Canvas, text: &mut Text, groups: &[Group], px: u32, at: (i32, i32), keep: &mut DebugKeep, pointer: (i32, i32)) -> (Vec<Button>, i32) {
    if groups.is_empty() {
        return (Vec::new(), at.1);
    }
    let small = (px * 5 / 6).max(11);
    let (pad, row) = (px as i32, Text::line_height(small) + px as i32 / 6);
    // Each cluster is as wide as its widest name and its widest value.
    let mut columns: Vec<(i32, i32)> = groups
        .iter()
        .map(|(title, figures)| {
            let names = figures.iter().map(|(name, _)| text.width(name, small)).max().unwrap_or(0);
            let values = figures.iter().map(|(_, value)| text.width(value, small)).max().unwrap_or(0);
            (names, values.max(text.width(title, small) - names - pad))
        })
        .collect();
    let mut rows = groups.iter().map(|(_, figures)| figures.len()).max().unwrap_or(0) as i32 + 1;
    let titles: Vec<&'static str> = groups.iter().map(|(title, _)| *title).collect();
    if keep.titles == titles {
        for (now, was) in columns.iter_mut().zip(&keep.columns) {
            *now = (now.0.max(was.0), now.1.max(was.1));
        }
        rows = rows.max(keep.rows);
    }
    (keep.titles, keep.columns, keep.rows) = (titles, columns.clone(), rows);

    let buttons = [(commands::icons::COPY, "Copy", Command::CopyDebug), (commands::icons::BRACES, "Copy as JSON", Command::CopyDebugJson)];
    let button_height = row + pad / 3;
    let width = columns.iter().map(|(names, values)| names + pad + values).sum::<i32>() + pad * 2 * groups.len() as i32;
    let height = rows * row + button_height + 3 * pad;
    canvas.round_fill(at.0, at.1, width, height, pad / 2, 0x000000, 190);
    canvas.round_outline(at.0, at.1, width, height, pad / 2, 1, MENU_EDGE_COLOR);

    let mut x = at.0 + pad;
    for ((title, figures), (names, values)) in groups.iter().zip(columns) {
        let mut y = at.1 + pad;
        text.draw(canvas, x, y, small, accent_soft(), title);
        canvas.fill(x, y + row - row / 6, names + pad + values, 1, MENU_EDGE_COLOR);
        y += row;
        for (name, value) in figures {
            text.draw(canvas, x, y, small, DIM_TEXT_COLOR, name);
            text.draw(canvas, x + names + pad, y, small, TEXT_COLOR, value);
            y += row;
        }
        x += names + pad + values + 2 * pad;
    }

    // The buttons, along the bottom.
    let mut drawn = Vec::new();
    let (mut x, y) = (at.0 + pad, at.1 + height - pad - button_height);
    for (icon, label, command) in buttons {
        let words = format!("{icon}  {label}");
        let button_width = text.width(&words, small) + pad;
        let over = (x..x + button_width).contains(&pointer.0) && (y..y + button_height).contains(&pointer.1);
        canvas.round_fill(x, y, button_width, button_height, pad / 3, if over { accent() } else { PLACEHOLDER_COLOR }, 256);
        canvas.round_outline(x, y, button_width, button_height, pad / 3, 1, if over { accent() } else { MENU_EDGE_COLOR });
        let words_top = y + (button_height - Text::line_height(small)) / 2;
        text.draw(canvas, x + pad / 2, words_top, small, if over { on_accent() } else { TEXT_COLOR }, &words);
        drawn.push(Button { x, y, width: button_width, height: button_height, command });
        x += button_width + pad / 2;
    }
    (drawn, at.1 + height + pad / 2)
}

/// Draws lines of text on a darkened panel. `place` is given the panel size
/// and returns its top-left corner.
pub(super) fn text_block(canvas: &mut Canvas, text: &mut Text, lines: &[String], px: u32, place: impl Fn(i32, i32) -> (i32, i32)) {
    if lines.is_empty() {
        return;
    }
    let pad = px as i32 / 2;
    let line_height = Text::line_height(px);
    // One line is a pill, a little wider for its round ends.
    let single = lines.len() == 1;
    let side = if single { pad * 3 / 2 } else { pad };
    let width = lines.iter().map(|line| text.width(line, px)).max().unwrap_or(0) + 2 * side;
    let height = line_height * lines.len() as i32 + 2 * pad;
    let (x, y) = place(width, height);
    panel(canvas, x, y, width, height, if single { height / 2 } else { pad });
    for (index, line) in lines.iter().enumerate() {
        text.draw(canvas, x + side, y + pad + index as i32 * line_height, px, TEXT_COLOR, line);
    }
}

/// Draws the delete confirmation in the middle of the window: a warning
/// sign and the question, above a Cancel and a Delete button. The one that
/// Enter would press is filled in, Delete in red.
pub(super) fn draw_confirm(canvas: &mut Canvas, text: &mut Text, confirm: &Confirm, px: u32) -> [[i32; 4]; 2] {
    let mut drawn = [[0; 4]; 2];
    let pad = px as i32;
    let line_height = Text::line_height(px);
    let (sign, sign_px) = (commands::icons::TRIANGLE_ALERT, px * 3 / 2);
    let sign_width = text.width(sign, sign_px) + pad;
    let note = "This cannot be undone.";
    let button_width = px as i32 * 7;
    let button_height = line_height + pad;
    let buttons_width = 2 * button_width + pad;
    let words = text.width(&confirm.question, px).max(text.width(note, px));
    let width = ((sign_width + words).max(buttons_width) + 3 * pad).min(canvas.width);
    let height = 2 * line_height + button_height + 4 * pad;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);

    panel(canvas, left, top, width, height, pad);
    let words_left = left + 3 * pad / 2 + sign_width;
    text.draw(canvas, left + 3 * pad / 2, top + 3 * pad / 2, sign_px, DANGER_COLOR, sign);
    text.draw_strong(canvas, words_left, top + 3 * pad / 2, px, TEXT_COLOR, &confirm.question);
    text.draw(canvas, words_left, top + 3 * pad / 2 + line_height, px, DIM_TEXT_COLOR, note);

    // The buttons sit at the right, as the last thing read.
    let buttons_top = top + height - 3 * pad / 2 - button_height;
    let buttons_left = left + width - 3 * pad / 2 - buttons_width;
    let buttons = [("Cancel", !confirm.yes, accent()), ("Delete", confirm.yes, DANGER_COLOR)];
    for (index, (label, selected, color)) in buttons.into_iter().enumerate() {
        let button_left = buttons_left + index as i32 * (button_width + pad);
        let round = pad / 2;
        if selected {
            canvas.round_fill(button_left, buttons_top, button_width, button_height, round, color, 256);
        } else {
            canvas.round_fill(button_left, buttons_top, button_width, button_height, round, PLACEHOLDER_COLOR, 256);
            canvas.round_outline(button_left, buttons_top, button_width, button_height, round, 1, MENU_EDGE_COLOR);
        }
        let label_left = button_left + (button_width - text.width(label, px)) / 2;
        // Delete says what it does in red, even when it is not the one
        // Enter would press.
        let color = match (selected, index) {
            (true, 0) => on_accent(),
            (false, 1) => DANGER_COLOR,
            _ => TEXT_COLOR,
        };
        text.draw_strong(canvas, label_left, buttons_top + pad / 2, px, color, label);
        drawn[index] = [button_left, buttons_top, button_width, button_height];
    }
    drawn
}

/// Draws the rename box, centered with its bottom edge at `bottom`.
/// `label` names what is being typed and `note`, at the right, says
/// something about it, such as how many files match.
/// Shades the selected stretch of a text field whose text starts at
/// `(x, y)`, for the text to be drawn over.
pub(super) fn draw_selection(canvas: &mut Canvas, text: &mut Text, field: &TextField, x: i32, y: i32, px: u32) {
    let Some((from, to)) = field.selection() else { return };
    let mut upto = |at: usize| text.width(&field.text[..at].iter().collect::<String>(), px);
    let (left, right) = (upto(from), upto(to));
    canvas.fill(x + left, y, right - left, Text::line_height(px), accent());
}

/// Draws a box to type in, in the middle of the window: an icon and what
/// is being asked for, the text in a field of its own, and under it a
/// word on what Enter does.
pub(super) fn draw_prompt(canvas: &mut Canvas, text: &mut Text, icon: &str, title: &str, field: &TextField, hint: &str, px: u32) {
    let typed = field.string();
    let before_cursor: String = field.text[..field.cursor].iter().collect();
    let pad = px as i32;
    let line_height = Text::line_height(px);
    let small = (px * 5 / 6).max(11);
    let heading = format!("{icon}  {title}");
    let hint = format!("{hint}  ·  Esc to cancel");
    let widest = text.width(&typed, px).max(text.width(&heading, px)).max(text.width(&hint, small));
    let width = (widest + 5 * pad).max(px as i32 * 32).min(canvas.width);
    let field_height = line_height + pad;
    let height = 2 * line_height + field_height + 7 * pad / 2;
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);

    panel(canvas, left, top, width, height, pad);
    text.draw_strong(canvas, left + 3 * pad / 2, top + pad, px, TEXT_COLOR, &heading);

    // The field: darker than the panel, edged in the accent as the thing
    // that has the keyboard.
    let (field_left, field_top, field_width) = (left + 3 * pad / 2, top + pad + line_height + pad / 2, width - 3 * pad);
    canvas.round_fill(field_left, field_top, field_width, field_height, pad / 2, 0x141518, 256);
    // Set into the panel: a shadow along its inside top.
    canvas.round_fill(field_left + pad / 2, field_top + 2, field_width - pad, 1, 0, 0x000000, 120);
    canvas.round_outline(field_left, field_top, field_width, field_height, pad / 2, 2, accent());
    let (text_left, text_top) = (field_left + pad / 2, field_top + pad / 2);
    draw_selection(canvas, text, field, text_left, text_top, px);
    text.draw(canvas, text_left, text_top, px, TEXT_COLOR, &typed);
    let cursor = text_left + text.width(&before_cursor, px);
    canvas.fill(cursor, text_top, (px as i32 / 10).max(2), line_height, TEXT_COLOR);

    text.draw(canvas, left + 3 * pad / 2, field_top + field_height + pad / 2, small, DIM_TEXT_COLOR, &hint);
}

/// The keys of one line of the shortcuts, each with how wide it is drawn
/// and whether as a key cap. A few words together describe something
/// that is not a key, and are written plainly.
pub(super) fn help_keys<'a>(text: &mut Text, keys: &'a str, px: u32) -> Vec<(&'a str, i32, bool)> {
    let pad = px as i32 / 2;
    let parts = keys.split("  ").filter(|key| !key.is_empty());
    parts.map(|key| if key.contains(' ') { (key, text.width(key, px), false) } else { (key, (text.width(key, px) + pad).max(px as i32 * 3 / 2), true) }).collect()
}

/// Draws the list of shortcuts as a panel in the middle of the window:
/// columns of sections, each key on a cap of its own.
/// Gives back where the panel is.
pub(super) fn draw_help(canvas: &mut Canvas, text: &mut Text, px: u32, search: &str) -> [i32; 4] {
    draw_help_sized(canvas, text, px, search, true)
}

/// The overlay at `px`, with the essentials or without, which is how it
/// first gives way to a window it does not fit: they are in the list too.
fn draw_help_sized(canvas: &mut Canvas, text: &mut Text, px: u32, search: &str, essentials: bool) -> [i32; 4] {
    // What is typed is looked for, each word of it, in a shortcut's keys,
    // what it does and the heading it is under. Those without it stay
    // where they are, faint, so that nothing moves about.
    let words: Vec<String> = search.to_lowercase().split_whitespace().map(str::to_string).collect();
    let matches = |title: &str, keys: &str, label: &str| {
        let all = format!("{title} {keys} {label}").to_lowercase();
        words.iter().all(|word| all.contains(word.as_str()))
    };
    let found: usize = commands::HELP.iter().flat_map(|sections| sections.iter()).map(|(title, entries)| entries.iter().filter(|(keys, label)| matches(title, keys, label)).count()).sum();
    let small = (px * 5 / 6).max(11);
    // As close as it was before lines got their air: it all has to fit.
    let row = Text::line_height(px) + px as i32 / 6;
    // The whole list is the smaller size and quieter, under the few keys
    // that matter most, drawn large; looking for something, it is all
    // there is, and what matches is lit.
    let searching = !words.is_empty();
    let list_row = Text::line_height(small) + px as i32 / 6;
    let cap_h = Text::line_height(px) + px as i32 / 3;
    let cell_h = cap_h + Text::line_height(px) + px as i32 / 2;
    let rows_of_essentials = commands::ESSENTIALS.len().div_ceil(5) as i32;
    let essentials = essentials && !searching;
    let essentials_h = if essentials { rows_of_essentials * cell_h + row } else { 0 };
    let (pad, gap, between) = (px as i32 * 3 / 2, px as i32, px as i32 / 3);
    let keys_width = |text: &mut Text, keys: &str| -> i32 {
        let parts = help_keys(text, keys, small);
        parts.iter().map(|(_, width, _)| width + between).sum::<i32>() - between
    };
    // Each column is as wide as its widest keys plus its widest description.
    let columns: Vec<(i32, i32, i32)> = commands::HELP
        .iter()
        .map(|sections| {
            let entries = sections.iter().flat_map(|(_, entries)| entries.iter());
            let keys = entries.clone().map(|(keys, _)| keys_width(text, keys)).max().unwrap_or(0);
            let labels = entries.map(|(_, label)| text.width(label, small)).max().unwrap_or(0);
            let rows: usize = sections.iter().map(|(_, entries)| entries.len() + 2).sum();
            (keys, labels, rows as i32 - 1)
        })
        .collect();
    let width = columns.iter().map(|(keys, labels, _)| keys + gap + labels).sum::<i32>()
        + 2 * pad * (columns.len() as i32 - 1)
        + 2 * pad;
    let (title, hint) = ("Keyboard shortcuts", "Type to search  ·  Esc to close  ·  > in Ctrl+K lists every command");
    let head = row * 2;
    let height = head + essentials_h + columns.iter().map(|c| c.2).max().unwrap_or(0) * list_row + row * 3 / 2 + 2 * pad;
    // Too big for the window, however large: drawn smaller until it fits.
    let room = (canvas.width - 2 * px as i32, canvas.height - 2 * px as i32);
    let too_big = width > room.0 || height > room.1;
    if too_big && essentials {
        return draw_help_sized(canvas, text, px, search, false);
    }
    if too_big && px > 10 {
        let fit = (f64::from(room.0) / f64::from(width)).min(f64::from(room.1) / f64::from(height));
        let smaller = ((f64::from(px) * fit) as u32).clamp(10, px - 1);
        return draw_help_sized(canvas, text, smaller, search, false);
    }
    let (left, top) = ((canvas.width - width) / 2, (canvas.height - height) / 2);
    panel(canvas, left, top, width, height, px as i32);
    text.draw(canvas, left + pad, top + pad, px * 5 / 4, TEXT_COLOR, title);
    let hint_width = text.width(hint, small);
    text.draw(canvas, left + width - pad - hint_width, top + height - pad - Text::line_height(small), small, DIM_TEXT_COLOR, hint);
    // The box what is typed goes in, at the right of the heading.
    let line = Text::line_height(px);
    let (box_w, box_h) = (px as i32 * 22, line + px as i32 * 2 / 3);
    let (box_x, box_y) = (left + width - pad - box_w, top + pad - px as i32 / 6);
    canvas.round_fill(box_x, box_y, box_w, box_h, px as i32 / 2, PLACEHOLDER_COLOR, 256);
    canvas.round_outline(box_x, box_y, box_w, box_h, px as i32 / 2, 1, if words.is_empty() { MENU_EDGE_COLOR } else { accent_soft() });
    let glass = commands::icons::SEARCH;
    let typed_left = box_x + px as i32 * 2 / 3 + text.width(glass, px) + px as i32 / 2;
    let typed_top = box_y + (box_h - line) / 2;
    text.draw(canvas, box_x + px as i32 * 2 / 3, typed_top, px, DIM_TEXT_COLOR, glass);
    match search.is_empty() {
        true => text.draw(canvas, typed_left, typed_top, px, DIM_TEXT_COLOR, "Type to search"),
        false => text.draw(canvas, typed_left, typed_top, px, TEXT_COLOR, search),
    }
    let caret = typed_left + if search.is_empty() { 0 } else { text.width(search, px) };
    canvas.fill(caret, typed_top, (px as i32 / 10).max(2), line, TEXT_COLOR);
    if !words.is_empty() {
        let count = if found == 0 { "no match".to_string() } else { format!("{found} found") };
        let count_left = box_x + box_w - px as i32 * 2 / 3 - text.width(&count, small);
        text.draw(canvas, count_left, box_y + (box_h - Text::line_height(small)) / 2, small, DIM_TEXT_COLOR, &count);
    }

    // The essentials, five across: each key on a cap of the larger size,
    // what it does under it.
    if essentials {
        let cell_w = (width - 2 * pad) / 5;
        let grid_top = top + pad + head;
        for (at, (keys, label)) in commands::ESSENTIALS.iter().enumerate() {
            let (cx, cy) = (left + pad + (at % 5) as i32 * cell_w, grid_top + (at / 5) as i32 * cell_h);
            let mut key_x = cx;
            for (key, width, cap) in help_keys(text, keys, px) {
                if cap {
                    canvas.round_fill(key_x, cy, width, cap_h, px as i32 / 3, PLACEHOLDER_COLOR, 256);
                    canvas.round_outline(key_x, cy, width, cap_h, px as i32 / 3, 1, KEY_EDGE_COLOR);
                }
                let inset = if cap { (width - text.width(key, px)) / 2 } else { 0 };
                text.draw(canvas, key_x + inset, cy + (cap_h - Text::line_height(px)) / 2, px, TEXT_COLOR, key);
                key_x += width + between;
            }
            text.draw(canvas, cx, cy + cap_h + px as i32 / 4, px, TEXT_COLOR, label);
        }
        let rule = grid_top + rows_of_essentials * cell_h + row / 3;
        canvas.fill(left + pad, rule, width - 2 * pad, 1, MENU_EDGE_COLOR);
    }

    let mut x = left + pad;
    for (sections, (keys_width, labels_width, _)) in commands::HELP.iter().zip(columns) {
        let mut y = top + pad + head + essentials_h;
        for (title, entries) in sections.iter() {
            let any = entries.iter().any(|(keys, label)| matches(title, keys, label));
            text.draw_strong(canvas, x, y, small, if any { accent_soft() } else { FAINT_TEXT_COLOR }, title);
            canvas.fill(x, y + list_row - list_row / 5, keys_width + gap + labels_width, 1, MENU_EDGE_COLOR);
            y += list_row;
            for (keys, label) in entries.iter() {
                let lit = matches(title, keys, label);
                if lit && !words.is_empty() {
                    canvas.round_fill(x - px as i32 / 3, y, keys_width + gap + labels_width + px as i32 * 2 / 3, list_row, px as i32 / 3, TEXT_COLOR, 22);
                }
                let mut key_x = x;
                for (key, width, cap) in help_keys(text, keys, small) {
                    let key_top = y + (list_row - Text::line_height(small)) / 2;
                    if !lit {
                        let inset = if cap { (width - text.width(key, small)) / 2 } else { 0 };
                        text.draw(canvas, key_x + inset, key_top, small, FAINT_TEXT_COLOR, key);
                    } else if cap && !searching {
                        // Just looking: the key's name, a weight up, with no
                        // cap; a wall of caps is what makes the list a lot.
                        text.draw_strong(canvas, key_x, key_top, small, 0xC9CBD1, key);
                        key_x += text.width_strong(key, small) + between * 2;
                        continue;
                    } else if cap {
                        let (cap_top, cap_height) = (y + list_row / 10, list_row - list_row / 5);
                        canvas.round_fill(key_x, cap_top, width, cap_height, px as i32 / 4, PLACEHOLDER_COLOR, 256);
                        canvas.round_outline(key_x, cap_top, width, cap_height, px as i32 / 4, 1, KEY_EDGE_COLOR);
                        let inset = (width - text.width(key, small)) / 2;
                        text.draw(canvas, key_x + inset, key_top, small, TEXT_COLOR, key);
                    } else {
                        text.draw(canvas, key_x, key_top, small, DIM_TEXT_COLOR, key);
                    }
                    key_x += width + between;
                }
                let label_top = y + (list_row - Text::line_height(small)) / 2;
                let color = if !lit { FAINT_TEXT_COLOR } else if searching { TEXT_COLOR } else { DIM_TEXT_COLOR };
                text.draw(canvas, x + keys_width + gap, label_top, small, color, label);
                y += list_row;
            }
            y += list_row;
        }
        x += keys_width + gap + labels_width + 2 * pad;
    }
    [left, top, width, height]
}
