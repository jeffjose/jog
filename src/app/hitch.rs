//! Hitches, told of on the terminal as they happen: a video frame that came
//! late, frames mpv decoded too late to show, the main thread held up by
//! one key or event. Each line says what else was going on, so that jank
//! is caught and its cause named without anyone watching for it. A run
//! that goes smoothly prints nothing. `--no-hitches` turns it off.

use super::*;

/// A frame this much longer than the video's own is a hitch: one frame
/// missed shows as twice as long.
const LATE: f64 = 1.75;
/// Nor is a gap under this, whatever the frame rate: the display's own jitter.
const LEAST: Duration = Duration::from_millis(20);
/// After a pause, a seek or a new file, playback is given this to settle.
const SETTLE: Duration = Duration::from_millis(300);
/// A turn of the event loop longer than this holds everything up.
const STALL: Duration = Duration::from_millis(100);
/// What comes this soon after the last is one burst, told of in one line.
const BURST: Duration = Duration::from_secs(1);
/// The window's first moments, while things are set up, are not judged.
const STARTUP: Duration = Duration::from_secs(2);
/// A key or click this recent is named as what came before.
const RECENT: Duration = Duration::from_secs(2);

/// One hitch, and what was going on.
struct Hitch {
    /// How long the frame took, against how long it should have; none
    /// for frames dropped without a gap, or a stall away from video.
    gap: Option<(Duration, Duration)>,
    /// Where in the video, in seconds.
    at: Option<f64>,
    cause: String,
    /// What else was going on: work beside it, the last key, panels up.
    beside: Vec<String>,
}

pub(super) struct Hitches {
    pub(super) on: bool,
    launched: Instant,
    /// The last new video frame: when, and where in the video.
    last_frame: Option<(Instant, f64)>,
    /// Until when playback is still settling.
    settling: Option<Instant>,
    /// The longest turn of the event loop since the last frame, and on what.
    busiest: Option<(Duration, String)>,
    /// mpv's counts of frames decoded late and not shown, as last seen.
    drops: Option<(f64, f64)>,
    /// The last key or click, and when.
    input: Option<(Instant, String)>,
    /// Said once each time playback starts again.
    back_told: bool,
    burst: Vec<Hitch>,
    burst_ends: Option<Instant>,
    /// For the line on quitting.
    total: usize,
    worst: Duration,
    causes: HashMap<String, usize>,
    /// Every steady frame, for `JOG_FRAMESTATS`: measuring playback.
    pub(super) stats: FrameStats,
}

/// Steady playback, frame by frame, said on quitting with
/// `JOG_FRAMESTATS=1`: for comparing one way of playing with another.
#[derive(Default)]
pub(super) struct FrameStats {
    /// Gaps between frames shown, in ms.
    gaps: Vec<f64>,
    /// Drawing each, in ms.
    draws: Vec<f64>,
    /// Frames the time between them called for: what was shown and what
    /// was not.
    expected: f64,
    /// mpv's own drops: decoder, output.
    dropped: (f64, f64),
    /// Over the whole run: frames the player said were ready, and frames
    /// drawn from it; fewer drawn is frames passed over in between.
    pub(super) told: usize,
    pub(super) taken: usize,
}

impl Hitches {
    pub(super) fn new(on: bool) -> Hitches {
        Hitches {
            on,
            launched: Instant::now(),
            last_frame: None,
            settling: None,
            busiest: None,
            drops: None,
            input: None,
            back_told: false,
            burst: Vec::new(),
            burst_ends: None,
            total: 0,
            worst: Duration::ZERO,
            causes: HashMap::new(),
            stats: FrameStats::default(),
        }
    }

    /// When the burst gathering now is to be told of.
    pub(super) fn due(&self) -> Option<Instant> {
        self.burst_ends
    }

    /// A key pressed or a button clicked.
    pub(super) fn input(&mut self, what: String) {
        self.input = Some((Instant::now(), what));
    }

