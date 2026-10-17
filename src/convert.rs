//! Writing a picture out in another format: JPEG XL, AVIF, JPEG, PNG or
//! WebP, each by an encoder built into jog.
//!
//! The usual settings aim at the same place for every lossy format: a
//! SSIMULACRA2 score of about 85, very good to the eye, at the size each
//! encoder needs for it. They were measured on a 4K photo and a 1024px
//! illustration, and follow Jake Archibald's comparison of JPEG XL and
//! AVIF (2026): AVIF in full colour (4:4:4) at a middling speed, jixel's
//! Medium effort, and a JPEG turned into JPEG XL losslessly rather than
//! encoded again.

use std::fs::OpenOptions;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use image::codecs::png::PngEncoder;
use image::DynamicImage;

use crate::decode::{self, Result};

/// JPEG has no transparency, so transparent areas become this colour.
const JPEG_BACKGROUND: u32 = 0xFFFFFF;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Jxl,
    Avif,
    Jpeg,
    Png,
    WebP,
}

impl Format {
    pub const ALL: [Format; 5] = [Format::Jxl, Format::Avif, Format::Jpeg, Format::Png, Format::WebP];

    pub fn name(self) -> &'static str {
        match self {
            Format::Jxl => "JPEG XL",
            Format::Avif => "AVIF",
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::WebP => "WebP",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Jxl => "jxl",
            Format::Avif => "avif",
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::WebP => "webp",
        }
    }

    /// The format a file is, by its name, of those jog writes.
    pub fn of(path: &Path) -> Option<Format> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "jxl" => Some(Format::Jxl),
            "avif" => Some(Format::Avif),
            "jpg" | "jpeg" | "jpe" | "jfif" => Some(Format::Jpeg),
            "png" => Some(Format::Png),
            "webp" => Some(Format::WebP),
            _ => None,
        }
    }

    /// By the word used for it in the config file or on the command line.
    pub fn parse(word: &str) -> Option<Format> {
        match word.to_ascii_lowercase().as_str() {
            "jxl" | "jpegxl" | "jpeg-xl" => Some(Format::Jxl),
            "avif" => Some(Format::Avif),
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "png" => Some(Format::Png),
            "webp" => Some(Format::WebP),
            _ => None,
        }
    }
}

/// How hard jixel works: slower is smaller, at the same quality.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effort {
    Fastest,
    Fast,
    Medium,
    Slow,
    ExtraSlow,
}

impl Effort {
    pub const ALL: [Effort; 5] = [Effort::Fastest, Effort::Fast, Effort::Medium, Effort::Slow, Effort::ExtraSlow];

