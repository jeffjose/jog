//! What the words Inspect shows mean: the containers, codecs, profiles,
//! color terms and kinds of HDR, as a line or two each, for the tooltips
//! over the Overview's boxes.

use super::*;

/// What a term is, in the box it is in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Role {
    Container,
    Stream,
    Codec,
    Profile,
    Size,
    Channels,
    Primaries,
    Transfer,
    Matrix,
    Pixels,
    Hdr,
}

impl Role {
    /// The role's name, as the tooltip's line has it before the term.
    fn name(self) -> &'static str {
        match self {
            Role::Container => "Container",
            Role::Stream => "Stream",
            Role::Codec => "Codec",
            Role::Profile => "Profile",
            Role::Size => "Size",
            Role::Channels => "Channels",
            Role::Primaries => "Primaries",
            Role::Transfer => "Transfer",
            Role::Matrix => "Matrix",
            Role::Pixels => "Pixels",
            Role::Hdr => "HDR",
        }
    }

    /// What the role is, whatever the term.
    fn about(self) -> &'static str {
        match self {
            Role::Container => "The wrapper: holds the streams in step, with their timing and tags.",
            Role::Stream => "One track of the file. A file can carry several, of video, sound and subtitles.",
            Role::Codec => "How the stream is compressed; the player needs a decoder for it.",
            Role::Profile => "Which of the codec's tools the stream may use; players say which they decode.",
            Role::Size => "Width × height, in pixels.",
            Role::Channels => "2 is stereo, 6 is 5.1, 8 is 7.1.",
            Role::Primaries => "The gamut: the reddest red, greenest green and bluest blue it can name.",
            Role::Transfer => "The curve from stored values to light.",
            Role::Matrix => "How RGB became brightness (Y) and color difference (U, V).",
            Role::Pixels => "How each pixel is stored.",
            Role::Hdr => "The kind of HDR, and the metadata that tells a screen how to show it.",
        }
    }
}

