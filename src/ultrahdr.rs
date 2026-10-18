//! Ultra HDR JPEGs: an ordinary picture for any screen, and after it a
//! second, small JPEG, the gain map, saying how much brighter each part
//! of the picture is in the HDR rendition. jog's screen is an SDR one, so
//! the HDR rendition is shown as such a screen can: everything scaled
//! down until the brightest the picture reaches is the screen's white,
//! which leaves the picture's ordinary white a step below it, as an HDR
//! screen shows it beside its highlights.

use std::ops::Range;

use memchr::memmem;
use rayon::prelude::*;

use image::RgbImage;

/// Which rendition of an Ultra HDR picture is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Hdr {
    /// The ordinary picture, as any viewer shows it.
    #[default]
    Sdr,
    /// The HDR rendition, made with the gain map.
    Full,
    /// The gain map itself, as gray: the lighter, the more boost.
    GainMap,
}

impl Hdr {
    pub fn next(self) -> Hdr {
        match self {
            Hdr::Sdr => Hdr::Full,
            Hdr::Full => Hdr::GainMap,
            Hdr::GainMap => Hdr::Sdr,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Hdr::Sdr => "SDR",
            Hdr::Full => "HDR",
            Hdr::GainMap => "Gain map",
        }
    }

    pub fn parse(text: &str) -> Option<Hdr> {
        match text {
            "sdr" => Some(Hdr::Sdr),
            "hdr" | "full" => Some(Hdr::Full),
            "gainmap" | "gain-map" | "gain" => Some(Hdr::GainMap),
            _ => None,
        }
    }
}

/// How the gain map is to be read, from its own XMP: by channel, the
/// least and most boost in stops, the curve its values are stored on,
/// the small amounts added before and taken away after boosting, and
/// the most boost the picture is meant to have.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Meta {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub gamma: [f32; 3],
    pub offset_sdr: [f32; 3],
    pub offset_hdr: [f32; 3],
    pub capacity_max: f32,
}

/// A gain map, decoded, and how to read it.
pub struct GainMap {
    pub image: RgbImage,
    pub meta: Meta,
}

/// How far into a file to look for the sign that it is an Ultra HDR one:
/// its XMP comes in the first few segments.
const LOOK: usize = 256 << 10;

/// Whether a file says it is an Ultra HDR one, from its first bytes.
pub fn is_ultra_hdr(head: &[u8]) -> bool {
    head.starts_with(b"\xFF\xD8") && memmem::find(&head[..head.len().min(LOOK)], b"hdrgm:Version").is_some()
}

/// Where in a file the gain map JPEG is, and how to read it.
pub fn find(bytes: &[u8]) -> Option<(Range<usize>, Meta)> {
    if !is_ultra_hdr(bytes) {
        return None;
    }
    let range = mpf_second(bytes).or_else(|| after_marker(bytes))?;
    let gain = bytes.get(range.clone())?;
    let meta = meta(&gain[..gain.len().min(LOOK)])?;
    Some((range, meta))
}

/// Decodes the gain map of an Ultra HDR file, if it is one.
pub fn gain_map(bytes: &[u8]) -> Option<GainMap> {
    let (range, meta) = find(bytes)?;
    let image = image::load_from_memory_with_format(&bytes[range], image::ImageFormat::Jpeg).ok()?.to_rgb8();
    Some(GainMap { image, meta })
}

/// The second picture an MPF index lists: in an Ultra HDR file, the gain
/// map. Its offset counts from the start of the index's own TIFF header.
fn mpf_second(bytes: &[u8]) -> Option<Range<usize>> {
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF {
        let marker = bytes[at + 1];
        // Past the header segments the picture itself begins.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let body = bytes.get(at + 4..at + 2 + length)?;
        if marker == 0xE2 && body.starts_with(b"MPF\0") {
            let base = at + 8;
            return mpf_entries(bytes, base);
        }
        at += 2 + length;
    }
    None
}