    pub fn name(self) -> &'static str {
        match self {
            Effort::Fastest => "fastest",
            Effort::Fast => "fast",
            Effort::Medium => "medium",
            Effort::Slow => "slow",
            Effort::ExtraSlow => "extraslow",
        }
    }

    pub fn parse(word: &str) -> Option<Effort> {
        Effort::ALL.into_iter().find(|effort| effort.name() == word.to_ascii_lowercase())
    }

    fn speed(self) -> jixel::Speed {
        match self {
            Effort::Fastest => jixel::Speed::Fastest,
            Effort::Fast => jixel::Speed::Fast,
            Effort::Medium => jixel::Speed::Medium,
            Effort::Slow => jixel::Speed::Slow,
            Effort::ExtraSlow => jixel::Speed::ExtraSlow,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct JxlSettings {
    /// 0 to 100; 90 is jixel's distance 1.0, the usual "visually lossless".
    pub quality: f32,
    pub effort: Effort,
    /// Every pixel kept exactly, at a much larger size.
    pub lossless: bool,
    /// A JPEG is repacked as it is, nothing lost and the JPEG recoverable
    /// bit for bit, rather than decoded and encoded again.
    pub recompress_jpeg: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AvifSettings {
    /// 0 to 100.
    pub quality: f32,
    /// 1 (slowest, smallest) to 10 (fastest).
    pub speed: u8,
    /// 8 or 10.
    pub depth: u8,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct JpegSettings {
    /// 0 to 100.
    pub quality: f32,
    /// Colour at full resolution (4:4:4) rather than halved (4:2:0).
    pub full_chroma: bool,
    pub progressive: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WebPSettings {
    /// 0 to 100.
    pub quality: f32,
    pub lossless: bool,
    /// 0 (fastest) to 6 (slowest, smallest).
    pub method: u8,
}

/// How each format is written.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    pub jxl: JxlSettings,
    pub avif: AvifSettings,
    pub jpeg: JpegSettings,
    pub webp: WebPSettings,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            jxl: JxlSettings { quality: 90.0, effort: Effort::Medium, lossless: false, recompress_jpeg: true },
            avif: AvifSettings { quality: 88.0, speed: 6, depth: 8 },
            jpeg: JpegSettings { quality: 90.0, full_chroma: true, progressive: true },
            webp: WebPSettings { quality: 90.0, lossless: false, method: 4 },
        }
    }
}

impl Settings {
    /// Takes in the `[convert]` table of the config file:
    ///
    /// ```toml
    /// [convert]
    /// jxl  = { quality = 90, effort = "medium", lossless = false, recompress-jpeg = true }
    /// avif = { quality = 88, speed = 6, depth = 8 }
    /// jpeg = { quality = 90, chroma = "444", progressive = true }
    /// webp = { quality = 90, lossless = false, method = 4 }
    /// ```
    pub fn apply(&mut self, table: &toml::Table) -> std::result::Result<(), String> {
        for (format, value) in table {
            let fields = value.as_table().ok_or_else(|| format!("convert.{format}: expected a table"))?;
            for (key, value) in fields {
                let wrong = || format!("convert.{format}.{key}: not a usable value");
                let number = || value.as_float().or_else(|| value.as_integer().map(|n| n as f64)).ok_or_else(wrong);
                let quality = || number().map(|n| n.clamp(0.0, 100.0) as f32);
                let flag = || value.as_bool().ok_or_else(wrong);
                match (format.as_str(), key.replace('_', "-").as_str()) {
                    ("jxl", "quality") => self.jxl.quality = quality()?,
                    ("jxl", "effort") => self.jxl.effort = value.as_str().and_then(Effort::parse).ok_or_else(wrong)?,
                    ("jxl", "lossless") => self.jxl.lossless = flag()?,
                    ("jxl", "recompress-jpeg") => self.jxl.recompress_jpeg = flag()?,
                    ("avif", "quality") => self.avif.quality = quality()?,
                    ("avif", "speed") => self.avif.speed = number()?.clamp(1.0, 10.0) as u8,
                    ("avif", "depth") => self.avif.depth = if number()? >= 10.0 { 10 } else { 8 },
                    ("jpeg", "quality") => self.jpeg.quality = quality()?,
                    ("jpeg", "chroma") => {
                        let said = value.as_str().map(str::to_string).or(value.as_integer().map(|n| n.to_string()));
                        self.jpeg.full_chroma = match said.as_deref() {
                            Some("444" | "4:4:4") => true,
                            Some("420" | "4:2:0") => false,
                            _ => return Err(wrong()),
                        }
                    }
                    ("jpeg", "progressive") => self.jpeg.progressive = flag()?,
                    ("webp", "quality") => self.webp.quality = quality()?,
                    ("webp", "lossless") => self.webp.lossless = flag()?,
                    ("webp", "method") => self.webp.method = number()?.clamp(0.0, 6.0) as u8,
                    _ => return Err(format!("convert.{format}.{key}: no such setting")),
                }
            }
        }
        Ok(())
    }

    /// The settings a format is written with, in a few words.
    pub fn describe(&self, format: Format) -> String {
        match format {
            Format::Jxl if self.jxl.lossless => format!("lossless, {}", self.jxl.effort.name()),
            Format::Jxl => format!("q{}, {}", self.jxl.quality, self.jxl.effort.name()),
            Format::Avif => format!("q{}, speed {}, {}-bit", self.avif.quality, self.avif.speed, self.avif.depth),
            Format::Jpeg => format!("q{}, {}", self.jpeg.quality, if self.jpeg.full_chroma { "4:4:4" } else { "4:2:0" }),
            Format::Png => "lossless".to_string(),
            Format::WebP if self.webp.lossless => "lossless".to_string(),
            Format::WebP => format!("q{}", self.webp.quality),
        }
    }
}

/// A picture written out: its bytes, and how, when that differs from what
/// the settings say (a JPEG repacked rather than encoded).
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub how: String,
}

/// Encodes `image` as `format`. `source` is the file's own bytes, for a
/// JPEG that JPEG XL can repack as it is.
pub fn encode(image: &DynamicImage, source: Option<&[u8]>, format: Format, settings: &Settings) -> Result<Encoded> {
    // The encoders are C, Rust and Rust; a panic in any of them is a file
    // that could not be written, not jog gone.
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encode_inner(image, source, format, settings)));
    run.unwrap_or_else(|_| Err(format!("the {} encoder failed", format.name()).into()))
}

