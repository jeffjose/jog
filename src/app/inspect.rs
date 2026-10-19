//! The inspector (F3): what a video is made of, in tabs. The summary of
//! the file, each of its streams, its HDR, and its tags and chapters, as
//! ffprobe reads them, with jog's own reading of the HDR metadata that
//! changes scene by scene. Any value is copied with a click, the lot as
//! JSON with c.

use serde_json::Value;
use winit::keyboard::{Key, NamedKey};

use super::*;

/// What was found in a file.
pub struct Inspection {
    probe: Value,
    hdr: Option<crate::hdrmeta::Info>,
    /// The first video frame's side data: what the bitstream carries
    /// that the container may not, HDR10 values written by the encoder.
    first_frame: Value,
    /// Its boxes or elements.
    tree: Result<Vec<crate::boxes::Node>, String>,
}

impl Inspection {
    /// Reads a file with ffprobe, and its HDR metadata with jog's reader.
    pub fn read(path: &Path) -> Result<Inspection, String> {
        let output = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "-show_chapters"])
            .arg(path)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("ffprobe did not run: {error}"))?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            return Err(said.lines().last().unwrap_or("ffprobe failed").to_string());
        }
        let probe: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
        let first_frame = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_frames", "-read_intervals", "%+#1", "-show_entries", "frame=side_data_list", "-print_format", "json"])
            .arg(path)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|output| serde_json::from_slice(&output.stdout).ok())
            .unwrap_or(Value::Null);
        Ok(Inspection { probe, hdr: crate::hdrmeta::read(path), first_frame, tree: crate::boxes::read(path) })
    }

    fn streams(&self) -> &[Value] {
        self.probe["streams"].as_array().map_or(&[], Vec::as_slice)
    }

    fn video(&self) -> Option<&Value> {
        self.streams().iter().find(|stream| stream["codec_type"] == "video" && stream["disposition"]["attached_pic"] != 1)
    }

    /// One side data entry of the video stream, by its type.
    fn side(&self, kind: &str) -> Option<&Value> {
        self.side_from(kind).map(|(side, _)| side)
    }

    /// One side data entry by its type, from the container if it has it,
    /// else from the first frame's own bitstream; and which it came from.
    fn side_from(&self, kind: &str) -> Option<(&Value, &'static str)> {
        fn find<'a>(list: &'a Value, kind: &str) -> Option<&'a Value> {
            list.as_array()?.iter().find(|side| side["side_data_type"] == kind)
        }
        let container = self.video().and_then(|video| find(&video["side_data_list"], kind));
        container.map(|side| (side, "container")).or_else(|| find(&self.first_frame["frames"][0]["side_data_list"], kind).map(|side| (side, "video bitstream (SEI)")))
    }

    /// Everything as JSON, ffprobe's reading with jog's HDR beside it.
    fn json(&self) -> String {
        let mut all = self.probe.clone();
        if let Some(hdr) = &self.hdr {
            let scenes: Vec<Value> = hdr
                .hdr10plus
                .iter()
                .map(|timed| serde_json::json!({ "at": timed.at, "maxscl": timed.value.maxscl, "average": timed.value.average, "target": timed.value.target }))
                .collect();
            all["jog_hdr"] = serde_json::json!({ "badges": hdr.badges(), "mastering": hdr.mastering, "light": hdr.light, "hdr10plus_scenes": scenes, "carried": hdr.carried });
        }
        serde_json::to_string_pretty(&all).unwrap_or_default()
    }
}

/// The tabs, in order.
const TABS: [&str; 5] = ["Summary", "Streams", "HDR", "Structure", "Tags · chapters"];
/// The tab of the boxes or elements the file is made of.
const STRUCTURE: usize = 3;

/// A card of label and value rows, under a heading.
struct Card {
    title: String,
    badges: Vec<String>,
    rows: Vec<(String, String)>,
}

impl Card {
    fn new(title: impl Into<String>) -> Card {
        Card { title: title.into(), badges: Vec::new(), rows: Vec::new() }
    }

    /// A row, when there is something to say.
    fn row(&mut self, label: &str, value: Option<String>) {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            self.rows.push((label.to_string(), value));
        }
    }
}

/// What a click on the inspector lands on.
#[derive(Clone, PartialEq, Debug)]
enum Hit {
    Tab(usize),
    Copy(String),
    CopyAll,
    Close,
    /// A scene of the HDR chart, or a chapter: that time in the video.
    Seek(f64),
    /// A box or element of the Structure tab, by where it is in the tree.
    Node(Vec<usize>),
}

