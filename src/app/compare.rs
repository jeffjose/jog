//! Two pictures compared: one over the other, the divider between them
//! following the pointer, zoomed and moved about together, and how near
//! the second is to the first, by SSIMULACRA2, when they are of a size.
//! The two marked, or the one marked and the one on view.

use image::RgbImage;
use winit::keyboard::{Key, NamedKey};

use super::convert_panel::{cut, draw_region, in_rect, shown_region, verdict, ZOOMS};
use super::*;

pub struct Compare {
    ticket: u64,
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
            _ => return self.toast("Mark two pictures (x) to compare them, or one to compare with the one on view"),
        };
        if ids.iter().any(|id| files::is_video(&self.files[*id])) {
            return self.toast("Compare is for pictures");
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
                let image = crate::decode::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
                Ok(convert::flat(&convert::turned(image, *transform)))
            };
            let images = open(&jobs[0]).and_then(|a| Ok([Arc::new(a), Arc::new(open(&jobs[1])?)]));
            let both = images.as_ref().ok().cloned();
            let _ = proxy.send_event(Event::CompareLoaded(ticket, images));
            if let Some([a, b]) = both {
                let (reference, other) = if first_original { (&a, &b) } else { (&b, &a) };
                let score = (a.dimensions() == b.dimensions()).then(|| convert::score(reference, other)).flatten();
                let _ = proxy.send_event(Event::CompareScore(ticket, score));
            }
        });
        self.compare = Some(Compare { ticket, ids, images: None, failed: None, score: None, original, zoom: 0, center: (0.5, 0.5), place: [0; 4] });
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

    pub(super) fn compare_key(&mut self, key: Key<&str>) {
        let Some(compare) = &mut self.compare else { return };
        match key {
            Key::Named(NamedKey::Escape) | Key::Character("q") => self.compare = None,
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
        if let Some(images) = &compare.images {
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
            let words = compare.failed.clone().map_or("Reading the two…".to_string(), |error| format!("Cannot read them: {error}"));
            text.draw(canvas, place[0] + pad, place[1] + pad, px, DIM_TEXT_COLOR, &words);
        }

        // Each one's name, size and file size, over its side.
        let about = |which: usize| {
            let dims = dims[which].map_or(String::new(), |(w, h)| format!("{w} x {h}   "));
            format!("{}   {dims}{}", names[which], meta::human_size(sizes[which]))
        };
        let (left_words, right_words) = (about(0), about(1));
        text.draw(canvas, pad, pad, px, TEXT_COLOR, &left_words);
        let right_w = text.width(&right_words, px);
        text.draw(canvas, canvas.width - pad - right_w, pad, px, TEXT_COLOR, &right_words);

        // How near, and the keys.
        let y = place[1] + place[3] + pad;
        let (original, other) = if compare.original == compare.ids[0] { (&names[0], &names[1]) } else { (&names[1], &names[0]) };
        let verdict_words = match (&compare.score, dims) {
            (Some(Some(score)), _) => format!("SSIMULACRA2 {score:.1}, {}: {other} against {original}, the larger file", verdict(*score)),
            (_, [Some(a), Some(b)]) if a != b => "Of different sizes: shown at the same, with no score".to_string(),
            (Some(None), _) => "No score".to_string(),
            (None, _) if compare.images.is_some() => "Scoring…".to_string(),
            _ => String::new(),
        };
        text.draw(canvas, pad, y, px, TEXT_COLOR, &verdict_words);
        let zoom_words = match compare.zoom {
            0 => "whole picture".to_string(),
            z => format!("{}%", z * 100),
        };
        let keys = format!("{zoom_words}   click to look there   wheel or z zoom   Tab swap   Esc close");
        text.draw(canvas, pad, y + line, small, DIM_TEXT_COLOR, &keys);
        if let Some(compare) = &mut self.compare {
            compare.place = place;
        }
    }
}
