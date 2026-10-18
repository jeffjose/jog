//! What a video says about its HDR: the curve its pixels are coded on (PQ
//! or HLG), the static HDR10 figures, and the metadata that changes as it
//! plays, HDR10+ (SMPTE ST 2094-40) and SMPTE ST 2094-50. Read from the
//! file itself, for the player passes none of it on: from an MP4's sample
//! tables, its `it35` and `mebx` metadata tracks and the T.35 messages in
//! its AV1 and HEVC samples, or from a WebM's BlockAdditional data.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The curve a video's values are coded on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Transfer {
    Sdr,
    Pq,
    Hlg,
}

impl Transfer {
    /// From the code H.273 gives it.
    fn from_code(code: u64) -> Option<Transfer> {
        match code {
            16 => Some(Transfer::Pq),
            18 => Some(Transfer::Hlg),
            1 | 6 | 13 | 14 | 15 => Some(Transfer::Sdr),
            _ => None,
        }
    }
}

/// A point of a 2094-50 gain curve: how much brighter or darker, in
/// stops, a value `x` times the reference white becomes, and the slope.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub m: f64,
}

/// A curve for one headroom, the brightest a screen can go over its
/// white in stops.
#[derive(Clone, PartialEq, Debug)]
pub struct Altr {
    pub headroom: f64,
    pub curve: Vec<Point>,
}

/// SMPTE ST 2094-50: gain curves for some headrooms, between which the
/// one for any screen is found.
#[derive(Clone, PartialEq, Debug)]
pub struct Agtm {
    /// Nits the reference white is at.
    pub reference_white: f64,
    /// The headroom the video is graded for, in stops: no gain there.
    pub baseline: f64,
    pub altr: Vec<Altr>,
}

/// HDR10+, SMPTE ST 2094-40: what the brightest of a scene is, and the
/// curve for a screen of a given peak.
#[derive(Clone, PartialEq, Debug)]
pub struct Hdr10Plus {
    /// The peak, in nits, of the screen the curve is for.
    pub target: f64,
    /// The brightest red, green and blue of the scene, and its mean
    /// brightest, in nits.
    pub maxscl: [f64; 3],
    pub average: f64,
    /// Where the curve leaves the straight line, as fractions, and the
    /// Bezier anchors after it.
    pub knee: (f64, f64),
    pub anchors: Vec<f64>,
}

impl Hdr10Plus {
    /// The curve: input and output as fractions of the scene's brightest
    /// and of the screen's peak.
    pub fn tone(&self, x: f64) -> f64 {
        let (kx, ky) = self.knee;
        if x <= kx {
            return if kx > 0.0 { x * ky / kx } else { 0.0 };
        }
        let t = ((x - kx) / (1.0 - kx).max(1e-9)).clamp(0.0, 1.0);
        // Bernstein form, anchors between a start at 0 and an end at 1.
        let mut points = vec![0.0];
        points.extend(&self.anchors);
        points.push(1.0);
        let order = points.len() - 1;
        let mut sum = 0.0;
        for (k, p) in points.iter().enumerate() {
            sum += binomial(order, k) * t.powi(k as i32) * (1.0 - t).powi((order - k) as i32) * p;
        }
        ky + (1.0 - ky) * sum
    }
}

fn binomial(n: usize, k: usize) -> f64 {
    (0..k).fold(1.0, |acc, i| acc * (n - i) as f64 / (i + 1) as f64)
}

/// A value and the time, in seconds, from which it holds.
#[derive(Clone, Debug)]
pub struct Timed<T> {
    pub at: f64,
    pub value: T,
}

/// Everything found about a video's HDR.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub transfer: Option<Transfer>,
    pub bt2020: bool,
    /// The mastering display's peak and floor, in nits.
    pub mastering: Option<(f64, f64)>,
    /// MaxCLL and MaxFALL, in nits.
    pub light: Option<(u32, u32)>,
    pub hdr10plus: Vec<Timed<Hdr10Plus>>,
    pub agtm: Vec<Timed<Agtm>>,
    /// How each kind of dynamic metadata is carried, in a word.
    pub carried: Vec<(&'static str, &'static str)>,
}

impl Info {
    /// Short names for what this video is, to be shown as badges.
    pub fn badges(&self) -> Vec<String> {
        let mut badges = Vec::new();
        match self.transfer {
            Some(Transfer::Pq) if self.hdr10plus.is_empty() && (self.mastering.is_some() || self.light.is_some()) => badges.push("HDR10".to_string()),
            Some(Transfer::Pq) if self.hdr10plus.is_empty() => badges.push("PQ".to_string()),
            Some(Transfer::Pq) => badges.push("HDR10+".to_string()),
            Some(Transfer::Hlg) => badges.push("HLG".to_string()),
            _ => {}
        }
        if !self.agtm.is_empty() {
            badges.push("2094-50".to_string());
        }
        if self.bt2020 {
            badges.push("BT.2020".to_string());
        }
        badges
    }

    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer, Some(Transfer::Pq | Transfer::Hlg)) || !self.agtm.is_empty() || !self.hdr10plus.is_empty()
    }

    pub fn agtm_at(&self, seconds: f64) -> Option<&Agtm> {
        at(&self.agtm, seconds)
    }

    pub fn hdr10plus_at(&self, seconds: f64) -> Option<&Hdr10Plus> {
        at(&self.hdr10plus, seconds)
    }
}

