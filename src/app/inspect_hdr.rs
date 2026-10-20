//! The inspector's HDR tab drawn, not only listed: a video's SMPTE ST
//! 2094-50 tone curves with the one for a chosen headroom, as hdr-explorer
//! draws them; the HDR10+ curve of the scene on view; and where all its
//! brightnesses stand on one scale of nits.

use super::*;
use super::inspect::Hit;
use crate::hdrmeta::{self, Agtm, Hdr10Plus, Info};

/// The colors of the charts.
const STORED: u32 = 0x8A8A8A;
const IDENTITY: u32 = 0x5A5A5A;
const PLUS: u32 = 0xB48CF0;
const CARD: u32 = 0x26262B;

/// What the HDR tab's charts are drawn from, beyond the file: for the
/// panel kept to be drawn again when one changes.
#[derive(Clone, PartialEq, Debug)]
pub struct HdrKey {
    headroom: i64,
    agtm: usize,
    plus: usize,
    gain: bool,
}

/// Which of a timeline's values holds at a time, by its place.
fn index_at<T>(timeline: &[hdrmeta::Timed<T>], seconds: f64) -> usize {
    timeline.partition_point(|timed| timed.at <= seconds + 1e-6).saturating_sub(1)
}

/// Nits as they read best: 0.005, 4.2, 203, 1000, 10k.
fn nits(value: f64) -> String {
    match value {
        v if v >= 10_000.0 => format!("{:.0}k", v / 1000.0),
        v if v >= 100.0 => format!("{v:.0}"),
        v if v >= 1.0 => format!("{v:.1}"),
        v => format!("{v:.3}"),
    }
}

impl App {
    /// Where the video inspected is, if it is the one playing; else its start.
    fn hdr_time(&self, id: usize) -> f64 {
        self.video.as_ref().filter(|video| video.id == id).and(self.video_time()).map_or(0.0, |(at, _)| at)
    }

    /// The key of what the charts show now.
    pub(super) fn hdr_key(&self, id: usize, info: Option<&Info>, gain: bool) -> HdrKey {
        let seconds = self.hdr_time(id);
        let (agtm, plus) = info.map_or((0, 0), |info| (index_at(&info.agtm, seconds), index_at(&info.hdr10plus, seconds)));
        HdrKey { headroom: (self.hdr_headroom * 100.0).round() as i64, agtm, plus, gain }
    }

