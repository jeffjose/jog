use std::error::Error;
use std::io::Cursor;
use std::path::Path;
use std::process::{Command, Stdio};

use fast_image_resize as fir;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage};
use memchr::memmem;

use crate::files;
use crate::frame::{fit_size, Frame, Transform};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// How a file should be turned into a frame.
#[derive(Clone, Copy, PartialEq)]
pub struct Spec {
    /// Box to shrink the image into, or `None` for full resolution.
    pub target: Option<(u32, u32)>,
    /// Rotation or mirroring the user asked for, on top of the EXIF one.
    pub transform: Transform,
    /// Colour shown through transparent pixels, as `0RGB`.
    pub background: u32,
    /// Whether this is for a thumbnail, which for a video should be a
    /// telling frame rather than the one playback opens on.
    pub thumbnail: bool,
}

pub fn load(path: &Path, spec: Spec) -> Result<Frame> {
    if files::is_video(path) {
        let (image, orientation) = decode(&video_still(path, spec.thumbnail)?)?;
        return finish(image, orientation, spec);
    }
    let bytes = std::fs::read(path)?;
    let (image, orientation) = if files::is_raw(path) {
        decode_raw_preview(&bytes, spec.target)?
    } else {
        decode(&bytes)?
    };
    finish(image, orientation, spec)
}

/// A still from a video, as an encoded image. A thumbnail is taken one
/// second in, where there is usually more to see than in the opening frame.
/// Otherwise it is the opening frame, so that the still shown while the
/// player starts is the picture playback begins on.
fn video_still(path: &Path, thumbnail: bool) -> Result<Vec<u8>> {
    let seeks: &[&str] = if thumbnail { &["1", "0"] } else { &["0"] };
    for seek in seeks {
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-ss", seek, "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "bmp", "-"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|error| format!("cannot run ffmpeg for a video still: {error}"))?;
        // Nothing comes out when the video is shorter than the seek.
        if !output.stdout.is_empty() {
            return Ok(output.stdout);
        }
    }
    Err("ffmpeg could not read a frame from this video".into())
}

/// The image at full resolution, upright, with any transparency intact.
pub fn open(path: &Path) -> Result<DynamicImage> {
    let bytes = std::fs::read(path)?;
    let (mut image, orientation) =
        if files::is_raw(path) { decode_raw_preview(&bytes, None)? } else { decode(&bytes)? };
    image.apply_orientation(orientation);
    Ok(image)
}

fn decode(bytes: &[u8]) -> Result<(DynamicImage, Orientation)> {
    let mut decoder = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    Ok((DynamicImage::from_decoder(decoder)?, orientation))
}

/// Camera RAW files carry camera-rendered JPEG previews. Showing one is far
/// faster than demosaicing the sensor data, so use the smallest preview that
/// covers `target`, or the largest there is.
fn decode_raw_preview(bytes: &[u8], target: Option<(u32, u32)>) -> Result<(DynamicImage, Orientation)> {
    let mut previews: Vec<(u64, (u32, u32), usize)> = memmem::find_iter(bytes, b"\xFF\xD8\xFF")
        .filter_map(|at| {
            let (w, h) = jpeg_size(&bytes[at..])?;
            Some((u64::from(w) * u64::from(h), (w, h), at))
        })
        .collect();
    previews.sort_unstable_by(|a, b| b.cmp(a));

    let covers = |&(_, (w, h), _): &(u64, (u32, u32), usize)| {
        target.is_some_and(|(tw, th)| w.max(h) >= tw.max(th) || w.min(h) >= tw.min(th))
    };
    let best = previews.iter().rposition(covers).unwrap_or(0);
    // Fall back to larger previews first, then smaller ones.
    let order = (0..=best).rev().chain(best + 1..previews.len());

    for index in order.take(6) {
        let Some(&(_, _, at)) = previews.get(index) else { break };
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

/// Dimensions of the JPEG starting at `data`, or `None` if the header is
/// malformed or it is a lossless JPEG (which is sensor data, not a preview).
fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
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
                return (width > 0 && height > 0).then_some((width.into(), height.into()));
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

fn finish(image: DynamicImage, orientation: Orientation, spec: Spec) -> Result<Frame> {
    use Orientation::*;
    let exif_swaps = matches!(orientation, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH);

    let mut rgb = flatten(image, spec.background);
    let (w, h) = rgb.dimensions();
    if let Some(target) = spec.target {
        // Rotating is cheapest after shrinking, so scale into the pre-rotation box.
        let swaps = exif_swaps ^ spec.transform.swaps_axes();
        let bounds = if swaps { (target.1, target.0) } else { target };
        let size = fit_size((w, h), bounds, false);
        if size != (w, h) {
            rgb = resize(&rgb, size)?;
        }
    }

    let mut oriented = DynamicImage::ImageRgb8(rgb);
    oriented.apply_orientation(orientation);
    let rgb = oriented.into_rgb8();

    let pixels = rgb
        .pixels()
        .map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]))
        .collect();
    let native = if exif_swaps { (h, w) } else { (w, h) };
    let frame = Frame { width: rgb.width(), height: rgb.height(), native, pixels };
    Ok(if spec.transform.is_identity() { frame } else { frame.transformed(spec.transform) })
}