/// The value that holds at a time: the last to begin at or before it.
fn at<T>(timeline: &[Timed<T>], seconds: f64) -> Option<&T> {
    let after = timeline.partition_point(|timed| timed.at <= seconds + 1e-6);
    timeline.get(after.saturating_sub(1)).map(|timed| &timed.value)
}

/// Reads what a video file says about its HDR. `None` for one that is not
/// an MP4 or WebM, or cannot be read.
pub fn read(path: &Path) -> Option<Info> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut info = match &head {
        [0x1A, 0x45, 0xDF, 0xA3, ..] => webm::read(&mut file)?,
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => mp4::read(&mut file)?,
        _ => return None,
    };
    for timeline in [&mut info.agtm] {
        timeline.sort_by(|a, b| a.at.total_cmp(&b.at));
    }
    info.hdr10plus.sort_by(|a, b| a.at.total_cmp(&b.at));
    // The same metadata in a track and in the samples: one copy will do.
    info.agtm.dedup_by(|a, b| (a.at - b.at).abs() < 1e-6 && a.value == b.value);
    info.hdr10plus.dedup_by(|a, b| (a.at - b.at).abs() < 1e-6 && a.value == b.value);
    info.carried.sort();
    info.carried.dedup();
    Some(info)
}

/// What an ITU-T T.35 message is, from the country and provider codes
/// that begin it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum T35 {
    Hdr10Plus,
    Agtm,
}

impl T35 {
    fn of(identifier: &[u8]) -> Option<T35> {
        match identifier {
            [0xB5, 0x00, 0x3C, 0x00, 0x01, ..] => Some(T35::Hdr10Plus),
            [0xB5, 0x00, 0x90, 0x00, 0x01, ..] => Some(T35::Agtm),
            _ => None,
        }
    }
}

/// Reads a T.35 message, its identifier still on it, into `info`.
fn take_t35(info: &mut Info, message: &[u8], at: f64, carriage: &'static str) {
    let Some(kind) = T35::of(message) else { return };
    take_payload(info, kind, &message[5..], at, carriage);
}

/// Reads the payload of a T.35 message of a known kind into `info`.
fn take_payload(info: &mut Info, kind: T35, payload: &[u8], at: f64, carriage: &'static str) {
    match kind {
        T35::Hdr10Plus => {
            if let Some(value) = parse_hdr10plus(payload) {
                info.hdr10plus.push(Timed { at, value });
                info.carried.push(("HDR10+", carriage));
            }
        }
        T35::Agtm => {
            if let Some(value) = parse_agtm(payload) {
                info.agtm.push(Timed { at, value });
                info.carried.push(("2094-50", carriage));
            }
        }
    }
}

/// Reads bits, the first of each byte first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, at: 0 }
    }

    fn read(&mut self, count: u32) -> Option<u64> {
        let mut value = 0u64;
        for _ in 0..count {
            let byte = *self.data.get(self.at / 8)?;
            value = value << 1 | u64::from(byte >> (7 - self.at % 8) & 1);
            self.at += 1;
        }
        Some(value)
    }

    fn flag(&mut self) -> Option<bool> {
        self.read(1).map(|bit| bit == 1)
    }
}

/// HDR10+ as ST 2094-40 lays it out, from the application identifier on:
/// one processing window; a message with more is not read.
fn parse_hdr10plus(payload: &[u8]) -> Option<Hdr10Plus> {
    let mut bits = Bits::new(payload);
    let (identifier, _version) = (bits.read(8)?, bits.read(8)?);
    let windows = bits.read(2)?;
    if identifier != 4 || windows != 1 {
        return None;
    }
    let target = bits.read(27)? as f64;
    let matrix = |bits: &mut Bits| -> Option<()> {
        let (rows, columns) = (bits.read(5)?, bits.read(5)?);
        for _ in 0..rows * columns {
            bits.read(4)?;
        }
        Some(())
    };
    if bits.flag()? {
        matrix(&mut bits)?;
    }
    // In units of a hundred-thousandth of 100,000 nits: a tenth of a nit.
    let tenths = |value: u64| value as f64 / 10.0;
    let maxscl = [tenths(bits.read(17)?), tenths(bits.read(17)?), tenths(bits.read(17)?)];
    let average = tenths(bits.read(17)?);
    for _ in 0..bits.read(4)? {
        bits.read(7)?;
        bits.read(17)?;
    }
    bits.read(10)?;
    if bits.flag()? {
        matrix(&mut bits)?;
    }
    let (mut knee, mut anchors) = ((0.0, 0.0), Vec::new());
    if bits.flag()? {
        knee = (bits.read(12)? as f64 / 4095.0, bits.read(12)? as f64 / 4095.0);
        for _ in 0..bits.read(4)? {
            anchors.push(bits.read(10)? as f64 / 1023.0);
        }
    }
    Some(Hdr10Plus { target, maxscl, average, knee, anchors })
}

/// A 16-bit value scaled down, as 2094-50 stores its numbers.
fn scaled(bits: &mut Bits, scale: f64, shift: f64, min: u64, max: u64) -> Option<f64> {
    Some((bits.read(16)?.clamp(min, max) as f64 - shift) / scale)
}