pub struct Inspector {
    id: usize,
    tab: usize,
    scroll: i32,
    /// How far down the content went, for scrolling.
    content_h: i32,
    view_h: i32,
    found: Option<Result<Inspection, String>>,
    /// The Structure tab: the boxes opened and the one picked out, each by
    /// where it is in the tree; and whether to scroll to the one picked.
    open: HashSet<Vec<usize>>,
    selected: Vec<usize>,
    follow: bool,
    drawn: Vec<(Hit, [i32; 4])>,
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A rational as ffprobe writes it, `30000/1001` or `10000000/10000`.
fn ratio(value: &Value) -> Option<f64> {
    let text = text_of(value)?;
    match text.split_once('/') {
        Some((a, b)) => {
            let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
            (b != 0.0).then(|| a / b)
        }
        None => text.parse().ok(),
    }
}

fn bitrate(value: &Value) -> Option<String> {
    let bits: f64 = text_of(value)?.parse().ok()?;
    Some(if bits >= 1e6 { format!("{:.1} Mb/s", bits / 1e6) } else { format!("{:.0} kb/s", bits / 1e3) })
}

/// The color terms ffprobe uses, as people know them.
fn color_words(value: &Value) -> Option<String> {
    let text = text_of(value)?;
    Some(
        match text.as_str() {
            "bt709" => "BT.709",
            "bt2020" => "BT.2020",
            "bt2020nc" => "BT.2020 non-constant",
            "bt2020c" => "BT.2020 constant",
            "smpte2084" => "PQ (SMPTE 2084)",
            "arib-std-b67" => "HLG (ARIB STD-B67)",
            "bt470bg" | "smpte170m" => "BT.601",
            "iec61966-2-1" => "sRGB",
            "smpte432" => "Display P3",
            "tv" => "Limited (16–235)",
            "pc" => "Full (0–255)",
            "unknown" => return None,
            other => other,
        }
        .to_string(),
    )
}

fn yes(value: &Value) -> bool {
    value == 1 || value == "1" || value == true
}

impl App {
    /// F3: the inspector, for the file on view.
    pub(super) fn open_inspector(&mut self) {
        let id = self.list[self.focus()];
        let path = self.files[id].clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = proxy.send_event(Event::Inspected(id, Inspection::read(&path)));
        });
        self.inspector = Some(Inspector { id, tab: 0, scroll: 0, content_h: 0, view_h: 0, found: None, open: HashSet::new(), selected: vec![0], follow: false, drawn: Vec::new() });
        self.menu = None;
        self.redraw();
    }

    pub(super) fn inspected(&mut self, id: usize, found: Result<Inspection, String>) {
        if let Some(inspector) = self.inspector.as_mut().filter(|inspector| inspector.id == id) {
            // The tree opens on what holds the tracks: moov, or Segment and
            // its Tracks.
            if let Some(Ok(nodes)) = found.as_ref().ok().map(|found| found.tree.as_ref()) {
                for (at, node) in nodes.iter().enumerate() {
                    if matches!(node.name.as_str(), "moov" | "Segment") {
                        inspector.open.insert(vec![at]);
                        if let Some(tracks) = node.children.iter().position(|child| child.name == "Tracks" || child.name == "trak") {
                            inspector.open.insert(vec![at, tracks]);
                        }
                    }
                }
            }
            inspector.found = Some(found);
            self.redraw();
        }
    }

    pub(super) fn inspector_key(&mut self, key: Key<&str>) {
        let (step, shift) = (Text::line_height(ui_px(self.window_size().1)) * 3, self.modifiers.shift_key());
        let Some(inspector) = &mut self.inspector else { return };
        // In the Structure tab the arrows go about the tree.
        if inspector.tab == STRUCTURE {
            let nodes = inspector.found.as_ref().and_then(|found| found.as_ref().ok()).and_then(|found| found.tree.as_ref().ok());
            let visible = nodes.map(|nodes| visible_nodes(nodes, &inspector.open)).unwrap_or_default();
            let at = visible.iter().position(|(path, _)| *path == inspector.selected);
            let has_children = |path: &[usize]| nodes.and_then(|nodes| node_at(nodes, path)).is_some_and(|node| !node.children.is_empty());
            let handled = match key {
                Key::Named(NamedKey::ArrowDown) => {
                    if let Some((path, _)) = visible.get(at.map_or(0, |at| (at + 1).min(visible.len() - 1))) {
                        inspector.selected = path.clone();
                    }
                    true
                }
                Key::Named(NamedKey::ArrowUp) => {
                    if let Some((path, _)) = visible.get(at.map_or(0, |at| at.saturating_sub(1))) {
                        inspector.selected = path.clone();
                    }
                    true
                }
                Key::Named(NamedKey::ArrowRight) => {
                    if has_children(&inspector.selected) {
                        inspector.open.insert(inspector.selected.clone());
                    }
                    true
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    // Closes it, or goes up to the one it is in.
                    if !inspector.open.remove(&inspector.selected) && inspector.selected.len() > 1 {
                        inspector.selected.pop();
                    }
                    true
                }
                Key::Named(NamedKey::Enter) => {
                    let path = inspector.selected.clone();
                    if !inspector.open.remove(&path) && has_children(&path) {
                        inspector.open.insert(path);
                    }
                    true
                }
                _ => false,
            };
            if handled {
                inspector.follow = true;
                return self.redraw();
            }
        }
        match key {
            Key::Named(NamedKey::Escape | NamedKey::F3) | Key::Character("q") => self.inspector = None,
            Key::Named(NamedKey::Tab | NamedKey::ArrowRight) if !shift => (inspector.tab, inspector.scroll) = ((inspector.tab + 1) % TABS.len(), 0),
            Key::Named(NamedKey::Tab | NamedKey::ArrowLeft) => (inspector.tab, inspector.scroll) = ((inspector.tab + TABS.len() - 1) % TABS.len(), 0),
            Key::Named(NamedKey::ArrowDown) => inspector.scroll += step,
            Key::Named(NamedKey::ArrowUp) => inspector.scroll -= step,
            Key::Named(NamedKey::PageDown) => inspector.scroll += inspector.view_h * 4 / 5,
            Key::Named(NamedKey::PageUp) => inspector.scroll -= inspector.view_h * 4 / 5,
            Key::Character(digit @ ("1" | "2" | "3" | "4" | "5")) => (inspector.tab, inspector.scroll) = (digit.parse::<usize>().unwrap_or(1) - 1, 0),
            Key::Character("c") => return self.inspector_hit(Hit::CopyAll),
            _ => return,
        }
        self.clamp_inspector_scroll();
        self.redraw();
    }

    fn clamp_inspector_scroll(&mut self) {
        if let Some(inspector) = &mut self.inspector {
            inspector.scroll = inspector.scroll.clamp(0, (inspector.content_h - inspector.view_h).max(0));
        }
    }

    pub(super) fn inspector_wheel(&mut self, lines: f64) {
        let step = Text::line_height(ui_px(self.window_size().1)) as f64 * 2.0;
        if let Some(inspector) = &mut self.inspector {
            inspector.scroll -= (lines * step) as i32;
        }
        self.clamp_inspector_scroll();
        self.redraw();
    }

    pub(super) fn inspector_click(&mut self, x: f64, y: f64) {
        let Some(inspector) = &self.inspector else { return };
        let on = |[left, top, w, h]: [i32; 4]| (left..left + w).contains(&(x as i32)) && (top..top + h).contains(&(y as i32));
        if let Some((hit, _)) = inspector.drawn.iter().rev().find(|(_, place)| on(*place)).cloned() {
            self.inspector_hit(hit);
        }
    }

    fn inspector_hit(&mut self, hit: Hit) {
        let copy = |app: &mut App, what: String, said: String| {
            println!("[copy] {what}");
            if app.snapshot.is_some() || ops::set_clipboard(&what) {
                app.toast(format!("Copied {said}"));
            } else {
                app.toast("No clipboard tool (xclip, xsel, wl-copy)");
            }
        };
        match hit {
            Hit::Tab(tab) => {
                if let Some(inspector) = &mut self.inspector {
                    (inspector.tab, inspector.scroll) = (tab, 0);
                }
            }
            Hit::Close => self.inspector = None,
            Hit::Copy(value) => {
                let said = if value.chars().count() > 40 { format!("{}…", value.chars().take(38).collect::<String>()) } else { value.clone() };
                copy(self, value, said);
            }
            Hit::CopyAll => {
                let json = self.inspector.as_ref().and_then(|inspector| inspector.found.as_ref()).and_then(|found| found.as_ref().ok()).map(Inspection::json);
                match json {
                    Some(json) => copy(self, json, "everything as JSON".to_string()),
                    None => self.toast("Still reading…"),
                }
            }
            Hit::Node(path) => {
                if let Some(inspector) = &mut self.inspector {
                    // A click on the one picked opens or closes it.
                    if inspector.selected == path && !inspector.open.remove(&path) {
                        inspector.open.insert(path);
                    } else {
                        inspector.selected = path;
                    }
                }
            }
            Hit::Seek(at) => {
                let viewing = self.inspector.as_ref().is_some_and(|inspector| Some(inspector.id) == self.video.as_ref().map(|video| video.id));
                if let (true, Some((_, length))) = (viewing, self.video_time()) {
                    self.inspector = None;
                    self.seek_to(at / length, true);
                    self.time_until = Some(Instant::now() + TOAST);
                }
            }
        }
        self.redraw();
    }

    /// The cards of a tab.
    fn inspector_cards(&self, found: &Inspection, tab: usize) -> Vec<Card> {
        let format = &found.probe["format"];
        let tags = |value: &Value, names: &[&str]| names.iter().find_map(|name| value["tags"].as_object()?.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).and_then(|(_, v)| text_of(v)));
        match tab {
            0 => {
                let mut file = Card::new("File");
                file.row("Container", text_of(&format["format_long_name"]));
                file.row("Length", text_of(&format["duration"]).and_then(|d| d.parse::<f64>().ok()).map(|d| format!("{}  ({d:.3} s)", clock(d))));
                file.row("Size", text_of(&format["size"]).and_then(|s| s.parse::<u64>().ok()).map(meta::human_size));
                file.row("Bitrate", bitrate(&format["bit_rate"]));
                let count = |kind: &str| found.streams().iter().filter(|s| s["codec_type"] == kind).count();
                file.row("Streams", Some(format!("{} video  ·  {} audio  ·  {} subtitles", count("video"), count("audio"), count("subtitle"))));
                file.row("Chapters", found.probe["chapters"].as_array().filter(|c| !c.is_empty()).map(|c| c.len().to_string()));
                let mut made = Card::new("Made");
                made.row("Title", tags(format, &["title"]));
                made.row("Made by", tags(format, &["encoder", "writing_application", "encoded_by"]));
                made.row("Created", tags(format, &["creation_time", "date"]));
                made.row("Brand", tags(format, &["major_brand"]).map(|brand| match tags(format, &["compatible_brands"]) {
                    Some(compatible) => format!("{brand}  ·  {compatible}"),
                    None => brand,
                }));
                vec![file, made]
            }
            1 => found
                .streams()
                .iter()
                .map(|s| {
                    let kind = text_of(&s["codec_type"]).unwrap_or_default();
                    let codec = tracks::codec_name(&text_of(&s["codec_name"]).unwrap_or_default());
                    let mut card = Card::new(format!("#{}  {}  ·  {codec}", text_of(&s["index"]).unwrap_or_default(), match kind.as_str() {
                        "video" => "Video",
                        "audio" => "Audio",
                        "subtitle" => "Subtitles",
                        "attachment" => "Attachment",
                        _ => "Data",
                    }));
                    if yes(&s["disposition"]["default"]) {
                        card.badges.push("default".into());
                    }
                    if yes(&s["disposition"]["forced"]) {
                        card.badges.push("forced".into());
                    }
                    card.row("Codec", text_of(&s["codec_long_name"]));
                    card.row("Profile", text_of(&s["profile"]));
                    card.row("Language", tags(s, &["language"]).map(|code| format!("{}  ({code})", tracks::language_name(&code))));
                    card.row("Title", tags(s, &["title"]));
                    match kind.as_str() {
                        "video" => {
                            card.row("Level", s["level"].as_i64().filter(|l| *l > 0).map(|l| if codec == "HEVC" { format!("{:.1}", l as f64 / 30.0) } else { format!("{:.1}", l as f64 / 10.0) }));
                            card.row("Size", s["width"].as_i64().zip(s["height"].as_i64()).map(|(w, h)| format!("{w} x {h}")));
                            card.row("Frame rate", ratio(&s["avg_frame_rate"]).filter(|r| *r > 0.0).map(|r| format!("{r:.3} fps")));
                            card.row("Pixels", text_of(&s["pix_fmt"]));
                            card.row("Bit depth", text_of(&s["bits_per_raw_sample"]).map(|b| format!("{b}-bit")));
                            card.row("Range", color_words(&s["color_range"]));
                            card.row("Primaries", color_words(&s["color_primaries"]));
                            card.row("Transfer", color_words(&s["color_transfer"]));
                            card.row("Matrix", color_words(&s["color_space"]));
                            card.row("Chroma siting", text_of(&s["chroma_location"]));
                            card.row("Scan", text_of(&s["field_order"]));
                            card.row("Aspect", text_of(&s["display_aspect_ratio"]));
                        }
                        "audio" => {
                            card.row("Sample rate", text_of(&s["sample_rate"]).map(|r| format!("{r} Hz")));
                            card.row("Channels", s["channels"].as_i64().map(|c| match text_of(&s["channel_layout"]) {
                                Some(layout) if layout != tracks::channels_name(c) => format!("{}  ({layout})", tracks::channels_name(c)),
                                Some(_) => tracks::channels_name(c),
                                None => tracks::channels_name(c),
                            }));
                            card.row("Sample format", text_of(&s["sample_fmt"]));
                        }
                        _ => {}
                    }
                    card.row("Bitrate", bitrate(&s["bit_rate"]).or_else(|| tags(s, &["BPS"]).and_then(|b| bitrate(&Value::String(b)))));
                    card.row("Codec tag", text_of(&s["codec_tag_string"]).filter(|t| !t.starts_with('[')));
                    card
                })
                .collect(),
            2 => {
                let mut signal = Card::new("Signal");
                let mut statics = Card::new("HDR10 static");
                let mut dynamic = Card::new("Dynamic");
                if let Some(v) = found.video() {
                    signal.row("Transfer", color_words(&v["color_transfer"]));
                    signal.row("Primaries", color_words(&v["color_primaries"]));
                    signal.row("Matrix", color_words(&v["color_space"]));
                    signal.row("Range", color_words(&v["color_range"]));
                    signal.row("Pixels", text_of(&v["pix_fmt"]));
                }
                let hdr = found.hdr.as_ref();
                let mastering = found.side("Mastering display metadata");
                statics.row("Mastering", mastering.and_then(|m| Some(format!("{:.0} nits  ·  {:.4} black", ratio(&m["max_luminance"])?, ratio(&m["min_luminance"])?))).or_else(|| hdr?.mastering.map(|(peak, floor)| format!("{peak:.0} nits  ·  {floor:.4} black"))));
                statics.row("Display", mastering.and_then(|m| {
                    let red = (ratio(&m["red_x"])?, ratio(&m["red_y"])?);
                    Some(match red {
                        (x, y) if (x - 0.68).abs() < 0.01 && (y - 0.32).abs() < 0.01 => "P3-D65".to_string(),
                        (x, y) if (x - 0.708).abs() < 0.01 && (y - 0.292).abs() < 0.01 => "BT.2020".to_string(),
                        (x, y) => format!("red at {x:.3}, {y:.3}"),
                    })
                }));
                let light = found.side("Content light level metadata");
                statics.row("MaxCLL", light.and_then(|l| text_of(&l["max_content"])).map(|n| format!("{n} nits")).or_else(|| hdr?.light.map(|(cll, _)| format!("{cll} nits"))));
                statics.row("MaxFALL", light.and_then(|l| text_of(&l["max_average"])).map(|n| format!("{n} nits")).or_else(|| hdr?.light.map(|(_, fall)| format!("{fall} nits"))));
                statics.row("From", found.side_from("Mastering display metadata").or_else(|| found.side_from("Content light level metadata")).map(|(_, from)| from.to_string()));
                if statics.rows.is_empty() {
                    statics.row("Values", Some("None given".into()));
                }
                if let Some(hdr) = hdr {
                    dynamic.badges = hdr.badges();
                    if !hdr.hdr10plus.is_empty() {
                        dynamic.row("HDR10+ scenes", Some(format!("{}  ·  {} frames with metadata", hdr_scenes(hdr).len(), hdr.hdr10plus.len())));
                        dynamic.row("Target screen", hdr.hdr10plus.first().map(|first| format!("{:.0} nits", first.value.target)));
                    }
                    if !hdr.agtm.is_empty() {
                        dynamic.row("2094-50 changes", Some(hdr.agtm.len().to_string()));
                    }
                    for (what, how) in &hdr.carried {
                        dynamic.row(&format!("{what} carried as"), Some(how.to_string()));
                    }
                }
                if let Some(dv) = found.side("DOVI configuration record") {
                    dynamic.badges.push("Dolby Vision".into());
                    dynamic.row("Dolby Vision", Some(format!("profile {}  ·  level {}", text_of(&dv["dv_profile"]).unwrap_or_default(), text_of(&dv["dv_level"]).unwrap_or_default())));
                    dynamic.row("Base layer", text_of(&dv["dv_bl_signal_compatibility_id"]).map(|id| match id.as_str() {
                        "0" => "none (DV only)".to_string(),
                        "1" => "HDR10".to_string(),
                        "2" => "SDR".to_string(),
                        "4" => "HLG".to_string(),
                        other => other.to_string(),
                    }));
                    dynamic.row("Layers", Some(format!("RPU {}  ·  EL {}", if yes(&dv["rpu_present_flag"]) { "yes" } else { "no" }, if yes(&dv["el_present_flag"]) { "yes" } else { "no" })));
                }
                if dynamic.rows.is_empty() {
                    dynamic.row("Metadata", Some("None  ·  one look for the whole video".into()));
                }
                vec![signal, statics, dynamic]
            }
            STRUCTURE => Vec::new(),
            _ => {
                let mut cards = Vec::new();
                let mut file_tags = Card::new("File tags");
                if let Some(all) = format["tags"].as_object() {
                    for (key, value) in all {
                        file_tags.row(key, text_of(value));
                    }
                }
                if file_tags.rows.is_empty() {
                    file_tags.row("Tags", Some("None".into()));
                }
                cards.push(file_tags);
                for s in found.streams() {
                    let Some(all) = s["tags"].as_object().filter(|all| !all.is_empty()) else { continue };
                    let mut card = Card::new(format!("Stream #{} tags", text_of(&s["index"]).unwrap_or_default()));
                    for (key, value) in all {
                        card.row(key, text_of(value));
                    }
                    cards.push(card);
                }
                cards
            }
        }
    }

    /// The chapters, for the last tab: start, end and title.
    fn chapters(found: &Inspection) -> Vec<(f64, f64, String)> {
        found.probe["chapters"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter_map(|c| Some((text_of(&c["start_time"])?.parse().ok()?, text_of(&c["end_time"])?.parse().ok()?, text_of(&c["tags"]["title"]).unwrap_or_default())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn draw_inspector(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        let Some(inspector) = self.inspector.take() else { return };
        let mut inspector = inspector;
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let (left, top) = (pad * 2, pad * 2);
        let (width, height) = (canvas.width - pad * 4, canvas.height - pad * 4);
        panel(canvas, left, top, width, height, pad * 2 / 3);
        inspector.drawn.clear();
        let inner = left + pad * 3 / 2;
        let inner_w = width - pad * 3;
        let mut y = top + pad;

        // The heading: what, the file, Copy as JSON and close.
        text.draw(canvas, inner, y, px, TEXT_COLOR, &format!("{}  Inspect", commands::icons::EYE));
        let name = self.name_of(inspector.id);
        let name_x = inner + text.width(&format!("{}  Inspect", commands::icons::EYE), px) + pad;
        let close_w = line;
        let copy_words = "Copy as JSON";
        let copy_w = text.width(copy_words, small) + pad;
        let copy_x = inner + inner_w - close_w - pad / 2 - copy_w;
        let name = clipped(text, &name, copy_x - name_x - pad, small);
        text.draw(canvas, name_x, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, &name);
        let button_h = small_line + pad / 2;
        canvas.round_fill(copy_x, y + (line - button_h) / 2, copy_w, button_h, pad / 3, PLACEHOLDER_COLOR, 256);
        text.draw(canvas, copy_x + pad / 2, y + (line - small_line) / 2, small, TEXT_COLOR, copy_words);
        inspector.drawn.push((Hit::CopyAll, [copy_x, y, copy_w, line]));
        let close_x = inner + inner_w - close_w;
        let x_w = text.width(commands::icons::X, px);
        text.draw(canvas, close_x + (close_w - x_w) / 2, y, px, DIM_TEXT_COLOR, commands::icons::X);
        inspector.drawn.push((Hit::Close, [close_x, y, close_w, line]));
        y += line + pad * 2 / 3;

        // The tabs.
        let mut x = inner;
        let tab_h = small_line + pad * 2 / 3;
        for (at, name) in TABS.iter().enumerate() {
            let w = text.width(name, small) + pad * 3 / 2;
            let on = at == inspector.tab;
            let hovered = (x..x + w).contains(&mouse.0) && (y..y + tab_h).contains(&mouse.1);
            canvas.round_fill(x, y, w, tab_h, pad / 3, if on { accent() } else if hovered { 0x3A3A40 } else { PLACEHOLDER_COLOR }, 256);
            text.draw(canvas, x + pad * 3 / 4, y + pad / 3, small, if on { on_accent() } else { TEXT_COLOR }, name);
            inspector.drawn.push((Hit::Tab(at), [x, y, w, tab_h]));
            x += w + pad / 3;
        }
        y += tab_h + pad;

        // The content, scrolled, clipped to the panel above the key line.
        let view_top = y;
        let view_bottom = top + height - pad - small_line - pad / 2;
        inspector.view_h = view_bottom - view_top;
        let mut sheet = canvas.copy(left, view_top, width, inspector.view_h);
        let mut drawn = Vec::new();
        let mut content_h = 0;
        if sheet.len() == (width * inspector.view_h) as usize {
            let mut sub = Canvas::new(&mut sheet, width as u32, inspector.view_h as u32);
            // The sheet's origin in the window; what is drawn on it is
            // already moved up by the scroll.
            let offset = (left, view_top);
            match &inspector.found {
                None => text.draw(&mut sub, pad * 3 / 2, pad, px, DIM_TEXT_COLOR, "Reading…"),
                Some(Err(error)) => text.draw(&mut sub, pad * 3 / 2, pad, px, DIM_TEXT_COLOR, &format!("Not readable · {error}")),
                Some(Ok(found)) => {
                    let cards = self.inspector_cards(found, inspector.tab);
                    content_h = self.draw_cards(&mut sub, text, px, &cards, inner_w, offset, mouse, &mut drawn, -inspector.scroll);
                    // The HDR tab: a bar for each HDR10+ scene, its brightest.
                    if inspector.tab == 2 {
                        content_h = self.draw_scene_chart(&mut sub, text, px, found, inner_w, content_h - inspector.scroll, offset, &mut drawn) + inspector.scroll;
                    }
                    if inspector.tab == STRUCTURE {
                        let tree = TreeState { open: &inspector.open, selected: &inspector.selected, follow: &mut inspector.follow, scroll: &mut inspector.scroll, view_h: inspector.view_h };
                        content_h = self.draw_structure(&mut sub, text, px, found, tree, inner_w, offset, mouse, &mut drawn);
                    }
                    if inspector.tab == 4 {
                        content_h = self.draw_chapters(&mut sub, text, px, found, inner_w, content_h - inspector.scroll, offset, mouse, &mut drawn) + inspector.scroll;
                    }
                }
            }
            canvas.dissolve(left, view_top, width, inspector.view_h, &sheet, 256);
        }
        inspector.content_h = content_h;
        // Hits inside the content count only where it shows.
        inspector.drawn.extend(drawn.into_iter().filter(|(_, [_, y, _, h])| *y + *h > view_top && *y < view_bottom));
        // A sign that there is more below.
        if content_h - inspector.scroll > inspector.view_h {
            canvas.fill(inner, view_bottom - 1, inner_w, 1, MENU_EDGE_COLOR);
        }
        let keys = if inspector.tab == STRUCTURE { "↑↓ boxes  ·  → open  ·  ← close  ·  click a value to copy it  ·  Tab next tab  ·  c copy all as JSON  ·  Esc close" } else { "Tab or 1–5 tab  ·  ↑↓ scroll  ·  click a value to copy it  ·  c copy all as JSON  ·  Esc close" };
        text.draw(canvas, inner, top + height - pad - small_line, small, DIM_TEXT_COLOR, keys);
        self.inspector = Some(inspector);
    }

    /// Lays the cards out in as many columns as fit, each card under the
    /// shortest column; draws them on `sub`, whose origin is at `offset`
    /// in the window. Returns how tall the columns came to.
    #[allow(clippy::too_many_arguments)]
    fn draw_cards(&self, sub: &mut Canvas, text: &mut Text, px: u32, cards: &[Card], inner_w: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>, scroll: i32) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let columns = (inner_w / (px as i32 * 26)).clamp(1, 3);
        let gap = pad;
        let card_w = (inner_w - gap * (columns - 1)) / columns;
        let mut heights = vec![0; columns as usize];
        let row_h = line + pad / 4;
        let label_w = cards.iter().flat_map(|card| card.rows.iter()).map(|(label, _)| text.width(label, small)).max().unwrap_or(0).min(card_w * 2 / 5);
        for card in cards {
            let column = heights.iter().enumerate().min_by_key(|(_, h)| **h).map_or(0, |(at, _)| at);
            let x = pad * 3 / 2 + column as i32 * (card_w + gap);
            let card_h = pad + line + pad / 2 + card.rows.len() as i32 * row_h + pad * 2 / 3;
            let y = heights[column] + scroll;
            sub.round_fill(x, y, card_w, card_h, pad / 2, 0x26262B, 256);
            let mut tx = x + pad;
            sub_title(sub, text, tx, y + pad, small, &card.title.to_uppercase());
            tx += text.width(&card.title.to_uppercase(), small) + pad / 2;
            for name in &card.badges {
                let w = text.width(name, small) + pad / 2;
                sub.round_fill(tx, y + pad, w, small_line, pad / 4, badge_color(name), 230);
                text.draw(sub, tx + pad / 4, y + pad, small, TEXT_COLOR, name);
                tx += w + pad / 3;
            }
            let mut ry = y + pad + line + pad / 2;
            for (label, value) in &card.rows {
                let label = clipped(text, label, label_w, small);
                text.draw(sub, x + pad, ry + (line - small_line) / 2, small, DIM_TEXT_COLOR, &label);
                let vx = x + pad + label_w + pad;
                let room = x + card_w - pad - vx;
                let shown = clipped(text, value, room, px);
                let vw = text.width(&shown, px);
                // Where it is in the window, for the pointer and clicks.
                let window_place = [vx - pad / 4 + offset.0, ry + offset.1, vw + pad / 2, row_h];
                let hovered = (window_place[0]..window_place[0] + window_place[2]).contains(&mouse.0) && (window_place[1]..window_place[1] + window_place[3]).contains(&mouse.1);
                if hovered {
                    sub.round_fill(vx - pad / 4, ry, vw + pad / 2, row_h, pad / 4, 0x3A3A40, 256);
                }
                text.draw(sub, vx, ry, px, TEXT_COLOR, &shown);
                drawn.push((Hit::Copy(value.clone()), window_place));
                ry += row_h;
            }
            heights[column] += card_h + gap;
        }
        heights.into_iter().max().unwrap_or(0)
    }

    /// The HDR10+ scenes as bars, the brightest of each, under the cards;
    /// a click on one goes to that scene. Returns the content's new height
    /// as drawn (scroll included).
    #[allow(clippy::too_many_arguments)]
    fn draw_scene_chart(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &Inspection, inner_w: i32, y: i32, offset: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let Some(hdr) = found.hdr.as_ref().filter(|hdr| !hdr.hdr10plus.is_empty()) else { return y };
        let pad = px as i32;
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let chart_h = px as i32 * 9;
        let card_h = pad + small_line + pad / 2 + chart_h + pad / 2 + small_line + pad;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, 0x26262B, 256);
        sub_title(sub, text, x + pad, y + pad, small, "HDR10+ · BRIGHTEST OF EACH SCENE");
        let scenes = hdr_scenes(hdr);
        let peak = scenes.iter().map(|(_, brightest, _)| *brightest).fold(1000.0_f64, f64::max);
        let (cx, cy, cw) = (x + pad, y + pad + small_line + pad / 2, inner_w - 2 * pad);
        let count = scenes.len() as i32;
        let gap = if cw / count.max(1) > 6 { 2 } else { 0 };
        let length = self.video_time().map(|(_, length)| length);
        for (at, (start, brightest, average)) in scenes.iter().enumerate() {
            let h = ((brightest / peak) * f64::from(chart_h)).max(2.0) as i32;
            let (bx, next) = (cx + at as i32 * cw / count.max(1), cx + (at as i32 + 1) * cw / count.max(1));
            let bar_w = (next - bx - gap).max(1);
            sub.fill(bx, cy + chart_h - h, bar_w, h, 0x3E7BD6);
            let mean_h = ((average / peak) * f64::from(chart_h)).max(1.0) as i32;
            sub.fill(bx, cy + chart_h - mean_h, bar_w, 2, TEXT_COLOR);
            if length.is_some() {
                drawn.push((Hit::Seek(*start), [bx + offset.0, cy + offset.1, bar_w.max(2), chart_h]));
            }
        }
        sub.fill(cx, cy + chart_h, cw, 1, MENU_EDGE_COLOR);
        let caption = format!("{} scenes  ·  bar: brightest  ·  line: mean  ·  top {peak:.0} nits  ·  click one to go there", scenes.len());
        text.draw(sub, cx, cy + chart_h + pad / 2, small, DIM_TEXT_COLOR, &caption);
        y + card_h + pad
    }

    /// The chapters as rows, each to be clicked to go there.
    #[allow(clippy::too_many_arguments)]
    fn draw_chapters(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &Inspection, inner_w: i32, y: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let chapters = Self::chapters(found);
        if chapters.is_empty() {
            return y;
        }
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let x = pad * 3 / 2;
        let row_h = line + pad / 3;
        let card_h = pad + Text::line_height(small) + pad / 2 + chapters.len() as i32 * row_h + pad / 2;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, 0x26262B, 256);
        sub_title(sub, text, x + pad, y + pad, small, &format!("CHAPTERS · {}", chapters.len()));
        let mut ry = y + pad + Text::line_height(small) + pad / 2;
        for (start, end, title) in chapters {
            let place = [x + offset.0, ry + offset.1, inner_w, row_h];
            if (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1) {
                sub.round_fill(x + pad / 2, ry, inner_w - pad, row_h, pad / 4, 0x3A3A40, 256);
            }
            let span = format!("{} – {}", clock(start), clock(end));
            text.draw(sub, x + pad, ry, px, DIM_TEXT_COLOR, &span);
            text.draw(sub, x + pad + px as i32 * 9, ry, px, TEXT_COLOR, &title);
            drawn.push((Hit::Seek(start), place));
            ry += row_h;
        }
        y + card_h + pad
    }
}

