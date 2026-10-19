//! A video file's structure, for the inspector: an MP4's boxes or a
//! Matroska file's elements as a tree, each with where it is and how big,
//! the fields of the ones that matter decoded, and its first bytes. The
//! media data itself (mdat, Clusters) is shown but not gone into.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A box or element.
#[derive(Clone, Debug, Default)]
pub struct Node {
    /// Its type: "moov", "colr"; for Matroska the element's name.
    pub name: String,
    /// A word on what it is.
    pub about: &'static str,
    pub offset: u64,
    pub size: u64,
    pub children: Vec<Node>,
    /// Its fields: the spec's name, the value as stored, and what that
    /// means.
    pub fields: Vec<(String, String, String)>,
    /// Its first bytes, header included.
    pub bytes: Vec<u8>,
    /// For a run of boxes of one kind after the first two, how many more
    /// there are, all shown as this one.
    pub more: usize,
}

/// The most nodes read, so a file with millions of fragments stays quick.
const MOST: usize = 4000;
/// How many of a box's bytes are kept to show.
const SHOWN_BYTES: usize = 48;

/// Reads the tree of an MP4 or a Matroska file.
pub fn read(path: &Path) -> Result<Vec<Node>, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let end = file.metadata().map_err(|error| error.to_string())?.len();
    let mut head = [0u8; 4];
    file.read_exact(&mut head).map_err(|error| error.to_string())?;
    let mut count = 0;
    if head == [0x1A, 0x45, 0xDF, 0xA3] {
        Ok(ebml(&mut file, 0, end, 0, &mut count))
    } else {
        Ok(mp4(&mut file, 0, end, 0, &mut count))
    }
}

fn bytes_at(file: &mut File, at: u64, len: usize) -> Vec<u8> {
    let mut buffer = vec![0u8; len];
    if file.seek(SeekFrom::Start(at)).is_err() {
        return Vec::new();
    }
    let read = file.read(&mut buffer).unwrap_or(0);
    buffer.truncate(read);
    buffer
}

// ---- MP4 ------------------------------------------------------------

/// What an MP4 box is, in a few words.
fn mp4_about(kind: &str) -> &'static str {
    match kind {
        "ftyp" => "File type: the brands it follows",
        "moov" => "Movie: everything about the tracks",
        "mvhd" => "Movie header: timescale and length",
        "trak" => "Track",
        "tkhd" => "Track header: id, size",
        "mdia" => "Media of the track",
        "mdhd" => "Media header: timescale, length, language",
        "hdlr" => "Handler: what kind of track",
        "minf" => "Media information",
        "stbl" => "Sample table: where each frame is",
        "stsd" => "Sample descriptions: the codec",
        "hvc1" | "hev1" => "HEVC video",
        "avc1" | "avc3" => "H.264 video",
        "av01" => "AV1 video",
        "vp09" => "VP9 video",
        "mp4a" => "AAC audio",
        "Opus" => "Opus audio",
        "hvcC" => "HEVC decoder configuration",
        "avcC" => "H.264 decoder configuration",
        "av1C" => "AV1 decoder configuration",
        "colr" => "Color information",
        "mdcv" => "Mastering display color volume (HDR10)",
        "clli" => "Content light level (HDR10)",
        "dvcC" | "dvvC" => "Dolby Vision configuration",
        "pasp" => "Pixel aspect ratio",
        "btrt" => "Bitrate",
        "stts" => "Frame durations",
        "ctts" => "Frame display offsets",
        "stss" => "Keyframes",
        "stsc" => "Frames per chunk",
        "stsz" => "Frame sizes",
        "stco" | "co64" => "Chunk offsets",
        "edts" => "Edits",
        "elst" => "Edit list",
        "udta" => "User data",
        "meta" => "Metadata",
        "ilst" => "Metadata items",
        "mdat" => "Media data: the frames and samples",
        "free" | "skip" => "Free space",
        "moof" => "Movie fragment",
        "mfra" => "Fragment random access",
        "sidx" => "Segment index",
        "dinf" => "Data information",
        "smhd" => "Sound header",
        "vmhd" => "Video header",
        "sgpd" | "sbgp" => "Sample groups",
        "uuid" => "Vendor box",
        _ => "",
    }
}