/// SMPTE ST 2094-50 as hdr-explorer reads it (`agtm_parser.ts`).
fn parse_agtm(payload: &[u8]) -> Option<Agtm> {
    let mut bits = Bits::new(payload);
    let _version = bits.read(3)?;
    if bits.read(3)? > 0 {
        return None;
    }
    bits.read(2)?;
    let custom_white = bits.flag()?;
    let adaptive = bits.flag()?;
    bits.read(6)?;
    let reference_white = if custom_white { scaled(&mut bits, 5.0, 0.0, 1, 50000)? } else { 203.0 };
    if !adaptive {
        return None;
    }
    let baseline = scaled(&mut bits, 10000.0, 0.0, 0, 60000)?;
    if bits.flag()? {
        // Curves made from the reference white alone, as ST 2094-50 lays
        // them down: one for SDR, and one a little above it.
        bits.read(7)?;
        return Some(Agtm { reference_white, baseline, altr: reference_white_curves(baseline) });
    }
    let count = bits.read(3)?;
    let chromaticities = bits.read(2)?;
    let common_mix = bits.flag()?;
    let common_curve = bits.flag()?;
    if chromaticities == 3 {
        for _ in 0..8 {
            bits.read(16)?;
        }
    }
    let mut altr = Vec::new();
    let mut shared: Option<(Vec<f64>, bool)> = None;
    for index in 0..count {
        let headroom = scaled(&mut bits, 10000.0, 0.0, 0, 60000)?;
        if index == 0 || !common_mix {
            let mixing = bits.read(2)?;
            if mixing == 3 {
                let present: Vec<bool> = (0..6).map(|_| bits.flag()).collect::<Option<_>>()?;
                for _ in present.iter().filter(|present| **present) {
                    bits.read(16)?;
                }
            } else {
                bits.read(6)?;
            }
        }
        let (xs, pchip) = match (&shared, index > 0 && common_curve) {
            (Some(shared), true) => shared.clone(),
            _ => {
                let length = bits.read(5)? + 1;
                let pchip = bits.flag()?;
                bits.read(2)?;
                let xs: Vec<f64> = (0..length).map(|_| scaled(&mut bits, 1000.0, 0.0, 0, 64000)).collect::<Option<_>>()?;
                if index == 0 {
                    shared = Some((xs.clone(), pchip));
                }
                (xs, pchip)
            }
        };
        let sign = if baseline < headroom { 1.0 } else { -1.0 };
        let ys: Vec<f64> = (0..xs.len()).map(|_| scaled(&mut bits, 10000.0, 0.0, 0, 60000).map(|y| y * sign)).collect::<Option<_>>()?;
        let slopes = match pchip {
            true => pchip_slopes(&xs, &ys),
            false => (0..xs.len()).map(|_| scaled(&mut bits, 36000.0 / std::f64::consts::PI, 18000.0, 1, 35999).map(f64::tan)).collect::<Option<_>>()?,
        };
        let curve = (0..xs.len()).map(|at| Point { x: xs[at], y: ys[at], m: slopes[at] }).collect();
        altr.push(Altr { headroom, curve });
    }
    Some(Agtm { reference_white, baseline, altr })
}

/// The curves ST 2094-50 makes from the reference white when a video
/// asks for them rather than carrying its own (its Annex C.3.8).
fn reference_white_curves(baseline: f64) -> Vec<Altr> {
    if baseline <= 0.0 {
        return Vec::new();
    }
    let share = (baseline / (1000.0f64 / 203.0).log2()).min(1.0);
    (0..2)
        .map(|which| {
            let headroom = if which == 0 { 0.0 } else { (8.0f64 / 3.0).log2() * share };
            let white = if which == 0 { 1.0 - 0.5 * share } else { 1.0 };
            let kappa = 0.65;
            let (x_knee, y_knee) = (1.0, white);
            let (x_max, y_max) = (baseline.exp2(), headroom.exp2());
            let x_mid = (1.0 - kappa) * x_knee + kappa * x_knee * y_max / y_knee;
            let y_mid = (1.0 - kappa) * y_knee + kappa * y_max;
            let (xa, ya) = (x_knee - 2.0 * x_mid + x_max, y_knee - 2.0 * y_mid + y_max);
            let (xb, yb) = (2.0 * x_mid - 2.0 * x_knee, 2.0 * y_mid - 2.0 * y_knee);
            let curve = (0..8)
                .map(|step| {
                    let t = f64::from(step) / 7.0;
                    let x = x_knee + t * (xb + t * xa);
                    let y = y_knee + t * (yb + t * ya);
                    let m = (2.0 * ya * t + yb) / (2.0 * xa * t + xb);
                    Point { x, y: (y / x).log2(), m: (x * m - y) / (std::f64::consts::LN_2 * x * y) }
                })
                .collect();
            Altr { headroom, curve }
        })
        .collect()
}

