//! The inspector (F3): what a video is made of, in tabs. The summary of
//! the file, each of its streams, its HDR, and its tags and chapters, as
//! ffprobe reads them, with jog's own reading of the HDR metadata that
//! changes scene by scene. Any value is copied with a click, the lot as
//! JSON with c.

use serde_json::Value;
use winit::keyboard::{Key, NamedKey};

use super::*;
use super::devtools::flow_x;

/// What was found in a file.
pub struct Inspection {
    pub(super) probe: Value,
    pub(super) hdr: Option<crate::hdrmeta::Info>,
    /// The first video frame's side data: what the bitstream carries
    /// that the container may not, HDR10 values written by the encoder.
    first_frame: Value,
    /// Its boxes or elements.
    pub(super) tree: Result<Vec<crate::boxes::Node>, String>,
    /// For a picture, what it carries: EXIF, color profile, XMP, gain map.
    pub(super) picture: Option<crate::picmeta::Picture>,
}

impl Inspection {
    /// Reads a video with ffprobe, its HDR metadata and structure with
    /// jog's readers; a picture with jog's.
    pub fn read(path: &Path) -> Result<Inspection, String> {
        if !files::is_video(path) {
            let picture = crate::picmeta::read(path)?;
            return Ok(Inspection { probe: Value::Null, hdr: None, first_frame: Value::Null, tree: Err("not a video".into()), picture: Some(picture) });
        }
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
        Ok(Inspection { probe, hdr: crate::hdrmeta::read(path), first_frame, tree: crate::boxes::read(path), picture: None })
    }

    pub(super) fn streams(&self) -> &[Value] {
        self.probe["streams"].as_array().map_or(&[], Vec::as_slice)
    }

    pub(super) fn video(&self) -> Option<&Value> {
        self.streams().iter().find(|stream| stream["codec_type"] == "video" && stream["disposition"]["attached_pic"] != 1)
    }

    /// One side data entry of the video stream, by its type.
    pub(super) fn side(&self, kind: &str) -> Option<&Value> {
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
        if let Some(picture) = &self.picture {
            let exif: Vec<Value> = picture.exif.iter().map(|(group, tag, value)| serde_json::json!({ "group": group, "tag": tag, "value": value })).collect();
            let xmp: serde_json::Map<String, Value> = picture.xmp.iter().map(|(name, value)| (name.clone(), Value::String(value.clone()))).collect();
            let gain = picture.gain.as_ref().map(|(meta, bytes)| serde_json::json!({ "min": meta.min, "max": meta.max, "gamma": meta.gamma, "offset_sdr": meta.offset_sdr, "offset_hdr": meta.offset_hdr, "capacity_max": meta.capacity_max, "bytes": bytes }));
            let all = serde_json::json!({ "format": picture.format, "size": picture.size, "bytes": picture.bytes, "icc": picture.icc, "exif": exif, "xmp": xmp, "gain_map": gain });
            return serde_json::to_string_pretty(&all).unwrap_or_default();
        }
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

/// The tab of the boxes or elements the file is made of.
const STRUCTURE: usize = 3;

impl Inspector {
    /// Goes to a tab: its cards and charts, if it is one of the first tabs,
    /// under that one's number, and from the top.
    pub(super) fn set_pane(&mut self, pane: Pane) {
        // One now in the drawer, or not for this kind of file: the Overview.
        let pane = if pane.applies(self.picture) && pane.drawer().is_none() { pane } else { Pane::Overview };
        self.pane = pane;
        self.tab = pane.legacy(self.picture).unwrap_or(usize::MAX);
        self.scroll = 0;
    }

    /// The tabs it shows, and where the one up is among them.
    /// The file it is about.
    pub(super) fn id(&self) -> usize {
        self.id
    }

    pub(super) fn panes(&self) -> (Vec<Pane>, usize) {
        let panes = Pane::shown(self.picture);
        let at = panes.iter().position(|pane| *pane == self.pane).unwrap_or(0);
        (panes, at)
    }
}

impl Inspector {
    /// Where the panel was last drawn.
    pub(super) fn place(&self) -> [i32; 4] {
        self.place
    }

    /// Whether the Structure tab is the one up.
    fn on_structure(&self) -> bool {
        !self.picture && self.tab == STRUCTURE
    }
}

/// A card of label and value rows, under a heading.
pub(super) struct Card {
    pub(super) title: String,
    pub(super) badges: Vec<String>,
    pub(super) rows: Vec<(String, String)>,
}

impl Card {
    pub(super) fn new(title: impl Into<String>) -> Card {
        Card { title: title.into(), badges: Vec::new(), rows: Vec::new() }
    }

    /// A row, when there is something to say.
    pub(super) fn row(&mut self, label: &str, value: Option<String>) {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            self.rows.push((label.to_string(), value));
        }
    }
}

/// What a click on the inspector lands on.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Hit {
    Tab(usize),
    Copy(String),
    CopyAll,
    /// Docks the panel beside the picture, or lets it float over it.
    Pin,
    /// Out into a window of its own, or back.
    PopOut,
    /// Docked along the bottom, or down the side.
    Dock,
    /// The drawer under the tab, with the file list, open or shut.
    Drawer,
    /// One of the drawer's tabs.
    DrawerTab(super::devtools::DrawerPane),
    /// A command, as from a key or the menu: the transport bar's and the
    /// action rows', run on the next turn of the event loop.
    Run(Command),
    /// The transport bar's thin bar, where it starts and how wide.
    Scrub(i32, i32),
    /// A track to play: audio or subtitles, and which; none for subtitles off.
    Track(bool, Option<i64>),
    /// The HDR tab: the headroom up or down, and the 2094-50 curves as
    /// gain rather than output.
    Headroom(bool),
    GainView,
    Close,
    /// A scene of the HDR chart, or a chapter: that time in the video.
    Seek(f64),
    /// A box or element of the Structure tab, by where it is in the tree.
    Node(Vec<usize>),
    /// A choice on the Convert tab.
    Convert(super::convert_tab::Knob),
}

