use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use exif::{Exif, In, Tag, Value};

/// File details shown by the info and EXIF overlays.
pub struct Meta {
    pub size: Option<u64>,
    pub exif: Vec<String>,
}

pub fn read(path: &Path) -> Meta {
    let size = std::fs::metadata(path).ok().map(|m| m.len());
    let exif = read_exif(path).map_or_else(|| vec!["No EXIF data".to_string()], |exif| describe(&exif));
    Meta { size, exif }
}

pub fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.0} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn read_exif(path: &Path) -> Option<Exif> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    exif::Reader::new().read_from_container(&mut reader).ok()
}

fn describe(exif: &Exif) -> Vec<String> {
    let text = |tag| {
        let field = exif.get_field(tag, In::PRIMARY)?;
        let value = match &field.value {
            Value::Ascii(parts) => String::from_utf8_lossy(parts.first()?).trim().to_string(),
            _ => field.display_value().with_unit(exif).to_string(),
        };
        (!value.is_empty()).then_some(value)
    };

    let mut lines = Vec::new();
    let camera: Vec<String> = [Tag::Make, Tag::Model].into_iter().filter_map(text).collect();
    if !camera.is_empty() {
        lines.push(camera.join(" "));
    }
    lines.extend(text(Tag::LensModel));

    let exposure = [
        text(Tag::ExposureTime),
        text(Tag::FNumber),
        text(Tag::PhotographicSensitivity).map(|iso| format!("ISO {iso}")),
        text(Tag::FocalLength),
    ];
    let exposure: Vec<String> = exposure.into_iter().flatten().collect();
    if !exposure.is_empty() {
        lines.push(exposure.join("   "));
    }
    lines.extend(text(Tag::DateTimeOriginal));

    if lines.is_empty() {
        lines.push("No EXIF data".to_string());
    }
    lines
}