/// Slopes for a smooth curve through points that never overshoots them.
fn pchip_slopes(x: &[f64], y: &[f64]) -> Vec<f64> {
    let n = x.len();
    if n < 2 {
        return vec![0.0; n];
    }
    let dx: Vec<f64> = (0..n - 1).map(|i| x[i + 1] - x[i]).collect();
    let ds: Vec<f64> = (0..n - 1).map(|i| if dx[i] == 0.0 { 0.0 } else { (y[i + 1] - y[i]) / dx[i] }).collect();
    if n == 2 {
        return vec![ds[0]; 2];
    }
    let mut slopes = vec![0.0; n];
    let end = |d0: f64, d1: f64, s0: f64, s1: f64| if d0 + d1 == 0.0 { 0.0 } else { ((2.0 * d0 + d1) * s0 - d0 * s1) / (d0 + d1) };
    slopes[0] = end(dx[0], dx[1], ds[0], ds[1]);
    slopes[n - 1] = end(dx[n - 2], dx[n - 3], ds[n - 2], ds[n - 3]);
    for i in 1..n - 1 {
        if ds[i - 1].signum() != ds[i].signum() {
            continue;
        }
        let denominator = (2.0 * dx[i - 1] + dx[i]) * ds[i - 1] + (dx[i - 1] + 2.0 * dx[i]) * ds[i];
        slopes[i] = if denominator == 0.0 { 0.0 } else { 3.0 * (dx[i - 1] + dx[i]) * ds[i - 1] * ds[i] / denominator };
    }
    slopes
}

impl Agtm {
    /// The gain, in stops, of a curve at `x`, as ST 2094-50 extends it
    /// past its ends: level to the left, and to the right falling so that
    /// the output stays at the last point's.
    fn gain(curve: &[Point], x: f64) -> f64 {
        let (Some(first), Some(last)) = (curve.first(), curve.last()) else { return 0.0 };
        if x <= first.x {
            return first.y;
        }
        if x >= last.x {
            return last.y + (last.x / x).log2();
        }
        let i = curve.partition_point(|p| p.x < x).clamp(1, curve.len() - 1);
        let (p0, p1) = (curve[i - 1], curve[i]);
        let width = p1.x - p0.x;
        let t = (x - p0.x) / width;
        let (m0, m1) = (p0.m * width, p1.m * width);
        let c3 = 2.0 * p0.y + m0 - 2.0 * p1.y + m1;
        let c2 = -3.0 * p0.y + 3.0 * p1.y - 2.0 * m0 - m1;
        p0.y + t * (m0 + t * (c2 + t * c3))
    }

    /// The stored curves and the one with no gain at the baseline,
    /// lowest headroom first.
    pub fn curves(&self) -> Vec<Altr> {
        let mut all: Vec<Altr> = self.altr.iter().filter(|altr| altr.headroom != self.baseline).cloned().collect();
        all.push(Altr { headroom: self.baseline, curve: vec![Point { x: 64.0, y: 0.0, m: 0.0 }] });
        all.sort_by(|a, b| a.headroom.total_cmp(&b.headroom));
        all
    }

    /// The output, as a multiple of the reference white, of one stored
    /// curve for an input `x`.
    pub fn output(altr: &Altr, x: f64) -> f64 {
        x * Self::gain(&altr.curve, x).exp2()
    }

    /// The output for a screen of `headroom` stops: the two stored
    /// curves either side of it blended by how near each is.
    pub fn output_at(&self, headroom: f64, x: f64) -> f64 {
        let all = self.curves();
        let above = all.partition_point(|altr| altr.headroom < headroom).min(all.len() - 1);
        let below = if all[above].headroom > headroom { above.saturating_sub(1) } else { above };
        let (low, high) = (&all[below], &all[above]);
        let weight = if high.headroom > low.headroom { ((headroom - low.headroom) / (high.headroom - low.headroom)).clamp(0.0, 1.0) } else { 0.0 };
        let gain = (1.0 - weight) * Self::gain(&low.curve, x) + weight * Self::gain(&high.curve, x);
        x * gain.exp2()
    }
}

/// MP4: the boxes, the sample tables, and the samples' T.35 messages.
mod mp4 {
    use super::*;

    /// A box's type and where its contents are in the file.
    struct Item {
        kind: [u8; 4],
        start: u64,
        end: u64,
    }

    /// The boxes from `start` to `end` of the file, one level only.
    fn children(file: &mut File, start: u64, end: u64) -> Vec<Item> {
        let mut items = Vec::new();
        let mut at = start;
        while at + 8 <= end {
            let mut head = [0u8; 16];
            if file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(&mut head[..8]).is_err() {
                break;
            }
            let mut size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
            let kind = [head[4], head[5], head[6], head[7]];
            let mut header = 8;
            if size == 1 {
                if file.read_exact(&mut head[8..16]).is_err() {
                    break;
                }
                size = u64::from_be_bytes(head[8..16].try_into().unwrap());
                header = 16;
            } else if size == 0 {
                size = end - at;
            }
            if size < header || at.checked_add(size).is_none_or(|stop| stop > end) {
                break;
            }
            items.push(Item { kind, start: at + header, end: at + size });
            at += size;
        }
        items
    }

