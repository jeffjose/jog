//! How a picture says its values are to be read as color: the curves and
//! primaries of its ICC profile, a PNG's gAMA, cHRM, cICP and sRGB chunks,
//! and the coding points (H.273) an AVIF or HEIF gives in its `colr` box.
//! For the inspector to show, curves drawn and figures listed.

/// A tone curve: from a value as stored, 0 to 1, to linear light.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    /// Stored as it is: linear.
    Identity,
    /// A power: `x ^ gamma`.
    Gamma(f64),
    /// A table, evenly spaced over 0 to 1.
    Table(Vec<f64>),
    /// ICC's parametric curve, its type (0 to 4) and parameters.
    Parametric(u16, Vec<f64>),
}

impl Curve {
    pub fn eval(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Identity => x,
            Curve::Gamma(gamma) => x.powf(*gamma),
            Curve::Table(table) => {
                let at = x * (table.len() - 1) as f64;
                let i = (at.floor() as usize).min(table.len() - 1);
                let j = (i + 1).min(table.len() - 1);
                table[i] + (table[j] - table[i]) * (at - i as f64)
            }
            Curve::Parametric(kind, p) => {
                let g = |i: usize| p.get(i).copied().unwrap_or(0.0);
                let (gamma, a, b, c, d, e, f) = (g(0), g(1), g(2), g(3), g(4), g(5), g(6));
                match kind {
                    0 => x.powf(gamma),
                    1 if x >= -b / a => (a * x + b).max(0.0).powf(gamma),
                    1 => 0.0,
                    2 if x >= -b / a => (a * x + b).max(0.0).powf(gamma) + c,
                    2 => c,
                    3 if x >= d => (a * x + b).max(0.0).powf(gamma),
                    3 => c * x,
                    4 if x >= d => (a * x + b).max(0.0).powf(gamma) + e,
                    4 => c * x + f,
                    _ => x,
                }
            }
        }
    }

    /// The curve in a few words: "gamma 2.20", "sRGB", "table of 1024,
    /// about gamma 1.80".
    pub fn describe(&self) -> String {
        // The power that would put middle gray where this curve puts it.
        let like = self.eval(0.5).max(1e-9).ln() / 0.5_f64.ln();
        match self {
            Curve::Identity => "linear".into(),
            Curve::Gamma(gamma) => format!("gamma {gamma:.2}"),
            Curve::Parametric(3, p) if (p[0] - 2.4).abs() < 0.01 && (p.get(4).copied().unwrap_or(0.0) - 0.04045).abs() < 0.002 => "sRGB".into(),
            Curve::Parametric(kind, _) => format!("parametric, type {kind}, about gamma {like:.2}"),
            Curve::Table(table) => format!("table of {}, about gamma {like:.2}", table.len()),
        }
    }
}

/// A picture's color as it says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Color {
    /// The ICC profile's version and the device class and color space it
    /// is for, as words.
    pub icc: Option<String>,
    /// Its tone curves, by channel: R, G, B, or K for gray.
    pub curves: Vec<(char, Curve)>,
    /// Its primaries and white point, as xy: red, green, blue, white.
    pub primaries: Option<[(f64, f64); 4]>,
    /// Whether it maps colors by a lookup table rather than curves and a
    /// matrix.
    pub lut: bool,
    /// The brightest the profile is for, in nits, if it says.
    pub luminance: Option<f64>,
    /// H.273 coding points, from an ICC `cicp` tag, a PNG's cICP or an
    /// AVIF's `colr`: primaries, transfer, matrix, full range.
    pub cicp: Option<[u8; 4]>,
    /// A PNG's gAMA: the power its values were encoded with, as stored.
    pub png_gamma: Option<f64>,
    /// A PNG's sRGB chunk, its rendering intent.
    pub srgb_intent: Option<u8>,
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// ICC's s15Fixed16Number.
fn fixed(bytes: &[u8], at: usize) -> Option<f64> {
    Some(f64::from(be32(bytes, at)? as i32) / 65536.0)
}

