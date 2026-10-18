use std::error::Error;
use std::io::Cursor;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use fast_image_resize as fir;
use rayon::prelude::*;
use image::metadata::Orientation;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageReader, RgbImage, RgbaImage};
use memchr::memmem;

use crate::files;
use crate::heif;
use crate::render;
use crate::ultrahdr::{self, GainMap, Hdr};
use crate::story;
use crate::frame::{fit_size, Animation, Frame, Transform};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// How a file should be turned into a frame.
#[derive(Clone, Copy, PartialEq)]
pub struct Spec {
    /// Box to shrink the image into, or `None` for full resolution.
    pub target: Option<(u32, u32)>,
    /// Whether an image smaller than the box is scaled up to fill it.
    pub enlarge: bool,
    /// Rotation or mirroring the user asked for, on top of the EXIF one.
    pub transform: Transform,
    /// Colour shown through transparent pixels, as `0RGB`.
    pub background: u32,
    /// Whether this is for a thumbnail, which for a video should be a
    /// telling frame rather than the one playback opens on.
    pub thumbnail: bool,
    /// Which rendition of an Ultra HDR picture to show; for any other
    /// picture, and for a thumbnail, it makes no difference.
    pub hdr: Hdr,
}

/// A decoded file at full resolution, flattened to RGB but not yet turned
/// the right way up. Frames of any size and rotation are made from it.
pub struct Source {
    pub image: RgbImage,
    /// How the file says the pixels should be turned to be upright.
    pub orientation: Transform,
    /// The rest of an animated image; `None` for a still one.
    pub animation: Option<SourceAnimation>,
    /// The gain map of an Ultra HDR picture, kept beside the ordinary one
    /// so that either rendition can be made from what is decoded.
    pub gain: Option<GainMap>,
    /// Whether the file was looked at for a gain map at all.
    pub looked: bool,
    /// What the file's first bytes say it is, and whether it is an Ultra
    /// HDR photo: for the badges beside its name, found here rather than
    /// by the thread drawing the window reading the file.
    pub format: Option<&'static str>,
    pub ultra_hdr: bool,
}

/// The pictures of an animation after the first, and how long every
/// picture shows, the first included.
pub struct SourceAnimation {
    pub frames: Vec<RgbImage>,
    pub delays: Vec<Duration>,
}

/// Decodes a file. `spec` says what the result is for, which picks the
/// still of a video and the preview of a RAW file.
pub fn read(path: &Path, spec: Spec) -> Result<Source> {
    // A thumbnail's file is not the one whose badges are shown.
    let (format, ultra_hdr) = match spec.thumbnail {
        true => (None, false),
        false => (files::sniff_file(path), files::is_jpeg(path) && ultrahdr::is_ultra_hdr(&head_of(path))),
    };
    let Decoded { image, orientation, frames, delays } = decode_file(path, spec.target, spec.thumbnail)?;
    let animation = (!frames.is_empty()).then(|| SourceAnimation {
        frames: frames.into_iter().map(|frame| flatten(DynamicImage::ImageRgba8(frame), spec.background)).collect(),
        delays,
    });
    // An Ultra HDR file's gain map, only once another rendition than the
    // ordinary one is asked for: reading it costs some milliseconds for
    // each such photo, which the ordinary picture has no use for. A
    // thumbnail is always the ordinary rendition.
    let looked = !spec.thumbnail && spec.hdr != Hdr::Sdr && animation.is_none() && files::is_jpeg(path);
    let gain = match looked {
        true => std::fs::read(path).ok().and_then(|bytes| ultrahdr::gain_map(&bytes)),
        false => None,
    };
    Ok(Source { image: flatten(image, spec.background), orientation: transform_for(orientation), animation, gain, looked, format, ultra_hdr })
}

/// The first part of a file, as much as the Ultra HDR marking is looked
/// for in: it comes before the picture.
fn head_of(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut head = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(256 << 10).read_to_end(&mut head);
    }
    head
}

/// A decoded file: its picture, or for an animation its first picture, the
/// rest, and how long each shows (the first included).
struct Decoded {
    image: DynamicImage,
    orientation: Orientation,
    frames: Vec<RgbaImage>,
    delays: Vec<Duration>,
}

impl Decoded {
    fn still((image, orientation): (DynamicImage, Orientation)) -> Decoded {
        Decoded { image, orientation, frames: Vec::new(), delays: Vec::new() }
    }
}

