//! Videos: the player started and stopped, its keys, sound, subtitles, speed and looping.

use super::*;
use crate::mpv::Crash;

/// Which of the players: the one playing or the spare holding the next
/// video ready, by the number their signals carry, or the seek preview's.
#[derive(Clone, Copy, Debug)]
enum Which {
    Main(u8),
    Spare(u8),
    Preview,
}

/// More players crashed than this in this long, and none are started again.
const CRASH_SPELL: Duration = Duration::from_secs(30);
const CRASHES_IN_SPELL: usize = 3;

impl App {

    /// Starts or stops the player so that it is playing exactly when a video
    /// is the image on view.
    pub(super) fn sync_video(&mut self) {
        let id = self.id();
        let playing = self.video.as_ref().map(|video| video.id);
        // With the grid up, the video that was on view plays on in a corner.
        let wanted = (self.grid.is_none() || self.pip) && self.overview.is_none() && self.playable(id);
        // A video that crashes the player is shown as a still, and said so
        // of once each time it comes up.
        let unplayable = files::is_video(&self.files[id]) && !self.playable(id);
        if unplayable && self.unplayable_said != Some(id) {
            self.toast("Not playable · it crashes the player");
        }
        self.unplayable_said = unplayable.then_some(id);
        if wanted && playing != Some(id) {
            self.start_video(id);
        } else if !wanted && playing.is_some() {
            self.remember_position();
            self.video = None;
            self.hover = None;
            if let Some(player) = &self.player {
                player.stop();
            }
        }
        self.preload_video();
    }

    /// Starts the player on another thread, once; `Event::Player` delivers
    /// it. Loading libmpv and starting mpv takes long enough to notice.
    pub(super) fn warm_player(&mut self) {
        if std::mem::replace(&mut self.player_asked, true) {
            return;
        }
        let make = self.player_maker();
        let active = self.active;
        std::thread::spawn(move || {
            if make(Which::Main(active)) {
                make(Which::Preview);
                make(Which::Spare(active ^ 1));
            }
        });
    }