/// How many bytes into a box's body its children begin, for the boxes
/// that hold others; None for one that does not.
fn mp4_children_at(kind: &str) -> Option<u64> {
    match kind {
        "moov" | "trak" | "mdia" | "minf" | "stbl" | "edts" | "udta" | "dinf" | "mvex" | "moof" | "traf" | "mfra" | "sinf" | "schi" | "ilst" | "tref" => Some(0),
        "meta" => Some(4),
        "stsd" => Some(8),
        // Visual sample entries: 78 bytes before their boxes.
        "hvc1" | "hev1" | "avc1" | "avc3" | "av01" | "vp09" | "dvh1" | "dvhe" | "encv" => Some(78),
        // Audio sample entries: 28.
        "mp4a" | "Opus" | "ac-3" | "ec-3" | "fLaC" | "enca" => Some(28),
        _ => None,
    }
}

fn mp4(file: &mut File, start: u64, end: u64, depth: usize, count: &mut usize) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut at = start;
    while at + 8 <= end && *count < MOST {
        let head = bytes_at(file, at, 16);
        if head.len() < 8 {
            break;
        }
        let mut size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        let kind: String = head[4..8].iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '.' }).collect();
        let mut header = 8;
        if size == 1 && head.len() >= 16 {
            size = u64::from_be_bytes(head[8..16].try_into().unwrap_or_default());
            header = 16;
        } else if size == 0 {
            size = end - at;
        }
        if size < header || at.checked_add(size).is_none_or(|stop| stop > end) {
            break;
        }

        let mut node = Node { name: kind.clone(), about: mp4_about(&kind), offset: at, size, bytes: bytes_at(file, at, SHOWN_BYTES.min(size as usize)), ..Node::default() };
        let body_at = at + header;
        let body_len = size - header;
        match mp4_children_at(&kind) {
            Some(skip) if depth < 12 && kind != "mdat" => node.children = mp4(file, body_at + skip, at + size, depth + 1, count),
            _ => {}
        }
        if body_len <= 4096 {
            let body = bytes_at(file, body_at, body_len as usize);
            node.fields = mp4_fields(&kind, &body);
        }
        if !fold(&mut nodes, &node) {
            *count += 1;
            nodes.push(node);
        }
        at += size;
    }
    nodes
}

fn be16(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?)))
}

fn be32(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?)))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn fourcc(b: &[u8], at: usize) -> Option<String> {
    Some(b.get(at..at + 4)?.iter().map(|c| *c as char).collect())
}

/// ITU-T H.273 color primaries, as named.
pub fn primaries_name(code: u64) -> &'static str {
    match code {
        1 => "BT.709",
        2 => "unspecified",
        5 => "BT.601 PAL",
        6 => "BT.601 NTSC",
        9 => "BT.2020",
        12 => "Display P3",
        _ => "other",
    }
}

/// ITU-T H.273 transfer characteristics, as named.
pub fn transfer_name(code: u64) -> &'static str {
    match code {
        1 => "BT.709",
        2 => "unspecified",
        13 => "sRGB",
        14 | 15 => "BT.2020 (SDR)",
        16 => "PQ (SMPTE 2084): HDR10",
        18 => "HLG (ARIB STD-B67)",
        _ => "other",
    }
}

/// ITU-T H.273 matrix coefficients, as named.
pub fn matrix_name(code: u64) -> &'static str {
    match code {
        0 => "RGB",
        1 => "BT.709",
        2 => "unspecified",
        5 | 6 => "BT.601",
        9 => "BT.2020 non-constant",
        10 => "BT.2020 constant",
        _ => "other",
    }
}