/// Reads an ICC profile into `color`.
pub fn read_icc(profile: &[u8], color: &mut Color) -> Option<()> {
    let version = profile.get(8..10)?;
    let class = String::from_utf8_lossy(profile.get(12..16)?).trim().to_string();
    let space = String::from_utf8_lossy(profile.get(16..20)?).trim().to_string();
    let class = match class.as_str() {
        "mntr" => "display",
        "scnr" => "input",
        "prtr" => "output",
        "spac" => "color space",
        "abst" => "abstract",
        "link" => "device link",
        "nmcl" => "named color",
        other => other,
    };
    color.icc = Some(format!("v{}.{}  ·  {class}  ·  {space}", version[0], version[1] >> 4));
    let count = be32(profile, 128)? as usize;
    let mut xyz = [None; 4];
    for n in 0..count.min(200) {
        let entry = 132 + n * 12;
        let signature = profile.get(entry..entry + 4)?;
        let offset = be32(profile, entry + 4)? as usize;
        let size = be32(profile, entry + 8)? as usize;
        let Some(tag) = profile.get(offset..offset + size) else { continue };
        let channel = match signature {
            b"rTRC" => Some('R'),
            b"gTRC" => Some('G'),
            b"bTRC" => Some('B'),
            b"kTRC" => Some('K'),
            _ => None,
        };
        if let Some(channel) = channel {
            if let Some(curve) = icc_curve(tag) {
                color.curves.push((channel, curve));
            }
            continue;
        }
        let read_xyz = |tag: &[u8]| -> Option<(f64, f64, f64)> { Some((fixed(tag, 8)?, fixed(tag, 12)?, fixed(tag, 16)?)) };
        match signature {
            b"rXYZ" => xyz[0] = read_xyz(tag),
            b"gXYZ" => xyz[1] = read_xyz(tag),
            b"bXYZ" => xyz[2] = read_xyz(tag),
            b"wtpt" => xyz[3] = read_xyz(tag),
            b"lumi" => color.luminance = read_xyz(tag).map(|(_, y, _)| y),
            b"A2B0" | b"A2B1" | b"B2A0" => color.lut = true,
            b"cicp" if tag.len() >= 12 => color.cicp = Some([tag[8], tag[9], tag[10], tag[11]]),
            _ => {}
        }
    }
    // As xy. A v4 profile's colorants are adapted to D50, as its white is.
    if let [Some(r), Some(g), Some(b), Some(w)] = xyz {
        let xy = |(x, y, z): (f64, f64, f64)| {
            let sum = x + y + z;
            if sum > 0.0 { (x / sum, y / sum) } else { (0.0, 0.0) }
        };
        color.primaries = Some([xy(r), xy(g), xy(b), xy(w)]);
    }
    color.curves.sort_by_key(|(channel, _)| "RGBK".find(*channel));
    Some(())
}

/// An ICC `curv` or `para` tag.
fn icc_curve(tag: &[u8]) -> Option<Curve> {
    match tag.get(..4)? {
        b"curv" => {
            let count = be32(tag, 8)? as usize;
            match count {
                0 => Some(Curve::Identity),
                1 => Some(Curve::Gamma(f64::from(be16(tag, 12)?) / 256.0)),
                _ => Some(Curve::Table((0..count.min(65536)).map(|i| be16(tag, 12 + i * 2).map(|v| f64::from(v) / 65535.0)).collect::<Option<Vec<_>>>()?)),
            }
        }
        b"para" => {
            let kind = be16(tag, 8)?;
            let params = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return None,
            };
            Some(Curve::Parametric(kind, (0..params).map(|i| fixed(tag, 12 + i * 4)).collect::<Option<Vec<_>>>()?))
        }
        _ => None,
    }
}