/// Memory an animation may take at full resolution before the rest of its
/// frames are left out; it then loops over the frames that fit.
const ANIMATION_BUDGET: usize = 1 << 30;
/// Frames that ask for less than this show for this long, as browsers do.
const MIN_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

/// Decodes whatever kind of file `path` is. `target` picks the preview of
/// a RAW file and `thumbnail` the still of a video.
fn decode_file(path: &Path, target: Option<(u32, u32)>, thumbnail: bool) -> Result<Decoded> {
    if files::is_video(path) {
        return decode(&video_still(path, thumbnail)?).map(Decoded::still);
    }
    if files::needs_ffmpeg(path) {
        return decode(&ffmpeg_image(path)?).map(Decoded::still);
    }
    let bytes = std::fs::read(path)?;
    if files::is_heif(path) {
        // An ffmpeg new enough reads these too, where libheif is missing.
        let image = heif::decode(&bytes).map(|image| (image, Orientation::NoTransforms));
        return image.or_else(|error| ffmpeg_image(path).and_then(|bmp| decode(&bmp)).map_err(|_| error)).map(Decoded::still);
    }
    if files::is_jxl(path) {
        let mut decoder = jxl_oxide::integration::JxlDecoder::new(Cursor::new(&bytes[..]))?;
        let orientation = decoder.orientation()?;
        return Ok(Decoded::still((DynamicImage::from_decoder(decoder)?, orientation)));
    }
    if files::is_raw(path) {
        return decode_raw_preview(&bytes, target).map(Decoded::still);
    }
    // TGA has no signature and ICO's is ambiguous, so the content is not
    // always enough to tell the format; the extension settles it.
    let mut reader = ImageReader::new(Cursor::new(&bytes[..])).with_guessed_format()?;
    if reader.format().is_none() {
        reader.set_format(image::ImageFormat::from_path(path)?);
    }
    use image::ImageFormat::{Gif, Png, WebP};
    let frames: Option<image::Frames> = match reader.format() {
        Some(Gif) => Some(image::codecs::gif::GifDecoder::new(Cursor::new(&bytes[..]))?.into_frames()),
        Some(Png) => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&bytes[..]))?;
            decoder.is_apng()?.then(|| decoder.apng()).transpose()?.map(AnimationDecoder::into_frames)
        }
        Some(WebP) => {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(&bytes[..]))?;
            decoder.has_animation().then(|| decoder.into_frames())
        }
        _ => None,
    };
    if let Some(frames) = frames {
        if let Some(decoded) = decode_animation(frames)? {
            return Ok(decoded);
        }
    }
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    Ok(Decoded::still((DynamicImage::from_decoder(decoder)?, orientation)))
}

/// Reads the frames of an animation, or `None` when there is only one, in
/// which case the plain decoder does a better job of a still image.
fn decode_animation(frames: image::Frames) -> Result<Option<Decoded>> {
    let mut pictures = Vec::new();
    let mut delays = Vec::new();
    let mut bytes = 0;
    for frame in frames {
        let frame = frame?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay = Duration::from_millis(u64::from(numerator) / u64::from(denominator.max(1)));
        delays.push(if delay < MIN_DELAY { DEFAULT_DELAY } else { delay });
        let picture = frame.into_buffer();
        bytes += picture.as_raw().len();
        if bytes > ANIMATION_BUDGET && !pictures.is_empty() {
            delays.pop();
            break;
        }
        pictures.push(picture);
    }
    if pictures.len() < 2 {
        return Ok(None);
    }
    let first = DynamicImage::ImageRgba8(pictures.remove(0));
    Ok(Some(Decoded { image: first, orientation: Orientation::NoTransforms, frames: pictures, delays }))
}

/// The transform that turns pixels stored with an EXIF orientation upright.
fn transform_for(orientation: Orientation) -> Transform {
    use Orientation::*;
    match orientation {
        NoTransforms => Transform::default(),
        Rotate90 => Transform::CW,
        Rotate180 => Transform::CW.then(Transform::CW),
        Rotate270 => Transform::CCW,
        FlipHorizontal => Transform::MIRROR,
        FlipVertical => Transform::FLIP,
        // A mirror after a turn is the opposite turn after a mirror.
        Rotate90FlipH => Transform::MIRROR.then(Transform::CCW),
        Rotate270FlipH => Transform::MIRROR.then(Transform::CW),
    }
}