fn encode_inner(image: &DynamicImage, source: Option<&[u8]>, format: Format, settings: &Settings) -> Result<Encoded> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let alpha = image.color().has_alpha() && image.to_rgba8().pixels().any(|px| px[3] != 255);
    let how = settings.describe(format);
    let bytes = match format {
        Format::Jxl => {
            let jxl = &settings.jxl;
            let jpeg = source.filter(|bytes| bytes.starts_with(b"\xFF\xD8\xFF"));
            if let Some(jpeg) = jpeg.filter(|_| jxl.recompress_jpeg && !jxl.lossless) {
                let config = jixel::JpegTranscodeConfig::default();
                match jixel::encode_jpeg_lossless_with_config(jpeg, &config) {
                    Ok(bytes) => return Ok(Encoded { bytes, how: "the JPEG repacked, nothing lost".to_string() }),
                    // Some JPEGs cannot be repacked: those are encoded.
                    Err(error) => eprintln!("jog: cannot repack the JPEG ({error:?}), encoding it instead"),
                }
            }
            let mut config = jixel::EncodeConfig::default().with_speed(jxl.effort.speed());
            config = if jxl.lossless { config.with_lossless(true) } else { config.with_quality(jxl.quality) };
            match alpha {
                true => jixel::encode_image_with_alpha(image.to_rgba8().as_raw(), width, height, &config),
                false => jixel::encode_image(image.to_rgb8().as_raw(), width, height, &config),
            }
            .map_err(|error| format!("JPEG XL: {error:?}"))?
        }
        Format::Avif => {
            use rgb::FromSlice;
            let avif = &settings.avif;
            let depth = if avif.depth >= 10 { ravif::BitDepth::Ten } else { ravif::BitDepth::Eight };
            let encoder = ravif::Encoder::new().with_quality(avif.quality).with_alpha_quality(avif.quality).with_speed(avif.speed).with_bit_depth(depth);
            let encoded = match alpha {
                true => {
                    let rgba = image.to_rgba8();
                    encoder.encode_rgba(ravif::Img::new(rgba.as_raw().as_rgba(), width, height))
                }
                false => {
                    let rgb = image.to_rgb8();
                    encoder.encode_rgb(ravif::Img::new(rgb.as_raw().as_rgb(), width, height))
                }
            };
            encoded.map_err(|error| format!("AVIF: {error}"))?.avif_file
        }
        Format::Jpeg => {
            let jpeg = &settings.jpeg;
            let rgb = decode::flatten(image.clone(), JPEG_BACKGROUND);
            let mut compress = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
            compress.set_size(width, height);
            compress.set_quality(jpeg.quality);
            if jpeg.full_chroma {
                compress.set_chroma_sampling_pixel_sizes((1, 1), (1, 1));
            } else {
                compress.set_chroma_sampling_pixel_sizes((2, 2), (2, 2));
            }
            if jpeg.progressive {
                compress.set_progressive_mode();
                compress.set_optimize_scans(true);
            }
            let mut started = compress.start_compress(Vec::new())?;
            started.write_scanlines(rgb.as_raw())?;
            started.finish()?
        }
        Format::Png => {
            let mut bytes = Vec::new();
            image.write_with_encoder(PngEncoder::new(Cursor::new(&mut bytes)))?;
            bytes
        }
        Format::WebP => {
            let webp = &settings.webp;
            let mut config = webp::WebPConfig::new().map_err(|()| "WebP: no encoder settings")?;
            config.quality = webp.quality;
            config.method = i32::from(webp.method);
            config.lossless = i32::from(webp.lossless);
            let (rgba, rgb);
            let encoder = match alpha {
                true => {
                    rgba = image.to_rgba8();
                    webp::Encoder::from_rgba(rgba.as_raw(), width as u32, height as u32)
                }
                false => {
                    rgb = image.to_rgb8();
                    webp::Encoder::from_rgb(rgb.as_raw(), width as u32, height as u32)
                }
            };
            encoder.encode_advanced(&config).map_err(|error| format!("WebP: {error:?}"))?.to_vec()
        }
    };
    Ok(Encoded { bytes, how })
}