    fn find<'a>(items: &'a [Item], kind: &[u8; 4]) -> Option<&'a Item> {
        items.iter().find(|item| &item.kind == kind)
    }

    fn bytes(file: &mut File, item: &Item) -> Option<Vec<u8>> {
        let length = usize::try_from(item.end - item.start).ok()?;
        // A table larger than this is not one jog wants to read whole.
        if length > 64 << 20 {
            return None;
        }
        let mut data = vec![0; length];
        file.seek(SeekFrom::Start(item.start)).ok()?;
        file.read_exact(&mut data).ok()?;
        Some(data)
    }

    fn u16_at(data: &[u8], at: usize) -> Option<u64> {
        Some(u64::from(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?)))
    }

    fn u32_at(data: &[u8], at: usize) -> Option<u64> {
        Some(u64::from(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?)))
    }

    fn u64_at(data: &[u8], at: usize) -> Option<u64> {
        Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
    }

    /// The boxes inside a block of bytes, one level only, as type and
    /// contents.
    fn inner(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut items = Vec::new();
        let mut at = 0;
        while at + 8 <= data.len() {
            let size = u32_at(data, at).unwrap_or(0) as usize;
            if size < 8 || at + size > data.len() {
                break;
            }
            items.push((data[at + 4..at + 8].try_into().unwrap(), &data[at + 8..at + size]));
            at += size;
        }
        items
    }

    /// What a track carries, from its sample description.
    enum Kind {
        Av1,
        Hevc,
        /// A track of T.35 messages of one kind, without their identifier.
        It35(T35),
        /// A multiplexed metadata track: its samples are boxes named by a
        /// local key, each key a kind of T.35 message.
        Mebx(Vec<([u8; 4], T35)>),
    }

    pub fn read(file: &mut File) -> Option<Info> {
        let end = file.seek(SeekFrom::End(0)).ok()?;
        let top = children(file, 0, end);
        let moov = find(&top, b"moov")?;
        let mut info = Info::default();
        for trak in children(file, moov.start, moov.end).iter().filter(|item| &item.kind == b"trak") {
            track(file, trak, &mut info);
        }
        Some(info)
    }

    fn track(file: &mut File, trak: &Item, info: &mut Info) -> Option<()> {
        let mdia_items = children(file, trak.start, trak.end);
        let mdia = find(&mdia_items, b"mdia")?;
        let parts = children(file, mdia.start, mdia.end);
        let mdhd = bytes(file, find(&parts, b"mdhd")?)?;
        let timescale = if mdhd.first() == Some(&1) { u32_at(&mdhd, 20)? } else { u32_at(&mdhd, 12)? } as f64;
        let minf = find(&parts, b"minf")?;
        let minf_items = children(file, minf.start, minf.end);
        let stbl = find(&minf_items, b"stbl")?;
        let tables = children(file, stbl.start, stbl.end);
        let stsd = bytes(file, find(&tables, b"stsd")?)?;
        let entry = inner(stsd.get(8..)?).into_iter().next()?;
        let kind = match &entry.0 {
            b"av01" | b"hvc1" | b"hev1" | b"avc1" | b"avc3" | b"vp09" => {
                // A visual sample entry's own boxes start after 78 bytes.
                for (name, body) in inner(entry.1.get(78..).unwrap_or_default()) {
                    colour(&name, body, info);
                }
                match &entry.0 {
                    b"av01" => Kind::Av1,
                    b"hvc1" | b"hev1" => Kind::Hevc,
                    _ => return Some(()),
                }
            }
            b"it35" => {
                let length = *entry.1.get(8)? as usize;
                Kind::It35(T35::of(entry.1.get(9..9 + length)?)?)
            }
            b"mebx" => {
                let mut keys = Vec::new();
                for (name, body) in inner(entry.1.get(8..)?) {
                    if &name == b"keys" {
                        for (local, key) in inner(body) {
                            let found = inner(key).into_iter().find(|(name, _)| name == b"setu").and_then(|(_, setup)| T35::of(setup.get(1..)?));
                            if let Some(kind) = found {
                                keys.push((local, kind));
                            }
                        }
                    }
                }
                Kind::Mebx(keys)
            }
            _ => return Some(()),
        };
        // An SDR video has no HDR metadata worth reading its samples for.
        if matches!(kind, Kind::Av1 | Kind::Hevc) && info.transfer == Some(Transfer::Sdr) {
            return Some(());
        }
        let samples = sample_table(file, &tables, timescale)?;
        for (at, offset, size) in samples {
            // The metadata comes before the picture in a sample: its head is
            // enough, and far less to read from a long film.
            let wanted = match kind {
                Kind::Av1 | Kind::Hevc => size.min(4096),
                _ => size,
            };
            let mut data = vec![0; wanted as usize];
            if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut data).is_err() {
                continue;
            }
            match &kind {
                Kind::Av1 => av1_obus(&data, at, info),
                Kind::Hevc => hevc_nals(&data, at, info),
                Kind::It35(t35) => take_payload(info, *t35, &data, at, "metadata track (it35)"),
                Kind::Mebx(keys) => {
                    for (local, payload) in inner(&data) {
                        if let Some((_, t35)) = keys.iter().find(|(key, _)| *key == local) {
                            take_payload(info, *t35, payload, at, "multiplexed track (mebx)");
                        }
                    }
                }
            }
        }
        Some(())
    }

    /// The colour boxes of a visual sample entry: how its values are coded,
    /// and the static HDR10 figures.
    fn colour(name: &[u8; 4], body: &[u8], info: &mut Info) {
        match name {
            b"colr" if body.starts_with(b"nclx") => {
                info.bt2020 |= u16_at(body, 4) == Some(9);
                if let Some(transfer) = u16_at(body, 6).and_then(Transfer::from_code) {
                    info.transfer = Some(transfer);
                }
            }
            b"mdcv" => {
                if let (Some(peak), Some(floor)) = (u32_at(body, 16), u32_at(body, 20)) {
                    info.mastering = Some((peak as f64 / 10000.0, floor as f64 / 10000.0));
                }
            }
            b"clli" => {
                if let (Some(cll), Some(fall)) = (u16_at(body, 0), u16_at(body, 2)) {
                    info.light = Some((cll as u32, fall as u32));
                }
            }
            _ => {}
        }
    }

    /// Each sample's time in seconds, where it is in the file, and its size.
    fn sample_table(file: &mut File, tables: &[Item], timescale: f64) -> Option<Vec<(f64, u64, u64)>> {
        let stts = bytes(file, find(tables, b"stts")?)?;
        let stsz = bytes(file, find(tables, b"stsz")?)?;
        let stsc = bytes(file, find(tables, b"stsc")?)?;
        let (offsets, wide) = match find(tables, b"stco") {
            Some(item) => (bytes(file, item)?, false),
            None => (bytes(file, find(tables, b"co64")?)?, true),
        };
        let ctts = find(tables, b"ctts").and_then(|item| bytes(file, item));

        let mut times = Vec::new();
        let mut clock = 0u64;
        for entry in 0..u32_at(&stts, 4)? as usize {
            let (count, delta) = (u32_at(&stts, 8 + entry * 8)?, u32_at(&stts, 12 + entry * 8)?);
            for _ in 0..count {
                times.push(clock as i64);
                clock += delta;
            }
        }
        // Presentation times are decode times put off by the ctts offsets.
        if let Some(ctts) = &ctts {
            let signed = ctts.first() == Some(&1);
            let mut sample = 0;
            for entry in 0..u32_at(ctts, 4)? as usize {
                let count = u32_at(ctts, 8 + entry * 8)?;
                let raw = u32_at(ctts, 12 + entry * 8)? as u32;
                let offset = if signed { i64::from(raw as i32) } else { i64::from(raw) };
                for _ in 0..count {
                    if let Some(time) = times.get_mut(sample) {
                        *time += offset;
                    }
                    sample += 1;
                }
            }
        }
        let fixed = u32_at(&stsz, 4)?;
        let count = u32_at(&stsz, 8)? as usize;
        let size = |sample: usize| if fixed > 0 { Some(fixed) } else { u32_at(&stsz, 12 + sample * 4) };
        let chunks = u32_at(&offsets, 4)? as usize;
        let chunk_offset = |chunk: usize| if wide { u64_at(&offsets, 8 + chunk * 8) } else { u32_at(&offsets, 8 + chunk * 4) };
        let runs: Vec<(usize, u64)> = (0..u32_at(&stsc, 4)? as usize).filter_map(|entry| Some((u32_at(&stsc, 8 + entry * 12)? as usize, u32_at(&stsc, 12 + entry * 12)?))).collect();

        let mut samples = Vec::with_capacity(count);
        let mut sample = 0;
        for chunk in 0..chunks {
            let per_chunk = runs.iter().rev().find(|(first, _)| *first <= chunk + 1).map_or(1, |(_, per)| *per);
            let mut offset = chunk_offset(chunk)?;
            for _ in 0..per_chunk {
                if sample >= count {
                    break;
                }
                let size = size(sample)?;
                let time = *times.get(sample).unwrap_or(&0) as f64 / timescale.max(1.0);
                samples.push((time, offset, size));
                offset += size;
                sample += 1;
            }
        }
        Some(samples)
    }

    /// A LEB128 number, and how many bytes it took.
    fn leb128(data: &[u8]) -> Option<(u64, usize)> {
        let mut value = 0u64;
        for (at, byte) in data.iter().take(8).enumerate() {
            value |= u64::from(byte & 0x7F) << (7 * at);
            if byte & 0x80 == 0 {
                return Some((value, at + 1));
            }
        }
        None
    }

    /// The metadata OBUs of an AV1 sample that carry T.35 messages.
    fn av1_obus(data: &[u8], at: f64, info: &mut Info) {
        let mut pos = 0;
        while pos < data.len() {
            let header = data[pos];
            let kind = header >> 3 & 0x0F;
            let extended = header & 0x04 != 0;
            let has_size = header & 0x02 != 0;
            let mut body = pos + 1 + usize::from(extended);
            let size = match has_size {
                true => match data.get(body..).and_then(leb128) {
                    Some((size, used)) => {
                        body += used;
                        size as usize
                    }
                    None => return,
                },
                false => data.len() - body,
            };
            let Some(obu) = data.get(body..body + size) else { return };
            // OBU_METADATA, of type ITU-T T.35.
            if kind == 5 {
                if let Some((4, used)) = leb128(obu) {
                    take_t35(info, &obu[used..], at, "AV1 bitstream (metadata OBU)");
                }
            }
            pos = body + size;
        }
    }

    /// The SEI messages of an HEVC sample that carry T.35 messages.
    fn hevc_nals(data: &[u8], at: f64, info: &mut Info) {
        let mut pos = 0;
        while pos + 4 <= data.len() {
            let length = u32_at(data, pos).unwrap_or(0) as usize;
            let start = pos + 4;
            let Some(nal) = data.get(start..(start + length).min(data.len())) else { return };
            // Prefix and suffix SEI.
            let kind = nal.first().map_or(0, |b| b >> 1 & 0x3F);
            if (kind == 39 || kind == 40) && nal.len() > 2 {
                sei(&unescape(&nal[2..]), at, info);
            }
            pos = start + length;
        }
    }

    /// An SEI payload without its emulation prevention bytes.
    fn unescape(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut zeros = 0;
        for &byte in data {
            if zeros >= 2 && byte == 3 {
                zeros = 0;
                continue;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            out.push(byte);
        }
        out
    }

    fn sei(data: &[u8], at: f64, info: &mut Info) {
        let mut pos = 0;
        let number = |pos: &mut usize| -> Option<usize> {
            let mut value = 0;
            loop {
                let byte = *data.get(*pos)?;
                *pos += 1;
                value += usize::from(byte);
                if byte != 0xFF {
                    return Some(value);
                }
            }
        };
        while pos < data.len() && data[pos] != 0x80 {
            let (Some(kind), Some(size)) = (number(&mut pos), number(&mut pos)) else { return };
            let Some(payload) = data.get(pos..pos + size) else { return };
            // User data registered by ITU-T T.35.
            if kind == 4 {
                take_t35(info, payload, at, "HEVC bitstream (SEI)");
            }
            pos += size;
        }
    }
}