    /// The HDR tab's charts, under its cards, from `y`. Returns where
    /// they end.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_hdr_charts(&self, sub: &mut Canvas, text: &mut Text, px: u32, id: usize, info: &Info, inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), gain: bool, drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let seconds = self.hdr_time(id);
        let mut y = y;
        if let Some(agtm) = info.agtm.get(index_at(&info.agtm, seconds)).map(|timed| &timed.value) {
            y = self.draw_agtm(sub, text, px, info, agtm, inner_w, y, offset, mouse, gain, drawn);
        }
        if let Some(plus) = info.hdr10plus.get(index_at(&info.hdr10plus, seconds)).map(|timed| &timed.value) {
            y = draw_plus(sub, text, px, plus, info.hdr10plus.len(), inner_w, y);
        }
        draw_ladder(sub, text, px, info, seconds, self.hdr_headroom, inner_w, y)
    }

    /// SMPTE ST 2094-50: every curve the video carries, and the one for
    /// the headroom chosen, blended from the two either side of it.
    #[allow(clippy::too_many_arguments)]
    fn draw_agtm(&self, sub: &mut Canvas, text: &mut Text, px: u32, info: &Info, agtm: &Agtm, inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), gain: bool, drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let (cx, cw) = (x + pad, inner_w - 2 * pad);
        let headroom = self.hdr_headroom;
        let curves = agtm.curves();
        let plot_h = cw.min(px as i32 * 16);
        let rows = curves.len() as i32 + 3;
        let button_h = small_line + pad / 3;
        let card_h = pad + small_line + pad / 2 + button_h + pad / 2 + plot_h + pad / 2 + small_line + pad / 2 + rows * (small_line + pad / 4) + pad;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, CARD, 256);
        let mut top = y + pad;
        line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, "2094-50 · TONE CURVES");
        // Output against input, or the gain in stops: as hdr-explorer's two views.
        let mut right = cx + cw;
        for (word, on) in [("Gain", gain), ("Output", !gain)].into_iter() {
            let w = text.width(word, small) + pad;
            right -= w;
            let place = [right, top - pad / 6, w, small_line + pad / 3];
            let hovered = in_rect(mouse, [place[0] + offset.0, place[1] + offset.1, place[2], place[3]]);
            pill(sub, text, place, word, small, on, hovered);
            if !on {
                drawn.push((Hit::GainView, [place[0] + offset.0, place[1] + offset.1, place[2], place[3]]));
            }
            right -= pad / 4;
        }
        top += small_line + pad / 2;

        // The headroom chosen, with buttons to move it.
        let mut left = cx;
        for (word, up) in [("−", false), ("+", true)] {
            let w = text.width(word, small) + pad;
            let place = [left, top, w, button_h];
            let hovered = in_rect(mouse, [place[0] + offset.0, place[1] + offset.1, w, button_h]);
            pill(sub, text, place, word, small, false, hovered);
            drawn.push((Hit::Headroom(up), [place[0] + offset.0, place[1] + offset.1, w, button_h]));
            left += w + pad / 3;
        }
        let said = match headroom {
            h if h == 0.0 => "Headroom 0 · SDR, as this screen".to_string(),
            h => format!("Headroom {h:.2} stops · {:.1}× white · {} nits", h.exp2(), nits(agtm.reference_white * h.exp2())),
        };
        text.draw(sub, left + pad / 3, top + (button_h - small_line) / 2, small, TEXT_COLOR, &said);
        top += button_h + pad / 2;

        // The chart: input as a multiple of the reference white across.
        let most = curves.iter().map(|altr| altr.headroom).fold(agtm.baseline, f64::max).max(headroom);
        let range = most.exp2().ceil().max(2.0);
        let place = [cx, top, cw, plot_h];
        let active = accent();
        if gain {
            // The gain, in stops, offset so that the plot's floor is the least.
            let samples: Vec<f64> = (1..=64).map(|i| f64::from(i) / 64.0 * range).collect();
            let gains = |f: &dyn Fn(f64) -> f64| samples.iter().map(|x| (f(*x) / x).log2()).fold((f64::MAX, f64::MIN), |(lo, hi), g| (lo.min(g), hi.max(g)));
            let (mut lo, mut hi) = (0.0_f64, 0.0_f64);
            for altr in &curves {
                let (l, h) = gains(&|x| Agtm::output(altr, x));
                (lo, hi) = (lo.min(l), hi.max(h));
            }
            let (lo, hi) = (lo.floor(), hi.ceil().max(lo.floor() + 1.0));
            let mut lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(move |_| -lo), IDENTITY, true)];
            for altr in &curves {
                lines.push((Box::new(move |x| (Agtm::output(altr, x) / x).log2() - lo), STORED, false));
            }
            lines.push((Box::new(move |x| (agtm.output_at(headroom, x) / x).log2() - lo), active, false));
            draw::plot(sub, place, (range, hi - lo), 1.0, &lines);
            top += plot_h + pad / 2;
            line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, &format!("across: input, 0 to {range:.0}× white  ·  up: gain, {lo:+.0} to {hi:+.0} stops"));
        } else {
            let mut lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(|x| x), IDENTITY, true)];
            for altr in &curves {
                lines.push((Box::new(move |x| Agtm::output(altr, x)), STORED, false));
            }
            lines.push((Box::new(move |_| headroom.exp2()), active, true));
            lines.push((Box::new(move |x| agtm.output_at(headroom, x)), active, false));
            draw::plot(sub, place, (range, range), 1.0, &lines);
            top += plot_h + pad / 2;
            line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, &format!("across: input · up: output · 0 to {range:.0}× white  ·  dashed: headroom"));
        }
        top += small_line + pad / 2;

        // Each curve, the two the one drawn is blended from marked.
        let above = curves.partition_point(|altr| altr.headroom < headroom).min(curves.len() - 1);
        let below = if curves[above].headroom > headroom { above.saturating_sub(1) } else { above };
        let row = small_line + pad / 4;
        for (at, altr) in curves.iter().enumerate() {
            let used = at == above || at == below;
            let swatch = (small_line / 3).max(4);
            sub.round_fill(cx, top + (small_line - swatch) / 2, swatch * 2, swatch, swatch / 2, if used { active } else { STORED }, 256);
            let base = if altr.headroom == agtm.baseline { "  ·  baseline, no change" } else { "" };
            let words = format!("{:.2} stops  ·  {:.1}× white  ·  {} nits{base}", altr.headroom, altr.headroom.exp2(), nits(agtm.reference_white * altr.headroom.exp2()));
            text.draw(sub, cx + swatch * 3, top, small, if used { TEXT_COLOR } else { DIM_TEXT_COLOR }, &words);
            top += row;
        }
        let (low, high) = (&curves[below], &curves[above]);
        let blend = if high.headroom > low.headroom {
            let weight = ((headroom - low.headroom) / (high.headroom - low.headroom)).clamp(0.0, 1.0);
            format!("In use: {:.0}% of {:.2}, {:.0}% of {:.2} stops", (1.0 - weight) * 100.0, low.headroom, weight * 100.0, high.headroom)
        } else {
            format!("In use: the {:.2} stops curve", low.headroom)
        };
        line(sub, text, cx, top, cw, small, TEXT_COLOR, &blend);
        top += row;
        line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, &format!("Reference white {} nits  ·  baseline {:.2} stops", nits(agtm.reference_white), agtm.baseline));
        top += row;
        let changes = info.agtm.len();
        line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, &format!("{changes} {} over the video", if changes == 1 { "set of curves" } else { "sets of curves" }));
        y + card_h + pad
    }
}

