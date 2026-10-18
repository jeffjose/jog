//! Two pictures compared: one over the other, the divider between them
//! following the pointer, zoomed and moved about together, and how near
//! the second is to the first, by SSIMULACRA2, when they are of a size.
//! The two marked, or the one marked and the one on view.

use image::RgbImage;
use winit::keyboard::{Key, NamedKey};

use super::convert_panel::{cut, draw_region, in_rect, shown_region, verdict, ZOOMS};
use super::*;

/// Where two pictures of a size differ: black where they agree, brighter
/// the more they differ, scaled so that the larger differences show
/// whatever their size. With how much of the picture differs at all, and
/// by how much at most, out of 255.
#[derive(Clone, Debug)]
pub struct Delta {
    pub image: Arc<RgbImage>,
    pub changed: f64,
    pub most: u8,
}

impl Delta {
    /// The difference of two pictures of a size: of each pixel, the most
    /// any of its channels differs.
    fn of(a: &RgbImage, b: &RgbImage) -> Delta {
        use rayon::prelude::*;
        let (w, h) = a.dimensions();
        let raw: Vec<u8> = a.as_raw().par_chunks(3).zip(b.as_raw().par_chunks(3)).map(|(p, q)| (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0)).collect();
        let mut counts = [0u64; 256];
        for value in &raw {
            counts[*value as usize] += 1;
        }
        let total = raw.len().max(1) as u64;
        // Differences of a step or two are noise in the encoding of most
        // pictures; more than that is a change.
        let changed = counts[3..].iter().sum::<u64>() as f64 / total as f64;
        let most = counts.iter().rposition(|count| *count > 0).unwrap_or(0) as u8;
        // The brightest is the 99.9th centile, so one stray pixel does not
        // leave the rest too dark to see.
        let mut seen = 0;
        let top = (0..256).rev().find(|value| {
            seen += counts[*value];
            seen * 1000 > total
        });
        let top = top.unwrap_or(1).max(4) as u32;
        let pixels: Vec<u8> = raw.par_iter().flat_map_iter(|value| {
            let level = (u32::from(*value) * 255 / top).min(255) as u8;
            [level; 3]
        }).collect();
        let image = RgbImage::from_raw(w, h, pixels).unwrap_or_else(|| RgbImage::new(w, h));
        Delta { image: Arc::new(image), changed, most }
    }
}

/// How long each of the two stays up when they flicker.
const FLICKER: Duration = Duration::from_millis(600);

pub struct Compare {
    ticket: u64,
    /// Showing where the two differ, rather than one beside the other.
    showing_delta: bool,
    /// Flickering between the two in place: the one shown, and when the
    /// other takes its turn.
    pub flicker: Option<(usize, Instant)>,
    delta: Option<Delta>,
    /// The two, the first on the left of the divider.
    ids: [usize; 2],
    images: Option<[Arc<RgbImage>; 2]>,
    failed: Option<String>,
    /// How near the smaller file is to the larger, taken for the original,
    /// once worked out; `None` within when they are not of a size. The
    /// larger, by id.
    score: Option<Option<f64>>,
    original: usize,
    zoom: u32,
    center: (f64, f64),
    /// Where the pictures were drawn, for clicks.
    place: [i32; 4],
}

