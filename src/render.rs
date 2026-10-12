use std::error::Error;

use fast_image_resize as fir;
use fir::IntoImageView;
use rayon::prelude::*;

use crate::frame::Frame;
use crate::view::Layout;

/// Paints `frame` into a `window`-sized buffer at `layout`, clearing the
/// rest. Scaling is nearest-neighbour unless `smooth` asks for filtering,
/// which looks better but costs more.
///
/// `picture` picks the frame of an animation; 0 is the first or only one.
///
/// Every row is done on its own core: at 4K the buffer is 33 MB, which one
/// core takes several milliseconds just to write, and the thread that
/// draws has usually just woken from a wait and is slow to get going.
#[allow(clippy::too_many_arguments)]
pub fn draw_image(dst: &mut [u32], window: (u32, u32), frame: &Frame, picture: usize, layout: Layout, smooth: bool, background: u32) {
    let pixels = frame.picture(picture);
    let (win_w, win_h) = (window.0 as i32, window.1 as i32);
    let visible = Visible {
        x0: layout.x.max(0),
        x1: (layout.x + layout.width as i32).min(win_w),
        y0: layout.y.max(0),
        y1: (layout.y + layout.height as i32).min(win_h),
    };
    if visible.x0 >= visible.x1 || visible.y0 >= visible.y1 || win_w <= 0 {
        return fill_all(dst, background);
    }
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    let span = visible.x0 as usize..visible.x1 as usize;
    let rows = visible.y0 as usize..visible.y1 as usize;

    if (frame.width, frame.height) == (layout.width, layout.height) {
        let (left, top) = ((visible.x0 - layout.x) as usize, (visible.y0 - layout.y) as usize);
        dst.par_chunks_exact_mut(win_w as usize).enumerate().for_each(|(y, row)| {
            if !rows.contains(&y) {
                return row.fill(background);
            }
            let source = &pixels[(top + y - rows.start) * fw + left..][..span.len()];
            clear_around(row, &span, background);
            row[span.clone()].copy_from_slice(source);
        });
        return;
    }
    if smooth {
        // The margins first, then the picture straight into the window
        // buffer, its rows taken by the scaler as they lie in the window.
        dst.par_chunks_exact_mut(win_w as usize).enumerate().for_each(|(y, row)| match rows.contains(&y) {
            true => clear_around(row, &span, background),
            false => row.fill(background),
        });
        if resample(dst, win_w as usize, frame, pixels, layout, &visible).is_ok() {
            return;
        }
    }

    // Sample at destination pixel centres.
    let nearest = |at: i32, shown: u32, source: usize| {
        ((at as u64 * 2 + 1) * source as u64 / (u64::from(shown) * 2)).min(source as u64 - 1) as usize
    };
    let columns: Vec<usize> =
        (visible.x0..visible.x1).map(|x| nearest(x - layout.x, layout.width, fw)).collect();
    dst.par_chunks_exact_mut(win_w as usize).enumerate().for_each(|(y, row)| {
        if !rows.contains(&y) {
            return row.fill(background);
        }
        clear_around(row, &span, background);
        let from = nearest(y as i32 - layout.y, layout.height, fh) * fw;
        let source = &pixels[from..from + fw];
        for (pixel, &column) in row[span.clone()].iter_mut().zip(&columns) {
            *pixel = source[column];
        }
    });
}

fn fill_all(dst: &mut [u32], background: u32) {
    dst.par_chunks_mut(1 << 16).for_each(|part| part.fill(background));
}

/// Clears what is to the left and right of `span` in a row.
fn clear_around(row: &mut [u32], span: &std::ops::Range<usize>, background: u32) {
    row[..span.start].fill(background);
    row[span.end..].fill(background);
}

/// The part of the image rectangle that is inside the window.
struct Visible {
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
}

/// A rectangle of a larger buffer, as an image the scaler can write into:
/// rows `stride` pixels apart, the first starting `offset` pixels in. The
/// scaler splits it into bands for its threads by way of the rows.
struct Strided<'a> {
    pixels: &'a mut [u32],
    offset: usize,
    stride: usize,
    width: u32,
    height: u32,
}

unsafe impl fir::ImageView for Strided<'_> {
    type Pixel = fir::pixels::U8x4;

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn iter_rows(&self, start_row: u32) -> impl Iterator<Item = &[Self::Pixel]> {
        let (width, offset, stride) = (self.width as usize, self.offset, self.stride);
        let rows = self.height.saturating_sub(start_row) as usize;
        (start_row as usize..).take(rows).map(move |y| {
            let pixels: &[u32] = &self.pixels[offset + y * stride..][..width];
            bytemuck::cast_slice(pixels)
        })
    }
}