/// A small rounded button: lit when on, lighter under the pointer.
fn pill(sub: &mut Canvas, text: &mut Text, [x, y, w, h]: [i32; 4], word: &str, small: u32, on: bool, hovered: bool) {
    let lit = on || hovered;
    sub.round_fill(x, y, w, h, h / 2, if lit { accent() } else { PLACEHOLDER_COLOR }, if hovered && !on { 200 } else { 256 });
    sub.round_outline(x, y, w, h, h / 2, 1, if lit { accent() } else { MENU_EDGE_COLOR });
    let word_w = text.width(word, small);
    text.draw(sub, x + (w - word_w) / 2, y + (h - Text::line_height(small)) / 2, small, if lit { on_accent() } else { TEXT_COLOR }, word);
}

fn in_rect((x, y): (i32, i32), [left, top, w, h]: [i32; 4]) -> bool {
    (left..left + w).contains(&x) && (top..top + h).contains(&y)
}

/// HDR10+: the curve of the scene on view, from its brightest to the
/// screen it was made for, where it leaves the straight line marked.
#[allow(clippy::too_many_arguments)]
fn draw_plus(sub: &mut Canvas, text: &mut Text, px: u32, plus: &Hdr10Plus, scenes: usize, inner_w: i32, y: i32) -> i32 {
    let pad = px as i32;
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    let x = pad * 3 / 2;
    let (cx, cw) = (x + pad, inner_w - 2 * pad);
    let plot_h = cw.min(px as i32 * 12);
    let row = small_line + pad / 4;
    let card_h = pad + small_line + pad / 2 + plot_h + pad / 2 + small_line + pad / 2 + 4 * row + pad;
    sub.round_fill(x, y, inner_w, card_h, pad / 2, CARD, 256);
    let mut top = y + pad;
    line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, "HDR10+ · THIS SCENE'S CURVE");
    top += small_line + pad / 2;
    let lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(|x| x), IDENTITY, true), (Box::new(move |x| plus.tone(x)), PLUS, false)];
    draw::plot(sub, [cx, top, cw, plot_h], (1.0, 1.0), 0.25, &lines);
    // Where it leaves the straight line.
    let (kx, ky) = plus.knee;
    let dot = (px as i32 / 3).max(5);
    let (dx, dy) = (cx + (kx * f64::from(cw)) as i32, top + plot_h - (ky * f64::from(plot_h)) as i32);
    sub.round_fill(dx - dot / 2, dy - dot / 2, dot, dot, dot / 2, TEXT_COLOR, 256);
    top += plot_h + pad / 2;
    line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, "across: share of the scene's brightest  ·  up: of the screen's  ·  dot: knee");
    top += small_line + pad / 2;
    let [r, g, b] = plus.maxscl;
    for words in [
        format!("For a screen of {} nits", nits(plus.target)),
        format!("Brightest red {} · green {} · blue {} nits", nits(r), nits(g), nits(b)),
        format!("Mean {} nits  ·  knee at {:.0}%, {:.0}%", nits(plus.average), kx * 100.0, ky * 100.0),
        format!("{} Bezier anchors  ·  {scenes} sets over the video", plus.anchors.len()),
    ] {
        line(sub, text, cx, top, cw, small, TEXT_COLOR, &words);
        top += row;
    }
    y + card_h + pad
}

