//! A section of a video, marked with Shift+A at its start and Shift+B at
//! its end: played over and over while it is marked, shown on the seek
//! bar, its ends dragged there to move them.

use super::*;

/// Either end of a section.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    A,
    B,
}

/// The ends marked so far, in seconds into the video.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Section {
    pub a: Option<f64>,
    pub b: Option<f64>,
}

impl Section {
    /// Both ends, when both are marked.
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.a?, self.b?))
    }
}

/// The least a section can be, in seconds: less and mpv would be looping
/// a frame or two at most, which is a stop and not a section.
const SHORTEST: f64 = 0.1;

/// How near the pointer must be to an end on the seek bar, in pixels, to
/// take hold of it rather than seek.
fn grip(bar: &SeekBar) -> f64 {
    f64::from(bar.unit)
}

impl App {
    /// The section of the video on view, if one is marked.
    pub(super) fn section(&self) -> Option<Section> {
        let video = self.video.as_ref()?;
        self.sections.get(&video.id).copied()
    }

    /// Marks an end of the section where the video is now.
    pub(super) fn mark_edge(&mut self, edge: Edge) {
        let Some(video) = &self.video else { return self.toast("A section is marked in a video") };
        let id = video.id;
        let Some((at, length)) = self.video_time() else { return };
        let mut section = self.sections.get(&id).copied().unwrap_or_default();
        match edge {
            Edge::A => {
                section.a = Some(at);
                // An end before the new start is no end of it.
                if section.b.is_some_and(|b| b <= at + SHORTEST) {
                    section.b = None;
                }
            }
            Edge::B => {
                let a = section.a.unwrap_or(0.0);
                if at <= a + SHORTEST {
                    return self.toast(format!("The end has to come after the start, at {}", clock(a)));
                }
                section = Section { a: Some(a), b: Some(at.min(length)) };
            }
        }
        self.sections.insert(id, section);
        self.apply_loop();
        self.say_section();
        self.redraw();
    }

    /// Unmarks the section of the video on view. Whether there was one.
    pub(super) fn clear_section(&mut self) -> bool {
        let Some(video) = &self.video else { return false };
        if self.sections.remove(&video.id).is_none() {
            return false;
        }
        self.apply_loop();
        self.toast("Section cleared");
        self.redraw();
        true
    }

    /// Says what is marked, and what to do next.
    fn say_section(&mut self) {
        let Some(section) = self.section() else { return };
        match section.span() {
            Some((a, b)) => self.toast(format!("Section {} – {} ({:.1} s), looping   Esc to clear", clock(a), clock(b), b - a)),
            None => self.toast(format!("Section from {}   Shift+B to end it", clock(section.a.unwrap_or(0.0)))),
        }
    }

    /// Tells the player to loop the section, or not to loop anything of
    /// its own when there is none.
    pub(super) fn apply_section_loop(&self) {
        let Some(player) = &self.player else { return };
        let span = self.section().and_then(|section| section.span());
        let (a, b) = span.map_or(("no".to_string(), "no".to_string()), |(a, b)| (format!("{a:.3}"), format!("{b:.3}")));
        player.set("ab-loop-a", &a);
        player.set("ab-loop-b", &b);
    }

    /// The end of the section near a point on the seek bar, to drag.
    pub(super) fn section_edge_at(&self, bar: &SeekBar, x: f64) -> Option<Edge> {
        let section = self.section()?;
        let (_, length) = self.video_time()?;
        let near = |at: Option<f64>| at.map(|at| (f64::from(bar.x) + f64::from(bar.width) * at / length - x).abs()).filter(|off| *off <= grip(bar));
        match (near(section.a), near(section.b)) {
            (Some(a), Some(b)) => Some(if b < a { Edge::B } else { Edge::A }),
            (Some(_), None) => Some(Edge::A),
            (None, Some(_)) => Some(Edge::B),
            (None, None) => None,
        }
    }

    /// An end dragged along the seek bar, to `fraction` of the video.
    pub(super) fn drag_edge(&mut self, edge: Edge, fraction: f64) {
        let Some(video) = &self.video else { return };
        let id = video.id;
        let Some((_, length)) = self.video_time() else { return };
        let Some(section) = self.sections.get_mut(&id) else { return };
        let at = fraction.clamp(0.0, 1.0) * length;
        // Kept on its own side of the other end.
        match edge {
            Edge::A => section.a = Some(section.b.map_or(at, |b| at.min(b - SHORTEST)).max(0.0)),
            Edge::B => section.b = Some(section.a.map_or(at, |a| at.max(a + SHORTEST)).min(length)),
        }
        self.hover = Some(fraction);
        self.request_preview(fraction);
        self.apply_loop();
        self.redraw();
    }

    /// The drag is over: the video goes to the end moved, to show the
    /// very frame it is now.
    pub(super) fn drop_edge(&mut self, edge: Edge) {
        let Some(section) = self.section() else { return };
        let Some((_, length)) = self.video_time() else { return };
        let at = match edge {
            Edge::A => section.a,
            Edge::B => section.b,
        };
        if let Some(at) = at {
            self.seek_to(at / length, true);
        }
        self.say_section();
    }

    /// Draws the section over the seek bar's track: the stretch between
    /// the ends in the accent, and each end as a flag to take hold of.
    pub(super) fn draw_section(&self, canvas: &mut Canvas, bar: &SeekBar, top: i32, thickness: i32, length: f64, labels: &mut Vec<(i32, i32, Ordering, String)>, px: u32) {
        let Some(section) = self.section() else { return };
        let x_of = |at: f64| bar.x + (f64::from(bar.width) * at / length) as i32;
        if let Some((a, b)) = section.span() {
            let (left, right) = (x_of(a), x_of(b));
            canvas.round_fill(left, top - thickness / 3, (right - left).max(2), thickness + thickness * 2 / 3, thickness / 2, accent(), 200);
        }
        let line = Text::line_height(px);
        for (edge, at) in [(Edge::A, section.a), (Edge::B, section.b)] {
            let Some(at) = at else { continue };
            let x = x_of(at);
            let tall = thickness * 3;
            canvas.fill(x - 1, bar.middle - tall / 2, 3, tall, accent_soft());
            let word = if edge == Edge::A { "A" } else { "B" };
            labels.push((x, bar.middle - tall / 2 - line, Ordering::Equal, word.to_string()));
        }
    }
}