/// What `term` means as a `role`, if it is one jog knows.
pub(super) fn explain(role: Role, term: &str) -> Option<String> {
    let lower = term.to_ascii_lowercase();
    let said = match role {
        Role::Container => match lower.as_str() {
            l if l.contains("quicktime") || l.contains("mp4") || l.contains("mov") => "MP4 and MOV are one family, from Apple's QuickTime: what phones and cameras write.",
            l if l.contains("matroska") || l.contains("webm") => "Matroska: open, and holds almost any codec, subtitles and attachments. WebM is a strict subset for the web.",
            l if l.contains("mpeg-ts") || l.contains("mpegts") || l.contains("transport") => "MPEG transport stream: for broadcast, in small packets that survive loss.",
            l if l.contains("avi") => "AVI: Microsoft's old container, with little room for metadata.",
            _ => return None,
        },
        Role::Stream => match lower.as_str() {
            "video" => "The pictures. A file usually has one.",
            "audio" => "One soundtrack. Several mean languages, commentary or mixes.",
            "subtitle" => "Text or pictures laid over the video.",
            "data" => "Timing or metadata, not shown.",
            "attachment" => "A font or picture carried along, often for styled subtitles.",
            _ => return None,
        },
        Role::Codec => match lower.as_str() {
            "av1" => "AV1: open and royalty-free, from AOMedia (2018). About a third smaller than HEVC for the same look; slow to encode.",
            "hevc" | "h265" => "HEVC (H.265): from 2013, about half the size of H.264. Holds 10-bit and HDR; patents keep it out of some browsers.",
            "h264" => "H.264 (AVC): from 2003, and still what plays everywhere. 8-bit, as a rule, so no HDR.",
            "vp9" => "VP9: Google's open codec, YouTube's mainstay before AV1.",
            "vp8" => "VP8: VP9's forerunner, from early WebM.",
            "mpeg2video" => "MPEG-2: DVD's and old broadcast's video.",
            "mpeg4" => "MPEG-4 Part 2: DivX and Xvid's era.",
            "prores" => "ProRes: Apple's codec for editing: large, light to decode, near lossless.",
            "aac" => "AAC: MP4's usual sound; plays everywhere.",
            "opus" => "Opus: open, and the best lossy sound codec for its size; WebM's and MKV's usual.",
            "mp3" => "MP3: the old standby; larger than AAC or Opus for the same sound.",
            "ac3" => "Dolby Digital (AC-3): 5.1 surround from DVDs and broadcast.",
            "eac3" => "Dolby Digital Plus: AC-3's successor; streaming's surround, and Atmos rides on it.",
            "truehd" => "Dolby TrueHD: lossless surround from Blu-ray; Atmos rides on it.",
            "dts" => "DTS: surround from DVD and Blu-ray.",
            "flac" => "FLAC: lossless sound, about half the size of raw.",
            "alac" => "ALAC: Apple's lossless sound.",
            l if l.starts_with("pcm_") => "PCM: raw, uncompressed sound.",
            "subrip" | "srt" => "SubRip (SRT): plain timed text.",
            "ass" | "ssa" => "ASS: styled subtitles, with fonts and placement.",
            "webvtt" => "WebVTT: the web's timed text.",
            "mov_text" => "MP4's own timed text.",
            "hdmv_pgs_subtitle" => "PGS: Blu-ray's subtitles, as pictures.",
            "dvd_subtitle" => "DVD's subtitles, as pictures.",
            "mjpeg" => "Motion JPEG: every frame a JPEG; large, simple.",
            "png" => "PNG: lossless pictures; often cover art.",
            _ => return None,
        },
        Role::Profile => match lower.as_str() {
            "main" => "Main: the everyday profile; 8-bit 4:2:0 (AV1's Main takes 10-bit too).",
            "main 10" => "Main 10: HEVC's 10-bit profile, which HDR needs.",
            "main still picture" => "Main Still Picture: a single frame, as HEIC photos are.",
            "high" => "High: H.264's usual, Blu-ray's and the web's.",
            "high 10" => "High 10: H.264 in 10 bits; few players decode it in hardware.",
            "baseline" | "constrained baseline" => "Baseline: H.264's simplest, for old phones and video calls.",
            "lc" => "Low Complexity: AAC's usual profile.",
            "he-aac" | "he-aacv2" => "High Efficiency AAC: for low bitrates, as radio streams.",
            "rext" => "Range extensions: HEVC past 4:2:0 or 10 bits; for production, not playback.",
            "professional" => "AV1's Professional: 4:2:2 or 12 bits.",
            _ => return None,
        },
        Role::Primaries => match lower.as_str() {
            "bt709" => "BT.709: HD's colors, the same as sRGB's.",
            "bt2020" => "BT.2020: the wide gamut of UHD and HDR. Screens show about P3 of it.",
            "smpte432" => "Display P3: the gamut of recent phones and Macs.",
            "smpte431" => "DCI-P3: cinema's gamut.",
            "bt470bg" | "smpte170m" => "SD video's colors.",
            _ => return None,
        },
        Role::Transfer => match lower.as_str() {
            "bt709" | "bt1886" => "BT.709: SDR's gamma curve.",
            "smpte2084" => "PQ (SMPTE 2084): HDR's curve; each value is a fixed brightness, up to 10,000 nits.",
            "arib-std-b67" => "HLG: HDR that looks right on SDR screens too; broadcast's choice, and phones'.",
            "iec61966-2-1" => "sRGB's curve.",
            "linear" => "Linear: values in proportion to light.",
            "smpte170m" | "bt470bg" => "SD video's gamma curve.",
            _ => return None,
        },
        Role::Matrix => match lower.as_str() {
            "bt709" => "BT.709's weights, HD's.",
            "bt2020nc" => "BT.2020, non-constant luminance: the usual for UHD and HDR.",
            "bt2020c" => "BT.2020, constant luminance: rare.",
            "smpte170m" | "bt470bg" => "SD's weights.",
            "gbr" | "rgb" => "None: stored as RGB.",
            "ictcp" => "ICtCp: a matrix made for HDR, Dolby Vision's.",
            _ => return None,
        },
        Role::Pixels => return pixels(&lower),
        Role::Hdr => match term {
            "HDR10" => "HDR10: PQ with one set of figures for the whole video: its brightest pixel and the screen it was mastered on.",
            "HDR10+" => "HDR10+: PQ with figures for each scene (SMPTE 2094-40), so a dimmer screen can map each one well.",
            "2094-50" => "SMPTE 2094-50: curves for each scene saying how to adapt to any screen's headroom; carried in the bitstream.",
            "PQ" => "PQ with no metadata: HDR, but the screen has to guess how to map it.",
            "HLG" => "HLG: HDR without metadata, made to look right on SDR screens too.",
            "BT.2020" => "BT.2020: wide color.",
            "Dolby Vision" => "Dolby Vision: Dolby's HDR, with figures for each scene.",
            _ => return None,
        },
        Role::Size | Role::Channels => return None,
    };
    Some(said.to_string())
}