/// Converts to 8-bit RGB, compositing any transparency over `background`.
pub fn flatten(image: DynamicImage, background: u32) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let bg = [(background >> 16) as u8, (background >> 8) as u8, background as u8];
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

    fn spec(target: Option<(u32, u32)>) -> Spec {
        Spec { target, transform: Transform::default(), background: 0, thumbnail: false }
    }

    fn rgb(pixel: u32) -> (u32, u32, u32) {
        (pixel >> 16 & 0xFF, pixel >> 8 & 0xFF, pixel & 0xFF)
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        // Left half red, right half blue.
        let image = RgbImage::from_fn(width, height, |x, _| {
            Rgb(if x < width / 2 { [255, 0, 0] } else { [0, 0, 255] })
        });
        let mut bytes = Vec::new();
        image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg).unwrap();
        bytes
    }

    /// A TIFF header whose only tag is Orientation = 6 (rotate 90 clockwise),
    /// then noise containing a stray JPEG marker, then the given previews.
    fn fake_raw(previews: &[Vec<u8>]) -> Vec<u8> {
        let mut raw = b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00".to_vec();
        raw.extend_from_slice(b"\x00\xFF\xD8\xFF\x00noise");
        for preview in previews {
            raw.extend_from_slice(preview);
            raw.extend_from_slice(&[0xAB; 64]);
        }
        raw
    }

    #[test]
    fn raw_preview_is_found_and_rotated() {
        let raw = fake_raw(&[jpeg(400, 200)]);
        let (image, orientation) = decode_raw_preview(&raw, None).unwrap();
        assert_eq!(orientation, Orientation::Rotate90);
        let frame = finish(image, orientation, spec(Some((100, 100)))).unwrap();
        assert_eq!((frame.width, frame.height, frame.native), (50, 100, (200, 400)));

        let at = |x: usize, y: usize| rgb(frame.pixels[y * 50 + x]);
        let (r, _, b) = at(25, 10);
        assert!(r > 200 && b < 50, "top should be red");
        let (r, _, b) = at(25, 90);
        assert!(r < 50 && b > 200, "bottom should be blue");
    }

    #[test]
    fn raw_preview_choice_depends_on_target() {
        let raw = fake_raw(&[jpeg(160, 120), jpeg(800, 600)]);
        let width = |target| decode_raw_preview(&raw, target).unwrap().0.width();
        assert_eq!(width(None), 800);
        assert_eq!(width(Some((100, 100))), 160);
        assert_eq!(width(Some((400, 400))), 800);
        assert_eq!(width(Some((4000, 4000))), 800);
    }

    #[test]
    fn user_transform_is_applied_after_fitting() {
        let image = decode(&jpeg(400, 200)).unwrap().0;
        let spec = Spec { transform: Transform::CW, ..spec(Some((100, 100))) };
        let frame = finish(image, Orientation::NoTransforms, spec).unwrap();
        assert_eq!((frame.width, frame.height, frame.native), (50, 100, (200, 400)));
    }

    #[test]
    fn transparency_is_composited_over_background() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 0])));
        let frame = finish(image, Orientation::NoTransforms, Spec { background: 0x123456, ..spec(None) }).unwrap();
        assert!(frame.pixels.iter().all(|&p| p == 0x123456));
    }
}
