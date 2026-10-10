use std::error::Error;
use std::io::Cursor;
use std::path::Path;

use fast_image_resize as fir;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage};
use memchr::memmem;

use crate::{files, BACKGROUND};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// An image scaled to fit the screen, in the `0RGB` layout the window
/// surface uses, so that showing it is a plain row copy.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

pub fn load(path: &Path, target: (u32, u32)) -> Result<Frame> {
    let bytes = std::fs::read(path)?;
    let (image, orientation) = if files::is_raw(path) {
        decode_raw_preview(&bytes)?
    } else {
        decode(&bytes)?
    };
    fit(image, orientation, target)
}

fn decode(bytes: &[u8]) -> Result<(DynamicImage, Orientation)> {
    let mut decoder = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    Ok((DynamicImage::from_decoder(decoder)?, orientation))
}

/// Camera RAW files carry a camera-rendered JPEG preview. Showing that is
/// far faster than demosaicing the sensor data, so find the largest one.
fn decode_raw_preview(bytes: &[u8]) -> Result<(DynamicImage, Orientation)> {
    let mut candidates: Vec<(u64, usize)> = memmem::find_iter(bytes, b"\xFF\xD8\xFF")
        .filter_map(|at| jpeg_area(&bytes[at..]).map(|area| (area, at)))
        .collect();
    candidates.sort_unstable_by(|a, b| b.cmp(a));

    for &(_, at) in candidates.iter().take(4) {
        if let Ok((image, mut orientation)) = decode(&bytes[at..]) {
            // Previews usually rely on the orientation of the enclosing file.
            if orientation == Orientation::NoTransforms {
                orientation = tiff_orientation(bytes).unwrap_or(orientation);
            }
            return Ok((image, orientation));
        }
    }
    Err("no embedded JPEG preview found in RAW file".into())
}

/// Pixel count of the JPEG starting at `data`, or `None` if the header is
/// malformed or it is a lossless JPEG (which is sensor data, not a preview).
fn jpeg_area(data: &[u8]) -> Option<u64> {
    let mut i = 2;
    loop {
        let marker = *data.get(i + 1)?;
        if data[i] != 0xFF {
            return None;
        }
        match marker {
            0xFF => {
                i += 1;
                continue;
            }
            0xC0..=0xC2 => {
                let height = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]);
                let width = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]);
                let area = u64::from(width) * u64::from(height);
                return (area > 0).then_some(area);
            }
            0xC4 | 0xDB..=0xFE => {
                let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]);
                if len < 2 {
                    return None;
                }
                i += 2 + usize::from(len);
            }
            _ => return None,
        }
    }
}

/// Orientation tag from the first IFD of a TIFF-structured file.
fn tiff_orientation(bytes: &[u8]) -> Option<Orientation> {
    let big_endian = match bytes.get(..2)? {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    let u16_at = |at: usize| {
        let b = [*bytes.get(at)?, *bytes.get(at + 1)?];
        Some(if big_endian { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    };
    let u32_at = |at: usize| {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };

    let ifd = u32_at(4)? as usize;
    for entry in 0..usize::from(u16_at(ifd)?) {
        let at = ifd + 2 + entry * 12;
        if u16_at(at)? == 0x0112 {
            return Orientation::from_exif(u8::try_from(u16_at(at + 8)?).ok()?);
        }
    }
    None
}

/// Largest size that fits `bounds` while keeping the aspect ratio. Never
/// enlarges.
pub fn fit_size(size: (u32, u32), bounds: (u32, u32)) -> (u32, u32) {
    let (w, h) = size;
    let (bw, bh) = bounds;
    if w <= bw && h <= bh {
        return size;
    }
    let scale = f64::min(f64::from(bw) / f64::from(w), f64::from(bh) / f64::from(h));
    let scaled = |v: u32, max: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max.max(1));
    (scaled(w, bw), scaled(h, bh))
}

fn fit(image: DynamicImage, orientation: Orientation, target: (u32, u32)) -> Result<Frame> {
    use Orientation::*;
    // Rotating is cheapest after shrinking, so scale into the pre-rotation box.
    let bounds = match orientation {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (target.1, target.0),
        _ => target,
    };

    let mut rgb = flatten(image);
    let size = fit_size(rgb.dimensions(), bounds);
    if size != rgb.dimensions() {
        rgb = resize(&rgb, size)?;
    }

    let mut oriented = DynamicImage::ImageRgb8(rgb);
    oriented.apply_orientation(orientation);
    let rgb = oriented.into_rgb8();

    let pixels = rgb
        .pixels()
        .map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]))
        .collect();
    Ok(Frame { width: rgb.width(), height: rgb.height(), pixels })
}