    /// The last key, if recent, as "after S 0.4s".
    fn after(&self, now: Instant) -> Option<String> {
        let (at, what) = self.input.as_ref()?;
        let ago = now.duration_since(*at);
        (ago < RECENT).then(|| format!("after {what} {:.1}s", ago.as_secs_f64()))
    }

    /// What a turn that began at `began` was on: the key or click that came
    /// in it, if one did, which a script or the timers may also send.
    fn on_what(&self, began: Instant, what: &str) -> String {
        match &self.input {
            Some((at, input)) if *at >= began => input.clone(),
            _ => what.to_string(),
        }
    }

    /// Video playback stopped being continuous: paused, seeked, or gone.
    pub(super) fn break_frames(&mut self) {
        self.last_frame = None;
    }

    /// A turn of the event loop has ended, having taken `took` on `what`.
    /// Returns a stall to tell of, when it was long and no video frame
    /// will carry it.
    fn turn(&mut self, took: Duration, what: &str, playing: bool) -> Option<Hitch> {
        if self.busiest.as_ref().is_none_or(|(most, _)| took > *most) {
            self.busiest = Some((took, what.to_string()));
        }
        if took < STALL || self.launched.elapsed() < STARTUP || playing {
            return None;
        }
        Some(Hitch { gap: None, at: None, cause: format!("stall {} on {what}", ms_short(took)), beside: Vec::new() })
    }

    /// A new video frame is up, `pos` seconds in, the video's own frames
    /// lasting `period`, with mpv's drop counts. Returns a hitch, if it
    /// was one, its cause and what else is named left to the caller.
    fn frame(&mut self, now: Instant, pos: f64, period: Duration, drops: (f64, f64)) -> Option<(Option<(Duration, Duration)>, f64, Option<(Duration, String)>)> {
        let before = self.last_frame.replace((now, pos));
        let busiest = self.busiest.take();
        let before_drops = self.drops;
        let dropped = self.drops.replace(drops).map_or(0.0, |(late, lost)| (drops.0 - late).max(0.0) + (drops.1 - lost).max(0.0));
        let Some((then, was)) = before else {
            self.settling = Some(now + SETTLE);
            return None;
        };
        let gap = now.duration_since(then);
        let moved = pos - was;
        let wall = gap.as_secs_f64();
        // Back to the start, a loop going round, or on far ahead, a seek:
        // not a frame following the one before. The time along, as last
        // reported, may not have moved since: the frame is new all the same.
        // So is a long while with the video hardly on, a pause.
        if moved < -0.05 || moved > wall * 4.0 + 0.5 || (wall > 0.25 && moved < wall * 0.4) {
            self.settling = Some(now + SETTLE);
            return None;
        }
        if self.settling.is_some_and(|until| now < until) {
            return None;
        }
        self.settling = None;
        self.stats.gaps.push(wall * 1000.0);
        self.stats.expected += wall / period.as_secs_f64();
        if let Some((late, lost)) = before_drops {
            self.stats.dropped.0 += (drops.0 - late).max(0.0);
            self.stats.dropped.1 += (drops.1 - lost).max(0.0);
        }
        let late = gap > period.mul_f64(LATE).max(LEAST);
        (late || dropped > 0.0).then_some((late.then_some((gap, period)), dropped, busiest))
    }

    /// Adds a hitch to the burst gathering.
    fn add(&mut self, hitch: Hitch, now: Instant) {
        self.total += 1;
        if let Some((gap, _)) = hitch.gap {
            self.worst = self.worst.max(gap);
        }
        *self.causes.entry(kind(&hitch.cause).to_string()).or_default() += 1;
        self.burst.push(hitch);
        self.burst_ends = Some(now + BURST);
    }

    /// Tells of the burst, if one has gathered and is over, or at once
    /// with `now` none.
    pub(super) fn flush(&mut self, now: Option<Instant>) {
        if self.burst_ends.is_some_and(|ends| now.is_none_or(|now| ends <= now)) {
            self.burst_ends = None;
            let burst = std::mem::take(&mut self.burst);
            if let Some(line) = describe(&burst) {
                eprintln!("{line}");
            }
        }
    }