/// A still from a video, as an encoded image. A thumbnail is taken one
/// second in, where there is usually more to see than in the opening frame.
/// Otherwise it is the opening frame, so that the still shown while the
/// player starts is the picture playback begins on.
fn video_still(path: &Path, thumbnail: bool) -> Result<Vec<u8>> {
    let seeks: &[&str] = if thumbnail { &["1", "0"] } else { &["0"] };
    // One frame needs no decoding threads, and a quick look at the start
    // of the file is enough to find the video stream in nearly every
    // file; the thorough look is the fallback.
    let quick = ["-threads", "1", "-probesize", "32k", "-analyzeduration", "0"];
    for options in [&quick[..], &[]] {
        for seek in seeks {
            let output = Command::new("ffmpeg")
                .args(["-v", "error"])
                .args(options)
                .args(["-ss", seek, "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "bmp", "-"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .map_err(|error| format!("cannot run ffmpeg for a video still: {error}"))?;
            // Nothing comes out when the video is shorter than the seek.
            if !output.stdout.is_empty() {
                // A thumbnail of the black a video fades in from says
                // nothing about it; one from further in does.
                let still = output.stdout;
                let further = if thumbnail && flat_bmp(&still) { later_still(path) } else { None };
                return Ok(further.unwrap_or(still));
            }
        }
    }
    Err("ffmpeg could not read a frame from this video".into())
}

/// The first frame that is not one flat colour among those a twentieth, a
/// sixth, a third and half of the way through a video.
fn later_still(path: &Path) -> Option<Vec<u8>> {
    let length = story::length(path)?;
    [0.05, 0.15, 0.33, 0.5].into_iter().find_map(|part| {
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-threads", "1", "-noaccurate_seek", "-ss", &format!("{:.3}", length * part), "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "bmp", "-"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        (!output.stdout.is_empty() && !flat_bmp(&output.stdout)).then_some(output.stdout)
    })
}

/// Whether an uncompressed BMP, as ffmpeg writes it, is all but one
/// colour. Reads a scattering of pixels straight out of the file.
fn flat_bmp(bytes: &[u8]) -> bool {
    let field = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let (Some(start), Some(depth)) = (field(10), field(28).map(|depth| (depth & 0xFFFF) / 8)) else { return false };
    let Some(data) = bytes.get(start..).filter(|_| depth >= 3) else { return false };
    let step = (data.len() / depth / 2000).max(1) * depth;
    let pixels: Vec<u32> = (data.chunks_exact(depth).step_by(step / depth))
        .map(|p| u32::from(p[2]) << 16 | u32::from(p[1]) << 8 | u32::from(p[0]))
        .collect();
    story::is_flat(&pixels)
}

/// An image in a format ffmpeg decodes and the built-in decoders do not,
/// as a BMP. Vector images are rasterised at their own size.
fn ffmpeg_image(path: &Path) -> Result<Vec<u8>> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "1", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "bmp", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run ffmpeg to decode this image: {error}"))?;
    if output.stdout.is_empty() {
        return Err("ffmpeg could not decode this image (is a decoder for it built in?)".into());
    }
    Ok(output.stdout)
}

/// The image at full resolution, upright, with any transparency intact.
/// `open`, with a decoder that panics on a broken file making an error of
/// it rather than taking the thread down.
pub fn open_guarded(path: &Path) -> Result<DynamicImage> {
    std::panic::catch_unwind(|| open(path)).unwrap_or_else(|_| Err("the decoder crashed on it".into()))
}

pub fn open(path: &Path) -> Result<DynamicImage> {
    let Decoded { mut image, orientation, .. } = decode_file(path, None, false)?;
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

/// Makes the frame `spec` asks for from a decoded file: shrunk to fit the
/// target, then turned upright and the way the user wants it.
pub fn finish(source: &Source, spec: Spec) -> Result<Frame> {
    let (w, h) = source.image.dimensions();
    let transform = source.orientation.then(spec.transform);
    let size = match spec.target {
        Some(target) => {
            // Turning is cheapest after scaling, so scale into the pre-rotation box.
            let bounds = if transform.swaps_axes() { (target.1, target.0) } else { target };
            // An animation is not enlarged: every picture of it would be
            // the size of the screen, which for a long one is gigabytes.
            fit_size((w, h), bounds, spec.enlarge && source.animation.is_none())
        }
        None => (w, h),
    };
    let fit = |image: &RgbImage| -> Result<Vec<u32>> {
        if size == (w, h) {
            return Ok(to_pixels(image));
        }
        // Enlarging is quicker done on the pixels in their final layout,
        // there being fewer of them to lay out first; shrinking, the
        // other way about.
        if size.0 > w || size.1 > h {
            return enlarge(&to_pixels(image), (w, h), size);
        }
        Ok(to_pixels(&resize(image, size)?))
    };
    let animation = match &source.animation {
        Some(animation) => Some(Animation {
            frames: animation.frames.iter().map(|frame| fit(frame)).collect::<Result<_>>()?,
            delays: animation.delays.clone(),
        }),
        None => None,
    };
    // An Ultra HDR picture in another rendition than the ordinary one is
    // made at the size it is shown, from the ordinary picture and the gain
    // map both brought to that size: far fewer pixels than the file's.
    let pixels = match (&source.gain, spec.hdr) {
        (Some(gain), Hdr::Full) => {
            let sdr = if size == (w, h) { source.image.clone() } else { resize(&source.image, size)? };
            let map = if gain.image.dimensions() == size { gain.image.clone() } else { resize(&gain.image, size)? };
            to_pixels(&ultrahdr::compose(&sdr, &map, &gain.meta))
        }
        (Some(gain), Hdr::GainMap) => to_pixels(&if gain.image.dimensions() == size { gain.image.clone() } else { resize(&gain.image, size)? }),
        _ => fit(&source.image)?,
    };
    let frame = Frame { width: size.0, height: size.1, native: (w, h), pixels, animation };
    Ok(if transform.is_identity() { frame } else { frame.transformed(transform) })
}

/// A smaller copy of a frame fitting `bounds`, for a thumbnail made from a
/// frame that is already decoded. Only the first picture of an animation.
pub fn shrink(frame: &Frame, bounds: (u32, u32)) -> Result<Frame> {
    let (width, height) = fit_size((frame.width, frame.height), bounds, false);
    if (width, height) == (frame.width, frame.height) {
        return Ok(Frame::still(width, height, frame.native, frame.pixels.clone()));
    }
    let source =
        fir::images::ImageRef::new(frame.width, frame.height, bytemuck::cast_slice(&frame.pixels), fir::PixelType::U8x4)?;
    let mut pixels = vec![0u32; width as usize * height as usize];
    let mut target =
        fir::images::Image::from_slice_u8(width, height, bytemuck::cast_slice_mut(&mut pixels), fir::PixelType::U8x4)?;
    let options = fir::ResizeOptions::new()
        .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3))
        // The fourth byte is padding, not alpha.
        .use_alpha(false);
    render::with_resizer(|resizer| resizer.resize(&source, &mut target, &options))?;
    Ok(Frame::still(width, height, frame.native, pixels))
}