/// WebM: the colour of the video track, and BlockAdditional T.35 data.
mod webm {
    use super::*;

    /// An EBML variable-length number, and how many bytes it took; ids
    /// keep their marker bit, sizes lose it.
    fn vint(file: &mut File, keep_marker: bool) -> Option<(u64, u64)> {
        let mut first = [0u8; 1];
        file.read_exact(&mut first).ok()?;
        let length = first[0].leading_zeros() as usize + 1;
        if length > 8 {
            return None;
        }
        // The marker bit is cleared with a mask as wide as a byte and a bit:
        // an eight-byte number has its marker in the lowest bit.
        let mut value = u64::from(if keep_marker { first[0] } else { first[0] & (0xFFu16 >> length) as u8 });
        let mut rest = [0u8; 8];
        file.read_exact(&mut rest[..length - 1]).ok()?;
        for byte in &rest[..length - 1] {
            value = value << 8 | u64::from(*byte);
        }
        // All ones means the size is not known.
        let unknown = !keep_marker && value == (1u64 << (7 * length)) - 1;
        Some((if unknown { u64::MAX } else { value }, length as u64))
    }

    fn element(file: &mut File) -> Option<(u64, u64, u64)> {
        let (id, _) = vint(file, true)?;
        let (size, _) = vint(file, false)?;
        let start = file.stream_position().ok()?;
        Some((id, start, size))
    }

