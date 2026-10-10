use std::fs::OpenOptions;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::DynamicImage;

use crate::decode::{self, Result};

const JPEG_QUALITY: u8 = 90;
/// JPEG has no transparency, so transparent areas become this colour.
const JPEG_BACKGROUND: u32 = 0xFFFFFF;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Format {
    Jpeg,
    Png,
    /// Lossless; that is the only WebP flavour the encoder writes.
    WebP,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::WebP => "WebP",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::WebP => "webp",
        }
    }

    fn of(path: &Path) -> Option<Format> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "jpg" | "jpeg" | "jpe" | "jfif" => Some(Format::Jpeg),
            "png" => Some(Format::Png),
            "webp" => Some(Format::WebP),
            _ => None,
        }
    }
}

/// Writes a copy of the image in another format next to the original and
/// returns its path. An existing file is never overwritten.
pub fn convert(path: &Path, format: Format) -> Result<PathBuf> {
    if Format::of(path) == Some(format) {
        return Err(format!("already {}", format.name()).into());
    }
    let target = path.with_extension(format.extension());
    let image = decode::open(path)?;
    // Creating the file fails if the name is taken, so nothing is clobbered.
    let file = OpenOptions::new().write(true).create_new(true).open(&target)?;
    let result = encode(image, format, BufWriter::new(file));
    if result.is_err() {
        let _ = std::fs::remove_file(&target);
    }
    result.map(|()| target)
}

fn encode(image: DynamicImage, format: Format, writer: BufWriter<std::fs::File>) -> Result<()> {
    match format {
        Format::Jpeg => {
            let rgb = decode::flatten(image, JPEG_BACKGROUND);
            JpegEncoder::new_with_quality(writer, JPEG_QUALITY).encode_image(&rgb)?;
        }
        Format::Png => image.write_with_encoder(PngEncoder::new(writer))?,
        Format::WebP => {
            // The encoder only takes 8-bit pixels.
            let image = if image.color().has_alpha() {
                DynamicImage::ImageRgba8(image.into_rgba8())
            } else {
                DynamicImage::ImageRgb8(image.into_rgb8())
            };
            image.write_with_encoder(WebPEncoder::new_lossless(writer))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn converts_between_every_format_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("jog-convert-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("picture.png");
        // Top half opaque red, bottom half fully transparent.
        let image = RgbaImage::from_fn(8, 8, |_, y| Rgba(if y < 4 { [255, 0, 0, 255] } else { [0, 0, 0, 0] }));
        image.save(&source).unwrap();

        let jpeg = convert(&source, Format::Jpeg).unwrap();
        assert_eq!(jpeg, dir.join("picture.jpg"));
        let decoded = image::open(&jpeg).unwrap().into_rgb8();
        assert!(decoded.get_pixel(4, 1)[0] > 200 && decoded.get_pixel(4, 1)[2] < 60);
        assert!(decoded.get_pixel(4, 6).0.iter().all(|&c| c > 200), "transparent becomes white");

        let webp = convert(&source, Format::WebP).unwrap();
        assert_eq!(image::open(&webp).unwrap().into_rgba8(), image, "lossless, alpha kept");
        let png = convert(&webp, Format::Png);
        assert!(png.is_err(), "picture.png already exists");
        assert!(convert(&jpeg, Format::Jpeg).is_err(), "already a JPEG");
        assert_eq!(image::open(&source).unwrap().into_rgba8(), image, "original untouched");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
