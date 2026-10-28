//! Writing a picture out in another format: JPEG XL, AVIF, JPEG, PNG or
//! WebP, each by an encoder built into jog.
//!
//! The usual settings aim at the same place for every lossy format: a
//! SSIMULACRA2 score of about 85, very good to the eye, at the size each
//! encoder needs for it. They were measured on a 4K photo and a 1024px
//! illustration, and follow Jake Archibald's comparison of JPEG XL and
//! AVIF (2026): AVIF in full color (4:4:4) at a middling speed, jixel's
//! Medium effort, and a JPEG turned into JPEG XL losslessly rather than
//! encoded again.

use std::fs::OpenOptions;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use image::codecs::png::PngEncoder;
use image::DynamicImage;

use crate::decode::{self, Result};
use crate::frame::Transform;

/// JPEG has no transparency, so transparent areas become this color.
const JPEG_BACKGROUND: u32 = 0xFFFFFF;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
    /// Color at full resolution (4:4:4) rather than halved (4:2:0).
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
    /// How videos are converted, by ffmpeg.
    pub video: crate::transcode::VideoSettings,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            jxl: JxlSettings { quality: 90.0, effort: Effort::Medium, lossless: false, recompress_jpeg: true },
            avif: AvifSettings { quality: 88.0, speed: 6, depth: 8 },
            jpeg: JpegSettings { quality: 90.0, full_chroma: true, progressive: true },
            webp: WebPSettings { quality: 90.0, lossless: false, method: 4 },
            video: crate::transcode::VideoSettings::default(),
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
            if format == "video" {
                self.video.apply(fields)?;
                continue;
            }
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

    /// Sets the quality a format is written at; lossless formats have none.
    pub fn set_quality(&mut self, format: Format, quality: f32) {
        match format {
            Format::Jxl => self.jxl.quality = quality,
            Format::Avif => self.avif.quality = quality,
            Format::Jpeg => self.jpeg.quality = quality,
            Format::WebP => self.webp.quality = quality,
            Format::Png => {}
        }
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
pub fn convert(path: &Path, format: Format, settings: &Settings, transform: Transform) -> Result<(PathBuf, String)> {
    if Format::of(path) == Some(format) {
        return Err(format!("already {}", format.name()).into());
    }
    let target = path.with_extension(format.extension());
    if target.exists() {
        return Err(format!("{} is there already", target.display()).into());
    }
    let source = std::fs::read(path)?;
    let image = turned(decode::open(path)?, transform);
    // A JPEG turned cannot be repacked as it is: it is encoded.
    let source = transform.is_identity().then_some(&source[..]);
    let mut encoded = encode(&image, source, format, settings)?;
    if !transform.is_identity() {
        encoded.how.push_str(", turned as shown");
    }
    // Creating the file fails if the name is taken, so nothing is clobbered.
    let mut file = OpenOptions::new().write(true).create_new(true).open(&target)?;
    if let Err(error) = file.write_all(&encoded.bytes) {
        let _ = std::fs::remove_file(&target);
        return Err(error.into());
    }
    Ok((target, encoded.how))
}

/// A picture encoded to see what the settings make of it, as the Convert
/// panel shows: how large, how long it took, how it looks decoded again,
/// and how near the original that is.
pub struct Trial {
    pub size: usize,
    pub how: String,
    pub took: std::time::Duration,
    /// Decoded again over white, as `flat` is; `None` when jog has no
    /// decoder for the format (AVIF without libheif).
    pub decoded: Option<image::RgbImage>,
    /// SSIMULACRA2: 90 and over is visually lossless, 70 good.
    pub score: Option<f64>,
}

/// The original over white, as trials are compared with it.
pub fn flat(image: &DynamicImage) -> image::RgbImage {
    decode::flatten(image.clone(), JPEG_BACKGROUND)
}

/// Encodes `image` as `format`, decodes it again, and scores it against
/// `flat`, the original over white.
#[cfg(test)]
pub fn trial(image: &DynamicImage, flat: &image::RgbImage, source: Option<&[u8]>, format: Format, settings: &Settings) -> Result<Trial> {
    let mut trial = trial_unscored(image, source, format, settings)?;
    trial.score = trial_score(flat, &trial);
    Ok(trial)
}

/// A trial without its score, which on a large picture takes seconds of
/// its own: the panel shows the size first and the score after.
pub fn trial_unscored(image: &DynamicImage, source: Option<&[u8]>, format: Format, settings: &Settings) -> Result<Trial> {
    let started = std::time::Instant::now();
    let encoded = encode(image, source, format, settings)?;
    let took = started.elapsed();
    let decoded = decode_encoded(&encoded.bytes, format).map(|image| decode::flatten(image, JPEG_BACKGROUND));
    Ok(Trial { size: encoded.bytes.len(), how: encoded.how, took, decoded, score: None })
}

/// How near a trial stays to the original. A JPEG repacked loses
/// nothing, and has no score to work out.
pub fn trial_score(flat: &image::RgbImage, trial: &Trial) -> Option<f64> {
    if trial.how.contains("repacked") {
        return None;
    }
    trial.decoded.as_ref().filter(|back| back.dimensions() == flat.dimensions()).and_then(|back| score(flat, back))
}

fn decode_encoded(bytes: &[u8], format: Format) -> Option<DynamicImage> {
    match format {
        Format::Jxl => {
            let decoder = jxl_oxide::integration::JxlDecoder::new(Cursor::new(bytes)).ok()?;
            DynamicImage::from_decoder(decoder).ok()
        }
        Format::Avif => crate::heif::decode(bytes).ok(),
        _ => image::load_from_memory(bytes).ok(),
    }
}

pub fn score(original: &image::RgbImage, distorted: &image::RgbImage) -> Option<f64> {
    use ssimulacra2::{ColorPrimaries, Rgb, TransferCharacteristic};
    let frame = |image: &image::RgbImage| {
        let data = image.pixels().map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]).collect();
        Rgb::new(data, image.width() as usize, image.height() as usize, TransferCharacteristic::SRGB, ColorPrimaries::BT709).ok()
    };
    ssimulacra2::compute_frame_ssimulacra2(frame(original)?, frame(distorted)?).ok()
}