    /// The line on quitting, when there was more than one.
    pub(super) fn summary(&mut self) {
        self.flush(None);
        if std::env::var_os("JOG_FRAMESTATS").is_some() {
            eprintln!("{}", self.stats.line());
        }
        if self.total < 2 {
            return;
        }
        let mut line = format!("jog: {} hitches", self.total);
        if self.worst > Duration::ZERO {
            line.push_str(&format!(" · worst {}", ms_short(self.worst)));
        }
        if let Some((cause, _)) = self.causes.iter().max_by_key(|(cause, count)| (**count, std::cmp::Reverse((*cause).clone()))) {
            line.push_str(&format!(" · mostly {cause}"));
        }
        eprintln!("{line}");
        self.total = 0;
    }
}

impl FrameStats {
    fn line(&self) -> String {
        let pct = |values: &[f64], p: f64| {
            let mut sorted = values.to_vec();
            sorted.sort_by(f64::total_cmp);
            sorted.get(((sorted.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0)
        };
        let shown = self.gaps.len() as f64;
        let wall: f64 = self.gaps.iter().sum::<f64>() / 1000.0;
        format!(
            "FRAMESTATS shown={} missed={:.0} missed_pct={:.1} fps={:.1} gap_p50={:.1} gap_p99={:.1} gap_max={:.1} dec_drop={:.0} vo_drop={:.0} draw_p50={:.1} draw_p95={:.1} told={} taken={}",
            self.gaps.len(),
            (self.expected - shown).max(0.0),
            100.0 * (self.expected - shown).max(0.0) / self.expected.max(1.0),
            shown / wall.max(1e-9),
            pct(&self.gaps, 0.5),
            pct(&self.gaps, 0.99),
            pct(&self.gaps, 1.0),
            self.dropped.0,
            self.dropped.1,
            pct(&self.draws, 0.5),
            pct(&self.draws, 0.95),
            self.told,
            self.taken,
        )
    }
}

/// A cause without its figures, to count alike ones together: "main busy
/// 41ms on S" is "main busy".
fn kind(cause: &str) -> &str {
    let end = cause.find(|c: char| c.is_ascii_digit()).unwrap_or(cause.len());
    cause[..end].trim_end_matches([' ', ','])
}

/// A duration as these lines write it: 48ms, 2.7s.
fn ms_short(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 1.0 { format!("{:.0}ms", seconds * 1000.0) } else { format!("{seconds:.1}s") }
}

/// The line for a burst of hitches: in full for one, summed up for more.
fn describe(burst: &[Hitch]) -> Option<String> {
    let first = burst.first()?;
    let at = |at: Option<f64>| at.map(|at| format!(" at {at:.1}s")).unwrap_or_default();
    let mut parts = Vec::new();
    if let [hitch] = burst {
        parts.push(match (hitch.gap, hitch.cause.starts_with("stall")) {
            (_, true) => format!("jog: {}", hitch.cause),
            (Some((gap, period)), _) => format!("jog: hitch {}{} (frames {}) · {}", ms_short(gap), at(hitch.at), ms_short(period), hitch.cause),
            (None, _) => format!("jog: frames dropped{} · {}", at(hitch.at), hitch.cause),
        });
    } else {
        let span = burst.len();
        let gaps: Vec<(Duration, Duration)> = burst.iter().filter_map(|hitch| hitch.gap).collect();
        let mut head = format!("jog: {span} hitches{}", at(first.at));
        if let Some((worst, period)) = gaps.iter().max_by_key(|(gap, _)| *gap) {
            head.push_str(&format!(" · worst {} (frames {})", ms_short(*worst), ms_short(*period)));
        }
        parts.push(head);
        // Each kind of cause once, the commonest first, with how often.
        let mut kinds: Vec<(&str, usize, &str)> = Vec::new();
        for hitch in burst {
            match kinds.iter_mut().find(|(kind_of, _, _)| *kind_of == kind(&hitch.cause)) {
                Some(found) => found.1 += 1,
                None => kinds.push((kind(&hitch.cause), 1, &hitch.cause)),
            }
        }
        kinds.sort_by_key(|(_, count, _)| std::cmp::Reverse(*count));
        parts.push(kinds.iter().map(|(_, count, cause)| if *count > 1 { format!("{cause} ×{count}") } else { cause.to_string() }).collect::<Vec<_>>().join(", "));
    }
    // What went on beside, as each hitch saw it, each said once.
    let mut beside: Vec<&str> = Vec::new();
    for hitch in burst {
        for what in &hitch.beside {
            if !beside.contains(&what.as_str()) && !(what.starts_with("after ") && beside.iter().any(|seen| seen.starts_with("after "))) {
                beside.push(what);
            }
        }
    }
    parts.extend(beside.into_iter().map(str::to_string));
    Some(parts.join(" · "))
}

/// The system's load against its cores, when it is high: "load 7.9/8".
fn load() -> Option<String> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    let load: f64 = text.split_whitespace().next()?.parse().ok()?;
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    (load > cores as f64 * 0.75).then(|| format!("load {load:.1}/{cores}"))
}

/// A key as these lines name it: Shift+Right, Ctrl+P, S, Space.
pub(super) fn key_name(key: Key<&str>, modifiers: ModifiersState) -> String {
    let mut name = String::new();
    for (held, word) in [(modifiers.control_key(), "Ctrl+"), (modifiers.alt_key(), "Alt+"), (modifiers.super_key(), "Super+")] {
        if held {
            name.push_str(word);
        }
    }
    match key {
        Key::Character(text) => {
            if modifiers.shift_key() && text.chars().all(|c| !c.is_alphabetic()) {
                name.push_str("Shift+");
            }
            // Ctrl+P as menus write it, not Ctrl+p.
            let text = if name.is_empty() { text.to_string() } else { text.to_uppercase() };
            name.push_str(if text == " " { "Space" } else { &text });
        }
        Key::Named(named) => {
            if modifiers.shift_key() {
                name.push_str("Shift+");
            }
            let word = format!("{named:?}");
            name.push_str(word.strip_prefix("Arrow").unwrap_or(&word));
        }
        _ => name.push_str("a key"),
    }
    name
}

/// What an event from elsewhere is, for a stall on it to name.
pub(super) fn event_name(event: &Event) -> &'static str {
    match event {
        Event::Loaded(_) => "a decoded picture",
        Event::Video(..) | Event::Spare(_) | Event::Player(_) => "player events",
        Event::Preview(_) | Event::PreviewPlayer(_) => "the seek preview",
        Event::BackFrames(..) => "back-step frames",
        Event::Converted(_) | Event::ConvertSource(..) | Event::ConvertTrial(..) | Event::ConvertScore(..) | Event::ConvertFit(..) => "converting",
        Event::Inspected(..) => "an inspection",
        Event::TracksWritten(..) => "tracks written",
        Event::FilesDone(_) => "files moved",
        Event::Listed(_) | Event::SpaceListed(..) => "a folder listing",
        Event::Scenes(_) => "scenes",
        _ => "an event",
    }
}

