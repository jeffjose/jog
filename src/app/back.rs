//! Stepping back a frame at once. mpv steps back by going to the keyframe
//! before and decoding forward from it, which on a long-GOP 4K video takes
//! seconds a step. So once a video has been paused a moment, a player of
//! its own, on a thread of its own, opens the same file a little earlier
//! and steps forward to where it is paused, drawing each frame as the main
//! player would draw it: the same size, view and colour. Shift+Left then
//! shows those, at once.
//!
//! Leaving them, by playing, seeking or anything but stepping, puts the
//! main player on the very frame shown, so that nothing jumps.

use std::sync::mpsc;

use super::*;

/// How far back from the pause the frames are fetched, in seconds.
const SPAN: f64 = 2.0;
/// How long a pause has to hold before they are, so that pausing for a
/// moment between seeks does not start it each time.
const SETTLE: Duration = Duration::from_millis(300);
/// The most memory the frames may take.
const BUDGET: usize = 1536 << 20;
/// How long the back player is waited on for a frame before it is given up.
const PATIENCE: Duration = Duration::from_secs(5);

/// Frames, oldest first, each with its time.
type Frames = VecDeque<(f64, Vec<u32>)>;

/// What the back player is asked to fetch.
pub struct Job {
    ticket: u64,
    path: PathBuf,
    anchor: f64,
    size: (u32, u32),
    view: Option<[String; 4]>,
    sid: Option<String>,
    half_frame: f64,
    most: usize,
}

/// The frames before a pause, and which is on show.
pub struct BackFrames {
    ticket: u64,
    id: usize,
    /// Where the main player is paused, in seconds.
    anchor: f64,
    /// The size they were drawn at, and the view, as the main player had
    /// them: drawn at another, they are no longer what it would show.
    size: (u32, u32),
    view: Option<[String; 4]>,
    frames: Frames,
    /// Still being fetched.
    filling: bool,
    /// The one on show instead of the main player's, if any.
    shown: Option<usize>,
    /// Steps asked for while they were still being fetched, back being
    /// less than nothing: taken once they are all there.
    pending: i32,
    /// Half a frame's time: nearer than this, two times are one frame.
    half_frame: f64,
}