/// A pixel format, read: yuv420p10le is YUV, color at half width and
/// height, 10 bits, little-endian.
fn pixels(format: &str) -> Option<String> {
    let (model, rest) = ["yuva", "yuvj", "yuv", "gbrp", "rgb", "gray", "nv12", "p010"].iter().find_map(|model| format.strip_prefix(model).map(|rest| (*model, rest)))?;
    let shared = match model {
        "nv12" => return Some("NV12: YUV 4:2:0 in 8 bits, as hardware decoders hand it over.".into()),
        "p010" => return Some("P010: YUV 4:2:0 in 10 bits, as hardware decoders hand it over.".into()),
        "gbrp" | "rgb" => "RGB, every pixel its own color",
        "gray" => "brightness only",
        _ => match rest.get(..3) {
            Some("420") => "YUV, color at half the width and height (4:2:0), as nearly all video",
            Some("422") => "YUV, color at half the width (4:2:2), as production video",
            Some("444") => "YUV, color for every pixel (4:4:4)",
            _ => "YUV",
        },
    };
    let digits: String = rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches('p').chars().take_while(char::is_ascii_digit).collect();
    let bits = if digits.is_empty() { "8".to_string() } else { digits };
    let full = if model == "yuvj" { ", full range" } else { "" };
    Some(format!("{shared}; {bits} bits{full}."))
}

/// A tooltip over a place of the panel: a title, then each term with
/// what it is.
#[derive(Clone, Debug)]
pub(super) struct Tip {
    pub place: [i32; 4],
    pub title: String,
    /// Each a heading and what it means.
    pub lines: Vec<(String, String)>,
}

impl Tip {
    /// A tooltip of terms, each in its role: the role's name and the term
    /// as the heading, and what the term means, or the role does.
    pub fn new(place: [i32; 4], title: &str, terms: Vec<(Role, String)>) -> Tip {
        let lines = terms.into_iter().map(|(role, term)| (format!("{} · {term}", role.name()), explain(role, &term).unwrap_or_else(|| role.about().to_string()))).collect();
        Tip { place, title: title.to_string(), lines }
    }
}

/// A tooltip for a fact as Inspect lists it, a label and its value: what
/// the label is, and what this value of it means where jog knows.
pub(super) fn fact_tip(place: [i32; 4], label: &str, value: &str) -> Option<Tip> {
    let lower = value.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    // A role and the term the value is, as the glossary names it.
    let term = |role: Role, term: Option<&str>| -> Option<(Role, String)> { Some((role, term?.to_string())) };
    let (about, said): (&str, Option<(Role, String)>) = match label {
        "Codec" => {
            let named = [("av1", "av1"), ("hevc", "hevc"), ("h.265", "hevc"), ("h.264", "h264"), ("avc", "h264"), ("vp9", "vp9"), ("vp8", "vp8"), ("prores", "prores"), ("aac", "aac"), ("opus", "opus"), ("e-ac-3", "eac3"), ("eac3", "eac3"), ("ac-3", "ac3"), ("truehd", "truehd"), ("dts", "dts"), ("flac", "flac"), ("mp3", "mp3"), ("subrip", "subrip"), ("pgs", "hdmv_pgs_subtitle")];
            ("How the stream is compressed.", term(Role::Codec, named.iter().find(|(word, _)| lower.contains(word)).map(|(_, name)| *name)))
        }
        "Profile" => ("Which of the codec's tools the stream may use.", term(Role::Profile, Some(&lower))),
        "Level" => ("The most a decoder must keep up with: the size, the frame rate and the bitrate. Roughly 4 is 1080p, 5 is 4K at 30, 5.1 4K at 60.", None),
        "Transfer" => {
            let named = if has(&["2084", "pq"]) { Some("smpte2084") } else if has(&["hlg", "b67"]) { Some("arib-std-b67") } else if has(&["srgb", "61966"]) { Some("iec61966-2-1") } else if has(&["709", "1886"]) { Some("bt709") } else { None };
            (Role::Transfer.about(), term(Role::Transfer, named))
        }
        "Primaries" => {
            let named = if has(&["2020"]) { Some("bt2020") } else if has(&["display p3", "432"]) { Some("smpte432") } else if has(&["dci", "431"]) { Some("smpte431") } else if has(&["709"]) { Some("bt709") } else { None };
            (Role::Primaries.about(), term(Role::Primaries, named))
        }
        "Matrix" => {
            let named = if has(&["2020"]) && has(&["non"]) { Some("bt2020nc") } else if has(&["2020"]) { Some("bt2020c") } else if has(&["709"]) { Some("bt709") } else if has(&["rgb", "gbr"]) { Some("rgb") } else { None };
            (Role::Matrix.about(), term(Role::Matrix, named))
        }
        "Pixels" => (Role::Pixels.about(), term(Role::Pixels, Some(&lower))),
        "Range" => ("Limited: black at 16 and white at 235 of 255, as video is stored; full: 0 to 255, as pictures are. One taken for the other looks washed out, or loses its shadows.", None),
        "Bit depth" => ("Bits for each channel. 8 bits band in smooth gradients like skies; HDR needs 10.", None),
        "Chroma siting" => ("Where each color sample sits among the brightness samples it is shared by. Guessed wrong, color is off by half a pixel at edges.", None),
        "Scan" => ("Progressive: whole frames. Interlaced: alternate lines from moments apart, as old TV was, combed on a modern screen unless deinterlaced.", None),
        "Aspect" => ("The shape it is shown at. Where it differs from the pixels', they are not square and are stretched to it.", None),
        "Codec tag" => ("The four letters the container names the codec by. For HEVC in MP4, Apple's players want hvc1, not hev1.", None),
        "Bitrate" => ("How much data a second, on average: more is larger and, for the same codec and settings, closer to the original.", None),
        "Frame rate" => ("Frames a second. 23.976 is film on video, 29.97 and 59.94 are NTSC video's, 25 and 50 PAL's.", None),
        "Sample rate" => ("Sound samples a second: 48 kHz is video's, 44.1 kHz CD's.", None),
        "Channels" => ("How many sound channels: 2 is stereo, 5.1 is six, the .1 being the bass.", None),
        "Sample format" => ("How each sound sample is stored: fltp is floating point, a channel at a time; s16 is 16-bit integers.", None),
        "Language" => ("The language the file says the track is in, as a player picks tracks by.", None),
        _ => return None,
    };
    let mut lines = vec![(label.to_string(), about.to_string())];
    if let Some((role, term)) = said {
        if let Some(meaning) = explain(role, &term) {
            lines.push((value.to_string(), meaning));
        }
    }
    Some(Tip { place, title: format!("{label} · {value}"), lines })
}