impl App {
    /// What else is going on, for a hitch line: work beside it, the
    /// last key, how loaded the system is, what is open over the video.
    fn hitch_beside(&self, now: Instant) -> Vec<String> {
        let mut beside = Vec::new();
        if back::decoding() {
            beside.push("back-step decoding".to_string());
        }
        if crate::transcode::busy() {
            beside.push("converting".to_string());
        }
        beside.extend(load());
        beside.extend(self.hitches.after(now));
        let open: Vec<&str> = [
            (self.show_list, "list"),
            (self.inspector.is_some(), "inspector"),
            (self.palette.is_some(), "palette"),
            (self.help, "help"),
            (self.show_debug, "debug"),
            (self.scenes_shown(), "scenes"),
            (self.strip_visible(), "strip"),
            (self.track_editor.is_some(), "tracks editor"),
            (self.track_picker.is_some(), "tracks"),
            (self.menu.is_some(), "menu"),
            (self.convert_panel.is_some(), "convert"),
            (self.video_panel.is_some(), "video convert"),
        ]
        .into_iter()
        .filter_map(|(up, name)| up.then_some(name))
        .collect();
        if !open.is_empty() {
            beside.push(format!("{} open", open.join(", ")));
        }
        beside
    }

    /// Whether a video is playing, not paused.
    fn video_playing(&self) -> bool {
        self.video.as_ref().is_some_and(|video| video.showing) && self.player.as_ref().is_some_and(|player| player.flag("pause") == Some(false))
    }