impl App {
    /// Compares the two marked, or the one marked and the one on view.
    pub(super) fn open_compare(&mut self) {
        let here = self.list[self.focus()];
        let marked: Vec<usize> = self.list.iter().copied().filter(|id| self.marks.contains(id)).collect();
        let ids = match marked.as_slice() {
            [a, b] => [*a, *b],
            [one] if *one != here => [*one, here],
            _ => return self.toast("Mark one or two pictures (x)"),
        };
        if ids.iter().any(|id| files::is_video(&self.files[*id])) {
            return self.toast("Pictures only");
        }
        self.compare_ticket += 1;
        let ticket = self.compare_ticket;
        let jobs: Vec<(PathBuf, Transform)> = ids.iter().map(|id| (self.files[*id].clone(), self.transforms.get(id).copied().unwrap_or_default())).collect();
        // The score is of one against the other: the larger file is taken
        // for the original, whichever side it is shown on.
        let size = |id: usize| std::fs::metadata(&self.files[id]).map_or(0, |meta| meta.len());
        let first_original = size(ids[0]) >= size(ids[1]);
        let original = if first_original { ids[0] } else { ids[1] };
        let proxy = self.proxy.clone();
        // Decoded, as they are shown, on another thread; then scored.
        std::thread::spawn(move || {
            let open = |(path, transform): &(PathBuf, Transform)| -> Result<RgbImage, String> {
                let image = crate::decode::open_guarded(path).map_err(|error| format!("{}: {error}", path.display()))?;
                Ok(convert::flat(&convert::turned(image, *transform)))
            };
            let images = open(&jobs[0]).and_then(|a| Ok([Arc::new(a), Arc::new(open(&jobs[1])?)]));
            let both = images.as_ref().ok().cloned();
            let _ = proxy.send_event(Event::CompareLoaded(ticket, images));
            if let Some([a, b]) = both {
                // Where they differ first, being quick; then the score.
                if a.dimensions() == b.dimensions() {
                    let _ = proxy.send_event(Event::CompareDelta(ticket, Delta::of(&a, &b)));
                }
                let (reference, other) = if first_original { (&a, &b) } else { (&b, &a) };
                let score = (a.dimensions() == b.dimensions()).then(|| convert::score(reference, other)).flatten();
                let _ = proxy.send_event(Event::CompareScore(ticket, score));
            }
        });
        self.compare = Some(Compare { ticket, showing_delta: false, flicker: None, delta: None, ids, images: None, failed: None, score: None, original, zoom: 0, center: (0.5, 0.5), place: [0; 4] });
        self.menu = None;
        self.redraw();
    }

    pub(super) fn compare_loaded(&mut self, ticket: u64, images: Result<[Arc<RgbImage>; 2], String>) {
        let Some(compare) = self.compare.as_mut().filter(|compare| compare.ticket == ticket) else { return };
        match images {
            Ok(images) => compare.images = Some(images),
            Err(error) => compare.failed = Some(error),
        }
        self.redraw();
    }

    pub(super) fn compare_score(&mut self, ticket: u64, score: Option<f64>) {
        if let Some(compare) = self.compare.as_mut().filter(|compare| compare.ticket == ticket) {
            compare.score = Some(score);
            self.redraw();
        }
    }

    pub(super) fn compare_delta(&mut self, ticket: u64, delta: Delta) {
        if let Some(compare) = self.compare.as_mut().filter(|compare| compare.ticket == ticket) {
            compare.delta = Some(delta);
            self.redraw();
        }
    }

    pub(super) fn compare_key(&mut self, key: Key<&str>) {
        let Some(compare) = &mut self.compare else { return };
        match key {
            Key::Named(NamedKey::Escape) | Key::Character("q") => self.compare = None,
            // One in place of the other and back, over and over: the eye
            // catches what changes.
            Key::Character("b") => {
                compare.flicker = match compare.flicker {
                    Some(_) => None,
                    None => Some((0, Instant::now() + FLICKER)),
                };
                compare.showing_delta = false;
            }
            // Where they differ, and back.
            Key::Character("d") => match compare.delta.is_some() {
                true => (compare.showing_delta, compare.flicker) = (!compare.showing_delta, None),
                false if compare.images.as_ref().is_some_and(|[a, b]| a.dimensions() != b.dimensions()) => return self.toast("Different sizes · no difference"),
                false => return self.toast("Still working…"),
            },
            // The other way round: the second on the left.
            Key::Named(NamedKey::Tab) => {
                compare.ids.swap(0, 1);
                if let Some(images) = &mut compare.images {
                    images.swap(0, 1);
                }
            }
            Key::Character("z") => {
                let at = ZOOMS.iter().position(|z| *z == compare.zoom).unwrap_or(0);
                compare.zoom = ZOOMS[(at + 1) % ZOOMS.len()];
            }
            _ => return,
        }
        self.redraw();
    }