/// Every brightness the video speaks of, on one scale of nits, from a
/// hundredth to ten thousand: where its white is, its brightest, the
/// screen it was mastered on and the headroom chosen.
#[allow(clippy::too_many_arguments)]
fn draw_ladder(sub: &mut Canvas, text: &mut Text, px: u32, info: &Info, seconds: f64, headroom: f64, inner_w: i32, y: i32) -> i32 {
    let pad = px as i32;
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    let x = pad * 3 / 2;
    let (cx, cw) = (x + pad, inner_w - 2 * pad);
    // What there is to mark, each with a color.
    let mut marks: Vec<(String, f64, u32)> = Vec::new();
    let white = info.agtm_at(seconds).map_or(203.0, |agtm| agtm.reference_white);
    marks.push(("SDR white".into(), white, TEXT_COLOR));
    if let Some((peak, floor)) = info.mastering {
        marks.push(("Mastering peak".into(), peak, 0xE5A50A));
        if floor > 0.0 {
            marks.push(("Mastering black".into(), floor, 0xE5A50A));
        }
    }
    if let Some((cll, fall)) = info.light {
        if cll > 0 {
            marks.push(("MaxCLL".into(), f64::from(cll), 0xE5534B));
        }
        if fall > 0 {
            marks.push(("MaxFALL".into(), f64::from(fall), 0xF08A4B));
        }
    }
    if let Some(plus) = info.hdr10plus_at(seconds) {
        marks.push(("HDR10+ screen".into(), plus.target, PLUS));
        marks.push(("Scene's brightest".into(), plus.maxscl.iter().copied().fold(0.0, f64::max), 0x8E5BD6));
    }
    if info.agtm_at(seconds).is_some() {
        marks.push(("Headroom chosen".into(), white * headroom.exp2(), accent()));
    }
    let row = small_line + pad / 4;
    let bar_h = (px as i32 / 2).max(6);
    let card_h = pad + small_line + pad / 2 + bar_h + small_line + pad / 2 + marks.len() as i32 * row + pad;
    sub.round_fill(x, y, inner_w, card_h, pad / 2, CARD, 256);
    let mut top = y + pad;
    line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, "BRIGHTNESS · NITS, ON A LOG SCALE");
    top += small_line + pad / 2;
    // A decade a sixth of the way: 0.01 to 10,000 nits.
    let (lo, hi) = (-2.0_f64, 4.0_f64);
    let at = |nits: f64| cx + ((nits.max(1e-3).log10() - lo) / (hi - lo) * f64::from(cw)).round() as i32;
    sub.round_fill(cx, top, cw, bar_h, bar_h / 2, 0x0B0B0B, 256);
    for decade in [0.01, 1.0, 100.0, 10_000.0] {
        let dx = at(decade).min(cx + cw - 1);
        sub.fill(dx, top, 1, bar_h, 0x3A3A3A);
        let label = nits(decade);
        let label_w = text.width(&label, small);
        text.draw(sub, (dx - label_w / 2).clamp(cx, cx + cw - label_w), top + bar_h, small, DIM_TEXT_COLOR, &label);
    }
    for (_, value, color) in &marks {
        let dx = at(*value).clamp(cx, cx + cw - 3);
        sub.fill(dx - 1, top - 2, 3, bar_h + 4, *color);
    }
    top += bar_h + small_line + pad / 2;
    for (name, value, color) in &marks {
        let swatch = (small_line / 3).max(4);
        sub.round_fill(cx, top + (small_line - swatch) / 2, swatch * 2, swatch, swatch / 2, *color, 256);
        text.draw(sub, cx + swatch * 3, top, small, TEXT_COLOR, name);
        let value = format!("{} nits", nits(*value));
        let value_w = text.width(&value, small);
        text.draw(sub, cx + cw - value_w, top, small, DIM_TEXT_COLOR, &value);
        top += row;
    }
    y + card_h + pad
}

/// A line of a card, cut short with an ellipsis where it would run past.
#[allow(clippy::too_many_arguments)]
fn line(sub: &mut Canvas, text: &mut Text, x: i32, y: i32, width: i32, px: u32, color: u32, words: &str) {
    let words = clipped(text, words, width, px);
    text.draw(sub, x, y, px, color, &words);
}