    fn read_bytes(file: &mut File, size: u64) -> Option<Vec<u8>> {
        if size > 1 << 20 {
            return None;
        }
        let mut data = vec![0; size as usize];
        file.read_exact(&mut data).ok()?;
        Some(data)
    }

    fn uint(data: &[u8]) -> u64 {
        data.iter().fold(0, |value, byte| value << 8 | u64::from(*byte))
    }

    fn float(data: &[u8]) -> f64 {
        match data.len() {
            4 => f64::from(f32::from_be_bytes(data.try_into().unwrap())),
            8 => f64::from_be_bytes(data.try_into().unwrap()),
            _ => 0.0,
        }
    }

    const SEGMENT: u64 = 0x1853_8067;
    const INFO: u64 = 0x1549_A966;
    const TIMECODE_SCALE: u64 = 0x2A_D7B1;
    const TRACKS: u64 = 0x1654_AE6B;
    const TRACK_ENTRY: u64 = 0xAE;
    const VIDEO: u64 = 0xE0;
    const COLOUR: u64 = 0x55B0;
    const CLUSTER: u64 = 0x1F43_B675;
    const TIMECODE: u64 = 0xE7;
    const BLOCK_GROUP: u64 = 0xA0;
    const BLOCK: u64 = 0xA1;
    const BLOCK_ADDITIONS: u64 = 0x75A1;
    const BLOCK_MORE: u64 = 0xA6;
    const BLOCK_ADDITIONAL: u64 = 0xA5;

    pub fn read(file: &mut File) -> Option<Info> {
        let end = file.seek(SeekFrom::End(0)).ok()?;
        file.seek(SeekFrom::Start(0)).ok()?;
        let mut info = Info::default();
        let mut scale = 1_000_000.0;
        walk(file, end, &mut info, &mut scale, 0)?;
        Some(info)
    }

    /// Goes through the elements up to `end`, into those that hold what
    /// is wanted, past all else.
    fn walk(file: &mut File, end: u64, info: &mut Info, scale: &mut f64, cluster_time: u64) -> Option<()> {
        let mut cluster_time = cluster_time;
        let mut block_time: Option<f64> = None;
        while file.stream_position().ok()? < end {
            let Some((id, start, size)) = element(file) else { break };
            let stop = if size == u64::MAX { end } else { (start + size).min(end) };
            match id {
                SEGMENT | INFO | TRACKS | TRACK_ENTRY | VIDEO | CLUSTER | BLOCK_GROUP | BLOCK_ADDITIONS | BLOCK_MORE => {
                    if id == BLOCK_GROUP {
                        block_time = None;
                    }
                    walk_inner(file, stop, info, scale, &mut cluster_time, &mut block_time)?;
                }
                COLOUR => colour(file, stop, info),
                _ => {}
            }
            file.seek(SeekFrom::Start(stop)).ok()?;
        }
        Some(())
    }