    /// A click looks closer there.
    pub(super) fn compare_click(&mut self, x: f64, y: f64) {
        let Some(compare) = &mut self.compare else { return };
        let Some(images) = &compare.images else { return };
        if !in_rect((x, y), compare.place) {
            return;
        }
        let size = (f64::from(images[0].width()), f64::from(images[0].height()));
        let [px, py, pw, ph] = compare.place;
        let (shown, dest) = shown_region(size, [px, py, pw, ph], compare.zoom, compare.center);
        let fx = ((x - f64::from(dest[0])) / f64::from(dest[2].max(1))).clamp(0.0, 1.0);
        let fy = ((y - f64::from(dest[1])) / f64::from(dest[3].max(1))).clamp(0.0, 1.0);
        compare.center = ((shown[0] + fx * shown[2]) / size.0, (shown[1] + fy * shown[3]) / size.1);
        if compare.zoom == 0 {
            compare.zoom = 1;
        }
        self.redraw();
    }

    pub(super) fn compare_wheel(&mut self, lines: f64) {
        let Some(compare) = &mut self.compare else { return };
        let at = ZOOMS.iter().position(|z| *z == compare.zoom).unwrap_or(0);
        let to = if lines > 0.0 { (at + 1).min(ZOOMS.len() - 1) } else { at.saturating_sub(1) };
        if to != at {
            compare.zoom = ZOOMS[to];
            self.redraw();
        }
    }

