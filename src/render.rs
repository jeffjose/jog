use std::error::Error;

use fast_image_resize as fir;

use crate::frame::Frame;
use crate::view::Layout;

/// Paints `frame` into a `window`-sized buffer at `layout`, clearing the
/// rest. Scaling is nearest-neighbour unless `smooth` asks for filtering,
/// which looks better but costs more.
pub fn draw_image(dst: &mut [u32], window: (u32, u32), frame: &Frame, layout: Layout, smooth: bool, background: u32) {
    dst.fill(background);
    let (win_w, win_h) = (window.0 as i32, window.1 as i32);
    let visible = Visible {
        x0: layout.x.max(0),
        x1: (layout.x + layout.width as i32).min(win_w),
        y0: layout.y.max(0),
        y1: (layout.y + layout.height as i32).min(win_h),
    };
    if visible.x0 >= visible.x1 || visible.y0 >= visible.y1 {
        return;
    }
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    let rows = dst.chunks_exact_mut(win_w as usize).skip(visible.y0 as usize);
    let rows = rows.take((visible.y1 - visible.y0) as usize);
    let span = visible.x0 as usize..visible.x1 as usize;

    if (frame.width, frame.height) == (layout.width, layout.height) {
        let (left, top) = ((visible.x0 - layout.x) as usize, (visible.y0 - layout.y) as usize);
        for (y, row) in rows.enumerate() {
            let source = &frame.pixels[(top + y) * fw + left..][..span.len()];
            row[span.clone()].copy_from_slice(source);
        }
        return;
    }
    if smooth && resample(dst, win_w as usize, frame, layout, &visible).is_ok() {
        return;
    }

    // Sample at destination pixel centres.
    let nearest = |at: i32, shown: u32, source: usize| {
        ((at as u64 * 2 + 1) * source as u64 / (u64::from(shown) * 2)).min(source as u64 - 1) as usize
    };
    let columns: Vec<usize> =
        (visible.x0..visible.x1).map(|x| nearest(x - layout.x, layout.width, fw)).collect();
    let rows = dst.chunks_exact_mut(win_w as usize).skip(visible.y0 as usize);
    for (y, row) in rows.take((visible.y1 - visible.y0) as usize).enumerate() {
        let from = nearest(visible.y0 - layout.y + y as i32, layout.height, fh) * fw;
        let source = &frame.pixels[from..from + fw];
        for (pixel, &column) in row[span.clone()].iter_mut().zip(&columns) {
            *pixel = source[column];
        }
    }
}

/// The part of the image rectangle that is inside the window.
struct Visible {
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
}

fn resample(dst: &mut [u32], win_w: usize, frame: &Frame, layout: Layout, visible: &Visible) -> Result<(), Box<dyn Error>> {
    let (width, height) = ((visible.x1 - visible.x0) as u32, (visible.y1 - visible.y0) as u32);
    let across = f64::from(frame.width) / f64::from(layout.width);
    let down = f64::from(frame.height) / f64::from(layout.height);

    let source = fir::images::ImageRef::new(
        frame.width,
        frame.height,
        bytemuck::cast_slice(&frame.pixels),
        fir::PixelType::U8x4,
    )?;
    let mut scaled = vec![0u32; width as usize * height as usize];
    let mut target =
        fir::images::Image::from_slice_u8(width, height, bytemuck::cast_slice_mut(&mut scaled), fir::PixelType::U8x4)?;
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
    fir::Resizer::new().resize(&source, &mut target, &options)?;

    let rows = dst.chunks_exact_mut(win_w).skip(visible.y0 as usize);
    for (row, source) in rows.zip(scaled.chunks_exact(width as usize)) {
        row[visible.x0 as usize..visible.x1 as usize].copy_from_slice(source);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32) -> Frame {
        let pixels = (0..width * height).collect();
        Frame { width, height, native: (width, height), pixels }
    }

    #[test]
    fn exact_size_is_copied_and_centred() {
        let mut buffer = [9; 12];
        let layout = Layout { width: 2, height: 1, x: 1, y: 1 };
        draw_image(&mut buffer, (4, 3), &frame(2, 1), layout, false, 7);
        assert_eq!(buffer, [7, 7, 7, 7, 7, 0, 1, 7, 7, 7, 7, 7]);
    }

    #[test]
    fn panned_image_is_clipped() {
        let mut buffer = [9; 4];
        let layout = Layout { width: 4, height: 4, x: -1, y: -2 };
        draw_image(&mut buffer, (2, 2), &frame(4, 4), layout, false, 7);
        assert_eq!(buffer, [9, 10, 13, 14]);
    }

    #[test]
    fn nearest_scaling_shrinks_and_enlarges() {
        let mut buffer = [9; 6];
        let layout = Layout { width: 2, height: 2, x: 0, y: 0 };
        draw_image(&mut buffer, (3, 2), &frame(4, 4), layout, false, 7);
        assert_eq!(buffer, [5, 7, 7, 13, 15, 7]);

        let mut buffer = [9; 8];
        let layout = Layout { width: 4, height: 2, x: 0, y: 0 };
        draw_image(&mut buffer, (4, 2), &frame(2, 1), layout, false, 7);
        assert_eq!(buffer, [0, 0, 1, 1, 0, 0, 1, 1]);
    }

    #[test]
    fn smooth_shrink_averages_pixels() {
        let source = Frame { width: 4, height: 4, native: (4, 4), pixels: vec![0x808080; 16] };
        let mut buffer = [9; 4];
        let layout = Layout { width: 2, height: 2, x: 0, y: 0 };
        draw_image(&mut buffer, (2, 2), &source, layout, true, 7);
        assert_eq!(buffer, [0x808080; 4]);
    }
}