/// Words broken into lines no wider than `width`.
pub(super) fn wrap(text: &mut Text, words: &str, width: i32, px: u32) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in words.split_whitespace() {
        let line = lines.last_mut().expect("one line at least");
        let tried = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if text.width(&tried, px) <= width || line.is_empty() {
            *line = tried;
        } else {
            lines.push(word.to_string());
        }
    }
    lines
}

/// Draws the innermost of `tips` under `pointer` (in `sub`'s terms) on
/// `sub`, beside the pointer and kept inside.
pub(super) fn draw_tip(sub: &mut Canvas, text: &mut Text, px: u32, tips: &[Tip], pointer: (i32, i32)) {
    let inside = |[x, y, w, h]: [i32; 4]| (x..x + w).contains(&pointer.0) && (y..y + h).contains(&pointer.1);
    // Drawn last is innermost.
    let Some(tip) = tips.iter().rev().find(|tip| inside(tip.place)) else { return };
    let pad = px as i32;
    let small = px;
    let (line, small_line) = (Text::line_height(px), Text::line_height(small));
    // Each term, then what it is: what the term means if jog knows, else
    // what its role is. Made wider until it fits the height there is, as
    // in a short dock along the bottom.
    let lay = |text: &mut Text, width: i32| {
        let mut rows: Vec<(String, u32, u32)> = vec![(tip.title.clone(), px, TEXT_COLOR)];
        for (heading, said) in &tip.lines {
            rows.push((heading.clone(), small, accent_soft()));
            for piece in wrap(text, said, width - pad * 2, small) {
                rows.push((piece, small, DIM_TEXT_COLOR));
            }
        }
        let height = pad + rows.iter().map(|(_, size, _)| if *size == px { line } else { small_line }).sum::<i32>() + pad / 3 * (tip.lines.len() as i32) + pad * 2 / 3;
        (rows, height)
    };
    let widest = sub.width - pad;
    let mut width = (px as i32 * 24).min(widest);
    let (mut rows, mut height) = lay(text, width);
    while height > sub.height - pad && width < widest {
        width = (width + px as i32 * 8).min(widest);
        (rows, height) = lay(text, width);
    }
    // Below and to the right of the pointer, or wherever it fits.
    let mut x = pointer.0 + pad;
    let mut y = pointer.1 + pad;
    if x + width > sub.width - pad / 2 {
        x = (pointer.0 - pad - width).max(pad / 2);
    }
    if y + height > sub.height - pad / 2 {
        y = (pointer.1 - pad - height).max(pad / 2);
    }
    sub.round_fill(x + 2, y + 3, width, height, pad / 2, 0x000000, 96);
    sub.round_fill(x, y, width, height, pad / 2, 0x24252C, 256);
    sub.round_outline(x, y, width, height, pad / 2, 1, MENU_EDGE_COLOR);
    let mut ty = y + pad * 2 / 3;
    for (at, (words, size, color)) in rows.iter().enumerate() {
        // A little room before each term.
        if at > 0 && *color == accent_soft() {
            ty += pad / 3;
        }
        text.draw(sub, x + pad, ty, *size, *color, words);
        ty += if *size == px { line } else { small_line };
    }
}