    /// A turn of the event loop is over: one that held everything up
    /// is told of, unless a video frame will be.
    pub(super) fn hitch_turn(&mut self, began: Instant, what: &str) {
        if !self.hitches.on {
            return;
        }
        let took = began.elapsed();
        let playing = self.video_playing();
        let what = self.hitches.on_what(began, what);
        if let Some(mut stall) = self.hitches.turn(took, &what, playing) {
            let now = Instant::now();
            stall.beside = self.hitch_beside(now);
            // The key that caused it is in the cause already.
            stall.beside.retain(|what| !what.starts_with("after "));
            self.hitches.add(stall, now);
        }
    }

    /// A new video frame has been drawn, in `drawing`, `overlays` of it
    /// for what is over the video.
    pub(super) fn hitch_frame(&mut self, drawing: Duration, overlays: Duration) {
        if !self.hitches.on {
            return;
        }
        let Some(player) = &self.player else { return };
        let paused = player.flag("pause") != Some(false);
        let Some(pos) = player.number("time-pos").filter(|_| !paused) else {
            return self.hitches.break_frames();
        };
        let fps = player.number("container-fps").filter(|fps| *fps > 0.0).unwrap_or(30.0);
        let count = |property| player.number(property).unwrap_or(0.0);
        let drops = (count("frame-drop-count"), count("vo-drop-frame-count"));
        let now = Instant::now();
        self.hitches.stats.draws.push((drawing + overlays).as_secs_f64() * 1000.0);
        self.hitches.stats.taken += 1;
        let Some((gap, dropped, busiest)) = self.hitches.frame(now, pos, Duration::from_secs_f64(1.0 / fps), drops) else {
            // Decoding frames before a pause, while the video plays on,
            // takes from it: it should have stopped when playing began.
            if back::decoding() && self.hitches.settling.is_none() && !std::mem::replace(&mut self.hitches.back_told, true) {
                eprintln!("jog: back-step still decoding during playback at {pos:.1}s");
            }
            if !back::decoding() {
                self.hitches.back_told = false;
            }
            return;
        };
        // Played faster or slower, frames come that much more or less often.
        // The speed is jog's own setting: asked of the player, it waited on
        // the player's core, tens of milliseconds just after a frame came
        // late, which made the next late too.
        let speed = Some(self.playing_speed()).filter(|speed| *speed > 0.0).unwrap_or(1.0);
        let gap = gap.map(|(gap, period)| (gap, period.div_f64(speed))).filter(|(gap, period)| *gap > period.mul_f64(LATE).max(LEAST));
        if gap.is_none() && dropped == 0.0 {
            return;
        }
        // The likeliest cause: the main thread held up, the frame itself
        // slow to draw, or else the decoder not keeping up.
        let half = gap.map_or(Duration::from_millis(8), |(gap, _)| gap / 2);
        let period = gap.map_or(Duration::from_secs_f64(1.0 / fps), |(_, period)| period);
        let cause = match busiest {
            Some((took, what)) if took >= half && what != "drawing" => format!("main busy {} on {what}", ms_short(took)),
            _ if drawing + overlays > period => format!("drawing {}+{}", ms_short(drawing), ms_short(overlays)),
            _ if dropped > 0.0 => format!("decoder behind, {dropped:.0} dropped"),
            _ => "decoder behind".to_string(),
        };
        let beside = self.hitch_beside(now);
        self.hitches.add(Hitch { gap, at: Some(pos), cause, beside }, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(40);

    fn played(hitches: &mut Hitches, start: Instant, steps: &[(u64, f64)]) -> Vec<bool> {
        steps.iter().map(|(ms, pos)| hitches.frame(start + Duration::from_millis(*ms), *pos, FRAME, (0.0, 0.0)).is_some()).collect()
    }

    /// Frames every 40ms for `count` frames, from `ms` on the clock and
    /// `pos` in the video.
    fn steady(ms: u64, pos: f64, count: u64) -> Vec<(u64, f64)> {
        (0..count).map(|frame| (ms + frame * 40, pos + frame as f64 * 0.04)).collect()
    }

    #[test]
    fn a_late_frame_in_steady_playback_is_a_hitch() {
        let mut hitches = Hitches::new(true);
        let start = Instant::now();
        // Settled after 300ms, then a frame 120ms after the last.
        let mut steps = steady(0, 0.0, 12);
        steps.extend(steady(560, 0.56, 3));
        let hitches: Vec<usize> = played(&mut hitches, start, &steps).iter().enumerate().filter(|(_, hitch)| **hitch).map(|(at, _)| at).collect();
        assert_eq!(hitches, [12]);
    }

    #[test]
    fn pauses_steps_and_seeks_are_not_hitches() {
        let mut hitches = Hitches::new(true);
        let start = Instant::now();
        let mut steps = steady(0, 0.0, 12);
        // Paused for two seconds: one frame on, then playing again.
        steps.extend(steady(2440, 0.48, 12));
        // A seek back, and one far forward.
        steps.extend(steady(2920, 0.1, 12));
        steps.extend(steady(3400, 60.0, 12));
        assert!(played(&mut hitches, start, &steps).iter().all(|hitch| !hitch));
    }

    #[test]
    fn frames_dropped_are_told_of_without_a_gap() {
        let mut hitches = Hitches::new(true);
        let start = Instant::now();
        played(&mut hitches, start, &[(0, 0.0), (400, 0.4)]);
        assert!(hitches.frame(start + Duration::from_millis(440), 0.44, FRAME, (2.0, 0.0)).is_some_and(|(gap, dropped, _)| gap.is_none() && dropped == 2.0));
    }

    #[test]
    fn a_burst_is_one_line_with_its_causes_counted() {
        let hitch = |ms: u64, cause: &str| Hitch { gap: Some((Duration::from_millis(ms), FRAME)), at: Some(6.3), cause: cause.into(), beside: vec!["back-step decoding".into()] };
        let one = describe(&[hitch(120, "decoder behind")]).unwrap();
        assert_eq!(one, "jog: hitch 120ms at 6.3s (frames 40ms) · decoder behind · back-step decoding");
        let many = describe(&[hitch(90, "decoder behind"), hitch(130, "main busy 70ms on S"), hitch(100, "decoder behind")]).unwrap();
        assert_eq!(many, "jog: 3 hitches at 6.3s · worst 130ms (frames 40ms) · decoder behind ×2, main busy 70ms on S · back-step decoding");
        let stall = Hitch { gap: None, at: None, cause: "stall 2.7s on S".into(), beside: vec![] };
        assert_eq!(describe(&[stall]).unwrap(), "jog: stall 2.7s on S");
    }

    #[test]
    fn keys_are_named_as_pressed() {
        assert_eq!(key_name(Key::Named(NamedKey::ArrowRight), ModifiersState::SHIFT), "Shift+Right");
        assert_eq!(key_name(Key::Character("S"), ModifiersState::SHIFT), "S");
        assert_eq!(key_name(Key::Character("p"), ModifiersState::CONTROL), "Ctrl+P");
        assert_eq!(key_name(Key::Character(" "), ModifiersState::empty()), "Space");
    }
}