/// Where the settings saved from the Convert panel are kept. They are
/// taken over the config file's, as the latest word.
fn saved_path() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("convert.toml"))
}

impl Settings {
    /// The settings in the form of the config file's `[convert]` table.
    pub fn to_toml(&self) -> String {
        let (jxl, avif, jpeg, webp) = (&self.jxl, &self.avif, &self.jpeg, &self.webp);
        format!(
            "# Saved from the Convert panel; taken over the config file's [convert].\n\
             jxl = {{ quality = {}, effort = \"{}\", lossless = {}, recompress-jpeg = {} }}\n\
             avif = {{ quality = {}, speed = {}, depth = {} }}\n\
             jpeg = {{ quality = {}, chroma = \"{}\", progressive = {} }}\n\
             webp = {{ quality = {}, lossless = {}, method = {} }}\n{}",
            jxl.quality, jxl.effort.name(), jxl.lossless, jxl.recompress_jpeg,
            avif.quality, avif.speed, avif.depth,
            jpeg.quality, if jpeg.full_chroma { "444" } else { "420" }, jpeg.progressive,
            webp.quality, webp.lossless, webp.method,
            self.video.to_toml(),
        )
    }

    /// Takes in the settings saved from the panel, if there are any.
    pub fn load_saved(&mut self) {
        let Some(text) = saved_path().and_then(|path| std::fs::read_to_string(path).ok()) else { return };
        match text.parse::<toml::Table>().map_err(|error| error.to_string()).and_then(|table| self.apply(&table)) {
            Ok(()) => {}
            Err(error) => eprintln!("jog: the saved convert settings: {error}"),
        }
    }

