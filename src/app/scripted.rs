//! Scripted runs: the off-screen snapshots tests take, and the benchmark.

use super::*;

impl App {

    /// Once the first image is up, presses the scripted keys and schedules
    /// the snapshot. Does nothing in a normal run.
    pub(super) fn start_snapshot(&mut self, event_loop: &ActiveEventLoop) {
        let Some(snapshot) = &mut self.snapshot else { return };
        if snapshot.at.is_some() {
            return;
        }
        self.mouse = snapshot.pointer;
        self.snapshot_step(event_loop);
    }

    /// Presses the scripted keys up to the next `{Wait}`, which stands for
    /// one waiting period, and schedules what follows.
    pub(super) fn snapshot_step(&mut self, event_loop: &ActiveEventLoop) {
        let Some(snapshot) = &mut self.snapshot else { return };
        snapshot.at = Some(Instant::now() + snapshot.wait);
        let script = std::mem::take(&mut snapshot.keys);
        let (keys, later) = script.split_once("{Wait}").unwrap_or((&script, ""));
        snapshot.keys = later.to_string();
        self.play_keys(keys, event_loop);
    }

    /// Presses the keys of a script: plain characters are typed; `{Enter}`,
    /// `{Tab}`, `{F2}` and so on press the named key.
    pub(super) fn play_keys(&mut self, keys: &str, event_loop: &ActiveEventLoop) {
        let mut rest = keys;
        // `{Hold:Shift}`, `{Hold:Alt}`, `{Hold:Ctrl}` keep a modifier held
        // through what follows, moves and all, as a hand on the key would;
        // `{Hold:}` lets go.
        let mut sticky = ModifiersState::empty();
        while let Some(c) = rest.chars().next() {
            let (mut repeating, mut pressing, mut releasing) = (false, true, true);
            let named = rest.strip_prefix('{').and_then(|inner| inner.split_once('}'));
            let key = match named {
                Some((name, after)) => {
                    rest = after;
                    // `{Down+Space}` only presses a key and `{Up+Space}` only
                    // lets it go, for one that is held.
                    (pressing, releasing) = (!name.starts_with("Up+"), !name.starts_with("Down+"));
                    // `{Shift+Right}` and `{Ctrl+Right}` hold a modifier, and
                    // `{Held+Right}` is the key repeating from being held.
                    repeating = name.starts_with("Held+");
                    let (held, name) = match name.split_once('+') {
                        Some(("Shift", name)) => (ModifiersState::SHIFT, name),
                        Some(("Ctrl", name)) => match name.strip_prefix("Shift+") {
                            Some(name) => (ModifiersState::CONTROL | ModifiersState::SHIFT, name),
                            None => (ModifiersState::CONTROL, name),
                        },
                        Some(("Held" | "Down" | "Up", name)) => (ModifiersState::empty(), name),
                        _ => (ModifiersState::empty(), name),
                    };
                    if let Some(which) = name.strip_prefix("Hold:") {
                        sticky = which.split('+').fold(ModifiersState::empty(), |all, key| match key {
                            "Shift" => all | ModifiersState::SHIFT,
                            "Alt" => all | ModifiersState::ALT,
                            "Ctrl" => all | ModifiersState::CONTROL,
                            _ => all,
                        });
                        self.modifiers = sticky;
                        // As the window would hear of it in a drag in Edit.
                        if self.editor.as_ref().is_some_and(|editor| editor.dragging()) {
                            self.editor_motion();
                        }
                        continue;
                    }
                    self.modifiers = held | sticky;
                    // `{Move:X,Y}` moves the pointer and `{Click}` clicks.
                    if let Some((x, y)) = name.strip_prefix("Move:").and_then(|to| to.split_once(',')) {
                        // A move asked for is a move, wherever the pointer was.
                        self.pointer_found = super::input::Arrival::Moved;
                        self.mouse_moved(x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
                        continue;
                    }
                    // `{Size:WxH}` asks for the window to be that size, as
                    // leaving fullscreen does.
                    if let Some((w, h)) = name.strip_prefix("Size:").and_then(|size| size.split_once('x')) {
                        if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                            self.ask_size((w, h));
                        }
                        continue;
                    }
                    // `{Jump:F}` goes F of the way along the video, as a
                    // click on the seek bar there does.
                    if let Some(fraction) = name.strip_prefix("Jump:").and_then(|f| f.parse().ok()) {
                        self.seek_to(fraction, true);
                        continue;
                    }
                    // `{Wheel:N}` turns the wheel N lines, up being positive.
                    if let Some(lines) = name.strip_prefix("Wheel:") {
                        self.mouse_wheel(MouseScrollDelta::LineDelta(0.0, lines.parse().unwrap_or(0.0)));
                        continue;
                    }
                    // `{RightClick}` is the other button.
                    if name == "RightClick" {
                        self.mouse_button(MouseButton::Right, true, event_loop);
                        self.mouse_button(MouseButton::Right, false, event_loop);
                        continue;
                    }
                    if name == "Click" {
                        self.mouse_button(MouseButton::Left, true, event_loop);
                        self.mouse_button(MouseButton::Left, false, event_loop);
                        continue;
                    }
                    // `{Press}` and `{Release}`, with moves between them,
                    // drag.
                    if name == "Press" || name == "Release" {
                        self.mouse_button(MouseButton::Left, name == "Press", event_loop);
                        continue;
                    }
                    match name {
                        "Enter" => Key::Named(NamedKey::Enter),
                        "Escape" => Key::Named(NamedKey::Escape),
                        "Tab" => Key::Named(NamedKey::Tab),
                        "Backspace" => Key::Named(NamedKey::Backspace),
                        "Delete" => Key::Named(NamedKey::Delete),
                        "Left" => Key::Named(NamedKey::ArrowLeft),
                        "Right" => Key::Named(NamedKey::ArrowRight),
                        "Up" => Key::Named(NamedKey::ArrowUp),
                        "Down" => Key::Named(NamedKey::ArrowDown),
                        "Space" => Key::Named(NamedKey::Space),
                        "PageDown" => Key::Named(NamedKey::PageDown),
                        "PageUp" => Key::Named(NamedKey::PageUp),
                        "Home" => Key::Named(NamedKey::Home),
                        "End" => Key::Named(NamedKey::End),
                        "F2" => Key::Named(NamedKey::F2),
                        "F3" => Key::Named(NamedKey::F3),
                        "F5" => Key::Named(NamedKey::F5),
                        "F12" => Key::Named(NamedKey::F12),
                        // `{Ctrl+k}`: a letter with a modifier held.
                        letter if letter.chars().count() == 1 => Key::Character(letter),
                        _ => continue,
                    }
                }
                None => {
                    let (typed, after) = rest.split_at(c.len_utf8());
                    rest = after;
                    Key::Character(typed)
                }
            };
            if pressing {
                if !repeating {
                    self.hitches.input(hitch::key_name(key.clone(), self.modifiers));
                }
                self.key_pressed(key.clone(), repeating, false, event_loop);
            }
            // A key typed is let go again at once.
            if releasing && !repeating {
                self.key_released(key);
            }
            self.modifiers = ModifiersState::empty();
        }
        self.mouse_moved(self.mouse.0, self.mouse.1);
    }