unsafe impl fir::ImageViewMut for Strided<'_> {
    fn iter_rows_mut(&mut self, start_row: u32) -> impl Iterator<Item = &mut [Self::Pixel]> {
        let (width, offset, stride) = (self.width as usize, self.offset, self.stride);
        let rows = self.height.saturating_sub(start_row) as usize;
        let from = offset + start_row as usize * stride;
        let pixels = self.pixels.get_mut(from..).unwrap_or_default();
        pixels.chunks_mut(stride).take(rows).map(move |row| bytemuck::cast_slice_mut(&mut row[..width]))
    }
}

fn resample(dst: &mut [u32], win_w: usize, frame: &Frame, pixels: &[u32], layout: Layout, visible: &Visible) -> Result<(), Box<dyn Error>> {
    let (width, height) = ((visible.x1 - visible.x0) as u32, (visible.y1 - visible.y0) as u32);
    let across = f64::from(frame.width) / f64::from(layout.width);
    let down = f64::from(frame.height) / f64::from(layout.height);

    let source = fir::images::ImageRef::new(
        frame.width,
        frame.height,
        bytemuck::cast_slice(pixels),
        fir::PixelType::U8x4,
    )?;
    let mut target = Strided {
        pixels: dst,
        offset: visible.y0 as usize * win_w + visible.x0 as usize,
        stride: win_w,
        width,
        height,
    };
    let options = fir::ResizeOptions::new()
        .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom))
        .crop(
            f64::from(visible.x0 - layout.x) * across,
            f64::from(visible.y0 - layout.y) * down,
            f64::from(width) * across,
            f64::from(height) * down,
        )
        // The fourth byte is padding, not alpha.
        .use_alpha(false);
    let source = source.image_view::<fir::pixels::U8x4>().ok_or("pixel type")?;
    fir::Resizer::new().resize_typed(&source, &mut target, &options)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32) -> Frame {
        let pixels = (0..width * height).collect();
        Frame::still(width, height, (width, height), pixels)
    }

    #[test]
    fn exact_size_is_copied_and_centred() {
        let mut buffer = [9; 12];
        let layout = Layout { width: 2, height: 1, x: 1, y: 1 };
        draw_image(&mut buffer, (4, 3), &frame(2, 1), 0, layout, false, 7);
        assert_eq!(buffer, [7, 7, 7, 7, 7, 0, 1, 7, 7, 7, 7, 7]);
    }

    #[test]
    fn panned_image_is_clipped() {
        let mut buffer = [9; 4];
        let layout = Layout { width: 4, height: 4, x: -1, y: -2 };
        draw_image(&mut buffer, (2, 2), &frame(4, 4), 0, layout, false, 7);
        assert_eq!(buffer, [9, 10, 13, 14]);
    }

    #[test]
    fn nearest_scaling_shrinks_and_enlarges() {
        let mut buffer = [9; 6];
        let layout = Layout { width: 2, height: 2, x: 0, y: 0 };
        draw_image(&mut buffer, (3, 2), &frame(4, 4), 0, layout, false, 7);
        assert_eq!(buffer, [5, 7, 7, 13, 15, 7]);

        let mut buffer = [9; 8];
        let layout = Layout { width: 4, height: 2, x: 0, y: 0 };
        draw_image(&mut buffer, (4, 2), &frame(2, 1), 0, layout, false, 7);
        assert_eq!(buffer, [0, 0, 1, 1, 0, 0, 1, 1]);
    }

    #[test]
    fn smooth_shrink_averages_pixels() {
        let source = Frame::still(4, 4, (4, 4), vec![0x808080; 16]);
        let mut buffer = [9; 4];
        let layout = Layout { width: 2, height: 2, x: 0, y: 0 };
        draw_image(&mut buffer, (2, 2), &source, 0, layout, true, 7);
        assert_eq!(buffer, [0x808080; 4]);
    }

    #[test]
    fn smooth_drawing_stays_inside_the_layout() {
        // A flat picture drawn smoothly into the middle of a larger
        // window: the margins are background, the picture its colour, and
        // when panned partly off the window only the part on it is drawn.
        let source = Frame::still(8, 8, (8, 8), vec![0x404040; 64]);
        let mut buffer = [9; 6 * 5];
        let layout = Layout { width: 4, height: 3, x: 1, y: 1 };
        draw_image(&mut buffer, (6, 5), &source, 0, layout, true, 7);
        for y in 0..5 {
            for x in 0..6 {
                let inside = (1..5).contains(&x) && (1..4).contains(&y);
                assert_eq!(buffer[y * 6 + x], if inside { 0x404040 } else { 7 }, "at {x},{y}");
            }
        }
        let mut buffer = [9; 6 * 5];
        let layout = Layout { width: 4, height: 3, x: -2, y: 3 };
        draw_image(&mut buffer, (6, 5), &source, 0, layout, true, 7);
        for y in 0..5 {
            for x in 0..6 {
                let inside = (0..2).contains(&x) && (3..5).contains(&y);
                assert_eq!(buffer[y * 6 + x], if inside { 0x404040 } else { 7 }, "at {x},{y}");
            }
        }
    }
}