/// A card's heading: small capitals, dim.
fn sub_title(canvas: &mut Canvas, text: &mut Text, x: i32, y: i32, px: u32, words: &str) {
    text.draw(canvas, x, y, px, DIM_TEXT_COLOR, words);
}

/// The HDR10+ metadata as scenes: frames in a row with the same values are
/// one scene. Each with when it starts, its brightest and its mean, in nits.
fn hdr_scenes(hdr: &crate::hdrmeta::Info) -> Vec<(f64, f64, f64)> {
    let mut scenes: Vec<(f64, f64, f64)> = Vec::new();
    let mut last: Option<([f64; 3], f64)> = None;
    for timed in &hdr.hdr10plus {
        let key = (timed.value.maxscl, timed.value.average);
        if last != Some(key) {
            scenes.push((timed.at, timed.value.maxscl.iter().copied().fold(0.0, f64::max), timed.value.average));
            last = Some(key);
        }
    }
    scenes
}

/// The boxes that show in the Structure tab: each top one, and the
/// children of those opened, in order, with how deep each is.
fn visible_nodes(nodes: &[crate::boxes::Node], open: &HashSet<Vec<usize>>) -> Vec<(Vec<usize>, usize)> {
    fn walk(nodes: &[crate::boxes::Node], open: &HashSet<Vec<usize>>, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, usize)>) {
        for (at, node) in nodes.iter().enumerate() {
            path.push(at);
            out.push((path.clone(), path.len() - 1));
            if open.contains(path) {
                walk(&node.children, open, path, out);
            }
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(nodes, open, &mut Vec::new(), &mut out);
    out
}

/// The box at a place in the tree.
fn node_at<'a>(nodes: &'a [crate::boxes::Node], path: &[usize]) -> Option<&'a crate::boxes::Node> {
    let (first, rest) = path.split_first()?;
    let node = nodes.get(*first)?;
    if rest.is_empty() { Some(node) } else { node_at(&node.children, rest) }
}