/// The back player's thread: fetches what it is asked, the latest ask
/// first, and sends the frames back when they are all there.
fn worker(jobs: mpsc::Receiver<Job>, proxy: EventLoopProxy<Event>) {
    let (wake_tx, wake) = mpsc::channel::<Signal>();
    let Ok(player) = Player::new(Role::Playback { silent: true }, move |signal| {
        let _ = wake_tx.send(signal);
    }) else {
        return;
    };
    // Each step shown as soon as it is decoded, not when its time would
    // come in playing: stepping is otherwise no quicker than playing.
    player.set("untimed", "yes");
    let mut next = None;
    loop {
        let Some(mut job) = next.take().or_else(|| jobs.recv().ok()) else { return };
        while let Ok(later) = jobs.try_recv() {
            job = later;
        }
        let started = Instant::now();
        player.hold(&job.path, Some((job.anchor - SPAN).max(0.0)));
        // Drawn as the main player draws it: the same view, filters and
        // subtitles.
        if let Some([zoom, pan_x, pan_y, filter]) = &job.view {
            player.set("video-zoom", zoom);
            player.set("video-pan-x", pan_x);
            player.set("video-pan-y", pan_y);
            player.set_filter(filter);
        }
        player.set("aid", "no");
        if let Some(sid) = &job.sid {
            player.set("sid", sid);
        }
        let (w, h) = job.size;
        let mut frames: Frames = VecDeque::new();
        let mut finished = false;
        while !finished {
            // A newer ask: this one is dropped for it.
            if let Ok(later) = jobs.try_recv() {
                next = Some(later);
                break;
            }
            match wake.recv_timeout(PATIENCE) {
                Ok(Signal::Events) => drop(player.events()),
                Ok(_) if player.frame_waiting() => {
                    let Some(at) = player.number_now("time-pos") else { continue };
                    if at >= job.anchor - job.half_frame {
                        finished = true;
                        continue;
                    }
                    let mut pixels = vec![0u32; w as usize * h as usize];
                    if player.render(&mut pixels, w, h) && frames.back().is_none_or(|(last, _)| at > last + job.half_frame) {
                        frames.push_back((at, pixels));
                        if frames.len() > job.most {
                            frames.pop_front();
                        }
                    }
                    player.command_later(&[b"frame-step"]);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        player.stop();
        if finished {
            let _ = proxy.send_event(Event::BackFrames(job.ticket, frames, started.elapsed()));
        }
    }
}

impl App {
    /// After the main player has said what it has to say: once paused a
    /// moment, the frames before are to be fetched; playing, they are of
    /// no more use.
    pub(super) fn tend_back(&mut self) {
        let Some(player) = &self.player else { return };
        let Some(video) = self.video.as_ref().filter(|video| video.showing) else { return };
        if player.flag("pause") != Some(true) {
            if self.back.as_ref().is_some_and(|back| back.shown.is_none()) {
                self.back = None;
            }
            self.back_due = None;
            return;
        }
        let Some(position) = player.number("time-pos") else { return };
        let fresh = self.back.as_ref().is_some_and(|back| {
            back.id == video.id && (back.shown.is_some() || (back.anchor - position).abs() < back.half_frame.max(0.004)) && back.size == self.window_size()
        });
        if !fresh && self.back_due.is_none() && player.flag("seeking") != Some(true) {
            self.back_due = Some(Instant::now() + SETTLE);
        }
    }

    /// The pause has held: the frames before it are asked of the back
    /// player.
    pub(super) fn fill_back(&mut self) {
        self.back_due = None;
        let Some(player) = &self.player else { return };
        let Some(video) = self.video.as_ref().filter(|video| video.showing) else { return };
        if player.flag("pause") != Some(true) {
            return;
        }
        let Some(anchor) = player.number("time-pos") else { return };
        if anchor < 0.05 {
            return;
        }
        let fps = player.number("container-fps").or_else(|| player.number("estimated-vf-fps")).filter(|fps| *fps > 1.0).unwrap_or(30.0);
        let size = self.window_size();
        let bytes = size.0 as usize * size.1 as usize * 4;
        let most = (BUDGET / bytes.max(1)).clamp(4, (SPAN * fps).ceil() as usize + 2);
        let view = self.video_view_sent.borrow().clone();
        let sid = player.text("sid");
        let half_frame = 0.5 / fps;
        self.back_ticket += 1;
        let ticket = self.back_ticket;
        if self.options.debug {
            eprintln!("jog: back: fetching {:.3}-{anchor:.3} at {}x{}, {most} frames at most", (anchor - SPAN).max(0.0), size.0, size.1);
        }
        let job = Job { ticket, path: self.files[video.id].clone(), anchor, size, view: view.clone(), sid, half_frame, most };
        let proxy = self.proxy.clone();
        let jobs = self.back_jobs.get_or_insert_with(|| {
            let (send, receive) = mpsc::channel();
            std::thread::Builder::new().name("back".into()).spawn(move || worker(receive, proxy)).ok();
            send
        });
        if jobs.send(job).is_err() {
            self.back_jobs = None;
            return;
        }
        self.back = Some(BackFrames { ticket, id: video.id, anchor, size, view, frames: VecDeque::new(), filling: true, shown: None, pending: 0, half_frame });
    }

    /// The frames before the pause are all there; the steps asked for
    /// meanwhile are taken.
    pub(super) fn back_frames(&mut self, ticket: u64, frames: Frames, took: Duration) {
        let Some(back) = self.back.as_mut().filter(|back| back.ticket == ticket) else { return };
        if self.options.debug {
            eprintln!("jog: back: {} frames kept, {:.3}-{:.3}, in {took:.1?}", frames.len(), frames.front().map_or(0.0, |f| f.0), frames.back().map_or(0.0, |f| f.0));
        }
        (back.frames, back.filling) = (frames, false);
        let pending = std::mem::take(&mut back.pending);
        for _ in 0..pending.unsigned_abs() {
            if !self.back_step(pending > 0) {
                break;
            }
        }
    }

    /// Whether the frames kept are of what is on screen now.
    fn back_matches(&self) -> bool {
        let (Some(back), Some(video)) = (&self.back, &self.video) else { return false };
        back.id == video.id && back.size == self.window_size() && back.view == *self.video_view_sent.borrow()
    }

    /// Shift+Left or Shift+Right among the frames kept. Whether it was
    /// done here; if not, the main player is to step.
    pub(super) fn back_step(&mut self, forward: bool) -> bool {
        if !self.back_matches() {
            return false;
        }
        let Some(back) = &mut self.back else { return false };
        // Still being fetched: the step waits for them, they being much
        // nearer than a step the slow way.
        if back.filling {
            back.pending += if forward { 1 } else { -1 };
            if back.pending > 0 {
                back.pending = 0;
                return false;
            }
            return true;
        }
        if back.frames.is_empty() {
            return false;
        }
        match (back.shown, forward) {
            // Live, and forward: the main player's own step.
            (None, true) => return false,
            (None, false) => back.shown = Some(back.frames.len() - 1),
            (Some(index), true) => back.shown = (index + 1 < back.frames.len()).then_some(index + 1),
            // Before the first kept: the main player goes there, and steps
            // back from it the slow way.
            (Some(0), false) => {
                self.leave_back();
                return false;
            }
            (Some(index), false) => back.shown = Some(index - 1),
        }
        self.time_until = Some(Instant::now() + TOAST);
        self.redraw();
        true
    }

    /// The frame on show instead of the main player's, and its time.
    pub(super) fn back_shown(&self) -> Option<(f64, &[u32])> {
        let back = self.back.as_ref()?;
        let index = back.shown?;
        let (at, pixels) = back.frames.get(index)?;
        Some((*at, pixels))
    }

    /// Done with the frames kept for now: the main player is put on the
    /// one on show, and those after it are let go, it being the pause now.
    pub(super) fn leave_back(&mut self) {
        let Some(back) = &mut self.back else { return };
        let Some(index) = back.shown.take() else { return };
        let Some(at) = back.frames.get(index).map(|(at, _)| *at) else { return };
        back.frames.truncate(index);
        back.anchor = at;
        if let Some(player) = &self.player {
            player.command_later(&[b"seek", format!("{at:.6}").as_bytes(), b"absolute+exact"]);
        }
        if let Some(video) = &mut self.video {
            video.seeking = Some((Instant::now(), at));
        }
        self.redraw();
    }
}