/// Writes a copy of the picture at `path` as `format`, next to it, and
/// says where and how. An existing file is never overwritten.
pub fn convert(path: &Path, format: Format, settings: &Settings) -> Result<(PathBuf, String)> {
    if Format::of(path) == Some(format) {
        return Err(format!("already {}", format.name()).into());
    }
    let target = path.with_extension(format.extension());
    if target.exists() {
        return Err(format!("{} is there already", target.display()).into());
    }
    let source = std::fs::read(path)?;
    let image = decode::open(path)?;
    let encoded = encode(&image, Some(&source), format, settings)?;
    // Creating the file fails if the name is taken, so nothing is clobbered.
    let mut file = OpenOptions::new().write(true).create_new(true).open(&target)?;
    if let Err(error) = file.write_all(&encoded.bytes) {
        let _ = std::fs::remove_file(&target);
        return Err(error.into());
    }
    Ok((target, encoded.how))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jog-convert-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Something with detail, so that lossy encoders have work to do.
    fn photo() -> RgbImage {
        RgbImage::from_fn(96, 64, |x, y| Rgb([(x * 2) as u8, (y * 3) as u8, ((x ^ y) * 4) as u8]))
    }

    #[test]
    fn every_format_writes_a_picture_that_reads_back() {
        let dir = scratch("all");
        let source = dir.join("picture.png");
        photo().save(&source).unwrap();
        let settings = Settings::default();
        for format in [Format::Jxl, Format::Avif, Format::Jpeg, Format::WebP] {
            let (target, _) = convert(&source, format, &settings).unwrap();
            assert_eq!(target, dir.join(format!("picture.{}", format.extension())));
            assert!(std::fs::metadata(&target).unwrap().len() > 100, "{format:?} wrote something");
        }
        // Those jog reads with built-in decoders read back at the same size.
        for extension in ["jpg", "webp"] {
            let back = image::open(dir.join(format!("picture.{extension}"))).unwrap();
            assert_eq!((back.width(), back.height()), (96, 64), "{extension}");
        }
        let jxl = std::fs::read(dir.join("picture.jxl")).unwrap();
        let decoder = jxl_oxide::integration::JxlDecoder::new(Cursor::new(&jxl[..])).unwrap();
        let back = DynamicImage::from_decoder(decoder).unwrap();
        assert_eq!((back.width(), back.height()), (96, 64));
        assert!(std::fs::read(dir.join("picture.avif")).unwrap()[4..12] == *b"ftypavif");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_jpeg_becomes_jpeg_xl_without_loss() {
        let dir = scratch("jpeg");
        let source = dir.join("photo.jpg");
        photo().save(&source).unwrap();
        let (target, how) = convert(&source, Format::Jxl, &Settings::default()).unwrap();
        assert!(how.contains("repacked"), "{how}");
        // The JPEG comes back out of it byte for byte.
        let jxl = std::fs::read(&target).unwrap();
        let image = jxl_oxide::JxlImage::builder().read(&jxl[..]).unwrap();
        let mut rebuilt = Vec::new();
        image.reconstruct_jpeg(&mut rebuilt).unwrap();
        assert!(rebuilt == std::fs::read(&source).unwrap(), "the very same JPEG");
        assert!(jxl.len() < rebuilt.len(), "and smaller");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lossless_formats_keep_every_pixel_and_transparency() {
        let dir = scratch("lossless");
        let source = dir.join("picture.png");
        let image = RgbaImage::from_fn(8, 8, |_, y| Rgba(if y < 4 { [255, 0, 0, 255] } else { [0, 0, 0, 0] }));
        image.save(&source).unwrap();
        let mut settings = Settings::default();
        settings.webp.lossless = true;
        let (webp, _) = convert(&source, Format::WebP, &settings).unwrap();
        assert_eq!(image::open(&webp).unwrap().into_rgba8(), image, "lossless, alpha kept");
        // JPEG has no transparency: it goes white.
        let (jpeg, _) = convert(&source, Format::Jpeg, &settings).unwrap();
        let decoded = image::open(&jpeg).unwrap().into_rgb8();
        assert!(decoded.get_pixel(4, 1)[0] > 200 && decoded.get_pixel(4, 1)[2] < 60);
        assert!(decoded.get_pixel(4, 6).0.iter().all(|&c| c > 200), "transparent becomes white");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nothing_is_overwritten_or_converted_to_itself() {
        let dir = scratch("clobber");
        let source = dir.join("picture.png");
        photo().save(&source).unwrap();
        let settings = Settings::default();
        assert!(convert(&source, Format::Png, &settings).is_err(), "already a PNG");
        std::fs::write(dir.join("picture.jpg"), b"mine").unwrap();
        assert!(convert(&source, Format::Jpeg, &settings).is_err());
        assert_eq!(std::fs::read(dir.join("picture.jpg")).unwrap(), b"mine");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_come_from_the_config_file() {
        let mut settings = Settings::default();
        let table: toml::Table = "jxl = { quality = 80, effort = \"slow\" }\navif = { speed = 4, depth = 10 }\njpeg = { chroma = \"420\" }\nwebp = { lossless = true }".parse().unwrap();
        settings.apply(&table).unwrap();
        assert_eq!((settings.jxl.quality, settings.jxl.effort), (80.0, Effort::Slow));
        assert_eq!((settings.avif.speed, settings.avif.depth), (4, 10));
        assert!(!settings.jpeg.full_chroma && settings.webp.lossless);
        let bad: toml::Table = "jxl = { colour = 3 }".parse().unwrap();
        assert!(settings.apply(&bad).is_err());
    }
}