/// A byte count as the tree shows it: 32 B, 1.2 KB, 812 MB.
fn bytes_words(size: u64) -> String {
    if size < 1024 { format!("{size} B") } else { meta::human_size(size) }
}

impl App {
    /// The Structure tab: the tree down the left, scrolled; the box picked
    /// out on the right, its fields and its first bytes. Returns how tall
    /// the tree is.
    #[allow(clippy::too_many_arguments)]
    fn draw_structure(&self, sub: &mut Canvas, text: &mut Text, px: u32, found: &Inspection, inspector: TreeState, inner_w: i32, offset: (i32, i32), mouse: (i32, i32), drawn: &mut Vec<(Hit, [i32; 4])>) -> i32 {
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        let x = pad * 3 / 2;
        let nodes = match &found.tree {
            Ok(nodes) => nodes,
            Err(error) => {
                text.draw(sub, x, pad, px, DIM_TEXT_COLOR, &format!("Not readable · {error}"));
                return 0;
            }
        };
        let tree_w = (inner_w * 9 / 20).max(px as i32 * 22);
        let row_h = line + pad / 5;
        let visible = visible_nodes(nodes, inspector.open);
        // Keep the one picked out in view after a key.
        if *inspector.follow {
            *inspector.follow = false;
            if let Some(at) = visible.iter().position(|(path, _)| path == inspector.selected) {
                let top = at as i32 * row_h;
                if top < *inspector.scroll {
                    *inspector.scroll = top;
                } else if top + row_h > *inspector.scroll + inspector.view_h {
                    *inspector.scroll = top + row_h - inspector.view_h;
                }
            }
        }
        let size_x = x + tree_w - pad / 2;
        for (row, (path, depth)) in visible.iter().enumerate() {
            let y = row as i32 * row_h - *inspector.scroll;
            if y + row_h < 0 || y > inspector.view_h {
                continue;
            }
            let Some(node) = node_at(nodes, path) else { continue };
            let picked = path == inspector.selected;
            let place = [x + offset.0, y + offset.1, tree_w, row_h];
            let hovered = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
            if picked || hovered {
                sub.round_fill(x, y, tree_w, row_h, pad / 4, if picked { accent() } else { 0x3A3A40 }, 256);
            }
            let fg = if picked { on_accent() } else { TEXT_COLOR };
            let dim = if picked { on_accent() } else { DIM_TEXT_COLOR };
            let nx = x + pad / 2 + *depth as i32 * pad;
            // A triangle for one that holds others: down when open.
            if !node.children.is_empty() {
                let s = (pad / 2) | 1;
                let (tx, ty) = (nx + pad / 6, y + (line - s) / 2);
                for i in 0..=s / 2 {
                    match inspector.open.contains(path) {
                        true => sub.fill(tx + i, ty + s / 4 + i, s - 2 * i, 1, dim),
                        false => sub.fill(tx + s / 4 + i, ty + i, 1, s - 2 * i, dim),
                    }
                }
            }
            let name_x = nx + pad;
            // A run folded into one line says how many more.
            let shown_name = if node.more > 0 { format!("{}  ×{} more", node.name, node.more) } else { node.name.clone() };
            text.draw(sub, name_x, y, px, fg, &shown_name);
            let size = bytes_words(node.size);
            let sw = text.width(&size, small);
            text.draw(sub, size_x - sw, y + (line - small_line) / 2, small, dim, &size);
            let about_x = name_x + text.width(&shown_name, px) + pad / 2;
            let room = size_x - sw - pad - about_x;
            if room > pad * 3 && !node.about.is_empty() {
                let about = clipped(text, node.about, room, small);
                text.draw(sub, about_x, y + (line - small_line) / 2, small, dim, &about);
            }
            drawn.push((Hit::Node(path.clone()), place));
        }

        // The box picked out: its name, what it is, where, its fields and
        // its first bytes. It stays where it is as the tree scrolls.
        let Some(node) = node_at(nodes, inspector.selected) else { return visible.len() as i32 * row_h };
        let (dx, dw) = (x + tree_w + pad, inner_w - tree_w - pad);
        let mut y = 0;
        let fields_h = node.fields.len() as i32 * (line + pad / 4);
        let hex_rows = node.bytes.len().div_ceil(16) as i32;
        let card_h = pad + line + small_line + pad / 2 + fields_h + pad + hex_rows * (small_line + pad / 6) + pad;
        sub.round_fill(dx, y, dw, card_h.min(inspector.view_h), pad / 2, 0x26262B, 256);
        y += pad;
        text.draw(sub, dx + pad, y, px, TEXT_COLOR, &node.name);
        let about = if node.about.is_empty() { String::new() } else { format!("  ·  {}", node.about) };
        let name_w = text.width(&node.name, px);
        let about = clipped(text, &about, dw - 3 * pad - name_w, small);
        text.draw(sub, dx + pad + name_w, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, &about);
        y += line;
        let place = format!("at byte {}  ·  {} long  ·  {} inside", node.offset, bytes_words(node.size), node.children.len());
        text.draw(sub, dx + pad, y, small, DIM_TEXT_COLOR, &place);
        y += small_line + pad / 2;
        let name_w = node.fields.iter().map(|(name, _, _)| text.width(name, small)).max().unwrap_or(0).min(dw / 2);
        for (name, value, means) in &node.fields {
            let name = clipped(text, name, name_w, small);
            text.draw(sub, dx + pad, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, &name);
            let vx = dx + pad + name_w + pad;
            let shown = clipped(text, value, dw / 4, px);
            let vw = text.width(&shown, px);
            let place = [vx - pad / 4 + offset.0, y + offset.1, vw + pad / 2, line];
            if (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1) {
                sub.round_fill(vx - pad / 4, y, vw + pad / 2, line, pad / 4, 0x3A3A40, 256);
            }
            text.draw(sub, vx, y, px, TEXT_COLOR, &shown);
            drawn.push((Hit::Copy(value.clone()), place));
            if !means.is_empty() {
                let mx = vx + vw + pad;
                let means = clipped(text, means, dx + dw - pad - mx, small);
                text.draw(sub, mx, y + (line - small_line) / 2, small, DIM_TEXT_COLOR, &means);
            }
            y += line + pad / 4;
        }
        y += pad / 2;
        for (row, chunk) in node.bytes.chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            let ascii: String = chunk.iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '.' }).collect();
            let words = format!("{:08X}  {}  {ascii}", node.offset + row as u64 * 16, hex.join(" "));
            let words = clipped(text, &words, dw - 2 * pad, small);
            text.draw(sub, dx + pad, y, small, if row == 0 { TEXT_COLOR } else { DIM_TEXT_COLOR }, &words);
            y += small_line + pad / 6;
        }
        visible.len() as i32 * row_h
    }
}

/// What the Structure tab needs of the inspector while it draws: which
/// boxes are open and picked out, and the scroll, which it moves to keep
/// the one picked out in view.
struct TreeState<'a> {
    open: &'a HashSet<Vec<usize>>,
    selected: &'a Vec<usize>,
    follow: &'a mut bool,
    scroll: &'a mut i32,
    view_h: i32,
}