    /// What starts one of the players, on whatever thread calls it, and
    /// sends it to the event loop; whether it started.
    fn player_maker(&self) -> impl Fn(Which) -> bool + Send + 'static {
        let proxy = self.proxy.clone();
        // A scripted test run must not make any sound.
        let silent = self.snapshot.is_some() || self.bench.is_some();
        move |which| {
            let wake = proxy.clone();
            let role = Role::Playback { silent, gpu: true };
            match which {
                Which::Main(index) => {
                    let player = Player::new(role, move |signal| drop(wake.send_event(Event::Video(index, signal))));
                    let started = player.is_ok();
                    let _ = proxy.send_event(Event::Player(player));
                    started
                }
                Which::Spare(index) => {
                    let spare = Player::new(role, move |signal| drop(wake.send_event(Event::Video(index, signal))));
                    let started = spare.is_ok();
                    let _ = proxy.send_event(Event::Spare(spare));
                    started
                }
                Which::Preview => {
                    let preview = Player::new(Role::Preview, move |signal| drop(wake.send_event(Event::Preview(signal))));
                    let started = preview.is_ok();
                    let _ = proxy.send_event(Event::PreviewPlayer(preview));
                    started
                }
            }
        }
    }

    /// A player crashed: another in its place, on another thread, unless
    /// players have been crashing one after another, which starting more
    /// would only go on with. Whether one is being started.
    fn replace_player(&mut self, which: Which) -> bool {
        let now = Instant::now();
        self.player_crashes.retain(|at| now.duration_since(*at) < CRASH_SPELL);
        self.player_crashes.push(now);
        if self.player_crashes.len() > CRASHES_IN_SPELL {
            eprintln!("jog: the player crashed {} times in {CRASH_SPELL:?}; not starting it again", self.player_crashes.len());
            return false;
        }
        let make = self.player_maker();
        std::thread::spawn(move || {
            make(which);
        });
        true
    }

    /// The main player crashed. The file it had open is noted as one that
    /// crashed it, and decoded on the CPU from now on; another player is
    /// started, and the video goes on from where it was, as it was.
    fn main_player_crashed(&mut self) {
        let Some(player) = self.player.take() else { return };
        let lost = player.lost();
        // Where it was: told of before it went, and kept.
        let at = player.number("time-pos");
        let paused = player.flag("pause") == Some(true);
        drop(player);
        self.hover = None;
        self.drop_back();
        let video = self.video.take();
        let name = lost.as_ref().and_then(|lost| lost.path.as_ref()).and_then(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let crash = lost.as_ref().and_then(|lost| lost.crash);
        let replaced = self.replace_player(Which::Main(self.active));
        // The video starts again where it was, once there is a player for it.
        if replaced && video.is_some() && crash != Some(Crash::Software) {
            self.enter_at = at;
            self.resume_paused = paused;
        }
        self.toast(match (crash, replaced) {
            (_, false) => "Not playing videos · the player keeps crashing".to_string(),
            (Some(Crash::Gpu), true) => format!("{name} crashed the GPU decoder · on the CPU now"),
            (Some(Crash::Software), true) => format!("Not playable · {name} crashes the player"),
            (None, true) => "The player crashed · started again".to_string(),
        });
        self.redraw();
    }

    /// Whether the file can be played: not if it has crashed the player
    /// with its CPU decoder too.
    pub(super) fn playable(&self, id: usize) -> bool {
        files::is_video(&self.files[id]) && crate::mpv::crashed_on(&self.files[id]) != Some(Crash::Software)
    }

    /// Asks the preview player for the frame at a point along the video.
    pub(super) fn request_preview(&mut self, fraction: f64) {
        let Some((_, length)) = self.video_time() else { return };
        self.preview_want = Some(fraction * length);
        self.pump_preview();
    }

    /// Sends the preview player the latest spot asked for, unless it is
    /// still fetching the one before. Pointer movement asks far faster than
    /// frames can be fetched; this way the requests in between are skipped.
    pub(super) fn pump_preview(&mut self) {
        let (Some(preview), Some(video), Some(want)) = (&self.preview, &self.video, self.preview_want) else {
            return;
        };
        // A request that never produced a frame must not block the rest.
        let busy = self.preview_busy.is_some_and(|since| since.elapsed() < PREVIEW_TIMEOUT);
        if busy {
            return;
        }
        // Another video than the one it has open: that one is opened, and
        // the spot is asked for afresh even if it is the same time along
        // as the last one asked of the video before.
        if self.preview_id != Some(video.id) {
            preview.open(&self.files[video.id]);
            self.preview_id = Some(video.id);
            self.preview_frame = None;
            self.preview_sent = None;
        }
        // The nearest keyframe is close enough, and much quicker to reach.
        // Paused, with the pointer still, the keyframe is followed by the
        // exact frame there, what a click would show.
        let paused = self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true));
        let exact = match self.preview_sent {
            Some((sent, exact)) if sent == want => match exact || !paused {
                true => return,
                false => true,
            },
            _ => false,
        };
        let how: &[u8] = if exact { b"absolute+exact" } else { b"absolute+keyframes" };
        preview.command_later(&[b"seek", want.to_string().as_bytes(), how]);
        self.preview_sent = Some((want, exact));
        self.preview_busy = Some(Instant::now());
    }

    pub(super) fn preview_signal(&mut self, signal: Signal) {
        let Some(preview) = &self.preview else { return };
        if let Signal::Events = signal {
            // Only to empty the queue, so that mpv keeps signaling; and to
            // put another in its place if it crashed.
            if preview.events().contains(&PlayerEvent::Crashed) {
                (self.preview, self.preview_id, self.preview_busy) = (None, None, None);
                self.replace_player(Which::Preview);
            }
            return;
        }
        // Size the picture to the video so that no bars are drawn into it.
        let native = self.shown.as_ref().map_or((16, 9), |(_, frame)| frame.native);
        let unit = ui_px(self.window_size().1);
        let (width, height) = fit_size(native, (unit * 16, unit * 12), true);
        let frame = self.preview_frame.get_or_insert_with(|| Frame::still(width, height, native, Vec::new()));
        (frame.width, frame.height) = (width, height);
        frame.pixels.resize(width as usize * height as usize, 0);
        preview.render(&mut frame.pixels, width, height);
        self.preview_busy = None;
        self.pump_preview();
        if self.hover.is_some() {
            self.redraw();
        }
    }

    /// Jumps to a point along the video, from 0 to 1: to that exact frame,
    /// or to the nearest keyframe, which is quick enough to do repeatedly
    /// while dragging.
    pub(super) fn seek_to(&mut self, fraction: f64, exact: bool) {
        let (Some(player), Some((_, length))) = (&self.player, self.video_time()) else { return };
        let target = fraction.clamp(0.0, 1.0) * length;
        let how: &[u8] = if exact { b"absolute+exact" } else { b"absolute+keyframes" };
        player.command_later(&[b"seek", target.to_string().as_bytes(), how]);
        if let Some(video) = &mut self.video {
            // Where it lands is only known beforehand when that is exact;
            // to a keyframe, the bar stands where it was aimed at meanwhile.
            video.seeking = exact.then(|| (Instant::now(), target));
            video.aimed = (!exact).then(|| (Instant::now(), target));
            video.aim_reached = None;
            video.seek_asked.get_or_insert_with(Instant::now);
            video.to_end = (target >= length - VIDEO_END).then(Instant::now);
        }
        self.redraw();
    }

    /// Where the seek bar's track is: inset from the sides and lifted off
    /// the bottom edge, or off the filmstrip when that is pinned open.
    pub(super) fn seek_bar(&self) -> SeekBar {
        let (width, height) = self.window_size();
        let unit = ui_px(height) as i32;
        let base = if self.strip_pinned || self.triage_on || self.scenes_shown() { Strip::for_height(height).top } else { height as i32 };
        // Room on the left for the three buttons and the elapsed time, and
        // on the right for the duration and the buttons there.
        let (end, buttons) = (width as i32, self.bar_buttons().len() as i32);
        let right = unit * 7 + buttons * unit * 9 / 4;
        // Beside the file list when that is up, not under it.
        let left = self.list_panel().filter(|panel| !self.list_right && !panel.embedded).map_or(0, |panel| panel.width);
        let end = end - self.right_inset();
        // The title and the time at the left, where there is room for them
        // and still a track long enough to seek along; else the time alone.
        let title_w = if end - left > unit * 75 { self.title_room(unit) } else { 0 };
        let lead = if title_w > 0 { unit * 17 / 2 + title_w + unit } else { unit * 13 };
        SeekBar { x: left + lead, width: (end - left - lead - right).max(1), middle: base - unit * 2, unit, left, title_w, end, buttons }
    }

    /// Tracks the pointer over the seek bar: shows it at full size, and the
    /// frame for the spot under the pointer.
    pub(super) fn video_pointer(&mut self, x: f64, y: f64) {
        let bar = self.seek_bar();
        // From a little above the bar down to the edge it is lifted off.
        // And up to the buttons over a section, when there is one.
        let tools_top = self.section_tools(&bar).first().map_or(bar.middle - bar.unit * 4, |(_, place)| place[1] - bar.unit / 2);
        // And up to the top of the volume slider, while it is up.
        let reach = self.hover.and_then(|_| self.volume_reach(&bar)).map_or(i32::MAX, |top| top - bar.unit / 2);
        let zone = f64::from((bar.middle - bar.unit * 4).min(tools_top).min(reach))..f64::from(bar.middle + bar.unit * 2);
        let fraction = ((x - f64::from(bar.x)) / f64::from(bar.width)).clamp(0.0, 1.0);
        let hover = zone.contains(&y).then_some(fraction);
        if hover != self.hover {
            self.hover = hover;
            if let Some(fraction) = hover.filter(|_| !self.bar_busy()) {
                self.request_preview(fraction);
            }
            self.redraw();
        }
    }

    /// Whether something else is open over the bar, a menu, a box or a
    /// window: the pointer is on that, and the frame under it is not shown.
    pub(super) fn bar_busy(&self) -> bool {
        self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some() || self.palette.is_some() || self.video_panel.is_some() || self.convert_panel.is_some() || self.compare.is_some()
    }

    pub(super) fn player_ready(&mut self, player: Result<Player, String>, event_loop: &ActiveEventLoop) {
        match player {
            Ok(player) => {
                player.set("speed", &self.playing_speed().to_string());
                player.set("mute", if self.muted { "yes" } else { "no" });
                // Drawing on the GPU: into a window of its own inside this one.
                if let Some(id) = self.video_window_id(usize::from(self.active)) {
                    player.set("wid", &id.to_string());
                }
                self.player = Some(player);
                self.apply_subtitles();
                self.apply_track_prefs();
                self.apply_checks();
                *self.video_view_sent.borrow_mut() = None;
                // A video may already be waiting for it.
                self.sync_video();
                // mpv signals only when an event joins an empty queue, and
                // may have queued some before it had anyone to signal.
                self.video_signal(Signal::Events);
            }
            Err(error) => {
                // Without a player, videos would only be entries that cannot
                // be shown. Finding that out here, rather than before the
                // window opens, keeps libmpv's loading time off the start.
                let videos: Vec<(usize, usize)> = (self.list.iter().enumerate())
                    .filter(|(_, id)| files::is_video(&self.files[**id]))
                    .map(|(pos, id)| (*id, pos))
                    .collect();
                let count = videos.len();
                eprintln!("jog: leaving out {count} videos; playing them needs libmpv (the libmpv1 or libmpv2 package): {error}");
                self.toast(format!("{count} videos left out · {error}"));
                if count > 0 {
                    self.drop_from_list(&videos, event_loop);
                }
            }
        }
    }

    pub(super) fn start_video(&mut self, id: usize) {
        self.warm_player();
        if self.player.is_none() {
            return;
        }
        // The section it had when jog was last quit, marked again.
        self.restore_section(id);
        // The video being left, if this replaces one; and its frames kept.
        self.remember_position();
        self.drop_back();
        // Paused on the video being left, the next is opened paused too, so
        // going back and forth across files to look does not set them
        // playing. Stopped at its end is no pause asked for: the next plays.
        // One put back after the player crashed is as it was.
        let paused = std::mem::take(&mut self.resume_paused)
            || self.video.is_some() && self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(true) && player.flag("eof-reached") != Some(true));
        // A seek carried over from the file beside this one says where to
        // begin; otherwise it is where the video was left.
        let carried = self.enter_at.take();
        let from = carried.or_else(|| self.resume_from(id));
        let mut held = false;
        let speed = self.playing_speed().to_string();
        // The tracks and timing kept for it, told before it opens.
        let kept = self.kept_tracks(id);
        let window = self.window_size();
        // What was drawn over the video goes with the player that had it,
        // which may now be the spare: drawn again for the one taking over.
        self.clear_video_layer();
        if let Some(player) = &mut self.player {
            let started = Instant::now();
            // Seeking one that is held somewhere else takes longer than
            // opening the file afresh, so only one held where this is to
            // begin is taken.
            let start = from.unwrap_or(0.0);
            match self.spare.as_mut().filter(|spare| spare.id == Some(id) && (start - spare.from).abs() < 0.01) {
                // The spare player has it open already, and takes over.
                Some(spare) => {
                    std::mem::swap(player, &mut spare.player);
                    self.active ^= 1;
                    let old = &spare.player;
                    old.stop();
                    spare.id = None;
                    player.set("speed", &speed);
                    player.set("mute", if self.muted { "yes" } else { "no" });
                    if let Some(volume) = old.number("volume") {
                        player.set("volume", &volume.to_string());
                    }
                    *self.video_view_sent.borrow_mut() = None;
                    // What it said while waiting was said to no one.
                    player.events();
                    player.set("pause", if paused { "yes" } else { "no" });
                    held = true;
                }
                None => {
                    if let Some((aid, sid, sub_delay, audio_delay)) = &kept {
                        player.choose_next(aid, sid, *sub_delay, *audio_delay);
                    }
                    player.play(&self.files[id], from, paused);
                    // Its first frame drawn ahead too, in the player's own
                    // process: the first draw at a size sets the conversion
                    // up, a hundred milliseconds that are not the window's.
                    if window.0 > 0 && window.1 > 0 {
                        player.draw_ahead(Some(window));
                    }
                    if self.options.debug {
                        eprintln!("jog: {:>7.1?} video asked for", started.elapsed());
                    }
                }
            }
            // Until the player reports for itself, this is where it is.
            // One counted from the end has to wait for the length.
            let seeking = from.filter(|seconds| *seconds >= 0.0).map(|seconds| (started, seconds));
            self.video = Some(Video { id, showing: false, started, seeking, aimed: None, aim_reached: None, start_took: None, seek_asked: None, seek_took: None, held, tracks: None, track_words: None, shown_at: None, watched: false, to_end: None });
            self.perf.video_lines = None;
            self.apply_video_view();
        }
        if let Some(seconds) = from.filter(|_| carried.is_none()) {
            self.toast(format!("Resumed at {}", clock(seconds)));
        }
        self.apply_loop();
        if held {
            // It may have nothing more to say before its frame is shown.
            self.video_signal(Signal::Events);
        }
    }

    /// Whether the file is a video that will be picked up where it was
    /// left, rather than played from its start.
    pub(super) fn resumes(&self, id: usize) -> bool {
        if !self.options.resume || !files::is_video(&self.files[id]) {
            return false;
        }
        if let Some(known) = self.resumes_known.borrow().get(&id) {
            return *known;
        }
        let path = &self.files[id];
        let left = match &self.positions {
            Some(positions) => positions.get(path),
            None => Positions::load().get(path),
        };
        self.resumes_known.borrow_mut().insert(id, left.is_some());
        left.is_some()
    }

    /// Where a video was left, when those places are being remembered.
    pub(super) fn resume_from(&mut self, id: usize) -> Option<f64> {
        match self.options.resume {
            true => self.positions.get_or_insert_with(Positions::load).get(&self.files[id]),
            false => None,
        }
    }

    /// Gives the spare player the video that comes after the file on view,
    /// or before it when going backwards, to hold ready on its first frame.
    /// Starting a video cold takes long enough to see; this way the one
    /// most likely to be wanted next plays at once.
    pub(super) fn preload_video(&mut self) {
        let len = self.list.len();
        if self.spare.is_none() || self.grid.is_some() || self.overview.is_some() {
            return;
        }
        // A section looping: what is wanted next is its start, again and
        // again, before any other file.
        let playing = self.video.as_ref().map(|video| video.id);
        if let (Some(id), Some((a, _))) = (playing, self.section().and_then(|section| section.looping())) {
            let kept = self.kept_tracks(id);
            let window = self.window_size();
            let Some(spare) = self.spare.as_mut().filter(|spare| spare.id != Some(id) || (spare.from - a).abs() >= 0.01) else { return };
            if let Some((aid, sid, sub_delay, audio_delay)) = &kept {
                spare.player.choose_next(aid, sid, *sub_delay, *audio_delay);
            }
            spare.player.hold(&self.files[id], Some(a));
            if window.0 > 0 && window.1 > 0 {
                spare.player.draw_ahead(Some(window));
            }
            (spare.id, spare.from) = (Some(id), a);
            return;
        }
        if len < 2 {
            return;
        }
        let next = self.list[if self.forward { (self.pos + 1) % len } else { (self.pos + len - 1) % len }];
        if !self.playable(next) || next == self.id() {
            return;
        }
        let from = self.resume_from(next);
        let start = from.unwrap_or(0.0);
        let kept = self.kept_tracks(next);
        let window = self.window_size();
        let Some(spare) = self.spare.as_mut().filter(|spare| spare.id != Some(next) || spare.from != start) else { return };
        if let Some((aid, sid, sub_delay, audio_delay)) = &kept {
            spare.player.choose_next(aid, sid, *sub_delay, *audio_delay);
        }
        spare.player.hold(&self.files[next], from);
        // Its first frame drawn ahead at the window's size, so that taking
        // over it has only to be copied in.
        if window.0 > 0 && window.1 > 0 {
            spare.player.draw_ahead(Some(window));
        }
        (spare.id, spare.from) = (Some(next), start);
    }

    /// The spare player has only to be listened to: mpv signals again only
    /// once its queue is empty, and stalls if a frame it has announced is
    /// not taken.
    pub(super) fn spare_signal(&mut self, signal: Signal) {
        let window = self.window_size();
        let Some(spare) = &self.spare else { return };
        match signal {
            // Crashed holding the next file ready: that file is noted, and
            // another spare holds it, decoded on the CPU.
            Signal::Events if spare.player.events().contains(&PlayerEvent::Crashed) => {
                self.spare = None;
                self.replace_player(Which::Spare(self.active ^ 1));
            }
            Signal::Events => {}
            // Drawing its frame ahead, it has drawn it already.
            Signal::Frame if spare.player.draws_ahead() => {}
            // Its frame is taken, which keeps it going, at the size it will
            // be drawn at when it takes over and on another thread: the
            // first draw at a size sets up the conversion, which for an HDR
            // video took 40-100 ms, a stall on every change of file.
            Signal::Frame if spare.player.warming() => {}
            Signal::Frame if window.0 > 0 && window.1 > 0 => spare.player.warm(window.0, window.1),
            Signal::Frame => drop(spare.player.render(&mut [0; 64], 8, 8)),
        }
    }

    /// Notes where the video on view has got to, to pick up there next
    /// time. One left near its start or its end starts over, as does a clip
    /// too short to lose one's place in.
    pub fn remember_position(&mut self) {
        let (Some(player), Some(video), true) = (&self.player, &self.video, self.options.resume) else { return };
        // Before its first frame the player has nothing true to report.
        let (Some(position), Some(length), true) = (player.number("time-pos"), player.number("duration"), video.showing)
        else {
            return;
        };
        let worth_it = length >= RESUME_MIN_LENGTH && position >= RESUME_MARGIN && position <= length - RESUME_MARGIN;
        let path = self.files[video.id].clone();
        self.positions.get_or_insert_with(Positions::load).set(&path, worth_it.then_some(position));
        self.resumes_known.borrow_mut().clear();
    }

    pub(super) fn video_signal(&mut self, signal: Signal) {
        if matches!(signal, Signal::Frame) {
            self.hitches.stats.told += 1;
        }
        let Some(player) = &self.player else { return };
        // Read the events even when no video is wanted: mpv only signals
        // again once its queue has been emptied.
        let mut events = if let Signal::Events = signal { player.events() } else { Vec::new() };
        if events.contains(&PlayerEvent::Crashed) {
            return self.main_player_crashed();
        }
        // Frames being saved, done.
        let replies: Vec<(u64, bool)> = events.iter().filter_map(|event| if let PlayerEvent::Replied(tag, ok) = event { Some((*tag, *ok)) } else { None }).collect();
        events.retain(|event| !matches!(event, PlayerEvent::Replied(..)));
        for (tag, ok) in replies {
            self.frame_saved(tag, ok);
        }
        self.tend_frame_save();
        let Some(player) = &self.player else { return };
        let mut video_started = None;
        // Whether the time along is written anywhere: on the seek bar, in
        // the info line, under the grid, or along the scenes. Elsewhere
        // its moving on with every frame changes nothing in the window.
        let time_shown = self.hover.is_some() || self.time_until.is_some() || self.show_info || self.show_debug || self.grid.is_some() || self.scenes_shown() || self.slide_at.is_some() || self.show_list;
        // Drawn on the GPU, the video moves on without jog drawing; what jog
        // draws of the time changes only with the second shown and the bar's
        // handle moving a pixel, and only then is it drawn again.
        let time_moved = !crate::mpv::gpu_video() || {
            let bar = self.seek_bar();
            let (at, length) = (player.number("time-pos").unwrap_or(0.0), player.number("duration").unwrap_or(0.0));
            let key = (at as i64, (f64::from(bar.width) * at / length.max(1e-9)) as i32);
            self.time_key.replace(Some(key)) != Some(key)
        };
        let Some(video) = &mut self.video else { return };
        // One held ready by the spare player announced its frame before it
        // was this player, so that has to be looked for.
        if video.held && !video.showing && player.holds(&self.files[video.id]) {
            events.insert(0, PlayerEvent::Showing);
        }
        let mut at_end = false;
        // Whether there is anything new to draw: a frame, or a property
        // that moved, the time along say. The player also answers each
        // setting made, and a frame drawn for the answer would set it
        // again, the bar on its way up moving the subtitles' margin: a
        // round of frames as fast as the answers come, until it stops.
        let mut changed = matches!(signal, Signal::Frame);
        for event in events {
            if event == PlayerEvent::AtEnd {
                at_end = true;
                continue;
            }
            if event == PlayerEvent::Changed || (event == PlayerEvent::Moved && time_shown && time_moved) {
                changed = true;
                continue;
            }
            if event == PlayerEvent::Moved {
                continue;
            }
            // The still stays up until the player has a frame to show.
            let showing = event == PlayerEvent::Showing;
            if showing && !video.showing {
                let took = video.started.elapsed();
                video.start_took.get_or_insert(took);
                video_started = Some(took);
                video.shown_at.get_or_insert_with(Instant::now);
                if self.options.debug {
                    eprintln!("jog: {took:>7.1?} video start {}", self.files[video.id].display());
                }
                if self.options.verbose {
                    let codec = player.text("video-format").unwrap_or_default();
                    let rate = player.number("container-fps").map(|rate| format!(" {rate:.2} fps")).unwrap_or_default();
                    let length = player.number("duration").map(|length| format!(", {} long", clock(length)));
                    let from = video.seeking.map(|(_, seconds)| format!(", resumed at {}", clock(seconds)));
                    let (length, from) = (length.unwrap_or_default(), from.unwrap_or_default());
                    eprintln!("jog:     playing in {}: {codec}{rate}{length}{from}", ms(took));
                }
            } else if showing {
                // Every seek ends with the player announcing frames again.
                if let Some(asked) = video.seek_asked.take() {
                    video.seek_took = Some(asked.elapsed());
                    if self.options.debug {
                        eprintln!("jog: {:>7.1?} seek", asked.elapsed());
                    }
                    video.to_end = video.to_end.map(|_| Instant::now());
                }
            }
            video.showing = showing;
        }
        // mpv stalls, for over half a second when changing file, if a frame
        // it has announced is not taken. Redrawing takes it, but that only
        // happens once the video is showing; until then take it here and
        // discard it. The hidden window of a scripted run is drawn like
        // any other, and takes its frames the same way.
        if matches!(signal, Signal::Frame) && !video.showing {
            // At the size it last drew at: another has it set itself up anew.
            let window: (u32, u32) = self.window.as_ref().map_or((0, 0), |w| w.inner_size().into());
            let (width, height) = self.video_render.unwrap_or(window);
            ui::size_for(&mut self.scene, width as usize * height as usize);
            self.scene_key = None;
            player.render(&mut self.scene, width, height);
        }
        // A video that played to its end makes way for the next file. One
        // taken to its end by hand, by seeking or stepping there, stays; and
        // the event may be left over from the file before this one. A seek
        // to short of the end is not that: what plays out after it goes on.
        let by_hand = video.to_end.is_some_and(|at| video.seek_asked.is_some() || at.elapsed() < SEEK_SETTLE);
        // Watched: it has played for a while, or right through. A seek on
        // past the first few seconds counts as well, and is seen to there.
        let over = at_end && player.flag("eof-reached") == Some(true);
        let watched = !video.watched && (over || video.shown_at.is_some_and(|at| at.elapsed() >= SEEN_AFTER));
        video.watched |= watched;
        // Which tracks it has is asked once it has been playing a moment:
        // asked while it starts, each answer waits on the player's core.
        let asked = video.tracks.is_none() && video.shown_at.is_some_and(|at| at.elapsed() >= TRACKS_AFTER);
        if asked {
            video.tracks = Some((tracks_of(player, "sub").len(), tracks_of(player, "audio").len()));
            video.track_words = tracks::playing_words(&tracks::track_list(player), self.subtitles);
        }
        let (id, showing) = (video.id, video.showing);
        if (showing && (changed || video_started.is_some())) || asked {
            self.redraw();
        }
        if watched {
            self.mark_seen(id);
        }
        if at_end && !by_hand && player.flag("eof-reached") == Some(true) {
            self.play_next();
        }
        self.settle_aim();
        // At the end of a section looping, back to its start.
        self.relay_section();
        if let (Some(took), Some("video")) = (video_started, self.bench_scene()) {
            self.bench_note("video", "start to first frame", took);
        }
    }

    /// A keyframe seek landed and played on to where it was aimed, or
    /// never will (looped back, or long gone): the bar follows playback
    /// again. Only once playback has been there a moment: straight after a
    /// seek the player may still tell of where it was, past the aim when
    /// that was back, and the bar would go there and then back.
    fn settle_aim(&mut self) {
        let (Some(player), Some(video)) = (&self.player, &mut self.video) else { return };
        let Some((at, aim)) = video.aimed else { return };
        let playing = player.flag("pause") != Some(true);
        // Paused short of it, the bar stays where it was put.
        if playing && at.elapsed() > AIM_GIVEN_UP {
            video.aimed = None;
            return;
        }
        let landed = video.seek_asked.is_none() && player.flag("seeking") != Some(true);
        let now = player.number("time-pos").unwrap_or(0.0);
        if !landed || now < aim - 0.02 {
            video.aim_reached = None;
            return;
        }
        let since = *video.aim_reached.get_or_insert_with(Instant::now);
        if since.elapsed() >= AIM_SETTLED {
            video.aimed = None;
            video.aim_reached = None;
        }
    }

    /// Goes on to the next file after a video, unless that is the
    /// slideshow's job, something is open that the user is in the middle
    /// of, or it was the last file. Looping the list goes on from the last
    /// file to the first, and does so even where videos otherwise stay.
    pub(super) fn play_next(&mut self) {
        let open = self.menu.is_some() || self.prompt.is_some() || self.confirm.is_some();
        let occupied = open || self.drag.is_some() || self.palette.is_some();
        let around = self.looping == Looping::List;
        if self.slide_at.is_some() || occupied || !(around || self.options.autoplay && self.pos + 1 < self.list.len()) {
            return;
        }
        match self.list.len() {
            // The whole list is this one video.
            1 => self.restart_video(),
            _ => self.step(true),
        }
    }

    /// Plays the video from its start, also when it had stopped at its end.
    pub(super) fn restart_video(&mut self) {
        self.seek_to(0.0, true);
        if let (Some(player), Some(_)) = (&self.player, &self.video) {
            player.set("pause", "no");
        }
    }

    /// Steps on to the next way of looping, and says which it is.
    pub(super) fn cycle_loop(&mut self) {
        self.looping = match self.looping {
            Looping::Off => Looping::File,
            Looping::File => Looping::List,
            Looping::List => Looping::Off,
        };
        self.apply_loop();
        // One already stopped at its end has to be set going again.
        let ended = self.video.is_some() && self.player.as_ref().is_some_and(|player| player.flag("eof-reached") == Some(true));
        match self.looping {
            Looping::File if ended => self.restart_video(),
            Looping::List if ended => self.play_next(),
            _ => {}
        }
        let (repeat, once) = (commands::icons::REPEAT, commands::icons::REPEAT_1);
        self.toast(match self.looping {
            Looping::Off => format!("{repeat}  Loop off"),
            Looping::File => format!("{once}  Loop this video"),
            Looping::List => format!("{repeat}  Loop the list"),
        });
    }

    /// Whether a video has been watched, as far as is remembered.
    pub(super) fn watched(&self, id: usize) -> bool {
        *self.seen_known.borrow_mut().entry(id).or_insert_with(|| self.seen.borrow_mut().get_or_insert_with(Seen::load).has(&self.files[id]))
    }

    /// Whether to mark a file as one not watched yet: a video that is not
    /// remembered as watched, where that is being kept track of.
    pub(super) fn is_new(&self, id: usize) -> bool {
        self.options.seen && files::is_video(&self.files[id]) && !self.watched(id)
    }

    /// How strongly a file is marked as not watched yet, from 0 to 1: the
    /// mark of one just watched fades away over a moment.
    pub(super) fn new_level(&self, id: usize) -> f64 {
        if self.is_new(id) {
            return 1.0;
        }
        let fading = self.seen_at.borrow().get(&id).map(|at| at.elapsed()).filter(|gone| *gone < DOT_FADE && self.options.animations);
        fading.map_or(0.0, |gone| {
            self.animating.set(true);
            1.0 - gone.as_secs_f64() / DOT_FADE.as_secs_f64()
        })
    }

    /// Remembers a video as watched.
    pub(super) fn mark_seen(&self, id: usize) {
        if self.options.seen && self.seen_known.borrow_mut().insert(id, true) != Some(true) {
            self.seen_at.borrow_mut().insert(id, Instant::now());
            self.seen.borrow_mut().get_or_insert_with(Seen::load).add(&self.files[id]);
        }
    }

    /// The two players that take turns, where they exist.
    pub(super) fn players(&self) -> impl Iterator<Item = &Player> {
        self.player.iter().chain(self.spare.iter().map(|spare| &spare.player))
    }

    /// Tells both players how subtitles are to look and whether they show.
    /// Those a file styles for itself keep their own look.
    pub(super) fn apply_subtitles(&self) {
        // mpv's sizes are in pixels of a picture 720 high, and its colors
        // are written `#AARRGGBB`.
        let size = (SUBTITLE_SIZE * f64::from(self.subtitle_size) / 100.0).to_string();
        let margin = self.subtitle_margin.to_string();
        let look: [(&str, &str); 6] = match self.options.subtitle_style {
            SubtitleStyle::Outline => [
                ("sub-border-size", "2.2"),
                ("sub-border-color", "#E6000000"),
                ("sub-shadow-offset", "0"),
                ("sub-shadow-color", "#00000000"),
                ("sub-back-color", "#00000000"),
                ("sub-blur", "0.3"),
            ],
            SubtitleStyle::Shadow => [
                ("sub-border-size", "0.9"),
                ("sub-border-color", "#40000000"),
                ("sub-shadow-offset", "1.4"),
                ("sub-shadow-color", "#C0000000"),
                ("sub-back-color", "#00000000"),
                ("sub-blur", "1.6"),
            ],
            // The shadow's offset is how far the panel reaches past the
            // letters.
            SubtitleStyle::Box => [
                ("sub-border-size", "0"),
                ("sub-border-color", "#00000000"),
                ("sub-shadow-offset", "7"),
                ("sub-shadow-color", "#00000000"),
                ("sub-back-color", "#A6000000"),
                ("sub-blur", "0"),
            ],
        };
        for player in self.players() {
            player.set("sub-font", &self.options.subtitle_font);
            player.set("sub-font-size", &size);
            player.set("sub-color", "#FFF4F4F4");
            player.set("sub-margin-y", &margin);
            player.set("sub-visibility", if self.subtitles { "yes" } else { "no" });
            for (name, value) in look {
                player.set(name, value);
            }
        }
    }

    /// Steps on to the video's next subtitle track, and after the last to
    /// none. Whether subtitles show is kept from one video to the next.
    pub(super) fn cycle_subtitles(&mut self) {
        let Some(player) = self.player.as_ref().filter(|_| self.video.is_some()) else { return };
        let tracks = tracks_of(player, "sub");
        if tracks.is_empty() {
            return self.toast("No subtitles");
        }
        let showing = tracks.iter().position(|track| track.selected).filter(|_| self.subtitles);
        let next = match showing {
            None => tracks.first(),
            Some(at) => tracks.get(at + 1),
        };
        if let Some(track) = next {
            player.set("sid", &track.id.to_string());
        }
        self.subtitles = next.is_some();
        self.remember_tracks();
        self.apply_subtitles();
        self.toast(match next {
            Some(track) => format!("{}  {}", commands::icons::CAPTIONS, track.name),
            None => format!("{}  Subtitles off", commands::icons::CAPTIONS_OFF),
        });
    }

    /// Steps on to the video's next audio track, and round to the first.
    pub(super) fn cycle_audio(&mut self) {
        let Some(player) = self.player.as_ref().filter(|_| self.video.is_some()) else { return };
        let tracks = tracks_of(player, "audio");
        if tracks.len() < 2 {
            return self.toast(if tracks.is_empty() { "No sound" } else { "One audio track" });
        }
        let at = tracks.iter().position(|track| track.selected).map_or(0, |at| (at + 1) % tracks.len());
        player.set("aid", &tracks[at].id.to_string());
        let said = format!("{}  {}  ·  {} of {}", commands::icons::AUDIO_LINES, tracks[at].name, at + 1, tracks.len());
        self.remember_tracks();
        self.toast(said);
    }

    /// Makes subtitles a step larger or smaller.
    pub(super) fn size_subtitles(&mut self, larger: bool) {
        let size = if larger { self.subtitle_size + 10 } else { self.subtitle_size.saturating_sub(10) };
        self.subtitle_size = size.clamp(25, 400);
        self.apply_subtitles();
        self.toast(format!("{}  Subtitles {}%", commands::icons::A_LARGE_SMALL, self.subtitle_size));
    }

    /// The buttons at the right of the seek bar, from the edge inwards:
    /// looping, sound on or off, and for a video that has them subtitles
    /// and audio tracks.
    pub(super) fn bar_buttons(&self) -> Vec<Command> {
        let (subtitles, audio) = self.video.as_ref().and_then(|video| video.tracks).unwrap_or((0, 0));
        // From the right edge: fullscreen, which leaves to the small window;
        // the cog, a menu of speeds and tracks; then how it plays.
        let mut buttons = vec![Command::ToggleFullscreen, Command::PlaybackSettings, Command::CycleLoop, Command::ToggleMute];
        if subtitles > 0 {
            buttons.push(Command::CycleSubtitles);
        }
        if audio > 1 {
            buttons.push(Command::CycleAudio);
        }
        buttons
    }

    /// Has the spare player hold the file before this one, a seek's length
    /// from its end, once seeking back has come near enough to the start
    /// of this video that the next press may land there.
    pub(super) fn preload_back(&mut self) {
        let len = self.list.len();
        let near = self.video_time().is_some_and(|(position, _)| position < SEEK_STEP * 2.0);
        if !near || len < 2 {
            return;
        }
        let before = self.list[(self.pos + len - 1) % len];
        if !files::is_video(&self.files[before]) || before == self.id() {
            return;
        }
        let kept = self.kept_tracks(before);
        let Some(spare) = self.spare.as_mut().filter(|spare| spare.id != Some(before) || spare.from != -SEEK_STEP) else { return };
        if let Some((aid, sid, sub_delay, audio_delay)) = &kept {
            spare.player.choose_next(aid, sid, *sub_delay, *audio_delay);
        }
        spare.player.hold(&self.files[before], Some(-SEEK_STEP));
        (spare.id, spare.from) = (Some(before), -SEEK_STEP);
    }

    /// Has a video play again whenever it ends, or no longer, whatever the
    /// list's loop: from the loop on its row in the file list.
    pub(super) fn toggle_file_loop(&mut self, id: usize) {
        // Every video loops already: that is the list's loop to change.
        if self.looping == Looping::File {
            return self.toast(format!("{}  Every video loops · l to change", commands::icons::REPEAT_1));
        }
        let looped = !self.looped.remove(&id);
        if looped {
            self.looped.insert(id);
        }
        // The one playing takes it at once; one at its end plays again.
        if self.video.as_ref().is_some_and(|video| video.id == id) {
            self.apply_loop();
            let ended = self.player.as_ref().is_some_and(|player| player.flag("eof-reached") == Some(true));
            if looped && ended {
                self.restart_video();
            }
        }
        let icon = commands::icons::REPEAT_1;
        let name = self.name_of(id);
        self.toast(if looped { format!("{icon}  Loop · {name}") } else { format!("Loop off · {name}") });
    }

    /// Tells the player whether a video is to play again when it ends:
    /// every one, with the loop on this video, or one looped on its own.
    pub(super) fn apply_loop(&self) {
        if let Some(player) = &self.player {
            let own = self.video.as_ref().is_some_and(|video| self.looped.contains(&video.id));
            player.set("loop-file", if self.looping == Looping::File || own { "inf" } else { "no" });
        }
        // The section of this video, or none: a video gone on to does not
        // loop the one before's.
        self.apply_section_loop();
    }

    /// Handles the keys that mean something else while a video plays.
    /// Returns whether the key was one of them.
    pub(super) fn video_key(&mut self, key: Key<&str>, repeat: bool) -> bool {
        let modifiers = self.modifiers;
        if self.player.is_none() || modifiers.alt_key() {
            return false;
        }
        // Escape unmarks a section before it does anything else.
        if key == Key::Named(NamedKey::Escape) && self.section().is_some() {
            return self.clear_section();
        }
        // With the scenes up, the arrows go from one of them to the next.
        if self.scenes_shown() && !modifiers.control_key() && !modifiers.shift_key() {
            let way = match key {
                Key::Named(NamedKey::ArrowRight) => Some(true),
                Key::Named(NamedKey::ArrowLeft) => Some(false),
                _ => None,
            };
            let jumped = match self.strip_tab {
                StripTab::Scenes => way.is_some_and(|forward| self.scene_jump(forward)),
                StripTab::Sections => way.is_some_and(|forward| self.entry_jump(forward)),
            };
            if jumped {
                return true;
            }
        }
        // A held key goes in the same strides as a pressed one.
        let step = SEEK_STEP;
        let arrow = |forward: bool| {
            if modifiers.control_key() {
                VideoAction::File(forward)
            } else if modifiers.shift_key() {
                VideoAction::Frame(forward)
            } else {
                VideoAction::Seek(if forward { step } else { -step })
            }
        };
        let action = match key {
            // The repeats of a space bar held down say nothing new.
            Key::Named(NamedKey::Space) if repeat => return true,
            // Let go at once it pauses or plays; kept down, it hurries the
            // video along. Which, only its release can say.
            Key::Named(NamedKey::Space) => {
                self.begin_hold(true);
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => arrow(true),
            Key::Named(NamedKey::ArrowLeft) => arrow(false),
            // Page Down and Up are the next and the file before, as > and
            // < are; the arrows are for getting about in the video.
            Key::Named(NamedKey::PageDown | NamedKey::PageUp) => return false,
            Key::Named(NamedKey::ArrowUp) => VideoAction::Volume(5),
            Key::Named(NamedKey::ArrowDown) => VideoAction::Volume(-5),
            _ => return false,
        };
        // A held key seeks only as fast as the seeks land. A repeat that
        // comes while one is still on its way, or while a file just gone
        // on to has yet to show, is let go: nothing is left over to play
        // out once the key is released, and each jump is one stride.
        let busy = self.video.as_ref().is_some_and(|video| !video.showing || video.seek_asked.is_some_and(|asked| asked.elapsed() < SEEK_SETTLE));
        if repeat && busy && matches!(action, VideoAction::Seek(_)) {
            return true;
        }
        // The files lie end to end as one timeline: a seek that would run
        // off the end of this video starts the next, and one that would run
        // off its start lands that far from the end of the one before,
        // however the key came to be pressed.
        if let VideoAction::Seek(seconds) = action {
            if let Some(enter) = self.seek_across(seconds) {
                self.enter_at = Some(enter);
                self.step(seconds > 0.0);
                self.enter_at = None;
                // Going on back may run into the file before that one next.
                if seconds < 0.0 {
                    self.preload_back();
                }
                return true;
            }
        }
        self.video_action(action);
        true
    }

    /// Where a seek of this many seconds lands in the file after or before
    /// this one, when it would run off the end or the start of the video:
    /// the start of the next, as zero, or as a negative number of seconds from
    /// the end of the previous. `None` when it stays within this video.
    pub(super) fn seek_across(&self, seconds: f64) -> Option<f64> {
        let (position, length) = self.video_time()?;
        let ended = self.player.as_ref().is_some_and(|player| player.flag("eof-reached") == Some(true));
        let to = if ended { length } else { position } + seconds;
        if self.list.len() < 2 {
            None
        } else if seconds > 0.0 && to >= length - VIDEO_END {
            Some(0.0)
        } else {
            // Less than this short of the start is the start. Further, and
            // it is the whole of the seek from the end of the file before,
            // which is where that file can be held ready.
            (seconds < 0.0 && to < -VIDEO_START).then_some(-seconds.abs())
        }
    }

    pub(super) fn video_action(&mut self, action: VideoAction) {
        // A step among the frames kept from before the pause, at once; and
        // anything else is from the frame on show, the main player put on
        // it first.
        match action {
            VideoAction::Frame(forward) if self.back_step(forward) => return,
            VideoAction::Frame(_) | VideoAction::Volume(_) => {}
            _ => self.leave_back(),
        }
        // Stopped at the end of the last video, play plays it again.
        let ended = self.video.is_some() && self.player.as_ref().is_some_and(|player| player.flag("eof-reached") == Some(true));
        if action == VideoAction::Pause && ended {
            return self.restart_video();
        }
        let (Some(player), Some(video)) = (&self.player, &mut self.video) else { return };
        // What pausing or playing turns it into: the player's own word for
        // it comes later, with its events.
        let was_paused = player.flag("pause") == Some(true);
        match action {
            VideoAction::File(forward) => return self.step(forward),
            // Queued, not waited for: the player's core gets to a command
            // only when it is done with what it has in hand, which in the
            // middle of a seek is the rest of the seek.
            VideoAction::Pause => player.command_later(&[b"cycle", b"pause"]),
            VideoAction::Seek(seconds) => {
                // To the nearest keyframe, which the player can show at
                // once, unless the very moment asked for is wanted: getting
                // there means decoding every frame from the keyframe before.
                let exact = self.precise;
                let how: &[u8] = if exact { b"relative+exact" } else { b"relative+keyframes" };
                player.command_later(&[b"seek", seconds.to_string().as_bytes(), how]);
                // mpv has not moved yet when asked straight after a seek, so
                // quick presses build on where the last one was heading.
                let from = match (video.seeking, video.aimed) {
                    (Some((at, target)), _) | (None, Some((at, target))) if at.elapsed() < SEEK_SETTLE => target,
                    _ => player.number("time-pos").unwrap_or(0.0),
                };
                let length = player.number("duration").unwrap_or(0.0);
                let target = (from + seconds).clamp(0.0, length);
                // Where it lands is only known beforehand when that is
                // exact; to a keyframe, the bar stands where it was aimed
                // at until playback gets there.
                video.seeking = exact.then(|| (Instant::now(), target));
                video.aimed = (!exact).then(|| (Instant::now(), target));
                video.aim_reached = None;
                video.to_end = (target >= length - VIDEO_END).then(Instant::now);
                video.seek_asked.get_or_insert_with(Instant::now);
            }
            // Both leave the video paused on the frame they land on.
            VideoAction::Frame(true) => {
                player.command_later(&[b"frame-step"]);
                video.to_end = Some(Instant::now());
            }
            VideoAction::Frame(false) => player.command_later(&[b"frame-back-step"]),
            VideoAction::Volume(change) => {
                player.command(&[b"add", b"volume", change.to_string().as_bytes()]);
                let volume = player.number("volume").unwrap_or(0.0);
                return self.toast(format!("Volume {volume:.0}%"));
            }
        }
        // Pausing says so for a moment, at the top right with the other
        // notes; playing again takes the word away.
        if action == VideoAction::Pause {
            let paused = !was_paused;
            match paused {
                true => self.toast(format!("{}  Paused", commands::icons::PAUSE)),
                false => self.toast = None,
            }
        }
        if matches!(action, VideoAction::Seek(_)) && self.video_time().is_some_and(|(position, _)| position >= SEEN_AFTER.as_secs_f64()) {
            if let Some(id) = self.video.as_ref().map(|video| video.id) {
                self.mark_seen(id);
            }
        }
        // The file a seek may be about to run into is held ready: the one
        // before once the start is near, and otherwise the next.
        match action {
            VideoAction::Seek(seconds) if seconds < 0.0 => self.preload_back(),
            VideoAction::Seek(_) => self.preload_video(),
            _ => {}
        }
        // Bring up the time and the larger progress bar for a moment.
        self.time_until = Some(Instant::now() + TOAST);
        self.redraw();
    }

    /// Moves the playback speed a step up or down the list, or with no
    /// direction back to normal. It applies to every video from then on.
    pub(super) fn change_speed(&mut self, faster: Option<bool>) {
        let at = SPEEDS.iter().position(|speed| *speed >= self.speed).unwrap_or(SPEEDS.len() - 1);
        self.speed = match faster {
            Some(true) => SPEEDS[(at + 1).min(SPEEDS.len() - 1)],
            Some(false) => SPEEDS[at.saturating_sub(1)],
            None => 1.0,
        };
        self.set_speed(self.speed);
    }

    /// Plays at `speed` from now on, this video and the ones after.
    pub(super) fn set_speed(&mut self, speed: f64) {
        self.speed = speed;
        if let Some(player) = &self.player {
            player.set("speed", &self.playing_speed().to_string());
        }
        self.toast(format!("Speed {}x", self.speed));
    }

    /// Where playback is and how long the video is, in seconds.
    pub(super) fn video_time(&self) -> Option<(f64, f64)> {
        let (player, video) = (self.player.as_ref()?, self.video.as_ref()?);
        // Until the player says it has this file open, what it says of the
        // time is of the file before: going to another while one was paused
        // halfway, the bar stood at half before it went back to the start.
        let open = player.text("path").is_some_and(|open| std::path::Path::new(&open) == self.files[video.id].as_path());
        if !open {
            return None;
        }
        let length = player.number("duration").filter(|length| *length > 0.0)?;
        // A frame kept from before a pause, on show: its own time. Straight
        // after a seek mpv still reports where it was.
        let position = match (self.back_shown(), video.seeking) {
            (Some((at, _)), _) => at,
            (None, Some((at, target))) if at.elapsed() < SEEK_SETTLE => target,
            // To a keyframe before where it was aimed: there, not back at
            // the keyframe, until playback has caught up with it (`settle_aim`).
            (None, _) => match video.aimed {
                Some((_, aim)) => aim,
                None => player.number("time-pos")?,
            },
        };
        Some((position.clamp(0.0, length), length))
    }

    pub(super) fn reset_view(&mut self) {
        if !self.keep_view {
            self.view = View { zoom: self.options.zoom, center: None, enlarge: self.view.enlarge };
        }
    }

    pub(super) fn step(&mut self, forward: bool) {
        let len = self.list.len();
        let pos = if forward { (self.pos + 1) % len } else { (self.pos + len - 1) % len };
        self.go_to(pos, forward);
    }

    /// Moves about 5% of the way through the list.
    pub(super) fn jump(&mut self, forward: bool) {
        let len = self.list.len() as i64;
        let distance = (len / 20).max(1) * if forward { 1 } else { -1 };
        self.go_to((self.pos as i64 + distance).rem_euclid(len) as usize, forward);
    }

    /// Position of the first image of the next or previous directory.
    pub(super) fn other_dir(&self, forward: bool) -> Option<usize> {
        let len = self.list.len();
        let dir = |pos: usize| self.files[self.list[pos]].parent();
        let step = |pos: usize| if forward { (pos + 1) % len } else { (pos + len - 1) % len };

        let here = dir(self.pos);
        let mut pos = step(self.pos);
        while pos != self.pos && dir(pos) == here {
            pos = step(pos);
        }
        if pos == self.pos {
            return None;
        }
        if !forward {
            let there = dir(pos);
            while pos > 0 && dir(pos - 1) == there {
                pos -= 1;
            }
        }
        Some(pos)
    }

    pub(super) fn remove_current(&mut self, event_loop: &ActiveEventLoop) {
        let id = self.list.remove(self.pos);
        self.transforms.remove(&id);
        self.marks.remove(&id);
        if let Some((_, all)) = &mut self.filter {
            all.retain(|other| *other != id);
        }
        if !self.refill(event_loop) {
            return;
        }
        if self.pos >= self.list.len() {
            self.pos = 0;
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
    }

    /// Called when files have left the list. If none of a kept search's
    /// matches remain, goes back to all the files; if no files remain at
    /// all, quits. Returns whether there is still something to show.
    pub(super) fn refill(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.list.is_empty() {
            return true;
        }
        match self.filter.take() {
            Some((_, all)) if !all.is_empty() => {
                self.list = all;
                self.pos = 0;
                if let Some(grid) = &mut self.grid {
                    grid.cursor = 0;
                }
                true
            }
            _ => {
                event_loop.exit();
                false
            }
        }
    }

    pub(super) fn advance_slideshow(&mut self, event_loop: &ActiveEventLoop) {
        self.slide_at = Some(Instant::now() + self.slide_delay);
        if self.pos + 1 == self.list.len() {
            match self.options.on_last_slide {
                LastSlide::Resume => {}
                LastSlide::Hold => {
                    self.slide_at = None;
                    return;
                }
                LastSlide::Quit => {
                    event_loop.exit();
                    return;
                }
            }
        }
        self.step(true);
    }

}