    // ---- Benchmark -----------------------------------------------------

    /// Sets up the benchmark: a script of scenes played through the files
    /// once the first picture is up, each timed, with the figures printed
    /// at the end. `launched` is when the process began.
    pub fn start_benchmark(&mut self, launched: Instant) {
        let mut scenes: VecDeque<(&'static str, &'static str, Duration)> = VecDeque::new();
        let mut scene = |name, keys, ms| scenes.push_back((name, keys, Duration::from_millis(ms)));
        // Pictures to go through: none, and the folder is of videos only.
        if self.files.iter().any(|file| !files::is_video(file)) {
            // On through the files at a reader's pace, so that each is decoded
            // ahead; then faster than they decode; then back over ones decoded.
            for _ in 0..15 {
                scene("step", "{Right}", 250);
            }
            for _ in 0..40 {
                scene("fast", "{Right}", 40);
            }
            scene("", "", 1500);
            for _ in 0..15 {
                scene("back", "{Left}", 250);
            }
            scene("", "", 400);
            for _ in 0..3 {
                scene("zoom", "{Up}", 350);
            }
            for _ in 0..3 {
                scene("zoom", "{Down}", 350);
            }
            scene("zoom", "0", 400);
            scene("grid", "g", 2000);
            scene("grid", "{PageDown}", 800);
            scene("grid", "{PageDown}", 800);
            scene("grid", "{PageUp}", 800);
            scene("grid", "g", 600);
        }
        let videos = self.files.iter().filter(|file| files::is_video(file)).count();
        if videos > 0 {
            scene("video", "@video", 3000);
            // About in it: five seconds on and back, as the arrows go, and
            // a long way along, as a click on the seek bar does.
            for _ in 0..6 {
                scene("seek", "{Right}", 800);
            }
            for _ in 0..6 {
                scene("seek back", "{Left}", 800);
            }
            for at in ["@jump:0.8", "@jump:0.2", "@jump:0.6", "@jump:0.4"] {
                scene("jump", at, 1200);
            }
        }
        if videos > 1 {
            // From one video to the next, held ready, and back, which is not.
            for _ in 0..5 {
                scene("next video", "{PageDown}", 1500);
            }
            for _ in 0..3 {
                scene("previous video", "{PageUp}", 1500);
            }
        }
        if videos > 0 {
            scene("", "{Right}", 500);
        }
        self.bench = Some(Bench { scenes, scene: "", at: None, samples: Vec::new(), launched, begun: false });
    }