fn mpf_entries(bytes: &[u8], base: usize) -> Option<Range<usize>> {
    let tiff = bytes.get(base..)?;
    let little = tiff.starts_with(b"II");
    let u16_at = |at: usize| tiff.get(at..at + 2).map(|b| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |at: usize| tiff.get(at..at + 4).map(|b| if little { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) });
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    for entry in 0..count {
        let at = ifd + 2 + entry * 12;
        if u16_at(at)? == 0xB002 {
            let entries = u32_at(at + 8)? as usize;
            // Each picture's entry is 16 bytes: its kind, size, offset.
            let second = entries + 16;
            let (size, offset) = (u32_at(second + 4)? as usize, u32_at(second + 8)? as usize);
            let start = base + offset;
            return (offset > 0 && size > 0 && bytes.get(start..start + 2) == Some(b"\xFF\xD8")).then_some(start..(start + size).min(bytes.len()));
        }
    }
    None
}

/// Where there is no index: the JPEG whose XMP says how to read a gain
/// map, found from that XMP back to where its picture begins.
fn after_marker(bytes: &[u8]) -> Option<Range<usize>> {
    let marker = memmem::rfind(bytes, b"hdrgm:GainMapMax")?;
    let start = memmem::rfind(&bytes[..marker], b"\xFF\xD8\xFF")?;
    (start > 0).then_some(start..bytes.len())
}

/// Reads the values that say how to apply a gain map from its XMP, given
/// as attributes or as elements, one value or one for each channel.
fn meta(xmp: &[u8]) -> Option<Meta> {
    let text = String::from_utf8_lossy(xmp);
    if value(&text, "BaseRenditionIsHDR").is_some_and(|values| values.first().is_some_and(|v| *v != 0.0)) || text.contains("hdrgm:BaseRenditionIsHDR=\"True\"") {
        return None;
    }
    let three = |name: &str, default: f32| -> [f32; 3] {
        match value(&text, name).as_deref() {
            Some([one]) => [*one; 3],
            Some([r, g, b, ..]) => [*r, *g, *b],
            _ => [default; 3],
        }
    };
    let max = three("GainMapMax", f32::NAN);
    if max.iter().any(|value| value.is_nan()) {
        return None;
    }
    let capacity_max = value(&text, "HDRCapacityMax").and_then(|values| values.first().copied()).unwrap_or(max.iter().copied().fold(0.0, f32::max));
    Some(Meta {
        min: three("GainMapMin", 0.0),
        max,
        gamma: three("Gamma", 1.0),
        offset_sdr: three("OffsetSDR", 1.0 / 64.0),
        offset_hdr: three("OffsetHDR", 1.0 / 64.0),
        capacity_max: capacity_max.max(0.0),
    })
}

/// The value or values of an `hdrgm:` field: `hdrgm:Name="2.3"`, or
/// `<hdrgm:Name>2.3</hdrgm:Name>`, or a list of `<rdf:li>` in it.
fn value(text: &str, name: &str) -> Option<Vec<f32>> {
    let key = format!("hdrgm:{name}");
    let mut from = 0;
    while let Some(found) = text[from..].find(&key) {
        let after = from + found + key.len();
        from = after;
        let rest = &text[after..];
        if let Some(quoted) = rest.strip_prefix("=\"") {
            return quoted.split('"').next()?.trim().parse().ok().map(|one| vec![one]);
        }
        if let Some(inside) = rest.strip_prefix('>') {
            let end = inside.find(&format!("</{key}"))?;
            let inside = &inside[..end];
            let items: Vec<f32> = inside.split("<rdf:li>").skip(1).filter_map(|item| item.split('<').next()?.trim().parse().ok()).collect();
            if !items.is_empty() {
                return Some(items);
            }
            return inside.trim().parse().ok().map(|one| vec![one]);
        }
    }
    None
}

