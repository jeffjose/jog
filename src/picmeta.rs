//! What a picture file carries besides its pixels, for the inspector: its
//! EXIF, every field; the color profile embedded and its name; the XMP;
//! and an Ultra HDR photo's gain map and how it is to be read.

use std::path::Path;

use exif::In;

/// What was found in a picture.
#[derive(Default)]
pub struct Picture {
    pub format: Option<String>,
    pub size: Option<(u32, u32)>,
    pub bytes: u64,
    /// EXIF fields: the group (Image, Photo, GPS, Thumbnail), the tag's
    /// name and its value as people read it.
    pub exif: Vec<(String, String, String)>,
    /// The embedded color profile's name, and how big it is.
    pub icc: Option<(String, usize)>,
    /// XMP properties, as namespace:name and value.
    pub xmp: Vec<(String, String)>,
    /// An Ultra HDR photo's gain map: how to read it, and its size.
    pub gain: Option<(crate::ultrahdr::Meta, usize)>,
    /// How its values are to be read as color: curves, primaries, coding points.
    pub color: crate::colorinfo::Color,
}

pub fn read(path: &Path) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut picture = Picture { bytes: bytes.len() as u64, ..Picture::default() };
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok();
    picture.format = reader.as_ref().and_then(|reader| reader.format()).map(|format| format!("{format:?}").to_uppercase());
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jxl")) {
        picture.format = Some("JPEG XL".into());
    }
    picture.size = reader.and_then(|reader| reader.into_dimensions().ok());
    if let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(&bytes)) {
        for field in exif.fields() {
            let group = match (field.ifd_num, field.tag.context()) {
                (In::THUMBNAIL, _) => "Thumbnail",
                (_, exif::Context::Gps) => "GPS",
                (_, exif::Context::Exif) => "Photo",
                (_, exif::Context::Interop) => "Interop",
                _ => "Image",
            };
            // Long binary values, maker notes, are left out.
            let value = field.display_value().with_unit(&exif).to_string();
            if value.len() <= 200 {
                picture.exif.push((group.to_string(), field.tag.to_string(), value.trim_matches('"').to_string()));
            }
        }
    }
    picture.icc = icc_profile(&bytes).map(|profile| (icc_name(&profile).unwrap_or_else(|| "unnamed".into()), profile.len()));
    if let Some(profile) = icc_profile(&bytes) {
        let _ = crate::colorinfo::read_icc(&profile, &mut picture.color);
    }
    if bytes.starts_with(b"\x89PNG") {
        crate::colorinfo::read_png(&bytes, &mut picture.color);
    }
    crate::colorinfo::read_nclx(&bytes, &mut picture.color);
    picture.xmp = xmp(&bytes);
    picture.gain = crate::ultrahdr::find(&bytes).map(|(range, meta)| (meta, range.len()));
    Ok(picture)
}

/// The ICC profile in a JPEG's APP2 segments, or a WebP's ICCP chunk.
fn icc_profile(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(b"\xFF\xD8") {
        let mut profile = Vec::new();
        let mut at = 2;
        while at + 4 <= bytes.len() && bytes[at] == 0xFF {
            let marker = bytes[at + 1];
            if marker == 0xDA || marker == 0xD9 {
                break;
            }
            let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            let body = bytes.get(at + 4..at + 2 + len)?;
            if marker == 0xE2 && body.starts_with(b"ICC_PROFILE\0") && body.len() > 14 {
                profile.extend_from_slice(&body[14..]);
            }
            at += 2 + len;
        }
        return (!profile.is_empty()).then_some(profile);
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
            if &bytes[at..at + 4] == b"ICCP" {
                return bytes.get(at + 8..at + 8 + len).map(<[u8]>::to_vec);
            }
            at += 8 + len + (len & 1);
        }
    }
    None
}

/// An ICC profile's name: its 'desc' tag, version 2 or 4.
fn icc_name(profile: &[u8]) -> Option<String> {
    let count = u32::from_be_bytes(profile.get(128..132)?.try_into().ok()?) as usize;
    for n in 0..count.min(100) {
        let entry = profile.get(132 + n * 12..144 + n * 12)?;
        if &entry[..4] != b"desc" {
            continue;
        }
        let offset = u32::from_be_bytes(entry[4..8].try_into().ok()?) as usize;
        let tag = profile.get(offset..)?;
        return match tag.get(..4)? {
            b"desc" => {
                let len = u32::from_be_bytes(tag.get(8..12)?.try_into().ok()?) as usize;
                Some(String::from_utf8_lossy(tag.get(12..12 + len)?).trim_end_matches('\0').to_string())
            }
            b"mluc" => {
                let size = u32::from_be_bytes(tag.get(20..24)?.try_into().ok()?) as usize;
                let at = u32::from_be_bytes(tag.get(24..28)?.try_into().ok()?) as usize;
                let units: Vec<u16> = tag.get(at..at + size)?.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
                Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
            }
            _ => None,
        };
    }
    None
}

/// The XMP packets in a file, wherever they are, as their properties:
/// attributes such as `xmp:Rating="5"` and simple elements such as
/// `<tiff:Make>Google</tiff:Make>`.
fn xmp(bytes: &[u8]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = memchr::memmem::find(&bytes[from..], b"<x:xmpmeta").map(|at| from + at) {
        let Some(end) = memchr::memmem::find(&bytes[start..], b"</x:xmpmeta>").map(|at| start + at) else { break };
        let packet = String::from_utf8_lossy(&bytes[start..end]);
        properties(&packet, &mut found);
        from = end;
    }
    found
}

fn properties(packet: &str, found: &mut Vec<(String, String)>) {
    // Attributes: name="value", where the name has a namespace.
    let mut rest = packet;
    while let Some(eq) = rest.find("=\"") {
        let name_start = rest[..eq].rfind(|c: char| c.is_whitespace()).map_or(0, |at| at + 1);
        let name = &rest[name_start..eq];
        let after = &rest[eq + 2..];
        let Some(close) = after.find('"') else { break };
        let value = &after[..close];
        if name.contains(':') && !name.starts_with("xmlns") && !name.starts_with("rdf:") && !name.starts_with("x:") {
            found.push((name.to_string(), value.to_string()));
        }
        rest = &after[close + 1..];
    }
    // Simple elements with text: <ns:name>value</ns:name>.
    let mut rest = packet;
    while let Some(open) = rest.find('<') {
        let tag_end = rest[open..].find('>').map(|at| open + at);
        let Some(tag_end) = tag_end else { break };
        let name = &rest[open + 1..tag_end];
        if name.contains(':') && !name.contains(' ') && !name.starts_with('/') && !name.starts_with("rdf:") && !name.starts_with("x:") {
            let close = format!("</{name}>");
            if let Some(end) = rest[tag_end..].find(&close) {
                let value = rest[tag_end + 1..tag_end + end].trim();
                if !value.is_empty() && !value.contains('<') {
                    found.push((name.to_string(), value.to_string()));
                }
            }
        }
        rest = &rest[tag_end + 1..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xmp_properties_are_read_from_attributes_and_elements() {
        let packet = r#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:xmp="ns" xmp:Rating="5" hdrgm:Version="1.0"><tiff:Make>Google</tiff:Make></rdf:Description></rdf:RDF>"#;
        let mut found = Vec::new();
        properties(packet, &mut found);
        assert!(found.contains(&("xmp:Rating".to_string(), "5".to_string())));
        assert!(found.contains(&("hdrgm:Version".to_string(), "1.0".to_string())));
        assert!(found.contains(&("tiff:Make".to_string(), "Google".to_string())));
    }
}