    /// Records a figure for the benchmark, under a scene and a metric.
    pub(super) fn bench_note(&mut self, scene: &str, metric: &str, took: Duration) {
        let Some(bench) = &mut self.bench else { return };
        let key = (scene.to_string(), metric.to_string());
        let ms = took.as_secs_f64() * 1000.0;
        match bench.samples.iter_mut().find(|(k, _)| *k == key) {
            Some((_, values)) => values.push(ms),
            None => bench.samples.push((key, vec![ms])),
        }
    }

    /// The scene the benchmark is in, if it is on.
    pub(super) fn bench_scene(&self) -> Option<&'static str> {
        self.bench.as_ref().filter(|bench| bench.begun).map(|bench| bench.scene)
    }

    /// Plays the next scene of the benchmark, or ends it.
    pub(super) fn bench_step(&mut self, event_loop: &ActiveEventLoop) {
        let Some(bench) = &mut self.bench else { return };
        bench.begun = true;
        let Some((scene, keys, wait)) = bench.scenes.pop_front() else { return self.finish_benchmark(event_loop) };
        bench.scene = scene;
        bench.at = Some(Instant::now() + wait);
        match keys {
            // To the first video in the list.
            "@video" => {
                if let Some(pos) = self.list.iter().position(|id| files::is_video(&self.files[*id])) {
                    self.go_to(pos, true);
                }
            }
            // That far along the video, as a click on the seek bar.
            keys if keys.starts_with("@jump:") => {
                if let Ok(fraction) = keys["@jump:".len()..].parse() {
                    self.seek_to(fraction, true);
                }
            }
            keys => self.play_keys(keys, event_loop),
        }
    }

    /// Prints the benchmark's figures and quits.
    pub(super) fn finish_benchmark(&mut self, event_loop: &ActiveEventLoop) {
        let Some(bench) = self.bench.take() else { return };
        let (width, height) = self.window_size();
        let backend = match self.window.as_ref().and_then(|window| window.display_handle().ok()).map(|handle| handle.as_raw()) {
            Some(winit::raw_window_handle::RawDisplayHandle::Wayland(_)) => "Wayland",
            Some(winit::raw_window_handle::RawDisplayHandle::Xlib(_) | winit::raw_window_handle::RawDisplayHandle::Xcb(_)) => "X11",
            _ => "an unknown display",
        };
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        println!("jog {} benchmark: {} files, a {width}x{height} window on {backend}, {cores} cores", crate::version(), self.list.len());
        println!();
        println!("{:<24} {:<32} {:>8} {:>8} {:>8} {:>5}", "scene", "metric", "median", "p90", "max", "n");
        // Scenes in the order they were played; within one, the metrics
        // in the order a reader wants them.
        const METRICS: [&str; 10] = [
            "launch to first picture",
            "launch to first frame",
            "key to picture",
            "decode, read and fit",
            "start to first frame",
            "key to frame",
            "scale the picture, rough",
            "scale the picture, smooth",
            "draw the page",
            "redraw",
        ];
        let mut samples = bench.samples;
        let scenes: Vec<String> = samples.iter().map(|((scene, _), _)| scene.clone()).fold(Vec::new(), |mut seen, scene| {
            if !seen.contains(&scene) {
                seen.push(scene);
            }
            seen
        });
        let rank = |(scene, metric): &(String, String)| (scenes.iter().position(|s| s == scene), METRICS.iter().position(|m| m == metric));
        samples.sort_by_key(|(key, _)| rank(key));
        let mut last_scene = String::new();
        for ((scene, metric), values) in &samples {
            let mut sorted = values.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let at = |part: f64| sorted[((sorted.len() as f64 * part) as usize).min(sorted.len() - 1)];
            let shown = if *scene == last_scene { "" } else { scene.as_str() };
            last_scene.clone_from(scene);
            println!("{shown:<24} {metric:<32} {:>8} {:>8} {:>8} {:>5}", ms(Duration::from_secs_f64(at(0.5) / 1000.0)), ms(Duration::from_secs_f64(at(0.9) / 1000.0)), ms(Duration::from_secs_f64(sorted[sorted.len() - 1] / 1000.0)), sorted.len());
        }
        let shown = samples.iter().find(|((scene, metric), _)| scene == "fast" && metric == "key to picture").map_or(0, |(_, values)| values.len());
        println!();
        println!("Stepping every 40 ms, {shown} of 40 presses had their picture up before the next press.");
        let (thumbs, _) = self.loader.stats(0).thumbs_kept;
        println!("Thumbnails decoded for the grid: {thumbs}.");
        if let Some(peak) = std::fs::read_to_string("/proc/self/status").ok().and_then(|status| {
            status.lines().find(|line| line.starts_with("VmHWM:")).and_then(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
        }) {
            println!("Peak memory {} MB.", peak / 1024);
        }
        event_loop.exit();
    }

    /// Writes what would be on screen to the snapshot file and quits.
    pub(super) fn finish_snapshot(&mut self, event_loop: &ActiveEventLoop) {
        if self.snapshot.as_ref().is_some_and(|snapshot| !snapshot.keys.is_empty()) {
            return self.snapshot_step(event_loop);
        }
        let Some(snapshot) = self.snapshot.take() else { return };
        if let (Some(player), true) = (&self.player, self.options.debug) {
            let count = |property| player.number(property).unwrap_or(0.0);
            let (position, late, dropped) = (count("time-pos"), count("frame-drop-count"), count("vo-drop-frame-count"));
            eprintln!("jog: video at {position:.1}s, {late} frames decoded late, {dropped} not shown");
        }
        let window = self.window_size();
        let mut pixels = vec![0u32; window.0 as usize * window.1 as usize];
        self.settle_at = None;
        self.paint(&mut pixels, window);
        self.draw_loupe(&mut pixels, window);

        let mut ppm = format!("P6 {} {} 255\n", window.0, window.1).into_bytes();
        ppm.extend(pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
        if let Err(error) = std::fs::write(&snapshot.path, ppm) {
            eprintln!("jog: cannot write {}: {error}", snapshot.path.display());
        }
        // A panel popped out, in its own window, beside it: `x.ppm` and
        // `x.popout.ppm`.
        if let Some((size, pixels)) = self.popout_pixels() {
            let mut ppm = format!("P6 {} {} 255\n", size.0, size.1).into_bytes();
            ppm.extend(pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
            let _ = std::fs::write(snapshot.path.with_extension("popout.ppm"), ppm);
        }
        event_loop.exit();
    }

}