/// A PNG's chunks that say how to read its color: its profile, gAMA,
/// cHRM, cICP and sRGB.
pub fn read_png(bytes: &[u8], color: &mut Color) {
    let mut at = 8;
    while let (Some(len), Some(kind)) = (be32(bytes, at), bytes.get(at + 4..at + 8)) {
        let len = len as usize;
        let Some(data) = bytes.get(at + 8..at + 8 + len) else { break };
        match kind {
            b"gAMA" => color.png_gamma = be32(data, 0).map(|v| f64::from(v) / 100_000.0),
            b"sRGB" => color.srgb_intent = data.first().copied(),
            b"cICP" if data.len() >= 4 => color.cicp = Some([data[0], data[1], data[2], data[3]]),
            b"cHRM" if color.primaries.is_none() => {
                let point = |i: usize| -> Option<(f64, f64)> { Some((f64::from(be32(data, i * 8)?) / 100_000.0, f64::from(be32(data, i * 8 + 4)?) / 100_000.0)) };
                if let (Some(w), Some(r), Some(g), Some(b)) = (point(0), point(1), point(2), point(3)) {
                    color.primaries = Some([r, g, b, w]);
                }
            }
            b"iCCP" => {
                // A name, a nul, the method (0, zlib), and the profile.
                if let Some(nul) = data.iter().position(|&b| b == 0) {
                    if let Some(profile) = data.get(nul + 2..).and_then(|packed| miniz_oxide::inflate::decompress_to_vec_zlib(packed).ok()) {
                        let _ = read_icc(&profile, color);
                    }
                }
            }
            b"IDAT" | b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
}

/// An AVIF's or HEIF's `colr` box of coding points.
pub fn read_nclx(bytes: &[u8], color: &mut Color) {
    let head = &bytes[..bytes.len().min(64 << 10)];
    if let Some(at) = memchr::memmem::find(head, b"colrnclx") {
        let body = at + 8;
        if let (Some(primaries), Some(transfer), Some(matrix), Some(&range)) = (be16(head, body), be16(head, body + 2), be16(head, body + 4), head.get(body + 6)) {
            color.cicp = Some([primaries as u8, transfer as u8, matrix as u8, range >> 7]);
        }
    }
}

/// H.273's coding points as words.
pub fn cicp_words([primaries, transfer, matrix, full]: [u8; 4]) -> String {
    let primaries = match primaries {
        1 => "BT.709",
        9 => "BT.2020",
        12 => "Display P3",
        11 => "DCI-P3",
        2 => "unspecified",
        _ => "other",
    };
    let transfer = match transfer {
        1 | 6 | 14 | 15 => "BT.709",
        13 => "sRGB",
        16 => "PQ",
        18 => "HLG",
        8 => "linear",
        2 => "unspecified",
        _ => "other",
    };
    let matrix = match matrix {
        0 => "RGB",
        1 => "BT.709",
        6 => "BT.601",
        9 => "BT.2020",
        2 => "unspecified",
        _ => "other",
    };
    let range = if full == 1 { "full range" } else { "limited range" };
    format!("{primaries}  ·  {transfer}  ·  {matrix}  ·  {range}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_evaluate_and_describe_themselves() {
        let srgb = Curve::Parametric(3, vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]);
        assert!((srgb.eval(0.5) - 0.2140).abs() < 1e-3);
        assert_eq!(srgb.describe(), "sRGB");
        assert_eq!(Curve::Gamma(2.2).describe(), "gamma 2.20");
        let table = Curve::Table((0..256).map(|i| (f64::from(i) / 255.0).powf(1.8)).collect());
        assert_eq!(table.describe(), "table of 256, about gamma 1.80");
    }

    #[test]
    fn coding_points_read_as_words() {
        assert_eq!(cicp_words([9, 16, 9, 0]), "BT.2020  ·  PQ  ·  BT.2020  ·  limited range");
        assert_eq!(cicp_words([1, 13, 0, 1]), "BT.709  ·  sRGB  ·  RGB  ·  full range");
    }
}