fn mp4_fields(kind: &str, b: &[u8]) -> Vec<(String, String, String)> {
    let mut fields: Vec<(String, String, String)> = Vec::new();
    let mut add = |name: &str, value: Option<String>, means: String| {
        if let Some(value) = value {
            fields.push((name.to_string(), value, means));
        }
    };
    match kind {
        "ftyp" => {
            add("major_brand", fourcc(b, 0), String::new());
            add("minor_version", be32(b, 4).map(|v| v.to_string()), String::new());
            let compatible: Vec<String> = (8..b.len()).step_by(4).filter_map(|at| fourcc(b, at)).collect();
            add("compatible_brands", Some(compatible.join(" ")), String::new());
        }
        "mvhd" | "mdhd" => {
            let version = b.first().copied().unwrap_or(0);
            let (scale, length) = if version == 1 { (be32(b, 20), be64(b, 24)) } else { (be32(b, 12), be32(b, 16)) };
            add("timescale", scale.map(|v| v.to_string()), "ticks per second".to_string());
            let seconds = scale.zip(length).filter(|(s, _)| *s > 0).map(|(s, l)| l as f64 / s as f64);
            add("duration", length.map(|v| v.to_string()), seconds.map_or(String::new(), |s| format!("{s:.3} s")));
            if kind == "mdhd" {
                let at = if version == 1 { 32 } else { 20 };
                let packed = be16(b, at).unwrap_or(0);
                let lang: String = [(packed >> 10) & 31, (packed >> 5) & 31, packed & 31].iter().map(|c| ((*c as u8) + 0x60) as char).collect();
                add("language", Some(lang), String::new());
            }
        }
        "tkhd" => {
            let version = b.first().copied().unwrap_or(0);
            let id_at = if version == 1 { 20 } else { 12 };
            add("track_ID", be32(b, id_at).map(|v| v.to_string()), String::new());
            let size_at = b.len().saturating_sub(8);
            add("width", be32(b, size_at).map(|v| format!("{}", v >> 16)), "pixels".to_string());
            add("height", be32(b, size_at + 4).map(|v| format!("{}", v >> 16)), "pixels".to_string());
        }
        "hdlr" => {
            let handler = fourcc(b, 8);
            let means = match handler.as_deref() {
                Some("vide") => "video",
                Some("soun") => "sound",
                Some("subt" | "sbtl" | "text") => "subtitles",
                Some("meta") => "metadata",
                _ => "",
            };
            add("handler_type", handler, means.to_string());
            let name: String = b.get(24..).unwrap_or_default().iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
            add("name", Some(name).filter(|n| !n.is_empty()), String::new());
        }
        "colr" => {
            let kind = fourcc(b, 0);
            let nclx = kind.as_deref() == Some("nclx");
            add("colour_type", kind.clone(), if nclx { "coded as numbers, not an ICC profile".into() } else { "an ICC profile".into() });
            if nclx {
                add("colour_primaries", be16(b, 4).map(|v| v.to_string()), be16(b, 4).map_or(String::new(), |v| primaries_name(v).to_string()));
                add("transfer_characteristics", be16(b, 6).map(|v| v.to_string()), be16(b, 6).map_or(String::new(), |v| transfer_name(v).to_string()));
                add("matrix_coefficients", be16(b, 8).map(|v| v.to_string()), be16(b, 8).map_or(String::new(), |v| matrix_name(v).to_string()));
                let full = b.get(10).map(|v| v >> 7);
                add("full_range_flag", full.map(|v| v.to_string()), if full == Some(1) { "Full (0–255)".into() } else { "Limited (16–235)".into() });
            }
        }
        "mdcv" => {
            let names = ["display_primaries G x", "G y", "B x", "B y", "R x", "R y", "white_point x", "y"];
            for (at, name) in names.iter().enumerate() {
                add(name, be16(b, at * 2).map(|v| v.to_string()), be16(b, at * 2).map_or(String::new(), |v| format!("{:.4}", v as f64 / 50000.0)));
            }
            add("max_display_mastering_luminance", be32(b, 16).map(|v| v.to_string()), be32(b, 16).map_or(String::new(), |v| format!("{:.0} nits", v as f64 / 10000.0)));
            add("min_display_mastering_luminance", be32(b, 20).map(|v| v.to_string()), be32(b, 20).map_or(String::new(), |v| format!("{:.4} nits", v as f64 / 10000.0)));
        }
        "clli" => {
            add("max_content_light_level", be16(b, 0).map(|v| v.to_string()), "nits: MaxCLL".to_string());
            add("max_pic_average_light_level", be16(b, 2).map(|v| v.to_string()), "nits: MaxFALL".to_string());
        }
        "hvcC" => {
            let profile = b.get(1).map(|v| v & 31);
            add("general_profile_idc", profile.map(|v| v.to_string()), match profile {
                Some(1) => "Main".into(),
                Some(2) => "Main 10".into(),
                Some(4) => "Range extensions".into(),
                _ => String::new(),
            });
            add("general_level_idc", b.get(12).map(|v| v.to_string()), b.get(12).map_or(String::new(), |v| format!("level {:.1}", f64::from(*v) / 30.0)));
            add("bitDepthLumaMinus8", b.get(17).map(|v| (v & 7).to_string()), b.get(17).map_or(String::new(), |v| format!("{}-bit", (v & 7) + 8)));
        }
        "avcC" => {
            add("AVCProfileIndication", b.get(1).map(|v| v.to_string()), match b.get(1) {
                Some(66) => "Baseline".into(),
                Some(77) => "Main".into(),
                Some(100) => "High".into(),
                Some(110) => "High 10".into(),
                _ => String::new(),
            });
            add("AVCLevelIndication", b.get(3).map(|v| v.to_string()), b.get(3).map_or(String::new(), |v| format!("level {:.1}", f64::from(*v) / 10.0)));
        }
        "dvcC" | "dvvC" => {
            add("dv_profile", b.get(2).map(|v| (v >> 1).to_string()), String::new());
            add("dv_level", b.get(2).zip(b.get(3)).map(|(a, c)| (((a & 1) << 5) | (c >> 3)).to_string()), String::new());
            add("rpu_present_flag", b.get(3).map(|v| ((v >> 2) & 1).to_string()), String::new());
            add("el_present_flag", b.get(3).map(|v| ((v >> 1) & 1).to_string()), String::new());
            add("bl_present_flag", b.get(3).map(|v| (v & 1).to_string()), String::new());
            add("dv_bl_signal_compatibility_id", b.get(4).map(|v| (v >> 4).to_string()), match b.get(4).map(|v| v >> 4) {
                Some(0) => "none".into(),
                Some(1) => "HDR10".into(),
                Some(2) => "SDR".into(),
                Some(4) => "HLG".into(),
                _ => String::new(),
            });
        }
        "hvc1" | "hev1" | "avc1" | "avc3" | "av01" | "vp09" => {
            add("width", be16(b, 24).map(|v| v.to_string()), "pixels".to_string());
            add("height", be16(b, 26).map(|v| v.to_string()), "pixels".to_string());
        }
        "mp4a" | "Opus" => {
            add("channelcount", be16(b, 16).map(|v| v.to_string()), String::new());
            add("samplerate", be32(b, 24).map(|v| (v >> 16).to_string()), "Hz".to_string());
        }
        "pasp" => {
            add("hSpacing", be32(b, 0).map(|v| v.to_string()), String::new());
            add("vSpacing", be32(b, 4).map(|v| v.to_string()), String::new());
        }
        "stsz" => {
            add("sample_count", be32(b, 8).map(|v| v.to_string()), "frames or samples".to_string());
        }
        "stss" => add("entry_count", be32(b, 4).map(|v| v.to_string()), "keyframes".to_string()),
        "elst" => add("entry_count", be32(b, 4).map(|v| v.to_string()), "edits".to_string()),
        _ => {}
    }
    fields
}

