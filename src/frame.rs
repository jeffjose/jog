/// A decoded image in the `0RGB` layout the window surface uses.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Size of the full-resolution image this was (possibly) shrunk from.
    pub native: (u32, u32),
    pub pixels: Vec<u32>,
}

impl Frame {
    pub fn is_native(&self) -> bool {
        (self.width, self.height) == self.native
    }

    pub fn transformed(&self, t: Transform) -> Frame {
        let (w, h) = (self.width as usize, self.height as usize);
        let (ow, oh) = if t.swaps_axes() { (h, w) } else { (w, h) };
        let mut pixels = Vec::with_capacity(ow * oh);
        for y in 0..oh {
            for x in 0..ow {
                let (sx, sy) = match t.turns {
                    0 => (x, y),
                    1 => (y, h - 1 - x),
                    2 => (w - 1 - x, h - 1 - y),
                    _ => (w - 1 - y, x),
                };
                let sx = if t.mirrored { w - 1 - sx } else { sx };
                pixels.push(self.pixels[sy * w + sx]);
            }
        }
        let native = if t.swaps_axes() { (self.native.1, self.native.0) } else { self.native };
        Frame { width: ow as u32, height: oh as u32, native, pixels }
    }
}

/// A rotation in quarter turns plus an optional mirror: the image is mirrored
/// left-to-right first, then turned clockwise.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Transform {
    turns: u8,
    mirrored: bool,
}

impl Transform {
    pub const CW: Transform = Transform { turns: 1, mirrored: false };
    pub const CCW: Transform = Transform { turns: 3, mirrored: false };
    pub const MIRROR: Transform = Transform { turns: 0, mirrored: true };
    pub const FLIP: Transform = Transform { turns: 2, mirrored: true };

    pub fn is_identity(self) -> bool {
        self == Transform::default()
    }

    pub fn swaps_axes(self) -> bool {
        self.turns % 2 == 1
    }

    /// Quarter turns clockwise, applied after any mirroring.
    pub fn turns(self) -> u8 {
        self.turns
    }

    pub fn mirrored(self) -> bool {
        self.mirrored
    }

    /// The transform equivalent to applying `self` and then `next`.
    pub fn then(self, next: Transform) -> Transform {
        // Mirroring reverses the direction of any rotation done before it.
        let earlier = if next.mirrored { 4 - self.turns } else { self.turns };
        Transform { turns: (next.turns + earlier) % 4, mirrored: self.mirrored ^ next.mirrored }
    }
}

/// Largest size that fits `bounds` while keeping the aspect ratio. Images
/// already smaller than `bounds` are only scaled up if `enlarge` is set.
pub fn fit_size(size: (u32, u32), bounds: (u32, u32), enlarge: bool) -> (u32, u32) {
    let (w, h) = size;
    let (bw, bh) = bounds;
    if !enlarge && w <= bw && h <= bh {
        return size;
    }
    let scale = f64::min(f64::from(bw) / f64::from(w), f64::from(bh) / f64::from(h));
    let scaled = |v: u32, max: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max.max(1));
    (scaled(w, bw), scaled(h, bh))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, pixels: &[u32]) -> Frame {
        Frame { width, height, native: (width, height), pixels: pixels.to_vec() }
    }

    #[test]
    fn fit_size_only_enlarges_on_request() {
        assert_eq!(fit_size((640, 480), (3840, 2160), false), (640, 480));
        assert_eq!(fit_size((640, 480), (3840, 2160), true), (2880, 2160));
        assert_eq!(fit_size((6000, 4000), (3840, 2160), false), (3240, 2160));
        assert_eq!(fit_size((3000, 4500), (3840, 2160), false), (1440, 2160));
    }

    #[test]
    fn transforms_move_pixels() {
        // 1 2 3
        // 4 5 6
        let f = frame(3, 2, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(f.transformed(Transform::CW).pixels, [4, 1, 5, 2, 6, 3]);
        assert_eq!(f.transformed(Transform::CCW).pixels, [3, 6, 2, 5, 1, 4]);
        assert_eq!(f.transformed(Transform::MIRROR).pixels, [3, 2, 1, 6, 5, 4]);
        assert_eq!(f.transformed(Transform::FLIP).pixels, [4, 5, 6, 1, 2, 3]);
        let turned = f.transformed(Transform::CW);
        assert_eq!((turned.width, turned.height, turned.native), (2, 3, (2, 3)));
    }

    #[test]
    fn composition_matches_sequential_application() {
        let f = frame(3, 2, &[1, 2, 3, 4, 5, 6]);
        let ops = [Transform::CW, Transform::CCW, Transform::MIRROR, Transform::FLIP];
        for a in ops {
            for b in ops {
                for c in ops {
                    let stepwise = f.transformed(a).transformed(b).transformed(c);
                    let composed = f.transformed(a.then(b).then(c));
                    assert_eq!(stepwise.pixels, composed.pixels, "{a:?} {b:?} {c:?}");
                }
            }
        }
    }
}