impl App {
    /// A picture's charts on its color tab: the tone curves its profile
    /// and chunks give, and how much of an Ultra HDR gain map is laid on
    /// for a screen of each headroom.
    pub(super) fn draw_picture_charts(&self, sub: &mut Canvas, text: &mut Text, px: u32, picture: &crate::picmeta::Picture, inner_w: i32, y: i32) -> i32 {
        let mut y = y;
        let color = &picture.color;
        if !color.curves.is_empty() || color.png_gamma.is_some() {
            y = draw_tone_curves(sub, text, px, color, inner_w, y);
        }
        if let Some((meta, _)) = &picture.gain {
            y = draw_gain_weight(sub, text, px, meta, inner_w, y);
        }
        y
    }
}

/// The curves from a stored value to linear light, by channel, against
/// sRGB's.
fn draw_tone_curves(sub: &mut Canvas, text: &mut Text, px: u32, color: &crate::colorinfo::Color, inner_w: i32, y: i32) -> i32 {
    use crate::colorinfo::Curve;
    let pad = px as i32;
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    let x = pad * 3 / 2;
    let (cx, cw) = (x + pad, inner_w - 2 * pad);
    let plot_h = cw.min(px as i32 * 14);
    let card_h = pad + small_line + pad / 2 + plot_h + pad / 2 + small_line + pad;
    sub.round_fill(x, y, inner_w, card_h, pad / 2, CARD, 256);
    let mut top = y + pad;
    line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, "TONE CURVES · STORED TO LINEAR");
    top += small_line + pad / 2;
    let srgb = Curve::Parametric(3, vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]);
    let mut lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(|x| x), IDENTITY, true), (Box::new(move |x| srgb.eval(x)), 0x4A6A8A, true)];
    for (channel, curve) in &color.curves {
        let tint = match channel {
            'R' => 0xE5534B,
            'G' => 0x3DD68C,
            'B' => 0x3E7BD6,
            _ => TEXT_COLOR,
        };
        lines.push((Box::new(move |x| curve.eval(x)), tint, false));
    }
    if let Some(stored) = color.png_gamma.filter(|g| *g > 0.0) {
        lines.push((Box::new(move |x: f64| x.powf(1.0 / stored)), 0xE5A50A, false));
    }
    draw::plot(sub, [cx, top, cw, plot_h], (1.0, 1.0), 0.25, &lines);
    top += plot_h + pad / 2;
    line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, "across: as stored · up: light · dashed blue: sRGB · gray: straight");
    y + card_h + pad
}

/// How much of a gain map is laid on, by the screen's headroom: none on
/// an SDR screen, all of it from the headroom it was made for.
fn draw_gain_weight(sub: &mut Canvas, text: &mut Text, px: u32, meta: &crate::ultrahdr::Meta, inner_w: i32, y: i32) -> i32 {
    let pad = px as i32;
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    let x = pad * 3 / 2;
    let (cx, cw) = (x + pad, inner_w - 2 * pad);
    let plot_h = (cw / 2).min(px as i32 * 8);
    let row = small_line + pad / 4;
    let card_h = pad + small_line + pad / 2 + plot_h + pad / 2 + small_line + pad / 2 + 2 * row + pad;
    sub.round_fill(x, y, inner_w, card_h, pad / 2, CARD, 256);
    let mut top = y + pad;
    line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, "GAIN MAP · HOW MUCH, BY SCREEN");
    top += small_line + pad / 2;
    let full = f64::from(meta.capacity_max).max(0.01);
    let across = (full + 1.0).ceil();
    let lines: Vec<(Box<dyn Fn(f64) -> f64 + '_>, u32, bool)> = vec![(Box::new(move |h: f64| (h / full).clamp(0.0, 1.0)), accent(), false)];
    draw::plot(sub, [cx, top, cw, plot_h], (across, 1.0), 1.0, &lines);
    top += plot_h + pad / 2;
    line(sub, text, cx, top, cw, small, DIM_TEXT_COLOR, &format!("across: the screen's headroom, 0 to {across:.0} stops · up: share of the gain map"));
    top += small_line + pad / 2;
    let most = meta.max.iter().copied().fold(f32::MIN, f32::max);
    line(sub, text, cx, top, cw, small, TEXT_COLOR, &format!("All of it from {full:.2} stops  ·  {:.1}× SDR white", full.exp2()));
    top += row;
    line(sub, text, cx, top, cw, small, TEXT_COLOR, &format!("The brightest raised up to {most:.2} stops  ·  {:.1}×", f64::from(most).exp2()));
    y + card_h + pad
}