// ---- Matroska -------------------------------------------------------

/// A variable-length integer of EBML: its value and length. With `id`, the
/// marker bit is kept, as element ids are written.
fn vint(b: &[u8], id: bool) -> Option<(u64, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || b.len() < len {
        return None;
    }
    let mut value = if id { u64::from(first) } else { u64::from(first) & ((1u64 << (8 - len)) - 1) };
    for byte in &b[1..len] {
        value = value << 8 | u64::from(*byte);
    }
    // All ones: a size not given, as a live stream writes.
    if !id && value == (1u64 << (7 * len)) - 1 {
        value = u64::MAX;
    }
    Some((value, len))
}

/// What a Matroska element is called, how its value is stored, and
/// whether it holds others. The kind: m master, u unsigned, f float,
/// s string, b binary.
fn ebml_element(id: u64) -> (&'static str, char) {
    match id {
        0x1A45DFA3 => ("EBML", 'm'),
        0x4282 => ("DocType", 's'),
        0x4287 => ("DocTypeVersion", 'u'),
        0x18538067 => ("Segment", 'm'),
        0x114D9B74 => ("SeekHead", 'm'),
        0x4DBB => ("Seek", 'm'),
        0x1549A966 => ("Info", 'm'),
        0x2AD7B1 => ("TimestampScale", 'u'),
        0x4489 => ("Duration", 'f'),
        0x7BA9 => ("Title", 's'),
        0x4D80 => ("MuxingApp", 's'),
        0x5741 => ("WritingApp", 's'),
        0x4461 => ("DateUTC", 'b'),
        0x1654AE6B => ("Tracks", 'm'),
        0xAE => ("TrackEntry", 'm'),
        0xD7 => ("TrackNumber", 'u'),
        0x83 => ("TrackType", 'u'),
        0x86 => ("CodecID", 's'),
        0x63A2 => ("CodecPrivate", 'b'),
        0x22B59C => ("Language", 's'),
        0x22B59D => ("LanguageBCP47", 's'),
        0x536E => ("Name", 's'),
        0x88 => ("FlagDefault", 'u'),
        0x55AA => ("FlagForced", 'u'),
        0x9C => ("FlagLacing", 'u'),
        0x23E383 => ("DefaultDuration", 'u'),
        0xE0 => ("Video", 'm'),
        0xB0 => ("PixelWidth", 'u'),
        0xBA => ("PixelHeight", 'u'),
        0x54B0 => ("DisplayWidth", 'u'),
        0x54BA => ("DisplayHeight", 'u'),
        0x55B0 => ("Colour", 'm'),
        0x55B1 => ("MatrixCoefficients", 'u'),
        0x55B2 => ("BitsPerChannel", 'u'),
        0x55B9 => ("Range", 'u'),
        0x55BA => ("TransferCharacteristics", 'u'),
        0x55BB => ("Primaries", 'u'),
        0x55BC => ("MaxCLL", 'u'),
        0x55BD => ("MaxFALL", 'u'),
        0x55D0 => ("MasteringMetadata", 'm'),
        0x55D9 => ("LuminanceMax", 'f'),
        0x55DA => ("LuminanceMin", 'f'),
        0xE1 => ("Audio", 'm'),
        0xB5 => ("SamplingFrequency", 'f'),
        0x9F => ("Channels", 'u'),
        0x6264 => ("BitDepth", 'u'),
        0x41E4 => ("BlockAdditionMapping", 'm'),
        0x41E7 => ("BlockAddIDType", 'u'),
        0x41ED => ("BlockAddIDExtraData", 'b'),
        0x1F43B675 => ("Cluster", 'm'),
        0x1C53BB6B => ("Cues", 'm'),
        0x1043A770 => ("Chapters", 'm'),
        0x45B9 => ("EditionEntry", 'm'),
        0xB6 => ("ChapterAtom", 'm'),
        0x91 => ("ChapterTimeStart", 'u'),
        0x92 => ("ChapterTimeEnd", 'u'),
        0x80 => ("ChapterDisplay", 'm'),
        0x85 => ("ChapString", 's'),
        0x1254C367 => ("Tags", 'm'),
        0x7373 => ("Tag", 'm'),
        0x67C8 => ("SimpleTag", 'm'),
        0x45A3 => ("TagName", 's'),
        0x4487 => ("TagString", 's'),
        0x1941A469 => ("Attachments", 'm'),
        0x61A7 => ("AttachedFile", 'm'),
        0x466E => ("FileName", 's'),
        0x4660 => ("FileMediaType", 's'),
        0xEC => ("Void", 'b'),
        0xBF => ("CRC-32", 'b'),
        0x73C5 => ("TrackUID", 'u'),
        0x73A4 => ("SegmentUUID", 'b'),
        0x55EE => ("MaxBlockAdditionID", 'u'),
        0x55AB => ("FlagHearingImpaired", 'u'),
        0x9A => ("FlagInterlaced", 'u'),
        0x55B7 => ("ChromaSitingHorz", 'u'),
        0x55B8 => ("ChromaSitingVert", 'u'),
        0x55D1 => ("PrimaryRChromaticityX", 'f'),
        0x55D2 => ("PrimaryRChromaticityY", 'f'),
        0x55D3 => ("PrimaryGChromaticityX", 'f'),
        0x55D4 => ("PrimaryGChromaticityY", 'f'),
        0x55D5 => ("PrimaryBChromaticityX", 'f'),
        0x55D6 => ("PrimaryBChromaticityY", 'f'),
        0x55D7 => ("WhitePointChromaticityX", 'f'),
        0x55D8 => ("WhitePointChromaticityY", 'f'),
        0xE7 => ("Timestamp", 'u'),
        0xA3 => ("SimpleBlock", 'b'),
        0xA0 => ("BlockGroup", 'm'),
        0xBB => ("CuePoint", 'm'),
        0xB3 => ("CueTime", 'u'),
        0xB7 => ("CueTrackPositions", 'm'),
        0xF7 => ("CueTrack", 'u'),
        0xF1 => ("CueClusterPosition", 'u'),
        0x53AB => ("SeekID", 'b'),
        0x53AC => ("SeekPosition", 'u'),
        0x63C0 => ("Targets", 'm'),
        0x68CA => ("TargetTypeValue", 'u'),
        0x63C5 => ("TagTrackUID", 'u'),
        0x447A => ("TagLanguage", 's'),
        0x4484 => ("TagDefault", 'u'),
        0x6D80 => ("ContentEncodings", 'm'),
        0x73C4 => ("ChapterUID", 'u'),
        0x437C => ("ChapLanguage", 's'),
        _ => ("", 'b'),
    }
}