/// The HDR rendition of a picture, its gain map already the same size, as
/// an SDR screen can show it: scaled so that the most boost the picture
/// is meant to have reaches the screen's white and no more.
pub fn compose(sdr: &RgbImage, gain: &RgbImage, meta: &Meta) -> RgbImage {
    // The gain map's values are 8 bits, so each channel's boost for each
    // of them is worked out once.
    let boost: [[f32; 256]; 3] = std::array::from_fn(|channel| {
        std::array::from_fn(|value| {
            let mut weight = value as f32 / 255.0;
            if meta.gamma[channel] != 1.0 && meta.gamma[channel] > 0.0 {
                weight = weight.powf(1.0 / meta.gamma[channel]);
            }
            let stops = meta.min[channel] * (1.0 - weight) + meta.max[channel] * weight;
            stops.exp2()
        })
    });
    let to_linear: [f32; 256] = std::array::from_fn(|value| srgb_to_linear(value as f32 / 255.0));
    let scale = 1.0 / meta.capacity_max.max(meta.max.iter().copied().fold(0.0, f32::max)).exp2();
    // Back to the screen's curve: by a table fine enough that no step
    // shows in a smooth sky.
    const STEPS: usize = 4096;
    let to_screen: Vec<u8> = (0..=STEPS).map(|step| (linear_to_srgb(step as f32 / STEPS as f32) * 255.0).round() as u8).collect();
    let (width, height) = sdr.dimensions();
    let mut out = RgbImage::new(width, height);
    out.par_chunks_mut(width as usize * 3).zip(sdr.par_chunks(width as usize * 3)).zip(gain.par_chunks(width as usize * 3)).for_each(|((out, sdr), gain)| {
        for at in 0..out.len() {
            let channel = at % 3;
            let lit = (to_linear[sdr[at] as usize] + meta.offset_sdr[channel]) * boost[channel][gain[at] as usize] - meta.offset_hdr[channel];
            let shown = (lit * scale).clamp(0.0, 1.0);
            out[at] = to_screen[(shown * STEPS as f32) as usize];
        }
    });
    out
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read_in_each_form_xmp_gives_them() {
        let text = r#"<x hdrgm:Version="1.0" hdrgm:GainMapMax="2.5" hdrgm:Gamma="1"/><hdrgm:OffsetSDR>0.1</hdrgm:OffsetSDR>
            <hdrgm:GainMapMin><rdf:Seq><rdf:li>0.1</rdf:li><rdf:li>0.2</rdf:li><rdf:li>0.3</rdf:li></rdf:Seq></hdrgm:GainMapMin>"#;
        assert_eq!(value(text, "GainMapMax"), Some(vec![2.5]));
        assert_eq!(value(text, "OffsetSDR"), Some(vec![0.1]));
        assert_eq!(value(text, "GainMapMin"), Some(vec![0.1, 0.2, 0.3]));
        let meta = meta(text.as_bytes()).unwrap();
        assert_eq!(meta.min, [0.1, 0.2, 0.3]);
        assert_eq!(meta.capacity_max, 2.5);
        assert_eq!(meta.offset_hdr, [1.0 / 64.0; 3]);
        assert!(meta_of(r#"hdrgm:Version="1.0""#).is_none(), "no GainMapMax, no gain map");
    }

    fn meta_of(text: &str) -> Option<Meta> {
        meta(text.as_bytes())
    }

    #[test]
    fn no_boost_leaves_the_picture_a_step_below_white_and_full_boost_reaches_it() {
        let meta = Meta { min: [0.0; 3], max: [2.0; 3], gamma: [1.0; 3], offset_sdr: [0.0; 3], offset_hdr: [0.0; 3], capacity_max: 2.0 };
        let sdr = RgbImage::from_pixel(2, 1, image::Rgb([255, 255, 255]));
        let mut gain = RgbImage::new(2, 1);
        gain.put_pixel(1, 0, image::Rgb([255, 255, 255]));
        let out = compose(&sdr, &gain, &meta);
        // White with no boost is a quarter of the light: about half way up the curve.
        let plain = out.get_pixel(0, 0)[0];
        assert!((130..=140).contains(&plain), "{plain}");
        assert_eq!(out.get_pixel(1, 0)[0], 255);
    }

    #[test]
    fn a_file_without_a_gain_map_is_not_one() {
        assert!(!is_ultra_hdr(b"\xFF\xD8\xFF\xE0 plain jpeg"));
        assert!(find(b"\xFF\xD8\xFF\xE1 hdrgm:Version=\"1.0\" nothing more").is_none());
    }
}