    fn walk_inner(file: &mut File, end: u64, info: &mut Info, scale: &mut f64, cluster_time: &mut u64, block_time: &mut Option<f64>) -> Option<()> {
        while file.stream_position().ok()? < end {
            let Some((id, start, size)) = element(file) else { break };
            let stop = if size == u64::MAX { end } else { (start + size).min(end) };
            match id {
                TIMECODE_SCALE => *scale = uint(&read_bytes(file, size)?) as f64,
                TIMECODE => *cluster_time = uint(&read_bytes(file, size)?),
                BLOCK => {
                    // The track number, then the time against the cluster's.
                    let _ = vint(file, false)?;
                    let mut relative = [0u8; 2];
                    file.read_exact(&mut relative).ok()?;
                    let ticks = *cluster_time as i64 + i64::from(i16::from_be_bytes(relative));
                    *block_time = Some(ticks as f64 * *scale / 1e9);
                }
                BLOCK_ADDITIONAL => {
                    let data = read_bytes(file, size)?;
                    take_t35(info, &data, block_time.unwrap_or(0.0), "WebM BlockAdditional");
                }
                COLOUR => colour(file, stop, info),
                INFO | TRACKS | TRACK_ENTRY | VIDEO | CLUSTER | BLOCK_GROUP | BLOCK_ADDITIONS | BLOCK_MORE => {
                    if id == BLOCK_GROUP {
                        *block_time = None;
                    }
                    walk_inner(file, stop, info, scale, cluster_time, block_time)?;
                }
                _ => {}
            }
            file.seek(SeekFrom::Start(stop)).ok()?;
        }
        Some(())
    }

    /// A video track's Colour element: how its values are coded, and the
    /// static HDR10 figures.
    fn colour(file: &mut File, end: u64, info: &mut Info) {
        let (mut cll, mut fall, mut peak, mut floor) = (None, None, None, None);
        while file.stream_position().map_or(false, |at| at < end) {
            let Some((id, start, size)) = element(file) else { break };
            let stop = (start + size).min(end);
            match id {
                // Primaries, transfer, MaxCLL, MaxFALL.
                0x55BB | 0x55BA | 0x55BC | 0x55BD => {
                    let Some(data) = read_bytes(file, size) else { break };
                    let value = uint(&data);
                    match id {
                        0x55BB => info.bt2020 |= value == 9,
                        0x55BA => info.transfer = Transfer::from_code(value).or(info.transfer),
                        0x55BC => cll = Some(value as u32),
                        _ => fall = Some(value as u32),
                    }
                }
                // MasteringMetadata, with its luminances.
                0x55D0 => {
                    while file.stream_position().map_or(false, |at| at < stop) {
                        let Some((inner, inner_start, inner_size)) = element(file) else { break };
                        if inner == 0x55D9 || inner == 0x55DA {
                            if let Some(data) = read_bytes(file, inner_size) {
                                if inner == 0x55D9 { peak = Some(float(&data)) } else { floor = Some(float(&data)) }
                            }
                        }
                        let _ = file.seek(SeekFrom::Start(inner_start + inner_size));
                    }
                }
                _ => {}
            }
            let _ = file.seek(SeekFrom::Start(stop));
        }
        if let (Some(cll), Some(fall)) = (cll, fall) {
            info.light = Some((cll, fall));
        }
        if let Some(peak) = peak {
            info.mastering = Some((peak, floor.unwrap_or(0.0)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_between_two_stored_ones_is_their_blend() {
        let agtm = Agtm {
            reference_white: 203.0,
            baseline: 2.0,
            altr: vec![Altr { headroom: 0.0, curve: vec![Point { x: 1.0, y: -1.0, m: 0.0 }, Point { x: 4.0, y: -2.0, m: 0.0 }] }],
        };
        // At the baseline there is no gain; at 0 the stored curve holds.
        assert!((agtm.output_at(2.0, 3.0) - 3.0).abs() < 1e-9);
        assert!((agtm.output_at(0.0, 0.5) - 0.25).abs() < 1e-9);
        // Half way, half the stops.
        assert!((agtm.output_at(1.0, 0.5) - 0.5 * (-0.5f64).exp2()).abs() < 1e-9);
        // Past the last point the output stays where it got to.
        assert!((Agtm::output(&agtm.altr[0], 8.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hdr10plus_tone_runs_from_nothing_to_the_peak() {
        let curve = Hdr10Plus { target: 400.0, maxscl: [0.0; 3], average: 0.0, knee: (0.1, 0.2), anchors: vec![0.5, 0.8] };
        assert!(curve.tone(0.0).abs() < 1e-9);
        assert!((curve.tone(0.05) - 0.1).abs() < 1e-9);
        assert!((curve.tone(1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_reference_white_message_makes_two_curves() {
        // Version 0; adaptive, the default white; baseline 2.3; reference white curves.
        let payload = [0x00, 0x40, (23000u16 >> 8) as u8, (23000u16 & 0xFF) as u8, 0x80];
        let agtm = parse_agtm(&payload).unwrap();
        assert_eq!(agtm.reference_white, 203.0);
        assert!((agtm.baseline - 2.3).abs() < 1e-9);
        assert_eq!(agtm.altr.len(), 2);
        assert_eq!(agtm.altr[0].headroom, 0.0);
    }
}