/// One RGBA pixel composited over an opaque colour.
fn blend_over(px: &[u8], bg: [u8; 3]) -> [u8; 3] {
    let alpha = u32::from(px[3]);
    let mix = |c: u8, back: u8| {
        let blended = u32::from(c) * alpha + u32::from(back) * (255 - alpha) + 128;
        // Rounds `blended / 255` exactly for every value it can take.
        ((blended + (blended >> 8)) >> 8) as u8
    };
    [mix(px[0], bg[0]), mix(px[1], bg[1]), mix(px[2], bg[2])]
}

/// Converts to 8-bit RGB, compositing any transparency over `background`.
/// Images with an alpha channel are very often opaque throughout, in which
/// case the channel is simply dropped.
pub fn flatten(image: DynamicImage, background: u32) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    let raw = rgba.as_raw();
    let mut rgb = vec![0u8; raw.len() / 4 * 3];
    let pairs = raw.chunks_exact(4).zip(rgb.chunks_exact_mut(3));
    if raw.chunks_exact(4).all(|px| px[3] == 255) {
        pairs.for_each(|(px, out)| out.copy_from_slice(&px[..3]));
    } else {
        let bg = [(background >> 16) as u8, (background >> 8) as u8, background as u8];
        pairs.for_each(|(px, out)| out.copy_from_slice(&blend_over(px, bg)));
    }
    RgbImage::from_raw(rgba.width(), rgba.height(), rgb).expect("buffer matches dimensions")
}

/// The pixels of an RGB image in the `0RGB` layout, a row per core.
fn to_pixels(rgb: &RgbImage) -> Vec<u32> {
    let width = rgb.width().max(1) as usize;
    let mut pixels = vec![0u32; rgb.as_raw().len() / 3];
    pixels.par_chunks_mut(width).zip(rgb.as_raw().par_chunks(width * 3)).for_each(|(out, row)| {
        for (pixel, p) in out.iter_mut().zip(row.chunks_exact(3)) {
            *pixel = u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]);
        }
    });
    pixels
}