/// Converts to 8-bit RGB, compositing any transparency over the background.
fn flatten(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let bg = [(BACKGROUND >> 16) as u8, (BACKGROUND >> 8) as u8, BACKGROUND as u8];
    let rgba = image.into_rgba8();
    let mut rgb = Vec::with_capacity(rgba.as_raw().len() / 4 * 3);
    for px in rgba.as_raw().chunks_exact(4) {
        let alpha = u32::from(px[3]);
        for c in 0..3 {
            let blended = u32::from(px[c]) * alpha + u32::from(bg[c]) * (255 - alpha);
            rgb.push(((blended + 127) / 255) as u8);
        }
    }
    RgbImage::from_raw(rgba.width(), rgba.height(), rgb).expect("buffer matches dimensions")
}

fn resize(src: &RgbImage, (width, height): (u32, u32)) -> Result<RgbImage> {
    let view = fir::images::ImageRef::new(src.width(), src.height(), src.as_raw(), fir::PixelType::U8x3)?;
    let mut dst = fir::images::Image::new(width, height, fir::PixelType::U8x3);
    let options = fir::ResizeOptions::new()
        .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3));
    fir::Resizer::new().resize(&view, &mut dst, &options)?;
    Ok(RgbImage::from_raw(width, height, dst.into_vec()).expect("buffer matches dimensions"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb};

    fn rgb(pixel: u32) -> (u32, u32, u32) {
        (pixel >> 16 & 0xFF, pixel >> 8 & 0xFF, pixel & 0xFF)
    }

    #[test]
    fn fit_size_only_shrinks() {
        assert_eq!(fit_size((640, 480), (3840, 2160)), (640, 480));
        assert_eq!(fit_size((6000, 4000), (3840, 2160)), (3240, 2160));
        assert_eq!(fit_size((3000, 4500), (3840, 2160)), (1440, 2160));
    }

    #[test]
    fn raw_preview_is_found_and_rotated() {
        // Left half red, right half blue.
        let preview = RgbImage::from_fn(400, 200, |x, _| Rgb(if x < 200 { [255, 0, 0] } else { [0, 0, 255] }));
        let mut jpeg = Vec::new();
        preview.write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg).unwrap();

        // A TIFF header whose only tag is Orientation = 6 (rotate 90 clockwise),
        // then noise containing a stray JPEG marker, then the preview.
        let mut raw = b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00".to_vec();
        raw.extend_from_slice(b"\x00\xFF\xD8\xFF\x00noise");
        raw.extend_from_slice(&jpeg);
        raw.extend_from_slice(&[0xAB; 64]);

        let (image, orientation) = decode_raw_preview(&raw).unwrap();
        assert_eq!(orientation, Orientation::Rotate90);
        let frame = fit(image, orientation, (100, 100)).unwrap();
        assert_eq!((frame.width, frame.height), (50, 100));

        let at = |x: usize, y: usize| rgb(frame.pixels[y * 50 + x]);
        let (r, _, b) = at(25, 10);
        assert!(r > 200 && b < 50, "top should be red");
        let (r, _, b) = at(25, 90);
        assert!(r < 50 && b > 200, "bottom should be blue");
    }

    #[test]
    fn transparency_is_composited_over_background() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 0])));
        let frame = fit(image, Orientation::NoTransforms, (10, 10)).unwrap();
        assert!(frame.pixels.iter().all(|&p| p == BACKGROUND));
    }
}
