use crate::frame::fit_size;

const MIN_SCALE: f64 = 0.02;
const MAX_SCALE: f64 = 64.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Zoom {
    /// Whole image visible.
    Fit,
    /// Image covers the window, cropping whatever overflows.
    Fill,
    /// Screen pixels per image pixel.
    Scale(f64),
}

/// Where the image lands on screen: its displayed size and top-left corner.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

/// Zoom and pan state. Sizes are `(width, height)`: `native` is the image at
/// full resolution and `window` the area it is shown in.
#[derive(Clone, Copy)]
pub struct View {
    pub zoom: Zoom,
    /// Image point shown at the middle of the window, in native pixels.
    /// `None` centers the image.
    pub center: Option<(f64, f64)>,
    /// Whether `Zoom::Fit` scales small images up to the window.
    pub enlarge: bool,
}

impl View {
    pub fn scale(&self, native: (u32, u32), window: (u32, u32)) -> f64 {
        let ratio = |win: u32, img: u32| f64::from(win) / f64::from(img);
        let (across, down) = (ratio(window.0, native.0), ratio(window.1, native.1));
        match self.zoom {
            Zoom::Fit if self.enlarge => across.min(down),
            Zoom::Fit => across.min(down).min(1.0),
            Zoom::Fill => across.max(down),
            Zoom::Scale(scale) => scale,
        }
    }

    pub fn layout(&self, native: (u32, u32), window: (u32, u32)) -> Layout {
        let scale = self.scale(native, window);
        let (width, height) = match self.zoom {
            // Agrees exactly with the size frames are pre-fitted to.
            Zoom::Fit => fit_size(native, window, self.enlarge),
            _ => {
                let scaled = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
                (scaled(native.0), scaled(native.1))
            }
        };
        let (cx, cy) = self.clamped_center(native, window, scale);
        let origin = |size: u32, win: u32, center: f64| {
            if size <= win {
                ((win - size) / 2) as i32
            } else {
                let offset = (f64::from(win) / 2.0 - center * scale).round() as i32;
                offset.clamp(win as i32 - size as i32, 0)
            }
        };
        Layout { width, height, x: origin(width, window.0, cx), y: origin(height, window.1, cy) }
    }

    /// Moves the image by a distance in screen pixels.
    pub fn pan(&mut self, dx: f64, dy: f64, native: (u32, u32), window: (u32, u32)) {
        let scale = self.scale(native, window);
        let (cx, cy) = self.clamped_center(native, window, scale);
        self.center = Some((cx - dx / scale, cy - dy / scale));
        self.center = Some(self.clamped_center(native, window, scale));
    }

    /// Zooms to `scale`, keeping the image point under `anchor` (a window
    /// position) where it is.
    pub fn zoom_to(&mut self, scale: f64, anchor: (f64, f64), native: (u32, u32), window: (u32, u32)) {
        let old = self.scale(native, window);
        let new = scale.clamp(MIN_SCALE, MAX_SCALE);
        let (cx, cy) = self.clamped_center(native, window, old);
        let from_middle = (anchor.0 - f64::from(window.0) / 2.0, anchor.1 - f64::from(window.1) / 2.0);
        let shift = |d: f64| d / old - d / new;
        self.zoom = Zoom::Scale(new);
        self.center = Some((cx + shift(from_middle.0), cy + shift(from_middle.1)));
        self.center = Some(self.clamped_center(native, window, new));
    }

    /// The center, kept where it would not expose space beyond the image.
    fn clamped_center(&self, native: (u32, u32), window: (u32, u32), scale: f64) -> (f64, f64) {
        let axis = |center: Option<f64>, img: u32, win: u32| {
            let (img, half) = (f64::from(img), f64::from(win) / 2.0 / scale);
            match center {
                Some(c) if img > 2.0 * half => c.clamp(half, img - half),
                _ => img / 2.0,
            }
        };
        (axis(self.center.map(|c| c.0), native.0, window.0), axis(self.center.map(|c| c.1), native.1, window.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE: (u32, u32) = (6000, 4000);
    const WINDOW: (u32, u32) = (3840, 2160);

    fn view(zoom: Zoom) -> View {
        View { zoom, center: None, enlarge: false }
    }

    #[test]
    fn fit_centers_the_image() {
        let layout = view(Zoom::Fit).layout(NATIVE, WINDOW);
        assert_eq!(layout, Layout { width: 3240, height: 2160, x: 300, y: 0 });
        let small = view(Zoom::Fit).layout((640, 480), WINDOW);
        assert_eq!(small, Layout { width: 640, height: 480, x: 1600, y: 840 });
        let enlarged = View { enlarge: true, ..view(Zoom::Fit) }.layout((640, 480), WINDOW);
        assert_eq!((enlarged.width, enlarged.height), (2880, 2160));
    }

    #[test]
    fn fill_covers_the_window() {
        let layout = view(Zoom::Fill).layout(NATIVE, WINDOW);
        assert_eq!(layout, Layout { width: 3840, height: 2560, x: 0, y: -200 });
    }

    #[test]
    fn panning_stops_at_the_edges() {
        let mut v = view(Zoom::Scale(1.0));
        assert_eq!(v.layout(NATIVE, WINDOW), Layout { width: 6000, height: 4000, x: -1080, y: -920 });
        v.pan(100.0, 0.0, NATIVE, WINDOW);
        assert_eq!(v.layout(NATIVE, WINDOW).x, -980);
        v.pan(1e6, 1e6, NATIVE, WINDOW);
        let layout = v.layout(NATIVE, WINDOW);
        assert_eq!((layout.x, layout.y), (0, 0));
        v.pan(-1e6, -1e6, NATIVE, WINDOW);
        let layout = v.layout(NATIVE, WINDOW);
        assert_eq!((layout.x, layout.y), (3840 - 6000, 2160 - 4000));
    }

    #[test]
    fn zoom_keeps_the_anchor_in_place() {
        let mut v = view(Zoom::Scale(1.0));
        let anchor = (1000.0, 500.0);
        // Image point under the anchor before zooming.
        let before = v.layout(NATIVE, WINDOW);
        let point = (anchor.0 - f64::from(before.x), anchor.1 - f64::from(before.y));
        v.zoom_to(2.0, anchor, NATIVE, WINDOW);
        let after = v.layout(NATIVE, WINDOW);
        assert_eq!(f64::from(after.x) + point.0 * 2.0, anchor.0);
        assert_eq!(f64::from(after.y) + point.1 * 2.0, anchor.1);
    }
}