    /// Keeps these as the settings Convert uses from now on.
    pub fn save(&self) -> std::io::Result<PathBuf> {
        let path = saved_path().ok_or_else(|| std::io::Error::other("no folder to keep it in"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, self.to_toml())?;
        Ok(path)
    }
}

/// The picture turned and mirrored as `transform` says: mirrored first,
/// then turned clockwise, as jog shows it.
pub fn turned(image: DynamicImage, transform: Transform) -> DynamicImage {
    let image = if transform.mirrored() { image.fliph() } else { image };
    match transform.turns() {
        1 => image.rotate90(),
        2 => image.rotate180(),
        3 => image.rotate270(),
        _ => image,
    }
}

/// Converts each of `paths` to `format`, one after another, saying how it
/// goes on stdout and stderr; for `jog --convert`. False if any failed.
pub fn convert_all(paths: &[PathBuf], format: Format, settings: &Settings) -> bool {
    // Pictures in the format already are not what is meant, and those
    // converted before are done: run again over a folder, only what is
    // new is converted.
    let paths: Vec<&PathBuf> = paths.iter().filter(|path| Format::of(path) != Some(format)).collect();
    let (todo, done): (Vec<&PathBuf>, Vec<&PathBuf>) = paths.into_iter().partition(|path| !path.with_extension(format.extension()).exists());
    // Two pictures that would make the same file, photo.jpg and photo.png
    // both photo.jxl: which is the original cannot be told, and taking the
    // wrong one makes a copy of a copy. Neither is converted.
    let mut targets: std::collections::HashMap<PathBuf, Vec<&PathBuf>> = std::collections::HashMap::new();
    for path in &todo {
        targets.entry(path.with_extension(format.extension())).or_default().push(path);
    }
    let mut clashes: Vec<_> = targets.into_iter().filter(|(_, sources)| sources.len() > 1).collect();
    clashes.sort();
    for (target, sources) in &clashes {
        let names: Vec<String> = sources.iter().map(|path| path.display().to_string()).collect();
        let all = if names.len() == 2 { "both" } else { "all" };
        eprintln!("jog: {} would {all} become {}: left alone", names.join(" and "), target.display());
    }
    let clashing: Vec<&PathBuf> = clashes.iter().flat_map(|(_, sources)| sources.iter().copied()).collect();
    let todo: Vec<&PathBuf> = todo.into_iter().filter(|path| !clashing.contains(path)).collect();
    let (total, mut failed, mut before, mut after) = (todo.len(), clashing.len(), 0, 0);
    let started = std::time::Instant::now();
    let skipped = if done.is_empty() { String::new() } else { format!(", {} done before", done.len()) };
    eprintln!("jog: converting {total} to {} ({}){skipped}", format.name(), settings.describe(format));
    for (index, path) in todo.iter().enumerate() {
        let count = format!("[{}/{total}]", index + 1);
        match convert(path, format, settings, Transform::default()) {
            Ok((to, how)) => {
                let (from_size, to_size) = (size(path), size(&to));
                (before, after) = (before + from_size, after + to_size);
                let change = (to_size as f64 / from_size.max(1) as f64 - 1.0) * 100.0;
                println!("{count} {} -> {}  {change:+.0}%  ({how})", path.display(), to.display());
            }
            Err(error) => {
                eprintln!("{count} {}: {error}", path.display());
                failed += 1;
            }
        }
    }
    let change = if before > 0 { format!(", {:+.0}%", (after as f64 / before as f64 - 1.0) * 100.0) } else { String::new() };
    let converted = total + clashing.len() - failed;
    match failed {
        0 => eprintln!("jog: {converted} converted in {:.0?}{change}", started.elapsed()),
        _ => eprintln!("jog: {converted} converted, {failed} not, in {:.0?}{change}", started.elapsed()),
    }
    failed == 0
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
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
            let (target, _) = convert(&source, format, &settings, Transform::default()).unwrap();
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
    fn a_picture_is_written_turned_as_it_is_shown() {
        let dir = scratch("turned");
        let source = dir.join("wide.png");
        // Red on the left half, wide.
        RgbImage::from_fn(40, 20, |x, _| if x < 20 { Rgb([255, 0, 0]) } else { Rgb([0, 0, 255]) }).save(&source).unwrap();
        let mut settings = Settings::default();
        settings.webp.lossless = true;
        let (out, how) = convert(&source, Format::WebP, &settings, Transform::CW).unwrap();
        assert!(how.contains("turned"), "{how}");
        let back = image::open(&out).unwrap().into_rgb8();
        assert_eq!(back.dimensions(), (20, 40), "tall now");
        // Turned clockwise, the left half is on top.
        assert_eq!(back.get_pixel(10, 5).0, [255, 0, 0]);
        assert_eq!(back.get_pixel(10, 35).0, [0, 0, 255]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_jpeg_becomes_jpeg_xl_without_loss() {
        let dir = scratch("jpeg");
        let source = dir.join("photo.jpg");
        photo().save(&source).unwrap();
        let (target, how) = convert(&source, Format::Jxl, &Settings::default(), Transform::default()).unwrap();
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
        let (webp, _) = convert(&source, Format::WebP, &settings, Transform::default()).unwrap();
        assert_eq!(image::open(&webp).unwrap().into_rgba8(), image, "lossless, alpha kept");
        // JPEG has no transparency: it goes white.
        let (jpeg, _) = convert(&source, Format::Jpeg, &settings, Transform::default()).unwrap();
        let decoded = image::open(&jpeg).unwrap().into_rgb8();
        assert!(decoded.get_pixel(4, 1)[0] > 200 && decoded.get_pixel(4, 1)[2] < 60);
        assert!(decoded.get_pixel(4, 6).0.iter().all(|&c| c > 200), "transparent becomes white");
        std::fs::remove_dir_all(&dir).unwrap();
    }


    /// A real screenshot, where an encoder's harder paths are taken that
    /// small made-up pictures never reach: lossless JPEG XL at each effort
    /// comes back to the pixel. jixel 0.4 wrote files at medium effort and
    /// slower that could not be read back.
    #[test]
    fn a_real_screenshot_comes_back_to_the_pixel_as_lossless_jpeg_xl() {
        let png = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/screenshot.png")).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        let original = image.to_rgba8();
        for effort in [Effort::Fast, Effort::Medium, Effort::Slow] {
            let mut settings = Settings::default();
            (settings.jxl.lossless, settings.jxl.effort) = (true, effort);
            let encoded = encode(&image, None, Format::Jxl, &settings).unwrap();
            let back = decode_encoded(&encoded.bytes, Format::Jxl).unwrap_or_else(|| panic!("{effort:?}: not readable"));
            assert!(back.to_rgba8() == original, "{effort:?}: pixels changed");
        }
    }
    #[test]
    fn nothing_is_overwritten_or_converted_to_itself() {
        let dir = scratch("clobber");
        let source = dir.join("picture.png");
        photo().save(&source).unwrap();
        let settings = Settings::default();
        assert!(convert(&source, Format::Png, &settings, Transform::default()).is_err(), "already a PNG");
        std::fs::write(dir.join("picture.jpg"), b"mine").unwrap();
        assert!(convert(&source, Format::Jpeg, &settings, Transform::default()).is_err());
        assert_eq!(std::fs::read(dir.join("picture.jpg")).unwrap(), b"mine");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_converted_again_converts_only_what_is_new() {
        let dir = scratch("again");
        photo().save(dir.join("a.png")).unwrap();
        let settings = Settings::default();
        let list = |dir: &Path| -> Vec<PathBuf> {
            let mut all: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
            all.sort();
            all
        };
        assert!(convert_all(&list(&dir), Format::WebP, &settings));
        let first = std::fs::metadata(dir.join("a.webp")).unwrap().modified().unwrap();
        photo().save(dir.join("b.png")).unwrap();
        assert!(convert_all(&list(&dir), Format::WebP, &settings), "nothing failed");
        assert!(dir.join("b.webp").exists());
        assert_eq!(std::fs::metadata(dir.join("a.webp")).unwrap().modified().unwrap(), first, "a.webp left alone");
        std::fs::remove_dir_all(&dir).unwrap();

        // Two pictures of one name: which is the original is not guessed.
        let dir = scratch("clash");
        photo().save(dir.join("a.png")).unwrap();
        photo().save(dir.join("c.png")).unwrap();
        photo().save(dir.join("c.jpg")).unwrap();
        assert!(!convert_all(&list(&dir), Format::Jxl, &settings), "said so");
        assert!(!dir.join("c.jxl").exists());
        assert!(dir.join("a.jxl").exists(), "the rest done all the same");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_trial_says_how_large_and_how_good() {
        let image = DynamicImage::ImageRgb8(photo());
        let flat = flat(&image);
        let settings = Settings::default();
        let jxl = trial(&image, &flat, None, Format::Jxl, &settings).unwrap();
        assert!(jxl.size > 0 && jxl.decoded.is_some());
        let good = jxl.score.unwrap();
        let mut worse = settings;
        worse.jxl.quality = 30.0;
        let low = trial(&image, &flat, None, Format::Jxl, &worse).unwrap();
        assert!(low.size < jxl.size && low.score.unwrap() < good, "{} < {good}", low.score.unwrap());
        let png = trial(&image, &flat, None, Format::Png, &settings).unwrap();
        assert!(png.score.unwrap() > 99.0, "lossless is perfect: {:?}", png.score);
    }

    #[test]
    fn saved_settings_read_back_the_same() {
        let mut settings = Settings::default();
        settings.jxl.effort = Effort::Slow;
        settings.avif.depth = 10;
        settings.jpeg.full_chroma = false;
        settings.webp.lossless = true;
        let table: toml::Table = settings.to_toml().parse().unwrap();
        let mut back = Settings::default();
        back.apply(&table).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn settings_come_from_the_config_file() {
        let mut settings = Settings::default();
        let table: toml::Table = "jxl = { quality = 80, effort = \"slow\" }\navif = { speed = 4, depth = 10 }\njpeg = { chroma = \"420\" }\nwebp = { lossless = true }".parse().unwrap();
        settings.apply(&table).unwrap();
        assert_eq!((settings.jxl.quality, settings.jxl.effort), (80.0, Effort::Slow));
        assert_eq!((settings.avif.speed, settings.avif.depth), (4, 10));
        assert!(!settings.jpeg.full_chroma && settings.webp.lossless);
        let bad: toml::Table = "jxl = { color = 3 }".parse().unwrap();
        assert!(settings.apply(&bad).is_err());
    }
}