    pub(super) fn draw_compare(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let mouse = self.mouse;
        let Some(compare) = &self.compare else { return };
        let names = compare.ids.map(|id| self.name_of(id));
        let sizes = compare.ids.map(|id| std::fs::metadata(&self.files[id]).map_or(0, |meta| meta.len()));
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        canvas.fill(0, 0, canvas.width, canvas.height, 0x0C0C0C);
        let place = [pad, pad * 2 + line, canvas.width - 2 * pad, canvas.height - (pad * 2 + line) - (pad * 2 + line + small_line)];

        // The two, the second from the divider on.
        let mut dims = [None, None];
        let delta = compare.delta.as_ref().filter(|_| compare.showing_delta);
        let flicker = compare.flicker.map(|(side, _)| side);
        if let (Some(images), Some(side)) = (&compare.images, flicker) {
            // The one whose turn it is, alone and whole, in the first's terms.
            dims = [Some(images[0].dimensions()), Some(images[1].dimensions())];
            let size = (f64::from(images[0].width()), f64::from(images[0].height()));
            let (shown, dest) = shown_region(size, place, compare.zoom, compare.center);
            let (kx, ky) = (f64::from(images[side].width()) / size.0, f64::from(images[side].height()) / size.1);
            if let Some(frame) = cut(&images[side], [shown[0] * kx, shown[1] * ky, shown[2] * kx, shown[3] * ky]) {
                draw_region(canvas, &frame, dest, compare.zoom.max(u32::from(kx != 1.0)));
            }
        } else if let (Some(images), Some(delta)) = (&compare.images, delta) {
            // Where they differ, bright on black, over the whole.
            dims = [Some(images[0].dimensions()), Some(images[1].dimensions())];
            let size = (f64::from(delta.image.width()), f64::from(delta.image.height()));
            let (shown, dest) = shown_region(size, place, compare.zoom, compare.center);
            if let Some(frame) = cut(&delta.image, shown) {
                draw_region(canvas, &frame, dest, compare.zoom);
            }
        } else if let Some(images) = &compare.images {
            dims = [Some(images[0].dimensions()), Some(images[1].dimensions())];
            let size = (f64::from(images[0].width()), f64::from(images[0].height()));
            let (shown, dest) = shown_region(size, place, compare.zoom, compare.center);
            let split = if in_rect(mouse, place) { (mouse.0 as i32).clamp(dest[0], dest[0] + dest[2]) } else { dest[0] + dest[2] / 2 };
            if let Some(frame) = cut(&images[0], shown) {
                draw_region(canvas, &frame, dest, compare.zoom);
            }
            // The second in the first's terms: the same share of it, should
            // it be of another size.
            let (kx, ky) = (f64::from(images[1].width()) / size.0, f64::from(images[1].height()) / size.1);
            let share = f64::from(split - dest[0]) / f64::from(dest[2].max(1));
            let right = [(shown[0] + shown[2] * share) * kx, shown[1] * ky, shown[2] * (1.0 - share) * kx, shown[3] * ky];
            if let Some(frame) = cut(&images[1], right).filter(|_| split < dest[0] + dest[2]) {
                draw_region(canvas, &frame, [split, dest[1], dest[0] + dest[2] - split, dest[3]], compare.zoom.max(u32::from(kx != 1.0)));
            }
            canvas.fill(split - 1, dest[1], 2, dest[3], TEXT_COLOR);
        } else {
            let words = compare.failed.clone().map_or("Reading…".to_string(), |error| format!("Not readable · {error}"));
            text.draw(canvas, place[0] + pad, place[1] + pad, px, DIM_TEXT_COLOR, &words);
        }

        // Each one's name, size and file size, over its side.
        let about = |which: usize| {
            let dims = dims[which].map_or(String::new(), |(w, h)| format!("{w} x {h}   "));
            format!("{}   {dims}{}", names[which], meta::human_size(sizes[which]))
        };
        let (left_words, right_words) = (about(0), about(1));
        // Flickering, the one not up is dimmed.
        let color = |which: usize| if flicker.is_some_and(|side| side != which) { DIM_TEXT_COLOR } else { TEXT_COLOR };
        text.draw(canvas, pad, pad, px, color(0), &left_words);
        let right_w = text.width(&right_words, px);
        text.draw(canvas, canvas.width - pad - right_w, pad, px, color(1), &right_words);

        // How near, and the keys.
        let y = place[1] + place[3] + pad;
        let (original, other) = if compare.original == compare.ids[0] { (&names[0], &names[1]) } else { (&names[1], &names[0]) };
        let verdict_words = match (&compare.score, dims) {
            _ if delta.is_some() => {
                let delta = delta.map_or((0.0, 0), |delta| (delta.changed, delta.most));
                format!("Difference  ·  {:.1}% changed  ·  max {}/255", delta.0 * 100.0, delta.1)
            }
            (Some(Some(score)), _) => format!("SSIMULACRA2 {score:.1}  ·  {}  ·  {other} vs {original}", verdict(*score)),
            (_, [Some(a), Some(b)]) if a != b => "Different sizes: no score".to_string(),
            (Some(None), _) => "No score".to_string(),
            (None, _) if compare.images.is_some() => "Scoring…".to_string(),
            _ => String::new(),
        };
        text.draw(canvas, pad, y, px, TEXT_COLOR, &verdict_words);
        let zoom_words = match compare.zoom {
            0 => "whole picture".to_string(),
            z => format!("{}%", z * 100),
        };
        let difference = if compare.showing_delta { "d side by side" } else { "d difference" };
        let flicking = if compare.flicker.is_some() { "b stop" } else { "b flicker" };
        let keys = format!("{zoom_words}  ·  click to look closer  ·  z zoom  ·  {difference}  ·  {flicking}  ·  Tab swap  ·  Esc close");
        text.draw(canvas, pad, y + line, small, DIM_TEXT_COLOR, &keys);
        if let Some(compare) = &mut self.compare {
            compare.place = place;
        }
    }
}

impl App {
    /// The other of the two takes its turn, when flickering. When the
    /// next turn is, for the event loop to wake for.
    pub(super) fn tend_flicker(&mut self, now: Instant) -> Option<Instant> {
        let compare = self.compare.as_mut()?;
        let (side, due) = compare.flicker.as_mut()?;
        if *due <= now {
            *side = 1 - *side;
            *due = now + FLICKER;
            self.redraw();
        }
        self.compare.as_ref()?.flicker.map(|(_, due)| due)
    }
}