fn ebml(file: &mut File, start: u64, end: u64, depth: usize, count: &mut usize) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut at = start;
    while at < end && *count < MOST {
        let head = bytes_at(file, at, 16);
        let Some((id, id_len)) = vint(&head, true) else { break };
        let Some((size, size_len)) = head.get(id_len..).and_then(|rest| vint(rest, false)) else { break };
        let header = (id_len + size_len) as u64;
        let size = if size == u64::MAX { end - at - header } else { size };
        let Some(stop) = (at + header).checked_add(size).filter(|stop| *stop <= end) else { break };

        let (name, kind) = ebml_element(id);
        let name = if name.is_empty() { format!("0x{id:X}") } else { name.to_string() };
        let mut node = Node { name: name.clone(), about: "", offset: at, size: header + size, bytes: bytes_at(file, at, SHOWN_BYTES.min((header + size) as usize)), ..Node::default() };
        if kind == 'm' && depth < 10 && name != "Cluster" {
            node.children = ebml(file, at + header, stop, depth + 1, count);
        } else if size <= 256 {
            let body = bytes_at(file, at + header, size as usize);
            let value = match kind {
                'u' => Some(body.iter().fold(0u64, |v, b| v << 8 | u64::from(*b)).to_string()),
                'f' if body.len() == 4 => Some(f32::from_be_bytes(body[..4].try_into().unwrap_or_default()).to_string()),
                'f' if body.len() == 8 => Some(f64::from_be_bytes(body[..8].try_into().unwrap_or_default()).to_string()),
                's' => Some(String::from_utf8_lossy(&body).trim_end_matches('\0').to_string()),
                _ => None,
            };
            let means = match (name.as_str(), value.as_deref().and_then(|v| v.parse::<u64>().ok())) {
                ("TrackType", Some(1)) => "video".to_string(),
                ("TrackType", Some(2)) => "audio".to_string(),
                ("TrackType", Some(17)) => "subtitles".to_string(),
                ("Primaries", Some(code)) => primaries_name(code).to_string(),
                ("TransferCharacteristics", Some(code)) => transfer_name(code).to_string(),
                ("MatrixCoefficients", Some(code)) => matrix_name(code).to_string(),
                ("Range", Some(1)) => "Limited (16–235)".to_string(),
                ("Range", Some(2)) => "Full (0–255)".to_string(),
                ("MaxCLL" | "MaxFALL", Some(_)) => "nits".to_string(),
                ("FlagDefault" | "FlagForced", Some(flag)) => if flag == 1 { "yes" } else { "no" }.to_string(),
                ("TimestampScale", Some(ns)) => format!("{ns} ns a tick"),
                _ => String::new(),
            };
            if let Some(value) = value {
                node.fields.push((name.clone(), value, means));
            }
        }
        if !fold(&mut nodes, &node) {
            *count += 1;
            nodes.push(node);
        }
        if size == 0 && header == 0 {
            break;
        }
        at = stop;
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ebml_numbers_read_as_written() {
        assert_eq!(vint(&[0x81], false), Some((1, 1)));
        assert_eq!(vint(&[0x40, 0x02], false), Some((2, 2)));
        assert_eq!(vint(&[0x1A, 0x45, 0xDF, 0xA3], true), Some((0x1A45DFA3, 4)));
        assert_eq!(vint(&[0xFF], false), Some((u64::MAX, 1)));
    }

    #[test]
    fn a_colr_box_is_decoded() {
        let body = [b'n', b'c', b'l', b'x', 0, 9, 0, 16, 0, 9, 0];
        let fields = mp4_fields("colr", &body);
        assert!(fields.iter().any(|(name, value, means)| name == "transfer_characteristics" && value == "16" && means.starts_with("PQ")));
    }
}

/// A box of the same kind as the two before it is not listed: it starts,
/// or joins, one line for the rest of the run, "Cluster, 1,234 more",
/// since a long video's frames come in thousands of Clusters or
/// fragments. Whether it was taken so.
fn fold(nodes: &mut Vec<Node>, node: &Node) -> bool {
    let len = nodes.len();
    if let Some(last) = nodes.last_mut().filter(|last| last.more > 0 && last.name == node.name) {
        last.more += 1;
        last.size += node.size;
        return true;
    }
    if len >= 2 && nodes[len - 1].name == node.name && nodes[len - 2].name == node.name {
        nodes.push(Node { name: node.name.clone(), about: "more of the same, not listed", offset: node.offset, size: node.size, more: 1, ..Node::default() });
        return true;
    }
    false
}