/// Shrinks an image with the sharper filter.
fn resize(src: &RgbImage, (width, height): (u32, u32)) -> Result<RgbImage> {
    let view = fir::images::ImageRef::new(src.width(), src.height(), src.as_raw(), fir::PixelType::U8x3)?;
    let mut dst = fir::images::Image::new(width, height, fir::PixelType::U8x3);
    let options = fir::ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3));
    render::with_resizer(|resizer| resizer.resize(&view, &mut dst, &options))?;
    Ok(RgbImage::from_raw(width, height, dst.into_vec()).expect("buffer matches dimensions"))
}

/// Enlarges `0RGB` pixels, with the filter that does not ring around hard
/// edges: what showing a small image larger used to do on the way to the
/// screen, and so looks the same.
fn enlarge(pixels: &[u32], (w, h): (u32, u32), (width, height): (u32, u32)) -> Result<Vec<u32>> {
    let source = fir::images::ImageRef::new(w, h, bytemuck::cast_slice(pixels), fir::PixelType::U8x4)?;
    let mut out = vec![0u32; width as usize * height as usize];
    let mut target = fir::images::Image::from_slice_u8(width, height, bytemuck::cast_slice_mut(&mut out), fir::PixelType::U8x4)?;
    let options = fir::ResizeOptions::new()
        .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom))
        // The fourth byte is padding, not alpha.
        .use_alpha(false);
    render::with_resizer(|resizer| resizer.resize(&source, &mut target, &options))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb};

    fn spec(target: Option<(u32, u32)>) -> Spec {
        Spec { target, enlarge: false, transform: Transform::default(), background: 0, thumbnail: false, hdr: Hdr::Sdr }
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
        let frame = finish(&source(image, orientation, 0), spec(Some((100, 100)))).unwrap();
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
        let frame = finish(&source(image, Orientation::NoTransforms, 0), spec).unwrap();
        assert_eq!((frame.width, frame.height, frame.native), (50, 100, (200, 400)));
    }

    #[test]
    fn blending_rounds_like_a_division() {
        for alpha in 0..=255u32 {
            for c in (0..=255u8).step_by(5) {
                for back in (0..=255u8).step_by(7) {
                    let exact = (f64::from(c) * f64::from(alpha) + f64::from(back) * f64::from(255 - alpha)) / 255.0;
                    let got = blend_over(&[c, c, c, alpha as u8], [back; 3])[0];
                    assert_eq!(got, exact.round() as u8, "c={c} alpha={alpha} back={back}");
                }
            }
        }
    }

    #[test]
    fn transparency_is_composited_over_background() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 0])));
        let frame = finish(&source(image, Orientation::NoTransforms, 0x123456), spec(None)).unwrap();
        assert!(frame.pixels.iter().all(|&p| p == 0x123456));
    }

    #[test]
    fn exif_orientations_match_the_image_crate() {
        // 3 x 2, every pixel different, so any wrong turn or mirror shows.
        let image = RgbImage::from_fn(3, 2, |x, y| Rgb([x as u8, y as u8, 7]));
        for exif in 1..=8 {
            let orientation = Orientation::from_exif(exif).unwrap();
            let mut expected = DynamicImage::ImageRgb8(image.clone());
            expected.apply_orientation(orientation);
            let expected: Vec<u32> = expected.into_rgb8().pixels().map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])).collect();
            let got = finish(&source(DynamicImage::ImageRgb8(image.clone()), orientation, 0), spec(None)).unwrap();
            assert_eq!(got.pixels, expected, "orientation {exif}");
        }
    }

    #[test]
    fn shrinking_a_frame_keeps_its_native_size() {
        let frame = Frame::still(400, 200, (4000, 2000), vec![0x808080; 80_000]);
        let small = shrink(&frame, (100, 100)).unwrap();
        assert_eq!((small.width, small.height, small.native), (100, 50, (4000, 2000)));
        assert!(small.pixels.iter().all(|&p| p == 0x808080));
    }

    fn source(image: DynamicImage, orientation: Orientation, background: u32) -> Source {
        Source { image: flatten(image, background), orientation: transform_for(orientation), animation: None, gain: None, looked: false, format: None, ultra_hdr: false }
    }
}