pub struct Inspector {
    id: usize,
    tab: usize,
    scroll: i32,
    /// How far down the content went, for scrolling.
    content_h: i32,
    view_h: i32,
    found: Option<Result<Inspection, String>>,
    /// Where the panel was drawn, so that clicks and the wheel beside it
    /// go to the video.
    place: [i32; 4],
    /// The panel as last drawn, and what it was drawn from: drawn again
    /// only when that changes, and otherwise copied in, so that a 4K video
    /// beside it is not held up by it on every frame.
    kept: Option<(PanelKey, Vec<u32>)>,
    /// A picture, with its own tabs, rather than a video.
    picture: bool,
    /// The Structure tab: the boxes opened and the one picked out, each by
    /// where it is in the tree; and whether to scroll to the one picked.
    open: HashSet<Vec<usize>>,
    selected: Vec<usize>,
    follow: bool,
    drawn: Vec<(Hit, [i32; 4])>,
    /// The 2094-50 curves shown as gain in stops rather than output.
    gain: bool,
    /// The tab up, and the transport bar's buttons as drawn this frame.
    pane: Pane,
    transport: Vec<(Hit, [i32; 4])>,
    /// The video's packets, for the Timeline, once read; and whether asked for.
    packets: Option<Result<super::devtools::Packets, String>>,
    packets_asked: bool,
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
        self.inspector = Some(Inspector { id, tab: 0, scroll: 0, content_h: 0, view_h: 0, found: None, place: [0; 4], kept: None, picture: !files::is_video(&self.files[id]), open: HashSet::new(), selected: vec![0], follow: false, drawn: Vec::new(), gain: false, pane: Pane::Overview, transport: Vec::new(), packets: None, packets_asked: false });
        // On the tab used last.
        let pane = self.inspect_pane;
        if let Some(inspector) = &mut self.inspector {
            inspector.set_pane(pane);
        }
        // Out in its own window, if that is where it was last.
        self.reopen_out(Popped::Inspector);
        self.menu = None;
        self.redraw();
    }

    /// The inspector, open while another file comes up, goes on to that
    /// one: on the same tab, when it is the same kind of file.
    pub(super) fn follow_inspector(&mut self) {
        let Some(inspector) = &self.inspector else { return };
        let Some(&id) = self.list.get(self.focus()) else { return };
        if inspector.id == id {
            return;
        }
        // On the same tab, where the other file has it.
        let pane = inspector.pane;
        self.open_inspector();
        if let Some(inspector) = self.inspector.as_mut() {
            inspector.set_pane(pane);
        }
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

    /// The inspector's keys. Whether it took the key: the rest go on to
    /// the video beside it, Space to play and pause and so on.
    pub(super) fn inspector_key(&mut self, key: Key<&str>) -> bool {
        let (step, shift) = (Text::line_height(ui_px(self.window_size().1)) * 3, self.modifiers.shift_key());
        let Some(inspector) = &mut self.inspector else { return false };
        // In the Structure tab the arrows go about the tree.
        if inspector.on_structure() {
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
                self.redraw();
                return true;
            }
        }
        match key {
            Key::Named(NamedKey::Escape | NamedKey::F3 | NamedKey::F12) => self.inspector = None,
            // The keys that open it close it too, as a browser's do.
            Key::Character("I" | "i") if self.modifiers.control_key() && self.modifiers.shift_key() => self.inspector = None,
            Key::Named(NamedKey::Tab) => {
                // The next tab, or with Shift the one before; the arrows are
                // the video's, to seek with.
                let back = shift;
                let (panes, at) = inspector.panes();
                let pane = panes[if back { (at + panes.len() - 1) % panes.len() } else { (at + 1) % panes.len() }];
                inspector.set_pane(pane);
                self.inspect_pane = pane;
            }
            Key::Named(NamedKey::ArrowDown) => inspector.scroll += step,
            Key::Named(NamedKey::ArrowUp) => inspector.scroll -= step,
            Key::Named(NamedKey::PageDown) => inspector.scroll += inspector.view_h * 4 / 5,
            Key::Named(NamedKey::PageUp) => inspector.scroll -= inspector.view_h * 4 / 5,
            Key::Character(digit @ ("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")) if digit.parse::<usize>().unwrap_or(99) <= inspector.panes().0.len() => {
                let pane = inspector.panes().0[digit.parse::<usize>().unwrap_or(1) - 1];
                inspector.set_pane(pane);
                self.inspect_pane = pane;
            }
            Key::Character("c") => {
                self.inspector_hit(Hit::CopyAll);
                return true;
            }
            _ => return false,
        }
        self.clamp_inspector_scroll();
        self.redraw();
        true
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

    pub(super) fn inspector_click(&mut self, x: f64, y: f64, event_loop: &ActiveEventLoop) {
        // The Files tab's list answers as the list does.
        if let Some(panel) = self.list_panel().filter(|panel| panel.embedded && panel.holds(x, y)) {
            let within = x as i32 - panel.left;
            let in_field = self.list_field().is_some_and(|[fx, fy, w, h]| (fx..fx + w).contains(&within) && (fy..fy + h).contains(&(y as i32)));
            if !in_field {
                self.list_search = None;
            }
            self.list_click(x, y, in_field, true, event_loop);
            return self.redraw();
        }
        let Some(inspector) = &self.inspector else { return };
        let on = |[left, top, w, h]: [i32; 4]| (left..left + w).contains(&(x as i32)) && (top..top + h).contains(&(y as i32));
        // The transport bar first, drawn over the panel every frame.
        let hit = inspector.transport.iter().chain(inspector.drawn.iter().rev()).find(|(_, place)| on(*place)).cloned();
        match hit {
            // Along the thin bar: to that point of the video.
            Some((Hit::Scrub(bar_x, bar_w), _)) => {
                let fraction = ((x - f64::from(bar_x)) / f64::from(bar_w.max(1))).clamp(0.0, 1.0);
                self.seek_to(fraction, true);
            }
            // The Convert tab's choices, its slider by where along.
            Some((Hit::Convert(knob), _)) => self.convert_knob(knob, x),
            Some((hit, _)) => self.inspector_hit(hit),
            None => {}
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
            Hit::Tab(at) => {
                if let Some(inspector) = &mut self.inspector {
                    let (panes, _) = inspector.panes();
                    if let Some(pane) = panes.get(at) {
                        inspector.set_pane(*pane);
                        self.inspect_pane = *pane;
                        self.save_panels();
                    }
                }
            }
            Hit::Run(command) => self.pending_command = Some(command),
            Hit::Scrub(..) | Hit::Convert(_) => {}
            Hit::Track(audio, id) => self.choose_track(audio, id),
            Hit::Close => self.inspector = None,
            Hit::Pin => self.toggle_inspector_pin(),
            Hit::Dock => self.toggle_inspector_dock(),
            Hit::DrawerTab(pane) => {
                self.drawer_pane = pane;
                self.save_panels();
                self.redraw();
            }
            Hit::Drawer => {
                self.inspect_drawer = !self.inspect_drawer;
                self.save_panels();
                self.redraw();
            }
            Hit::PopOut => self.toggle_popout(Popped::Inspector),
            Hit::Headroom(up) => self.hdr_headroom = (self.hdr_headroom + if up { 0.25 } else { -0.25 }).clamp(0.0, 6.0),
            Hit::GainView => {
                if let Some(inspector) = &mut self.inspector {
                    inspector.gain = !inspector.gain;
                }
            }
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
        if let Some(picture) = &found.picture {
            return picture_cards(picture, tab);
        }
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


    pub(super) fn draw_inspector(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        let Some(inspector) = self.inspector.take() else { return };
        let mut inspector = inspector;
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        // A column down the side the file list is not on, as wide as the
        // list and laid out as it is: over the picture, or pinned beside it.
        // Popped out, the whole of its own window.
        let popped = self.drawing_popout == Some(Popped::Inspector);
        // Docked along the bottom: across the window, under the picture.
        let bottom = self.inspector_bottom && !popped;
        let (width, height) = match (popped, bottom) {
            (true, _) => (canvas.width, canvas.height),
            (false, true) => (canvas.width, self.inspector_height(canvas.height)),
            (false, false) => (panel_width(canvas.width, px, self.inspector_units), canvas.height),
        };
        let left = if self.list_right || popped || bottom { 0 } else { canvas.width - width };
        let top = if bottom { canvas.height - height } else { 0 };
        inspector.place = [left, top, width, height];
        // Nothing in it has changed: the panel as it was.
        // The pointer over the panel, as the panel is to be drawn: not by
        // `over_inspector`, the panel being out of its place just now.
        let over = ((left..left + width).contains(&mouse.0) && (top..top + height).contains(&mouse.1)).then_some(mouse);
        let hdr_info = inspector.found.as_ref().and_then(|found| found.as_ref().ok()).and_then(|found| found.hdr.as_ref());
        let hdr = (inspector.tab == 2 && !inspector.picture).then(|| self.hdr_key(inspector.id, hdr_info, inspector.gain));
        // The transport bar along the foot, drawn afresh every frame.
        let transport_h = pad * 5 / 2;
        // Performance's figures change all the time: drawn again twice a second.
        let tick = match inspector.pane {
            Pane::Performance => Some(self.perf.arrived.elapsed().as_millis() / 500),
            // The packets come, and the video moves along: twice a second.
            Pane::Timeline => Some(u128::from(inspector.packets.is_some()) + self.perf.arrived.elapsed().as_millis() / 500 * 2),
            Pane::Console => Some(self.messages.len() as u128 * 1000 + self.perf.arrived.elapsed().as_secs() as u128 / 30),
            Pane::Convert => Some(self.convert_tab_key()),
            // Which file it is compared with, and whether that is read.
            Pane::Compare => Some(self.marks.iter().map(|id| *id as u128 + 1).sum::<u128>() * 4 + self.compare_found.as_ref().map_or(0, |(id, answer)| (*id as u128) * 2 + u128::from(answer.is_some()))),
            Pane::Color => Some(self.scopes.iter().enumerate().map(|(at, on)| u128::from(*on) << at).sum()),
            // What is playing, as the info line says it, and whether it
            // has started.
            Pane::Tracks => Some({
                let words = self.video.as_ref().and_then(|video| video.track_words.as_ref()).map_or(0, |words| words.bytes().fold(0u128, |hash, byte| hash.wrapping_mul(31).wrapping_add(u128::from(byte))));
                words.wrapping_mul(2) + u128::from(self.video.as_ref().is_some_and(|video| video.id == inspector.id && video.showing))
            }),
            // The list lights up under the pointer and plays along: every frame.
            Pane::Files => Some(self.perf.arrived.elapsed().as_nanos()),
            _ if self.inspect_drawer => Some(self.perf.arrived.elapsed().as_nanos()),
            _ => None,
        };
        let key = PanelKey { pane: inspector.pane, tick, tab: inspector.tab, scroll: inspector.scroll, found: inspector.found.is_some(), selected: inspector.selected.clone(), open: inspector.open.len(), follow: inspector.follow, pointer: over, place: inspector.place, window: (canvas.width, canvas.height), pinned: self.inspector_pinned, hdr };
        // Over the picture it lets it show through faintly, as the file
        // list does there: the panel, kept whole, laid on with a little of
        // what is under it.
        let through = if self.inspector_pinned || popped { 0 } else { 24 };
        if let Some((_, pixels)) = inspector.kept.as_ref().filter(|(was, pixels)| *was == key && pixels.len() == (width * height) as usize) {
            canvas.dissolve(left, top, width, height, pixels, 256 - through);
            inspector.transport = self.draw_transport(canvas, text, px, [left, top + height - transport_h, width, transport_h], inspector.picture);
            self.inspector = Some(inspector);
            return;
        }
        let behind = (through > 0).then(|| canvas.copy(left, top, width, height));

        canvas.fill(left, top, width, height, GRID_COLOR);
        if bottom {
            canvas.fill(left, top, width, 1, MENU_EDGE_COLOR);
        } else {
            canvas.fill(if self.list_right { left + width - 1 } else { left }, top, 1, height, MENU_EDGE_COLOR);
        }
        inspector.drawn.clear();
        let inner = left + pad * 3 / 2;

        // The heading, as the file list's: what and the file, then the
        // round buttons from the right: close, pin, copy as JSON.
        let side = pad * 3 / 2;
        let heading = format!("{}  Inspect", commands::icons::EYE);
        text.draw(canvas, inner, top + pad * 3 / 4, px, TEXT_COLOR, &heading);
        // Popped out, the pin has nothing to do; the pop-out button, lit,
        // puts it back.
        let mut buttons = vec![(Hit::Close, commands::icons::X, false)];
        if !popped {
            buttons.push((Hit::Pin, commands::icons::PIN, self.inspector_pinned));
            // Down the side or along the bottom, as a browser's tools dock.
            buttons.push((Hit::Dock, commands::icons::LAYOUT_GRID, self.inspector_bottom));
            // The file list in a drawer under the tab.
            buttons.push((Hit::Drawer, commands::icons::LIST, self.inspect_drawer));
        }
        buttons.push((Hit::PopOut, if popped { commands::icons::POP_IN } else { commands::icons::EXTERNAL_LINK }, false));
        buttons.push((Hit::CopyAll, commands::icons::BRACES, false));
        let mut button_left = left + width;
        for (at, (hit, icon, on)) in buttons.into_iter().enumerate() {
            button_left = left + width - (at as i32 + 1) * side - pad / 2 - at as i32 * pad / 3;
            let place = Button { x: button_left, y: top + pad / 2, width: side, height: side, command: Command::Inspect };
            let hovered = (place.x..place.x + side).contains(&mouse.0) && (place.y..place.y + side).contains(&mouse.1);
            draw::round_button(canvas, text, &place, icon, px, hovered, on, false);
            inspector.drawn.push((hit, [place.x, place.y, side, side]));
        }
        let name_x = inner + text.width(&heading, px) + pad / 2;
        let name = clipped(text, &self.name_of(inspector.id), button_left - name_x - pad / 2, small);
        text.draw(canvas, name_x, top + pad * 3 / 4 + (line - small_line) / 2, small, DIM_TEXT_COLOR, &name);
        let mut y = top + pad / 2 + side + pad / 2;
        canvas.fill(pad / 2 + left, y, width - pad, 1, MENU_EDGE_COLOR);
        y += pad / 2;

        // The tabs, as a browser's developer tools have them: one row of
        // names, the one up underlined, a small label before each group;
        // moved along to keep the one up in view when they do not all fit.
        let (panes, current) = inspector.panes();
        y = self.draw_tab_row(canvas, text, px, [left, y, width], &panes, current, mouse, &mut inspector.drawn);

        // The content, scrolled, clipped to the panel above the transport bar,
        // or above the drawer, when that is open under another tab.
        let view_top = y;
        let foot = top + height - transport_h;
        let drawer = self.inspect_drawer;
        // Docked along the bottom the drawer stands beside the tab, there
        // being little height; down the side, under it.
        let beside = drawer && bottom;
        let content_w = if beside { width * 3 / 5 } else { width };
        let inner_w = content_w - pad * 3;
        let view_bottom = if drawer && !beside { view_top + (foot - view_top) * 11 / 20 } else { foot };
        inspector.view_h = view_bottom - view_top;
        let mut sheet = canvas.copy(left, view_top, content_w, inspector.view_h);
        let mut drawn = Vec::new();
        let mut content_h = 0;
        if sheet.len() == (content_w * inspector.view_h) as usize {
            let mut sub = Canvas::new(&mut sheet, content_w as u32, inspector.view_h as u32);
            // The sheet's origin in the window; what is drawn on it is
            // already moved up by the scroll.
            let offset = (left, view_top);
            match &inspector.found {
                None => text.draw(&mut sub, pad * 3 / 2, pad, px, DIM_TEXT_COLOR, "Reading…"),
                Some(Err(error)) => text.draw(&mut sub, pad * 3 / 2, pad, px, DIM_TEXT_COLOR, &format!("Not readable · {error}")),
                Some(Ok(found)) => {
                    // The Timeline's packets, read once, on another thread.
                    if inspector.pane == Pane::Timeline && !inspector.packets_asked {
                        inspector.packets_asked = true;
                        let (id, path, proxy) = (inspector.id, self.files[inspector.id].clone(), self.proxy.clone());
                        std::thread::spawn(move || drop(proxy.send_event(Event::Packets(id, super::devtools::Packets::read(&path)))));
                    }
                    if inspector.on_structure() {
                        super::devtools::set_flow_x(pad * 3 / 2);
                        let tree = TreeState { open: &inspector.open, selected: &inspector.selected, follow: &mut inspector.follow, scroll: &mut inspector.scroll, view_h: inspector.view_h };
                        content_h = self.draw_structure(&mut sub, text, px, found, tree, inner_w, offset, mouse, &mut drawn);
                    } else {
                        // The tab as blocks, laid out in as many columns as fit.
                        let mut cards = match inspector.pane {
                            Pane::Issues => Vec::new(),
                            _ if inspector.tab == usize::MAX => Vec::new(),
                            _ => self.inspector_cards(found, inspector.tab),
                        };
                        // A picture's tags are its EXIF and its XMP both.
                        if inspector.picture && inspector.pane == Pane::Metadata {
                            cards.extend(self.inspector_cards(found, 3));
                        }
                        // The static HDR figures as ffprobe found them, where
                        // jog's own reading has none.
                        let info = found.hdr.as_ref().filter(|info| info.is_hdr() && !inspector.picture).map(|info| {
                            let mut info = info.clone();
                            let mastering = found.side("Mastering display metadata");
                            info.mastering = info.mastering.or_else(|| mastering.and_then(|m| Some((ratio(&m["max_luminance"])?, ratio(&m["min_luminance"])?))));
                            let light = found.side("Content light level metadata");
                            info.light = info.light.or_else(|| light.and_then(|l| Some((l["max_content"].as_u64()? as u32, l["max_average"].as_u64()? as u32))));
                            info
                        });
                        let mut blocks: Vec<Block> = Vec::new();
                        match inspector.pane {
                            Pane::Overview => blocks.push(Block::Hero),
                            Pane::Issues => blocks.push(Block::Issues),
                            Pane::Color if info.is_some() => blocks.push(Block::HdrHero),
                            _ => {}
                        }
                        blocks.extend(cards.iter().map(Block::Card));
                        match inspector.pane {
                            Pane::Overview => blocks.push(Block::Anatomy),
                            Pane::Tracks => blocks.push(Block::Tracks),
                            Pane::Color if inspector.picture => blocks.extend([Block::PictureCharts, Block::Scopes]),
                            Pane::Color => blocks.extend([Block::SceneChart, Block::Agtm, Block::Plus, Block::Ladder, Block::Scopes]),
                            Pane::Timeline => blocks.extend([Block::Bitrate, Block::Chapters]),
                            Pane::Convert if inspector.picture => blocks.push(Block::Convert),
                            Pane::Convert => blocks.extend([Block::ConvertSource, Block::ConvertOutput, Block::ConvertResult]),
                            Pane::Compare => blocks.push(Block::Compare),
                            _ => {}
                        }
                        // Columns of at least 26 lines of text, up to four.
                        let gap = pad;
                        let avail = content_w - pad * 3;
                        let columns = ((avail + gap) / (px as i32 * 26 + gap)).clamp(1, 4).min(blocks.len().max(1) as i32);
                        // No wider than reads well: a lone block in a wide
                        // dock keeps to the left rather than stretching.
                        let column_w = ((avail - gap * (columns - 1)) / columns).min(px as i32 * 48);
                        let mut heights = vec![pad / 2; columns as usize];
                        let seconds = self.video.as_ref().filter(|video| video.id == inspector.id).and(self.video_time()).map_or(0.0, |(at, _)| at);
                        for block in &blocks {
                            let column = heights.iter().enumerate().min_by_key(|(_, h)| **h).map_or(0, |(at, _)| at);
                            super::devtools::set_flow_x(pad * 3 / 2 + column as i32 * (column_w + gap));
                            let top = heights[column] - inspector.scroll;
                            let end = match block {
                                Block::Hero => self.draw_overview_hero(&mut sub, text, px, found, column_w, top),
                                Block::HdrHero => self.draw_hdr_hero(&mut sub, text, px, info.as_ref(), column_w, top),
                                Block::Issues => self.draw_issues(&mut sub, text, px, found, column_w, top),
                                Block::Card(card) => self.draw_card(&mut sub, text, px, card, column_w, top, offset, mouse, &mut drawn),
                                Block::Anatomy => self.draw_anatomy(&mut sub, text, px, found, column_w, top),
                                Block::Tracks => self.draw_track_choices(&mut sub, text, px, inspector.id, column_w, top, offset, mouse, &mut drawn),
                                Block::PictureCharts => match found.picture.as_ref() {
                                    Some(picture) => self.draw_picture_charts(&mut sub, text, px, picture, column_w, top),
                                    None => top,
                                },
                                Block::SceneChart => self.draw_scene_chart(&mut sub, text, px, found, column_w, top, offset, &mut drawn),
                                Block::Agtm => match info.as_ref().and_then(|info| info.agtm_at(seconds).map(|agtm| (info, agtm))) {
                                    Some((info, agtm)) => self.draw_agtm(&mut sub, text, px, info, agtm, column_w, top, offset, mouse, inspector.gain, &mut drawn),
                                    None => top,
                                },
                                Block::Plus => match info.as_ref().and_then(|info| info.hdr10plus_at(seconds).map(|plus| (info, plus))) {
                                    Some((info, plus)) => super::inspect_hdr::draw_plus(&mut sub, text, px, plus, info.hdr10plus.len(), column_w, top),
                                    None => top,
                                },
                                Block::Ladder => match info.as_ref() {
                                    Some(info) => super::inspect_hdr::draw_ladder(&mut sub, text, px, info, seconds, self.hdr_headroom, column_w, top),
                                    None => top,
                                },
                                Block::Scopes => self.draw_actions(&mut sub, text, px, &self.scope_actions(), column_w, top, offset, mouse, &mut drawn),
                                Block::Bitrate => self.draw_bitrate(&mut sub, text, px, inspector.packets.as_ref(), inspector.id, column_w, top, offset, &mut drawn),
                                Block::Chapters => self.draw_timeline(&mut sub, text, px, column_w, top, offset, mouse, &mut drawn),
                                Block::Convert => self.draw_actions(&mut sub, text, px, &self.convert_actions(inspector.picture), column_w, top, offset, mouse, &mut drawn),
                                Block::ConvertSource => self.draw_convert_source(&mut sub, text, px, found, inspector.id, column_w, top, offset, mouse, &mut drawn),
                                Block::ConvertOutput => self.draw_convert_output(&mut sub, text, px, found, column_w, top, offset, mouse, &mut drawn),
                                Block::ConvertResult => self.draw_convert_result(&mut sub, text, px, found, inspector.id, column_w, top, offset, mouse, &mut drawn),
                                Block::Compare => self.draw_compare_tab(&mut sub, text, px, found, inspector.id, column_w, top, offset, mouse, &mut drawn),
                            };
                            if end > top {
                                heights[column] = end + inspector.scroll + gap / 2;
                            }
                        }
                        content_h = heights.into_iter().max().unwrap_or(0);
                    }
                }
            }
            canvas.dissolve(left, view_top, content_w, inspector.view_h, &sheet, 256);
        }
        inspector.content_h = content_h;
        // The drawer: what is about jog rather than the file, under the tab
        // (down the side) or beside it (along the bottom), with a row of
        // its own tabs: the playlist, the console, how jog is doing.
        if drawer {
            let (drawer_top, drawer_bottom) = if beside { (view_top, foot) } else { (view_bottom, foot) };
            let (left, width) = if beside { (left + content_w, width - content_w) } else { (left, width) };
            canvas.fill(left, drawer_top, width, drawer_bottom - drawer_top, GRID_COLOR);
            if beside {
                canvas.fill(left, drawer_top, 1, drawer_bottom - drawer_top, MENU_EDGE_COLOR);
            } else {
                canvas.fill(left, drawer_top, width, 1, MENU_EDGE_COLOR);
            }
            let drawer_panes: Vec<Pane> = DrawerPane::ALL.iter().map(|pane| pane.as_pane()).collect();
            let at = DrawerPane::ALL.iter().position(|pane| *pane == self.drawer_pane).unwrap_or(0);
            let mut tab_hits = Vec::new();
            let body_top = self.draw_tab_row(canvas, text, px, [left, drawer_top + 1, width], &drawer_panes, at, mouse, &mut tab_hits);
            inspector.drawn.extend(tab_hits.into_iter().map(|(hit, place)| match hit {
                Hit::Tab(at) => (Hit::DrawerTab(DrawerPane::ALL[at]), place),
                other => (other, place),
            }));
            match self.drawer_pane {
                DrawerPane::Playlist => {
                    self.inspector_list.set(Some([left, body_top - pad / 2, width, drawer_bottom]));
                    self.inspector_list_drawing.set(true);
                    if let Some(panel) = self.list_panel() {
                        for pos in panel.first..panel.first + panel.rows {
                            self.want_duration(self.list[pos]);
                        }
                        let mut column = canvas.copy(left, 0, width, drawer_bottom);
                        if column.len() == (width * drawer_bottom) as usize {
                            self.draw_list(&mut Canvas::new(&mut column, width as u32, drawer_bottom as u32), text, &panel);
                            let from = body_top - pad / 2;
                            canvas.paste(left, from, width, drawer_bottom - from, &column[(from * width) as usize..]);
                        }
                    }
                    self.inspector_list_drawing.set(false);
                }
                other => {
                    self.inspector_list.set(None);
                    let h = drawer_bottom - body_top;
                    let mut sheet = canvas.copy(left, body_top, width, h);
                    if h > 0 && sheet.len() == (width * h) as usize {
                        let mut sub = Canvas::new(&mut sheet, width as u32, h as u32);
                        let mut hits = Vec::new();
                        let drawer_inner = width - pad * 3;
                        match other {
                            DrawerPane::Console => drop(self.draw_console(&mut sub, text, px, drawer_inner, 0, (left, body_top), mouse, &mut hits)),
                            _ => {
                                let cards = self.performance_cards();
                                self.draw_cards(&mut sub, text, px, &cards, drawer_inner, (left, body_top), mouse, &mut hits, 0);
                            }
                        }
                        canvas.paste(left, body_top, width, h, &sheet);
                        inspector.drawn.extend(hits.into_iter().filter(|(_, [_, y, _, hh])| *y + *hh > body_top && *y < drawer_bottom));
                    }
                }
            }
        } else {
            self.inspector_list.set(None);
        }
        // Hits inside the content count only where it shows.
        inspector.drawn.extend(drawn.into_iter().filter(|(_, [_, y, _, h])| *y + *h > view_top && *y < view_bottom));
        // A sign that there is more below.
        if content_h - inspector.scroll > inspector.view_h {
            canvas.fill(inner, view_bottom - 1, inner_w, 1, MENU_EDGE_COLOR);
        }
        inspector.kept = Some((key, canvas.copy(left, top, width, height)));
        if let Some(behind) = behind.filter(|behind| behind.len() == (width * height) as usize) {
            canvas.dissolve(left, top, width, height, &behind, through);
        }
        inspector.transport = self.draw_transport(canvas, text, px, [left, top + height - transport_h, width, transport_h], inspector.picture);
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
            sub.round_fill(x, y, card_w, card_h, pad / 2, super::devtools::WELL, 256);
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
        let x = flow_x();
        let chart_h = px as i32 * 9;
        let card_h = pad + small_line + pad / 2 + chart_h + pad / 2 + small_line + pad;
        sub.round_fill(x, y, inner_w, card_h, pad / 2, super::devtools::WELL, 256);
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
        let caption = clipped(text, &caption, cw, small);
        text.draw(sub, cx, cy + chart_h + pad / 2, small, DIM_TEXT_COLOR, &caption);
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
        let x = flow_x();
        let nodes = match &found.tree {
            Ok(nodes) => nodes,
            Err(error) => {
                text.draw(sub, x, pad, px, DIM_TEXT_COLOR, &format!("Not readable · {error}"));
                return 0;
            }
        };
        // The tree on top, all the width; the box picked out under it,
        // which stays put as the tree scrolls.
        let tree_w = inner_w;
        let per_row = 8;
        let card_h = node_at(nodes, inspector.selected).map_or(0, |node| {
            let hex_rows = node.bytes.len().div_ceil(per_row) as i32;
            pad + line + small_line + pad / 2 + node.fields.len() as i32 * (line + pad / 4) + pad / 2 + hex_rows * (small_line + pad / 6) + pad
        }).min(inspector.view_h * 3 / 5);
        let tree_h = inspector.view_h - card_h - pad;
        let row_h = line + pad / 5;
        let visible = visible_nodes(nodes, inspector.open);
        // Keep the one picked out in view after a key.
        if *inspector.follow {
            *inspector.follow = false;
            if let Some(at) = visible.iter().position(|(path, _)| path == inspector.selected) {
                let top = at as i32 * row_h;
                if top < *inspector.scroll {
                    *inspector.scroll = top;
                } else if top + row_h > *inspector.scroll + tree_h {
                    *inspector.scroll = top + row_h - tree_h;
                }
            }
        }
        let size_x = x + tree_w - pad / 2;
        for (row, (path, depth)) in visible.iter().enumerate() {
            let y = row as i32 * row_h - *inspector.scroll;
            if y < 0 || y + row_h > tree_h {
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
        let content = visible.len() as i32 * row_h + card_h + pad;
        let Some(node) = node_at(nodes, inspector.selected) else { return content };
        let (dx, dw) = (x, inner_w);
        let mut y = inspector.view_h - card_h;



        sub.round_fill(dx, y, dw, card_h, pad / 2, super::devtools::WELL, 256);
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
            let shown = clipped(text, value, dw / 3, px);
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
        for (row, chunk) in node.bytes.chunks(per_row).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            let ascii: String = chunk.iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '.' }).collect();
            let words = format!("{:08X}  {}  {ascii}", node.offset + (row * per_row) as u64, hex.join(" "));
            let words = clipped(text, &words, dw - 2 * pad, small);
            text.draw(sub, dx + pad, y, small, if row == 0 { TEXT_COLOR } else { DIM_TEXT_COLOR }, &words);
            y += small_line + pad / 6;
        }
        content
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

/// A picture's cards, by tab: the summary, its EXIF by group, its color
/// profile and gain map, its XMP.
fn picture_cards(picture: &crate::picmeta::Picture, tab: usize) -> Vec<Card> {
    // A field left empty by the camera reads as NaN: not there.
    let exif = |tag: &str| picture.exif.iter().find(|(_, name, _)| name == tag).map(|(_, _, value)| value.clone()).filter(|value| !value.contains("NaN"));
    // 1/20.0024 s, as cameras round it: 1/20 s.
    let exposure = exif("ExposureTime").map(|time| match time.strip_prefix("1/").and_then(|rest| rest.strip_suffix(" s")).and_then(|d| d.parse::<f64>().ok()) {
        Some(d) => format!("1/{d:.0} s"),
        None => time,
    });
    match tab {
        0 => {
            let mut file = Card::new("File");
            file.row("Format", picture.format.clone());
            file.row("Size", picture.size.map(|(w, h)| format!("{w} x {h}  ·  {:.1} MP", f64::from(w) * f64::from(h) / 1e6)));
            file.row("On disk", Some(meta::human_size(picture.bytes)));
            file.row("Color profile", picture.icc.as_ref().map(|(name, _)| name.clone()));
            file.row("Ultra HDR", picture.gain.as_ref().map(|(meta, _)| format!("gain map, up to {:.1} stops", meta.max.iter().copied().fold(0.0, f32::max))));
            let mut shot = Card::new("Taken");
            shot.row("Camera", [exif("Make"), exif("Model")].into_iter().flatten().reduce(|a, b| format!("{a} {b}")));
            shot.row("Lens", exif("LensModel"));
            shot.row("When", exif("DateTimeOriginal").or_else(|| exif("DateTime")));
            shot.row("Exposure", [exposure, exif("FNumber"), exif("PhotographicSensitivity").map(|iso| format!("ISO {iso}"))].into_iter().flatten().reduce(|a, b| format!("{a}  ·  {b}")));
            shot.row("Focal length", exif("FocalLength"));
            shot.row("Software", exif("Software"));
            shot.row("Where", exif("GPSLatitude").zip(exif("GPSLongitude")).map(|(lat, lon)| format!("{lat}  ·  {lon}")));
            if shot.rows.is_empty() {
                shot.row("EXIF", Some("None".into()));
            }
            vec![file, shot]
        }
        1 => {
            let mut cards: Vec<Card> = Vec::new();
            for (group, tag, value) in &picture.exif {
                if cards.last().is_none_or(|card| card.title != *group) {
                    cards.push(Card::new(group.clone()));
                }
                if let Some(card) = cards.last_mut() {
                    card.row(tag, Some(value.clone()));
                }
            }
            if cards.is_empty() {
                let mut none = Card::new("EXIF");
                none.row("Fields", Some("None".into()));
                cards.push(none);
            }
            cards
        }
        2 => {
            let mut color = Card::new("Color profile");
            match &picture.icc {
                Some((name, bytes)) => {
                    color.row("Name", Some(name.clone()));
                    color.row("Embedded", Some(meta::human_size(*bytes as u64)));
                }
                None => color.row("Profile", Some("None embedded  ·  shown as sRGB".into())),
            }
            color.row("EXIF color space", exif("ColorSpace"));
            // What the profile and the file's own chunks say, read.
            let c = &picture.color;
            color.row("Profile", c.icc.clone());
            for (channel, curve) in &c.curves {
                let name = match channel {
                    'R' => "Red curve",
                    'G' => "Green curve",
                    'B' => "Blue curve",
                    _ => "Gray curve",
                };
                color.row(name, Some(curve.describe()));
            }
            if let Some([r, g, b, w]) = c.primaries {
                let xy = |(x, y): (f64, f64)| format!("{x:.3}, {y:.3}");
                color.row("Red", Some(xy(r)));
                color.row("Green", Some(xy(g)));
                color.row("Blue", Some(xy(b)));
                color.row("White", Some(xy(w)));
            }
            if c.lut {
                color.row("Mapping", Some("by lookup table".into()));
            }
            color.row("Peak", c.luminance.map(|nits| format!("{nits:.0} nits")));
            color.row("Coding points", c.cicp.map(crate::colorinfo::cicp_words));
            color.row("PNG gAMA", c.png_gamma.map(|g| format!("{g:.5}  ·  gamma {:.2}", 1.0 / g.max(1e-9))));
            color.row("PNG sRGB", c.srgb_intent.map(|intent| ["perceptual", "relative colorimetric", "saturation", "absolute colorimetric"].get(usize::from(intent)).copied().unwrap_or("other").to_string()));
            let mut gain = Card::new("Ultra HDR gain map");
            match &picture.gain {
                Some((meta, bytes)) => {
                    gain.badges.push("Ultra HDR".into());
                    let stops = |v: [f32; 3]| if v[0] == v[1] && v[1] == v[2] { format!("{:.2}", v[0]) } else { format!("{:.2}, {:.2}, {:.2}", v[0], v[1], v[2]) };
                    gain.row("Boost, least", Some(format!("{} stops", stops(meta.min))));
                    gain.row("Boost, most", Some(format!("{} stops", stops(meta.max))));
                    gain.row("Gamma", Some(stops(meta.gamma)));
                    gain.row("Offset SDR", Some(stops(meta.offset_sdr)));
                    gain.row("Offset HDR", Some(stops(meta.offset_hdr)));
                    gain.row("Headroom", Some(format!("{:.2} stops  ·  {:.1}x", meta.capacity_max, 2f32.powf(meta.capacity_max))));
                    gain.row("Gain map", Some(meta::human_size(*bytes as u64)));
                }
                None => gain.row("Gain map", Some("None  ·  an SDR picture".into())),
            }
            vec![color, gain]
        }
        _ => {
            let mut cards: Vec<Card> = Vec::new();
            for (name, value) in &picture.xmp {
                let space = name.split(':').next().unwrap_or_default().to_string();
                if cards.last().is_none_or(|card| card.title != space) {
                    cards.push(Card::new(space));
                }
                if let Some(card) = cards.last_mut() {
                    card.row(name.split(':').nth(1).unwrap_or(name), Some(value.clone()));
                }
            }
            if cards.is_empty() {
                let mut none = Card::new("XMP");
                none.row("Properties", Some("None".into()));
                cards.push(none);
            }
            cards
        }
    }
}

impl App {
    /// Whether a window position is on the inspector's panel.
    pub(super) fn over_inspector(&self, x: f64, y: f64) -> bool {
        // Popped out, it is not in the main window at all; nor is the main
        // window's what a popped-out one is answering.
        self.drawing_popout.is_none() && !self.popped(Popped::Inspector) && self.inspector.as_ref().is_some_and(|inspector| {
            let [left, top, w, h] = inspector.place;
            (left..left + w).contains(&(x as i32)) && (top..top + h).contains(&(y as i32))
        })
    }
}

impl App {
    /// The inspector's width, and whether it stands on the left, for the
    /// picture or video to keep clear of it: the same sums it is drawn by.
    pub(super) fn inspector_room(&self) -> Option<(i32, bool)> {
        self.inspector.as_ref().filter(|_| self.inspector_pinned && !self.inspector_bottom && !self.popped(Popped::Inspector))?;
        let (width, height) = self.window_size();
        Some((panel_width(width as i32, ui_px(height), self.inspector_units), self.list_right))
    }

    /// How tall the inspector is, docked along the bottom.
    pub(super) fn inspector_height(&self, window_height: i32) -> i32 {
        let px = f64::from(ui_px(window_height as u32));
        ((self.inspector_rows * px) as i32).clamp((px * 12.0) as i32, window_height * 2 / 3)
    }

    /// The room the inspector takes along the bottom, pinned there.
    pub(super) fn inspector_bottom_room(&self) -> Option<i32> {
        self.inspector.as_ref().filter(|_| self.inspector_pinned && self.inspector_bottom && !self.popped(Popped::Inspector))?;
        Some(self.inspector_height(self.window_size().1 as i32))
    }

    /// Docks the inspector along the bottom, or back down the side;
    /// remembered for next time.
    pub(super) fn toggle_inspector_dock(&mut self) {
        self.inspector_bottom = !self.inspector_bottom;
        self.save_panels();
        self.apply_video_view();
        self.redraw();
    }

    /// Pins the inspector beside the picture, which makes room for it, or
    /// lets it lie over it; remembered for next time.
    pub(super) fn toggle_inspector_pin(&mut self) {
        self.inspector_pinned = !self.inspector_pinned;
        self.save_panels();
        self.apply_video_view();
        self.redraw();
    }
}

impl App {
    /// Tells the player to draw the video in the part of the window the
    /// picture has, clear of the panels pinned beside and below it; and
    /// back across the whole window once none is.
    pub(super) fn keep_video_clear(&self) {
        let (width, height) = self.window_size();
        let (left, (stage_w, stage_h)) = self.stage();
        let across = f64::from(width.max(1));
        let right = (width as i32 - left - stage_w as i32).max(0);
        let below = height.saturating_sub(stage_h);
        let want = (f64::from(left.max(0)) / across, f64::from(right) / across, f64::from(below) / f64::from(height.max(1)));
        if self.video_margins.get() == want {
            return;
        }
        self.video_margins.set(want);
        for player in self.players() {
            player.set("video-margin-ratio-left", &format!("{:.4}", want.0));
            player.set("video-margin-ratio-right", &format!("{:.4}", want.1));
            player.set("video-margin-ratio-bottom", &format!("{:.4}", want.2));
            // Subtitles in the video, not under the panel beside it.
            player.set("sub-use-margins", if want == (0.0, 0.0, 0.0) { "yes" } else { "no" });
        }
        // The zoom and pan are of that part, which has changed.
        self.apply_video_view();
        self.redraw();
    }
}

/// What the inspector's panel is drawn from: when none of it changes, the
/// panel drawn last is the panel.
#[derive(Clone, PartialEq, Debug)]
pub struct PanelKey {
    pane: Pane,
    /// For a tab whose figures keep changing: which half second it is.
    tick: Option<u128>,
    tab: usize,
    scroll: i32,
    found: bool,
    selected: Vec<usize>,
    open: usize,
    follow: bool,
    /// The pointer, while it is over the panel.
    pointer: Option<(i32, i32)>,
    place: [i32; 4],
    window: (i32, i32),
    /// Pinned beside the picture, or over it.
    pinned: bool,
    /// The HDR tab's charts: the headroom, the metadata at the time on view.
    hdr: Option<super::inspect_hdr::HdrKey>,
}

impl App {
    /// A video's packets read, for the Timeline, if it is still the one
    /// inspected.
    pub(super) fn packets_read(&mut self, id: usize, packets: Result<super::devtools::Packets, String>) {
        if let Some(inspector) = self.inspector.as_mut().filter(|inspector| inspector.id == id) {
            inspector.packets = Some(packets);
            self.redraw();
        }
    }
}
