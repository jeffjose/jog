//! Edit: the picture on view drawn on, cropped and its corners rounded,
//! then saved over itself or as a copy beside it. The marks stay
//! editable until it is saved: picked out with the arrow, one or several,
//! moved, sized by their handles, restyled from the panel, whose numbers
//! are dragged or typed as Figma's are. Saving is done on another thread,
//! from the file decoded again at full size.

use std::sync::Mutex;

use image::DynamicImage;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use crate::annotate::{self, Dash, Place, Point, Shape, Tool, BADGE_NAMES, BOLD, BOXED, BULLETS, COLORS, HIGHLIGHT, ITALIC, PALETTE};
use crate::lettering::{self, Look, FACES};

use super::*;

/// What is drawn and done to the picture: everything Undo goes back over.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Doc {
    pub shapes: Vec<Shape>,
    /// What is kept of it: left, top, right, bottom, in its pixels.
    pub crop: Option<[f32; 4]>,
    /// How round its corners are, in its pixels.
    pub radius: f32,
}

/// The two ways out with what was drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SaveAs {
    #[default]
    Overwrite,
    Copy,
}

/// How the next mark is drawn; the panel sets it along with the marks
/// picked out, as hotshot does.
#[derive(Clone, Copy, Debug)]
struct Style {
    color: u32,
    /// Out of 256.
    opacity: u32,
    /// How wide lines are, and how tall words, in the picture's pixels.
    stroke: f32,
    letters: f32,
    filled: bool,
    heads: [bool; 2],
    dash: Dash,
    /// How long dashes are, or how wide dots, and the gap between them;
    /// nought for what suits the width.
    dash_length: f32,
    gap: f32,
    face: usize,
    text: u8,
    badge: usize,
    square: bool,
}

/// A number in the panel, dragged sideways or typed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    X,
    Y,
    W,
    H,
    Stroke,
    Opacity,
    Radius,
    DashLength,
    Gap,
    Times,
    Corners,
    CropW,
    CropH,
    /// How far a mark is turned, in degrees.
    Angle,
    /// The color, in hex.
    Hex,
}

pub struct Editor {
    pub id: usize,
    /// The picture's size as it is shown, turned if it is.
    native: (u32, u32),
    doc: Doc,
    /// The doc as it was opened, to tell whether anything has changed.
    opened: Doc,
    past: Vec<Doc>,
    future: Vec<Doc>,
    tool: Tool,
    /// The tool to go back to once a color is picked.
    before_pipette: Tool,
    style: Style,
    selected: Vec<usize>,
    /// The mark under the pointer, with the arrow in hand.
    hovered: Option<usize>,
    /// The words being typed into, by the shape's place; where among them
    /// the caret is, in letters; and where a selection runs from, to the
    /// caret.
    typing: Option<usize>,
    caret: usize,
    anchor: Option<usize>,
    /// A number of the panel being typed: which, what is typed so far,
    /// and whether the first key replaces what was there.
    editing: Option<(Field, String, bool)>,
    /// The crop being set, before Enter takes it, and the shape it is
    /// kept to, width over height.
    crop_draft: Option<[f32; 4]>,
    aspect: Option<f32>,
    drag: Option<EditDrag>,
    picker: Option<Picker>,
    /// The colors beyond the eight: open, with the hue, saturation and
    /// value it shows, kept apart so that a gray keeps its hue.
    flyout: Option<(f32, f32, f32)>,
    /// Where it opens, when a right click opened it rather than the panel.
    flyout_at: Option<(i32, i32)>,
    /// The ways a tool can draw, listed over it: which tool, and where its
    /// button is.
    tool_menu: Option<(Tool, i32)>,
    /// The straight edges found in the picture, its sides and seams: down at
    /// these places across, and across at these down, for marks and the crop
    /// to rest against.
    edges: (Vec<f32>, Vec<f32>),
    /// The lines what is being moved has come to rest against: across at
    /// these places, and down at these.
    guides: (Vec<f32>, Vec<f32>),
    /// Escape with something unsaved: the box asking what to do, and the
    /// choice in it that Enter takes.
    leaving: Option<usize>,
    /// As last drawn: where the picture is, the part of it on show, and
    /// what there is to click.
    place: Place,
    stage: [i32; 4],
    hits: Vec<(EditHit, [i32; 4])>,
    handles: Vec<(Handle, (i32, i32))>,
    tips: Vec<([i32; 4], String)>,
    reach: i32,
    cursor: CursorIcon,
}

/// The emoji to pick from: what is typed to find one, and how far down
/// the list it is scrolled, in rows.
#[derive(Default)]
struct Picker {
    query: String,
    scroll: usize,
}

/// What a drag is doing.
#[derive(Clone, Debug)]
enum EditDrag {
    /// A mark being drawn, from where the drag began.
    Draw { from: Point },
    Move { from: Point, before: Doc, moved: bool, was_selected: bool },
    Marquee { from: Point, to: Point, adding: bool },
    Grip { grip: (i8, i8), frame: [f32; 4], from: Point, before: Doc },
    End { end: usize, before: Doc, moved: bool },
    Bend { before: Doc, moved: bool },
    Round { corner: (i8, i8), before: Doc },
    Zoom { rim: bool, before: Doc },
    /// Picking out letters of the words being typed.
    Caret,
    /// Turning it about `centre`, from the angle the pointer began at.
    Rotate { centre: Point, from: f32, before: Doc },
    Crop { from: Point },
    CropGrip { grip: (i8, i8), start: [f32; 4] },
    CropMove { from: Point, start: [f32; 4] },
    /// A number dragged sideways: where from, and what it was.
    Scrub { field: Field, x: f64, start: f32, before: Doc, moved: bool },
    /// The color picker's square, or its strip of hues.
    Pick { hue: bool, before: Doc },
}

/// The handles of the mark picked out, and of the crop.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    /// A corner or the middle of a side, by which way it pulls.
    Grip(i8, i8),
    End(usize),
    Bend,
    Round(i8, i8),
    ZoomMove,
    ZoomRim,
    /// Just outside a corner: turns it.
    Rotate,
    Crop(i8, i8),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum EditHit {
    Tool(Tool),
    Undo,
    Redo,
    Reset,
    Color(u32),
    /// The colors beyond the eight, opened or closed.
    More,
    Pipette,
    Square,
    HueStrip,
    Field(Field),
    Filled(bool),
    Heads([bool; 2]),
    Dash(Dash),
    Face(usize),
    Toggle(u8),
    BadgeLook(usize),
    Lens(bool),
    Aspect(Option<f32>),
    ApplyCrop,
    ResetCrop,
    Save(SaveAs),
    /// The picture as edited, onto the clipboard.
    Clipboard,
    /// One of the choices on leaving with something unsaved, by its place
    /// in `LEAVE`.
    Leave(usize),
    Emoji(usize),
    Inside,
}

/// What came of saving, sent back from the thread that did it.
pub struct Saved {
    id: usize,
    target: PathBuf,
    over: bool,
    result: Result<(u64, Option<PathBuf>), String>,
}

/// How long the originals kept for Undo after a save over them are kept.
const BACKUPS_KEPT: Duration = Duration::from_secs(7 * 24 * 3600);
/// The emoji picker: how many across, and how many rows show.
const EMOJI_ACROSS: usize = 10;
const EMOJI_DOWN: usize = 7;
/// How near, on screen, what is moved comes to an edge before it rests
/// against it.
const SNAP: f32 = 6.0;
/// The shapes a crop can be kept to, by what they are called.
const ASPECTS: [(&str, Option<f32>); 5] = [("Free", None), ("Orig", Some(0.0)), ("1:1", Some(1.0)), ("4:3", Some(4.0 / 3.0)), ("16:9", Some(16.0 / 9.0))];

/// What can be done on leaving with something unsaved, and the key each
/// is pressed with.
const LEAVE: [(&str, &str); 4] = [("Keep editing", "Esc"), ("Discard", "D"), ("Save copy", "C"), ("Overwrite", "S")];

/// The colors chosen lately from beyond the eight, the latest first.
static RECENT: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn remember_color(color: u32) {
    if let Ok(mut recent) = RECENT.lock() {
        recent.retain(|had| *had != color);
        recent.insert(0, color);
        recent.truncate(8);
    }
}

/// A number as the panel shows it: whole, or to a tenth when small.
fn shown(value: f32) -> String {
    if value >= 20.0 || (value - value.round()).abs() < 0.05 { format!("{}", value.round()) } else { format!("{value:.1}") }
}

impl Editor {
    fn new(id: usize, native: (u32, u32)) -> Editor {
        Editor {
            id,
            native,
            doc: Doc::default(),
            opened: Doc::default(),
            past: Vec::new(),
            future: Vec::new(),
            tool: Tool::Rect,
            before_pipette: Tool::Select,
            style: Style {
                color: COLORS[0],
                opacity: 256,
                stroke: annotate::widths(native)[1],
                letters: annotate::text_sizes(native)[1],
                filled: false,
                heads: [false, true],
                dash: Dash::Solid,
                dash_length: 0.0,
                gap: 0.0,
                face: 0,
                text: 0,
                badge: 0,
                square: false,
            },
            selected: Vec::new(),
            hovered: None,
            typing: None,
            caret: 0,
            anchor: None,
            editing: None,
            crop_draft: None,
            aspect: None,
            drag: None,
            picker: None,
            flyout: None,
            flyout_at: None,
            tool_menu: None,
            edges: (Vec::new(), Vec::new()),
            guides: (Vec::new(), Vec::new()),
            leaving: None,
            place: Place::WHOLE,
            stage: [0; 4],
            hits: Vec::new(),
            handles: Vec::new(),
            tips: Vec::new(),
            reach: 6,
            cursor: CursorIcon::Default,
        }
    }

    pub fn dirty(&self) -> bool {
        self.doc != self.opened
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Kept for Undo before a change.
    fn snap(&mut self) {
        self.past.push(self.doc.clone());
        self.future.clear();
    }

    /// What `before` was kept for Undo, if anything has changed since.
    fn settle(&mut self, before: Doc) {
        if before != self.doc {
            self.past.push(before);
            self.future.clear();
        }
    }

    fn undo(&mut self) {
        self.finish_typing();
        if let Some(doc) = self.past.pop() {
            self.future.push(std::mem::replace(&mut self.doc, doc));
            self.selected.clear();
        }
    }

    fn redo(&mut self) {
        self.finish_typing();
        if let Some(doc) = self.future.pop() {
            self.past.push(std::mem::replace(&mut self.doc, doc));
            self.selected.clear();
        }
    }

    /// Everything back as it was opened, which Undo can still take back.
    fn reset(&mut self) {
        self.finish_typing();
        if self.dirty() {
            self.snap();
            self.doc = self.opened.clone();
            self.selected.clear();
            self.crop_draft = None;
        }
    }

    /// What a new mark of `tool` is drawn as wide, or as tall, as.
    fn default_stroke(&self, tool: Tool) -> f32 {
        match tool {
            Tool::Text | Tool::Badge => self.style.letters,
            Tool::Emoji => self.style.letters * 2.0,
            _ => self.style.stroke,
        }
    }

    /// The one mark picked out, if it is one.
    fn one(&self) -> Option<usize> {
        match self.selected.as_slice() {
            [one] if *one < self.doc.shapes.len() => Some(*one),
            _ => None,
        }
    }

    /// The mark the panel speaks of: the one being typed, or the first of
    /// those picked out.
    fn subject(&self) -> Option<usize> {
        self.typing.or(self.selected.first().copied()).filter(|at| *at < self.doc.shapes.len())
    }

    /// The marks a change goes to: those picked out, or being typed.
    fn targets(&self) -> Vec<usize> {
        let mut targets = self.selected.clone();
        targets.extend(self.typing);
        targets.retain(|at| *at < self.doc.shapes.len());
        targets
    }

    /// A new mark of `tool` at `at`, drawn as the panel says.
    fn fresh(&self, tool: Tool, at: Point) -> Shape {
        let mut shape = Shape::new(tool, self.style.color, self.default_stroke(tool), at);
        shape.opacity = self.style.opacity;
        match tool {
            Tool::Rect | Tool::Oval => shape.filled = self.style.filled,
            Tool::Arrow => shape.heads = self.style.heads,
            Tool::Zoom => shape.filled = self.style.square,
            _ => {}
        }
        if tool.is_line() || matches!(tool, Tool::Rect | Tool::Oval) {
            shape.set_dash(self.style.dash);
            if self.style.dash_length > 0.0 {
                (shape.dash_length, shape.gap) = (self.style.dash_length, self.style.gap);
            }
        }
        shape.face = self.style.face;
        shape.style = self.style.text;
        shape.look = self.style.badge;
        shape
    }

    /// Changes the marks picked out, kept for Undo if they changed.
    fn restyle(&mut self, change: impl Fn(&mut Shape)) {
        let before = self.doc.clone();
        for at in self.targets() {
            change(&mut self.doc.shapes[at]);
        }
        self.settle(before);
    }

    /// The color of what is picked out, and of what is drawn next, with
    /// nothing kept for Undo: the color picker's drags do that at the end.
    fn recolor(&mut self, color: u32) {
        self.style.color = color;
        for at in self.targets() {
            self.doc.shapes[at].color = color;
        }
    }

    /// The part of the picture on show: all of it while cropping, so the
    /// crop can be dragged anywhere.
    fn region(&self) -> [f32; 4] {
        let whole = self.whole();
        match (self.tool, self.doc.crop) {
            (Tool::Crop, _) | (_, None) => whole,
            (_, Some(crop)) => crop,
        }
    }

    fn whole(&self) -> [f32; 4] {
        [0.0, 0.0, self.native.0 as f32, self.native.1 as f32]
    }

    /// The size of what would be saved.
    fn out_size(&self) -> (u32, u32) {
        match self.doc.crop {
            Some([l, t, r, b]) => ((r - l).round() as u32, (b - t).round() as u32),
            None => self.native,
        }
    }

    fn max_radius(&self) -> f32 {
        let (w, h) = self.out_size();
        w.min(h) as f32 / 2.0
    }

    fn set_tool(&mut self, tool: Tool) {
        self.finish_typing();
        self.commit_field();
        // Leaving the crop takes it as it is.
        if self.tool == Tool::Crop && tool != Tool::Crop {
            self.apply_crop();
        }
        if tool == Tool::Crop {
            self.crop_draft = Some(self.doc.crop.unwrap_or(self.whole()));
            self.selected.clear();
        }
        // The pipette keeps what it would color.
        if tool == Tool::Pipette {
            if self.tool != Tool::Pipette {
                self.before_pipette = self.tool;
            }
        } else if tool != Tool::Select {
            self.selected.clear();
        }
        self.picker = (tool == Tool::Emoji).then(Picker::default);
        self.tool = tool;
    }

    /// A mark just made: picked out, with the arrow in hand to adjust it.
    fn made(&mut self, at: usize) {
        self.selected = vec![at];
        self.tool = Tool::Select;
        self.picker = None;
    }

    /// Typing into the words of mark `at`, the caret at letter `caret`, or
    /// all of them picked out to be typed over.
    fn start_typing(&mut self, at: usize, caret: Option<usize>) {
        let count = self.doc.shapes[at].text.chars().count();
        self.typing = Some(at);
        self.selected.clear();
        (self.caret, self.anchor) = match caret {
            Some(caret) => (caret.min(count), None),
            None => (count, Some(0)),
        };
    }

    /// The letters picked out, first to last, if any are.
    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|anchor| *anchor != self.caret)?;
        Some((anchor.min(self.caret), anchor.max(self.caret)))
    }

    fn typed(&self) -> Option<&str> {
        self.typing.map(|at| self.doc.shapes[at].text.as_str())
    }

    /// The letters picked out, as they read.
    fn selected_text(&self) -> Option<String> {
        let (from, to) = self.selection()?;
        Some(self.typed()?.chars().skip(from).take(to - from).collect())
    }

    /// `words` typed at the caret, over what is picked out.
    fn insert(&mut self, words: &str) {
        let Some(at) = self.typing else { return };
        // A badge is one line.
        let words: String = if self.doc.shapes[at].tool == Tool::Badge { words.replace('\n', " ") } else { words.to_string() };
        let (from, to) = self.selection().unwrap_or((self.caret, self.caret));
        let text: Vec<char> = self.doc.shapes[at].text.chars().collect();
        let mut out: String = text[..from].iter().collect();
        out.push_str(&words);
        out.extend(&text[to..]);
        self.doc.shapes[at].text = out;
        self.caret = from + words.chars().count();
        self.anchor = None;
    }

    /// What is picked out gone, or else the letter before the caret, or
    /// with `forward` the one after it.
    fn erase(&mut self, forward: bool) {
        let Some(at) = self.typing else { return };
        let count = self.doc.shapes[at].text.chars().count();
        let span = self.selection().or(match forward {
            true => (self.caret < count).then_some((self.caret, self.caret + 1)),
            false => (self.caret > 0).then_some((self.caret - 1, self.caret)),
        });
        if let Some((from, to)) = span {
            self.anchor = Some(from);
            self.caret = to;
            self.insert("");
        }
    }

    /// The caret taken to `to`; with `extend`, what is picked out grown to
    /// it, and otherwise nothing picked out.
    fn put_caret(&mut self, to: usize, extend: bool) {
        let count = self.typed().map_or(0, |text| text.chars().count());
        if extend {
            self.anchor = self.anchor.or(Some(self.caret));
        } else {
            self.anchor = None;
        }
        self.caret = to.min(count);
    }

    /// Where the caret goes for an arrow, Home or End: a letter along, the
    /// line above or below at as near the same place along it, or the
    /// start or end of the line.
    fn caret_for(&self, key: NamedKey, extend: bool) -> usize {
        let text: Vec<char> = self.typed().unwrap_or_default().chars().collect();
        let caret = self.caret.min(text.len());
        let line_start = |at: usize| text[..at].iter().rposition(|c| *c == '\n').map_or(0, |n| n + 1);
        let line_end = |at: usize| text[at..].iter().position(|c| *c == '\n').map_or(text.len(), |n| at + n);
        let (start, end) = (line_start(caret), line_end(caret));
        match key {
            // Picked out, the arrows go to its ends first, unless with Shift
            // they are growing it.
            NamedKey::ArrowLeft => self.selection().filter(|_| !extend).map_or(caret.saturating_sub(1), |(from, _)| from),
            NamedKey::ArrowRight => self.selection().filter(|_| !extend).map_or((caret + 1).min(text.len()), |(_, to)| to),
            NamedKey::Home => start,
            NamedKey::End => end,
            NamedKey::ArrowUp if start > 0 => {
                let above = line_start(start - 1);
                (above + (caret - start)).min(start - 1)
            }
            NamedKey::ArrowDown if end < text.len() => {
                let below = end + 1;
                (below + (caret - start)).min(line_end(below))
            }
            NamedKey::ArrowUp => 0,
            NamedKey::ArrowDown => text.len(),
            _ => caret,
        }
    }

    /// Words typed and done with: left, and picked out, if there are any;
    /// gone if not.
    fn finish_typing(&mut self) {
        self.anchor = None;
        let Some(at) = self.typing.take() else { return };
        if self.doc.shapes.get(at).is_some_and(|shape| shape.text.trim().is_empty()) {
            self.doc.shapes.remove(at);
            // Words that came to nothing leave nothing to undo.
            if self.past.last() == Some(&self.doc) {
                self.past.pop();
            }
            self.selected.clear();
        } else if at < self.doc.shapes.len() {
            self.made(at);
        }
    }

    /// The crop as it is set, taken, and the arrow back in hand.
    fn apply_crop(&mut self) {
        if let Some(draft) = self.crop_draft.take() {
            // All of it is no crop at all.
            let draft = (draft != self.whole()).then_some(draft);
            if draft != self.doc.crop {
                self.snap();
                self.doc.crop = draft;
                self.doc.radius = self.doc.radius.min(self.max_radius());
            }
        }
        if self.tool == Tool::Crop {
            self.tool = Tool::Select;
        }
    }

    /// A crop kept to the shape asked for: as large as fits in the
    /// picture about the middle of the one there was.
    fn shape_crop(&mut self, aspect: Option<f32>) {
        let (w, h) = (self.native.0 as f32, self.native.1 as f32);
        self.aspect = aspect.map(|ratio| if ratio == 0.0 { w / h } else { ratio });
        let Some(ratio) = self.aspect else { return };
        let [l, t, r, b] = self.crop_draft.unwrap_or(self.whole());
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        let (cw, ch) = if w / h > ratio { (h * ratio, h) } else { (w, w / ratio) };
        let (x, y) = (within(cx - cw / 2.0, 0.0, w - cw), within(cy - ch / 2.0, 0.0, h - ch));
        self.crop_draft = Some([x, y, x + cw, y + ch]);
    }

    fn hit_at(&self, x: i32, y: i32) -> Option<EditHit> {
        self.hits.iter().rev().find(|(_, place)| inside(*place, (x, y))).map(|(hit, _)| *hit)
    }

    fn handle_at(&self, x: i32, y: i32) -> Option<Handle> {
        self.handles.iter().find(|(_, (hx, hy))| (hx - x).abs() <= self.reach && (hy - y).abs() <= self.reach).map(|(handle, _)| *handle)
    }

    /// The box of the mark the panel speaks of, as X, Y, W and H say it.
    fn subject_box(&self) -> Option<[f32; 4]> {
        let shape = &self.doc.shapes[self.one()?];
        Some(match shape.tool {
            Tool::Rect | Tool::Oval | Tool::Blur | Tool::Spotlight | Tool::Zoom | Tool::Arrow | Tool::Line | Tool::Image => shape.frame(),
            Tool::Text | Tool::Emoji | Tool::Badge | Tool::Pen | Tool::Marker => shape.bounds(),
            _ => return None,
        })
    }

    /// A number of the panel, as it is now.
    fn field(&self, field: Field) -> Option<f32> {
        let subject = self.subject().map(|at| &self.doc.shapes[at]);
        let crop = self.crop_draft.unwrap_or(self.whole());
        Some(match field {
            Field::X | Field::Y | Field::W | Field::H => {
                let [l, t, r, b] = self.subject_box()?;
                [l, t, r - l, b - t][field as usize - Field::X as usize]
            }
            Field::Stroke => subject.map_or(self.default_stroke(self.tool), |shape| shape.stroke),
            Field::Opacity => subject.map_or(self.style.opacity, |shape| shape.opacity) as f32 * 100.0 / 256.0,
            Field::Radius => subject?.radius,
            Field::DashLength => subject.map_or(self.style.dash_length, |shape| shape.dash_length),
            Field::Gap => subject.map_or(self.style.gap, |shape| shape.gap),
            Field::Times => subject?.zoom_times(),
            Field::Corners => self.doc.radius,
            Field::CropW => crop[2] - crop[0],
            Field::Angle => {
                let shape = subject.filter(|shape| shape.turnable())?;
                // Shown from -180 to 180, as Figma shows it.
                let degrees = shape.angle.to_degrees();
                if degrees > 180.0 { degrees - 360.0 } else { degrees }
            }
            Field::CropH => crop[3] - crop[1],
            Field::Hex => return None,
        })
    }

    /// A number of the panel set: to the marks picked out, and to what is
    /// drawn next where that keeps it. Nothing is kept for Undo here.
    fn set_field(&mut self, field: Field, value: f32) {
        let targets = self.targets();
        let (w, h) = (self.native.0 as f32, self.native.1 as f32);
        match field {
            Field::X | Field::Y => {
                let Some([l, t, _, _]) = self.subject_box() else { return };
                let (dx, dy) = if field == Field::X { (value - l, 0.0) } else { (0.0, value - t) };
                for at in targets {
                    self.doc.shapes[at].translate(dx, dy);
                }
            }
            Field::W | Field::H => {
                let (Some([l, t, r, b]), Some(at)) = (self.subject_box(), self.one()) else { return };
                let value = value.max(1.0);
                let times = if field == Field::W { (value / (r - l).max(1.0), 1.0) } else { (1.0, value / (b - t).max(1.0)) };
                self.doc.shapes[at].scale((l, t), times);
            }
            Field::Stroke => {
                let value = value.max(0.5);
                let kind = self.subject().map_or(self.tool, |at| self.doc.shapes[at].tool);
                if kind.lettered() {
                    self.style.letters = if kind == Tool::Emoji { value / 2.0 } else { value };
                } else {
                    self.style.stroke = value;
                }
                for at in targets {
                    self.doc.shapes[at].stroke = value;
                }
            }
            Field::Opacity => {
                let opacity = (value.clamp(0.0, 100.0) * 2.56).round() as u32;
                self.style.opacity = opacity;
                for at in targets {
                    self.doc.shapes[at].opacity = opacity;
                }
            }
            Field::Radius => {
                for at in targets {
                    let [l, t, r, b] = self.doc.shapes[at].frame();
                    self.doc.shapes[at].radius = value.clamp(0.0, (r - l).min(b - t) / 2.0);
                }
            }
            Field::DashLength | Field::Gap => {
                let value = value.max(0.5);
                if field == Field::DashLength {
                    self.style.dash_length = value;
                } else {
                    self.style.gap = value;
                }
                for at in targets {
                    let shape = &mut self.doc.shapes[at];
                    if field == Field::DashLength { shape.dash_length = value } else { shape.gap = value }
                }
                // What is drawn next keeps the other as it is on what was set.
                if let Some(at) = self.subject() {
                    (self.style.dash_length, self.style.gap) = (self.doc.shapes[at].dash_length, self.doc.shapes[at].gap);
                }
            }
            Field::Times => {
                if let Some(at) = self.one() {
                    let shape = &mut self.doc.shapes[at];
                    let [ol, ot, or, ob] = shape.frame();
                    let [l, t, r, b] = shape.zoomed();
                    let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
                    let times = value.clamp(1.25, 12.0);
                    let (hw, hh) = ((or - ol) * times / 2.0, (ob - ot) * times / 2.0);
                    if shape.points.len() >= 4 {
                        shape.points[2] = (cx - hw, cy - hh);
                        shape.points[3] = (cx + hw, cy + hh);
                    }
                }
            }
            Field::Corners => self.doc.radius = value.clamp(0.0, self.max_radius()),
            Field::CropW | Field::CropH => {
                let [l, t, r, b] = self.crop_draft.unwrap_or(self.whole());
                let (mut cw, mut ch) = (r - l, b - t);
                if field == Field::CropW { cw = value } else { ch = value }
                if let Some(ratio) = self.aspect {
                    if field == Field::CropW { ch = cw / ratio } else { cw = ch * ratio }
                }
                let (cw, ch) = (within(cw, 8.0, w - l), within(ch, 8.0, h - t));
                self.crop_draft = Some([l, t, l + cw, t + ch]);
            }
            Field::Angle => {
                for at in targets {
                    if self.doc.shapes[at].turnable() {
                        self.doc.shapes[at].angle = value.to_radians().rem_euclid(std::f32::consts::TAU);
                    }
                }
            }
            Field::Hex => {}
        }
    }

    /// The width of the line, or the size of the words, `times` as large:
    /// of what is picked out, and of what is drawn next.
    fn grow(&mut self, times: f32) {
        let Some(now) = self.field(Field::Stroke) else { return };
        let before = self.doc.clone();
        self.set_field(Field::Stroke, (now * times).max(0.5));
        self.settle(before);
    }

    /// How much a number changes for each pixel it is dragged.
    fn step(&self, field: Field) -> f32 {
        let per_pixel = 1.0 / self.place.scale;
        match field {
            Field::Opacity | Field::Angle => 1.0,
            Field::Times => 0.02,
            Field::Stroke | Field::DashLength | Field::Gap => per_pixel * 0.25,
            _ => per_pixel,
        }
    }

    /// What is typed into a number taken, kept for Undo.
    fn commit_field(&mut self) {
        let Some((field, typed, _)) = self.editing.take() else { return };
        if field == Field::Hex {
            if let Ok(color) = u32::from_str_radix(typed.trim_start_matches('#'), 16).map(|c| c & 0xFFFFFF) {
                if typed.trim_start_matches('#').len() == 6 {
                    let before = self.doc.clone();
                    self.recolor(color);
                    remember_color(color);
                    self.settle(before);
                    self.flyout = self.flyout.map(|_| annotate::to_hsv(color));
                }
            }
            return;
        }
        if let Ok(value) = typed.trim().trim_end_matches('%').parse::<f32>() {
            let before = self.doc.clone();
            self.set_field(field, value);
            self.settle(before);
        }
    }

    /// The emoji the picker shows, by what is typed: every word of it in
    /// the name.
    fn emoji_found(&self) -> Vec<usize> {
        let query = self.picker.as_ref().map(|picker| picker.query.to_lowercase()).unwrap_or_default();
        let words: Vec<&str> = query.split_whitespace().collect();
        let list = annotate::emoji_list();
        (0..list.len()).filter(|at| words.iter().all(|word| list[*at].1.contains(word))).collect()
    }

    /// An emoji put in the middle of what is on show, picked out.
    fn place_emoji(&mut self, which: usize) {
        let Some((emoji, _)) = annotate::emoji_list().get(which) else { return };
        let [l, t, r, b] = self.region();
        let side = self.default_stroke(Tool::Emoji);
        let mut shape = self.fresh(Tool::Emoji, ((l + r - side) / 2.0, (t + b - side) / 2.0));
        shape.text = emoji.to_string();
        self.snap();
        self.doc.shapes.push(shape);
        self.made(self.doc.shapes.len() - 1);
    }

    /// What is picked out moved by `by`, from where it was in `before`,
    /// resting against the edges and middles of the rest and of the
    /// picture when near them, unless `free`.
    fn move_selected(&mut self, before: &Doc, by: Point, free: bool) {
        let mut by = by;
        self.guides = (Vec::new(), Vec::new());
        let bounds = |shapes: &[Shape], which: &[usize]| {
            which.iter().map(|at| shapes[*at].bounds()).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
        };
        if let (false, Some([l, t, r, b])) = (free, bounds(&before.shapes, &self.selected)) {
            let mut others: Vec<[f32; 4]> = (0..before.shapes.len()).filter(|at| !self.selected.contains(at)).map(|at| before.shapes[at].bounds()).collect();
            others.push(self.region());
            let (gap_x, gap_y, guides) = snap_box([l + by.0, t + by.1, r + by.0, b + by.1], &others, (&self.edges.0, &self.edges.1), SNAP / self.place.scale);
            by = (by.0 + gap_x, by.1 + gap_y);
            self.guides = guides;
        }
        for at in self.selected.clone() {
            let mut shape = before.shapes[at].clone();
            shape.translate(by.0, by.1);
            self.doc.shapes[at] = shape;
        }
    }

    /// Where a point being dragged rests: on an edge found in the picture,
    /// the picture's own sides and middle, or the sides and middles of the
    /// marks but `skip`, when near one, along the ways `axes` says; the
    /// line it rests on shown.
    fn rest(&mut self, p: Point, axes: (bool, bool), skip: Option<usize>) -> Point {
        let near = SNAP / self.place.scale;
        let [_, _, w, h] = self.whole();
        let mut out = p;
        for axis in 0..2 {
            if (axis == 0 && !axes.0) || (axis == 1 && !axes.1) {
                continue;
            }
            let value = if axis == 0 { p.0 } else { p.1 };
            let side = if axis == 0 { w } else { h };
            let mut theirs: Vec<f32> = if axis == 0 { self.edges.0.clone() } else { self.edges.1.clone() };
            theirs.extend([0.0, side / 2.0, side]);
            for (at, shape) in self.doc.shapes.iter().enumerate() {
                if Some(at) != skip {
                    let [l, t, r, b] = shape.bounds();
                    theirs.extend(if axis == 0 { [l, (l + r) / 2.0, r] } else { [t, (t + b) / 2.0, b] });
                }
            }
            let best = theirs.into_iter().filter(|line| (line - value).abs() < near).min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()));
            if let Some(line) = best {
                if axis == 0 {
                    out.0 = line;
                    self.guides.0.push(line);
                } else {
                    out.1 = line;
                    self.guides.1.push(line);
                }
            }
        }
        out
    }

    /// The marks a box from `from` to `to` touches picked out, along with
    /// those already, if `adding`.
    fn pick_within(&mut self, from: Point, to: Point, adding: bool) {
        let [l, t, r, b] = [from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1)];
        if !adding {
            self.selected.clear();
        }
        for at in 0..self.doc.shapes.len() {
            let [sl, st, sr, sb] = self.doc.shapes[at].bounds();
            if sl <= r && sr >= l && st <= b && sb >= t && !self.selected.contains(&at) {
                self.selected.push(at);
            }
        }
    }

    /// A side or corner of the crop taken to `p`: the opposite one still,
    /// or with `middle` the middle, the other side going out as far; kept
    /// to its shape if it has one, or with `shift` to the one it had; and
    /// inside the picture.
    fn pull_crop(&mut self, grip: (i8, i8), start: [f32; 4], p: Point, shift: bool, middle: bool) {
        let (w, h) = (self.native.0 as f32, self.native.1 as f32);
        let [mut l, mut t, mut r, mut b] = start;
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        let least = 8.0 / self.place.scale;
        match grip.0 {
            -1 => l = within(p.0, 0.0, r - least),
            1 => r = within(p.0, l + least, w),
            _ => {}
        }
        match grip.1 {
            -1 => t = within(p.1, 0.0, b - least),
            1 => b = within(p.1, t + least, h),
            _ => {}
        }
        if middle {
            // The other side as far out, as far as the picture lets it.
            match grip.0 {
                -1 => r = (2.0 * cx - l).min(w),
                1 => l = (2.0 * cx - r).max(0.0),
                _ => {}
            }
            match grip.1 {
                -1 => b = (2.0 * cy - t).min(h),
                1 => t = (2.0 * cy - b).max(0.0),
                _ => {}
            }
        }
        let ratio = self.aspect.or(shift.then(|| (start[2] - start[0]) / (start[3] - start[1]).max(1.0)));
        if let Some(ratio) = ratio {
            // The side dragged says how large; the other follows, from the
            // corner across, or about the middle for a side.
            let (cw, ch) = (r - l, b - t);
            let (cw, ch) = if grip.1 == 0 || (grip.0 != 0 && cw / ratio >= ch) { (cw, cw / ratio) } else { (ch * ratio, ch) };
            let (cw, ch) = {
                let fit = (w / cw).min(h / ch).min(1.0);
                (cw * fit, ch * fit)
            };
            let x = match (middle, grip.0) {
                (true, _) | (_, 0) => (l + r - cw) / 2.0,
                (_, -1) => r - cw,
                _ => l,
            };
            let y = match (middle, grip.1) {
                (true, _) | (_, 0) => (t + b - ch) / 2.0,
                (_, -1) => b - ch,
                _ => t,
            };
            let (x, y) = (within(x, 0.0, w - cw), within(y, 0.0, h - ch));
            (l, t, r, b) = (x, y, (x + cw).min(w), (y + ch).min(h));
        }
        self.crop_draft = Some([l, t, r, b]);
    }
}

/// The straight edges of a picture `w` wide, the sort a screenshot is full
/// of: a window's sides, a card's, a divider. Where the light changes
/// sharply between two rows along a stretch a twelfth of the picture long
/// or more, there is one across; between two columns, one down. Where
/// they are, in the picture's pixels, down and across: the longest few
/// hundred, one for each, not the two or three a soft edge spreads over.
fn find_edges(pixels: &[u32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    use rayon::prelude::*;
    const STEP: i32 = 24;
    if w < 2 || h < 2 || pixels.len() < w * h {
        return (Vec::new(), Vec::new());
    }
    let light: Vec<i32> = pixels.par_iter().map(|p| (((p >> 16 & 255) * 3 + (p >> 8 & 255) * 6 + (p & 255)) / 10) as i32).collect();
    // The longest run along a line of places that change sharply, gaps of
    // a pixel or two let through, as text over an edge makes.
    let longest = |n: usize, sharp: &dyn Fn(usize) -> bool| {
        let (mut best, mut run, mut gap) = (0, 0, 0);
        for at in 0..n {
            if sharp(at) {
                run += 1 + gap;
                gap = 0;
                best = best.max(run);
            } else if run > 0 && gap < 2 {
                gap += 1;
            } else {
                (run, gap) = (0, 0);
            }
        }
        best
    };
    let across: Vec<(usize, usize)> = (1..h).into_par_iter().map(|y| (y, longest(w, &|x| (light[y * w + x] - light[(y - 1) * w + x]).abs() >= STEP))).collect();
    let down: Vec<(usize, usize)> = (1..w).into_par_iter().map(|x| (x, longest(h, &|y| (light[y * w + x] - light[y * w + x - 1]).abs() >= STEP))).collect();
    // Of each few lines together, the strongest; the longest of those.
    let keep = |found: Vec<(usize, usize)>, least: usize| {
        let mut lines: Vec<(usize, usize)> = Vec::new();
        for (at, run) in found.into_iter().filter(|(_, run)| *run >= least) {
            match lines.last_mut() {
                Some(last) if at - last.0 <= 2 => {
                    if run > last.1 {
                        *last = (at, run);
                    }
                }
                _ => lines.push((at, run)),
            }
        }
        lines.sort_by(|a, b| b.1.cmp(&a.1));
        lines.truncate(200);
        lines.into_iter().map(|(at, _)| at as f32).collect::<Vec<f32>>()
    };
    (keep(down, (h / 12).max(24)), keep(across, (w / 12).max(24)))
}

/// How far a box would go to rest against the edges and middles of
/// `others` near it, and the lines of `lines` (down, then across), across
/// and down; and the lines it rests against.
fn snap_box([l, t, r, b]: [f32; 4], others: &[[f32; 4]], lines: (&[f32], &[f32]), near: f32) -> (f32, f32, (Vec<f32>, Vec<f32>)) {
    let mut gaps = [0.0, 0.0];
    let mut guides = (Vec::new(), Vec::new());
    for axis in 0..2 {
        let (from, to) = if axis == 0 { (l, r) } else { (t, b) };
        let ours = [from, (from + to) / 2.0, to];
        let mut theirs: Vec<f32> = others.iter().flat_map(|o| if axis == 0 { [o[0], (o[0] + o[2]) / 2.0, o[2]] } else { [o[1], (o[1] + o[3]) / 2.0, o[3]] }).collect();
        theirs.extend_from_slice(if axis == 0 { lines.0 } else { lines.1 });
        let best = ours.iter().flat_map(|mine| theirs.iter().map(move |theirs| theirs - mine)).filter(|gap| gap.abs() < near).min_by(|a, b| a.abs().total_cmp(&b.abs()));
        if let Some(gap) = best {
            gaps[axis] = gap;
            let lines: Vec<f32> = ours.iter().map(|v| v + gap).filter(|v| theirs.iter().any(|t| (t - v).abs() < 0.01)).collect();
            if axis == 0 { guides.0 = lines } else { guides.1 = lines }
        }
    }
    (gaps[0], gaps[1], guides)
}

impl App {
    pub(super) fn open_editor(&mut self) {
        if self.editor.is_some() {
            return;
        }
        let id = self.id();
        // The marks of a save that failed are there to go back to.
        if let Some(editor) = self.edit_stash.take() {
            if editor.id == id {
                self.editor = Some(editor);
                return self.redraw();
            }
            self.edit_stash = Some(editor);
        }
        let Some(frame) = self.source().filter(|_| self.shown.as_ref().is_some_and(|(shown, _)| *shown == id)) else {
            return self.toast("Not loaded yet");
        };
        if files::is_video(&self.files[id]) || frame.animation.is_some() {
            return self.toast("Edit · stills only");
        }
        self.editor = Some(Editor::new(id, frame.native));
        self.menu = None;
        // The picture's straight edges, for marks and the crop to rest
        // against: looked for off this thread, in the file decoded at full
        // size, where a line a pixel wide is still sharp; in the picture as
        // it is shown, made smaller, only if that fails.
        let proxy = self.proxy.clone();
        let (path, transform) = (self.files[id].clone(), self.transforms.get(&id).copied().unwrap_or_default());
        std::thread::spawn(move || {
            let full = crate::decode::open(&path).ok().map(|image| {
                let image = convert::turned(image, transform).to_rgb8();
                let pixels: Vec<u32> = image.pixels().map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])).collect();
                (pixels, image.width(), image.height())
            });
            let (pixels, w, h) = full.unwrap_or_else(|| (frame.pixels.clone(), frame.width, frame.height));
            let (down, across) = find_edges(&pixels, w as usize, h as usize);
            let (fx, fy) = (frame.native.0 as f32 / w.max(1) as f32, frame.native.1 as f32 / h.max(1) as f32);
            let _ = proxy.send_event(Event::EditEdges(id, down.into_iter().map(|x| x * fx).collect(), across.into_iter().map(|y| y * fy).collect()));
        });
        self.redraw();
    }

    /// `--edit`: the file jog opens on, in Edit as soon as it is loaded; a
    /// video is left as it is, with a word on the terminal.
    pub fn edit_on_open(&mut self) {
        let id = self.id();
        if files::is_video(&self.files[id]) {
            eprintln!("jog: --edit is for pictures; {} is a video", self.name_of(id));
            return;
        }
        self.edit_waiting = Some(id);
    }

    /// The straight edges found in the picture being edited.
    pub(super) fn edit_edges(&mut self, id: usize, down: Vec<f32>, across: Vec<f32>) {
        if let Some(editor) = self.editor.as_mut().filter(|editor| editor.id == id) {
            editor.edges = (down, across);
        }
    }

    fn close_editor(&mut self) {
        self.editor = None;
        self.set_edit_cursor(CursorIcon::Default);
        self.redraw();
    }

    fn set_edit_cursor(&mut self, icon: CursorIcon) {
        if let Some(editor) = &mut self.editor {
            if editor.cursor == icon {
                return;
            }
            editor.cursor = icon;
        }
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }

    pub(super) fn editor_key(&mut self, key: Key<&str>) {
        let control = self.modifiers.control_key() || self.modifiers.super_key();
        let shift = self.modifiers.shift_key();
        let Some(editor) = &mut self.editor else { return };
        // The box asking what to do with what is unsaved has every key
        // while it is up: its own, Tab and the arrows to go through them,
        // and Enter for the one picked out.
        if let Some(at) = editor.leaving {
            let count = LEAVE.len();
            let choice = match key {
                Key::Named(NamedKey::Escape) => Some(0),
                Key::Character("d" | "D") => Some(1),
                Key::Character("c" | "C") => Some(2),
                Key::Character("s" | "S") if control && shift => Some(2),
                Key::Character("s" | "S") => Some(3),
                Key::Named(NamedKey::Enter | NamedKey::Space) => Some(at),
                Key::Named(NamedKey::Tab) if shift => {
                    editor.leaving = Some((at + count - 1) % count);
                    None
                }
                Key::Named(NamedKey::Tab | NamedKey::ArrowRight) => {
                    editor.leaving = Some((at + 1) % count);
                    None
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    editor.leaving = Some((at + count - 1) % count);
                    None
                }
                _ => None,
            };
            return match choice {
                Some(choice) => self.leave_with(choice),
                None => self.redraw(),
            };
        }
        // A number being typed takes the keys a number has.
        if let Some((field, typed, fresh)) = &mut editor.editing {
            let field = *field;
            match key {
                Key::Named(NamedKey::Enter | NamedKey::Tab) => editor.commit_field(),
                Key::Named(NamedKey::Escape) => editor.editing = None,
                Key::Named(NamedKey::Backspace) => {
                    if std::mem::take(fresh) { typed.clear() } else { typed.pop(); }
                }
                Key::Named(arrow @ (NamedKey::ArrowUp | NamedKey::ArrowDown)) if field != Field::Hex => {
                    let by = if shift { 10.0 } else { 1.0 } * if arrow == NamedKey::ArrowUp { 1.0 } else { -1.0 };
                    let typed_now = typed.trim_end_matches('%').parse::<f32>().ok();
                    let now = typed_now.or(editor.field(field)).unwrap_or(0.0);
                    if let Some((_, typed, fresh)) = &mut editor.editing {
                        *typed = shown(if field == Field::Angle { now + by } else { (now + by).max(0.0) });
                        *fresh = false;
                    }
                    let before = editor.doc.clone();
                    editor.set_field(field, now + by);
                    editor.settle(before);
                }
                Key::Character(c) => {
                    let fits = |c: char| if field == Field::Hex { c.is_ascii_hexdigit() || c == '#' } else { c.is_ascii_digit() || c == '.' || (c == '-' && field == Field::Angle) };
                    if c.chars().all(fits) {
                        if std::mem::take(fresh) {
                            typed.clear();
                        }
                        typed.push_str(&c.to_uppercase());
                    }
                }
                _ => {}
            }
            return self.redraw();
        }
        // Words being typed take every key but the ones that end them.
        if let Some(at) = editor.typing {
            let badge = editor.doc.shapes[at].tool == Tool::Badge;
            match key {
                Key::Character("b") if control => return self.edit_option(EditHit::Toggle(BOLD)),
                Key::Character("i") if control => return self.edit_option(EditHit::Toggle(ITALIC)),
                Key::Character("a") if control => {
                    editor.anchor = Some(0);
                    editor.caret = editor.typed().map_or(0, |text| text.chars().count());
                }
                // The clipboard, off this thread: its helpers are programs.
                Key::Character("c" | "x") if control => {
                    if let Some(words) = editor.selected_text() {
                        std::thread::spawn(move || ops::set_clipboard(&words));
                        if key == Key::Character("x") {
                            editor.erase(false);
                        }
                    }
                }
                Key::Character("v") if control => {
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        if let Some(words) = ops::clipboard() {
                            let _ = proxy.send_event(Event::EditPaste(words));
                        }
                    });
                }
                Key::Named(NamedKey::Enter) if control || badge => editor.finish_typing(),
                Key::Named(NamedKey::Escape) => editor.finish_typing(),
                _ if control => {}
                Key::Named(NamedKey::Enter) => editor.insert("\n"),
                Key::Named(NamedKey::Space) => editor.insert(" "),
                Key::Named(NamedKey::Backspace) => editor.erase(false),
                Key::Named(NamedKey::Delete) => editor.erase(true),
                Key::Named(way @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::Home | NamedKey::End)) => {
                    let to = editor.caret_for(way, shift);
                    editor.put_caret(to, shift);
                }
                Key::Character(typed) => editor.insert(typed),
                _ => {}
            }
            return self.redraw();
        }
        // So does the search of the emoji picker.
        if let Some(picker) = editor.picker.as_mut().filter(|_| !control) {
            match key {
                Key::Named(NamedKey::Escape) => editor.set_tool(Tool::Select),
                Key::Named(NamedKey::Backspace) => drop(picker.query.pop()),
                Key::Named(NamedKey::Space) => picker.query.push(' '),
                Key::Named(NamedKey::ArrowDown) => picker.scroll += 1,
                Key::Named(NamedKey::ArrowUp) => picker.scroll = picker.scroll.saturating_sub(1),
                Key::Named(NamedKey::Enter) => {
                    if let Some(first) = editor.emoji_found().first() {
                        editor.place_emoji(*first);
                    }
                }
                Key::Character(typed) => {
                    picker.query.push_str(typed);
                    picker.scroll = 0;
                }
                _ => {}
            }
            return self.redraw();
        }
        match key {
            Key::Character("c") if control => return self.copy_edit(),
            Key::Character("s" | "S") if control => {
                let way = if shift { SaveAs::Copy } else { SaveAs::Overwrite };
                return self.save_edit(way);
            }
            Key::Character("z" | "Z") if control && shift => editor.redo(),
            Key::Character("z") if control => editor.undo(),
            Key::Character("y") if control => editor.redo(),
            Key::Character("a") if control => {
                editor.selected = (0..editor.doc.shapes.len()).collect();
                editor.tool = Tool::Select;
            }
            Key::Character("b") if control => return self.edit_option(EditHit::Toggle(BOLD)),
            Key::Character("i") if control => return self.edit_option(EditHit::Toggle(ITALIC)),
            // What is on the clipboard, read off this thread: a picture, to
            // lay over this one; or words, as new words.
            Key::Character("v") if control => {
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let picture = ops::clipboard_image().and_then(|png| image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok());
                    let _ = match picture {
                        Some(picture) => proxy.send_event(Event::EditPasteImage(picture.to_rgba8())),
                        None => match ops::clipboard() {
                            Some(words) => proxy.send_event(Event::EditPaste(words)),
                            None => Ok(()),
                        },
                    };
                });
            }
            Key::Character("d") if control => {
                if !editor.selected.is_empty() {
                    let step = 12.0 / editor.place.scale;
                    let copies: Vec<Shape> = editor
                        .selected
                        .iter()
                        .map(|at| {
                            let mut copy = editor.doc.shapes[*at].clone();
                            copy.translate(step, step);
                            copy
                        })
                        .collect();
                    editor.snap();
                    let first = editor.doc.shapes.len();
                    editor.doc.shapes.extend(copies);
                    editor.selected = (first..editor.doc.shapes.len()).collect();
                    editor.tool = Tool::Select;
                }
            }
            // Over or under the rest, one step or all the way.
            Key::Character(way @ ("]" | "[" | "}" | "{")) if control => {
                if let Some(at) = editor.one() {
                    let last = editor.doc.shapes.len() - 1;
                    let to = match way {
                        "]" => (at + 1).min(last),
                        "[" => at.saturating_sub(1),
                        "}" => last,
                        _ => 0,
                    };
                    if to != at {
                        editor.snap();
                        let shape = editor.doc.shapes.remove(at);
                        editor.doc.shapes.insert(to, shape);
                        editor.selected = vec![to];
                    }
                }
            }
            _ if control => return,
            Key::Named(NamedKey::Escape) => {
                if editor.tool_menu.is_some() {
                    editor.tool_menu = None;
                } else if editor.flyout.is_some() {
                    editor.flyout = None;
                } else if editor.tool == Tool::Crop {
                    // The crop as it was before this one was begun.
                    editor.crop_draft = None;
                    editor.tool = Tool::Select;
                } else if editor.tool == Tool::Pipette {
                    editor.tool = editor.before_pipette;
                } else if !editor.selected.is_empty() {
                    editor.selected.clear();
                } else if editor.tool != Tool::Select {
                    editor.set_tool(Tool::Select);
                } else if editor.dirty() {
                    editor.leaving = Some(0);
                } else {
                    return self.close_editor();
                }
            }
            Key::Named(NamedKey::Enter) if editor.tool == Tool::Crop => editor.apply_crop(),
            // Words picked out: Enter goes back to typing them.
            Key::Named(NamedKey::Enter) => {
                if let Some(at) = editor.one().filter(|at| matches!(editor.doc.shapes[*at].tool, Tool::Text | Tool::Badge)) {
                    editor.snap();
                    editor.start_typing(at, None);
                }
            }
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                if !editor.selected.is_empty() {
                    editor.snap();
                    let mut gone = std::mem::take(&mut editor.selected);
                    gone.sort_unstable();
                    for at in gone.into_iter().rev() {
                        editor.doc.shapes.remove(at);
                    }
                }
            }
            Key::Named(arrow @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown)) => {
                let step = if shift { 10.0 } else { 1.0 } / editor.place.scale;
                let (dx, dy) = match arrow {
                    NamedKey::ArrowLeft => (-step, 0.0),
                    NamedKey::ArrowRight => (step, 0.0),
                    NamedKey::ArrowUp => (0.0, -step),
                    _ => (0.0, step),
                };
                if editor.tool == Tool::Crop {
                    // The crop moves, inside the picture.
                    if let Some([l, t, r, b]) = editor.crop_draft {
                        let [_, _, w, h] = editor.whole();
                        let (dx, dy) = (within(dx, -l, w - r), within(dy, -t, h - b));
                        editor.crop_draft = Some([l + dx, t + dy, r + dx, b + dy]);
                    }
                } else if !editor.selected.is_empty() {
                    editor.snap();
                    for at in editor.selected.clone() {
                        editor.doc.shapes[at].translate(dx, dy);
                    }
                }
            }
            // The four widths there were, still a key each.
            Key::Character(digit @ ("1" | "2" | "3" | "4")) => {
                let size = digit.parse::<usize>().unwrap_or(1) - 1;
                let kind = editor.subject().map_or(editor.tool, |at| editor.doc.shapes[at].tool);
                let value = match kind {
                    Tool::Text | Tool::Badge => annotate::text_sizes(editor.native)[size],
                    Tool::Emoji => annotate::text_sizes(editor.native)[size] * 2.0,
                    _ => annotate::widths(editor.native)[size],
                };
                let before = editor.doc.clone();
                editor.set_field(Field::Stroke, value);
                editor.settle(before);
            }
            // Thinner and thicker, smaller and larger, as hotshot has them.
            Key::Character(way @ ("[" | "]")) => editor.grow(if way == "]" { 1.15 } else { 1.0 / 1.15 }),
            // Through the colors, as hotshot's C does.
            Key::Character("c") => {
                let next = COLORS.iter().position(|c| *c == editor.style.color).map_or(0, |at| (at + 1) % COLORS.len());
                return self.edit_option(EditHit::Color(COLORS[next]));
            }
            Key::Character("f") => {
                let filled = !editor.style.filled;
                return self.edit_option(EditHit::Filled(filled));
            }
            Key::Character("g") => {
                let face = (editor.style.face + 1) % FACES.len();
                return self.edit_option(EditHit::Face(face));
            }
            Key::Character("d") => {
                let dash = match editor.style.dash {
                    Dash::Solid => Dash::Dashed,
                    Dash::Dashed => Dash::Dotted,
                    Dash::Dotted => Dash::Solid,
                };
                return self.edit_option(EditHit::Dash(dash));
            }
            Key::Character(typed) => {
                if let Some(tool) = Tool::of_key(typed) {
                    editor.set_tool(tool);
                }
            }
            _ => {}
        }
        self.redraw();
    }

    /// One of the panel's choices: kept for what is drawn next, and given
    /// to the marks picked out.
    fn edit_option(&mut self, hit: EditHit) {
        let Some(editor) = &mut self.editor else { return };
        match hit {
            EditHit::Color(color) => {
                let before = editor.doc.clone();
                editor.recolor(color);
                editor.settle(before);
                if editor.flyout.is_some() {
                    remember_color(color);
                    editor.flyout = Some(annotate::to_hsv(color));
                }
            }
            EditHit::Filled(filled) => {
                editor.style.filled = filled;
                editor.restyle(|shape| {
                    if matches!(shape.tool, Tool::Rect | Tool::Oval) {
                        shape.filled = filled;
                    }
                });
            }
            EditHit::Heads(heads) => {
                editor.style.heads = heads;
                editor.restyle(|shape| {
                    if shape.tool.is_line() {
                        shape.heads = heads;
                    }
                });
            }
            EditHit::Dash(dash) => {
                editor.style.dash = dash;
                (editor.style.dash_length, editor.style.gap) = (0.0, 0.0);
                editor.restyle(|shape| {
                    if shape.tool.is_line() || matches!(shape.tool, Tool::Rect | Tool::Oval) {
                        shape.set_dash(dash);
                    }
                });
            }
            EditHit::Face(face) => {
                editor.style.face = face;
                editor.restyle(|shape| shape.face = face);
            }
            EditHit::Toggle(flag) => {
                // On for all if it was off for the first, as a style bar has it.
                let on = editor.subject().map_or(editor.style.text, |at| editor.doc.shapes[at].style) & flag == 0;
                editor.style.text = if on { editor.style.text | flag } else { editor.style.text & !flag };
                editor.restyle(|shape| {
                    if shape.tool == Tool::Text {
                        shape.style = if on { shape.style | flag } else { shape.style & !flag };
                    }
                });
            }
            EditHit::BadgeLook(look) => {
                editor.style.badge = look;
                editor.restyle(|shape| shape.look = look);
            }
            EditHit::Lens(square) => {
                editor.style.square = square;
                editor.restyle(|shape| {
                    if shape.tool == Tool::Zoom {
                        shape.filled = square;
                    }
                });
            }
            _ => {}
        }
        self.redraw();
    }

    /// A right click, as in hotshot: the colors, under the pointer, for the
    /// mark it is on, which it picks out, or for what is drawn next.
    pub(super) fn editor_right_click(&mut self) {
        let (x, y) = self.mouse;
        let Some(editor) = &mut self.editor else { return };
        if editor.leaving.is_some() || editor.drag.is_some() {
            return;
        }
        let p = editor.place.to_picture((x as f32, y as f32));
        let reach = 6.0 / editor.place.scale;
        if inside(editor.stage, (x as i32, y as i32)) && editor.typing.is_none() {
            if let Some(at) = editor.doc.shapes.iter().rposition(|shape| shape.hit(p, reach)) {
                if !editor.selected.contains(&at) {
                    editor.selected = vec![at];
                    editor.tool = Tool::Select;
                }
            }
        }
        let color = editor.subject().map_or(editor.style.color, |at| editor.doc.shapes[at].color);
        editor.flyout = Some(annotate::to_hsv(color));
        editor.flyout_at = Some((x as i32, y as i32));
        editor.tool_menu = None;
        self.redraw();
    }

    /// A picture from the clipboard, laid over the one being edited in the
    /// middle of what is on show: as large as it is, or made to fit in most
    /// of what is on show; picked out, to be moved and sized.
    pub(super) fn edit_paste_image(&mut self, picture: image::RgbaImage) {
        let Some(editor) = &mut self.editor else { return };
        let (w, h) = (picture.width() as f32, picture.height() as f32);
        if w < 1.0 || h < 1.0 {
            return;
        }
        let [l, t, r, b] = editor.region();
        let fit = ((r - l) * 0.6 / w).min((b - t) * 0.6 / h).min(1.0);
        let (pw, ph) = (w * fit, h * fit);
        let at = ((l + r - pw) / 2.0, (t + b - ph) / 2.0);
        let mut shape = editor.fresh(Tool::Image, at);
        shape.points = vec![at, (at.0 + pw, at.1 + ph)];
        shape.radius = 0.0;
        shape.image = Some(annotate::Picture(Arc::new(picture)));
        editor.finish_typing();
        editor.snap();
        editor.doc.shapes.push(shape);
        editor.made(editor.doc.shapes.len() - 1);
        self.toast(format!("Pasted  ·  {} × {}", w as u32, h as u32));
    }

    /// Text from the clipboard: where the caret is, or as new words in the
    /// middle of what is on show.
    pub(super) fn edit_paste(&mut self, words: String) {
        let Some(editor) = &mut self.editor else { return };
        let words = words.trim_end_matches('\n');
        if words.is_empty() {
            return;
        }
        if editor.typing.is_none() {
            let [l, t, r, b] = editor.region();
            let shape = editor.fresh(Tool::Text, ((l + r) / 2.0, (t + b) / 2.0));
            editor.snap();
            editor.doc.shapes.push(shape);
            editor.start_typing(editor.doc.shapes.len() - 1, Some(0));
        }
        editor.insert(words);
        self.redraw();
    }

    /// The wheel: through the emoji, while they are up; otherwise, as
    /// hotshot has it, the width of the line or the size of the words.
    pub(super) fn editor_wheel(&mut self, lines: f64) {
        let Some(editor) = &mut self.editor else { return };
        match &mut editor.picker {
            Some(picker) => {
                let rows = (lines.abs().ceil() as usize).max(1) * 2;
                picker.scroll = if lines > 0.0 { picker.scroll.saturating_sub(rows) } else { picker.scroll + rows };
            }
            None => editor.grow(1.15f32.powf(lines as f32)),
        }
        self.redraw();
    }

    /// The color of the picture as it was opened, under a point of it.
    fn picture_color(&self, p: Point) -> Option<u32> {
        let editor = self.editor.as_ref()?;
        let frame = self.source()?;
        let (fx, fy) = (frame.width as f32 / editor.native.0.max(1) as f32, frame.height as f32 / editor.native.1.max(1) as f32);
        let (x, y) = ((p.0 * fx).floor(), (p.1 * fy).floor());
        if x < 0.0 || y < 0.0 || x >= frame.width as f32 || y >= frame.height as f32 {
            return None;
        }
        Some(frame.pixels[y as usize * frame.width as usize + x as usize] & 0xFFFFFF)
    }

    pub(super) fn editor_mouse(&mut self, pressed: bool) {
        let (x, y) = self.mouse;
        if !pressed {
            return self.editor_release();
        }
        let adding = self.modifiers.shift_key() || self.modifiers.control_key();
        let hit = self.editor.as_ref().and_then(|editor| editor.hit_at(x as i32, y as i32));
        let handle = self.editor.as_ref().and_then(|editor| editor.handle_at(x as i32, y as i32));
        // While the box about what is unsaved is up, only it takes a click.
        if self.editor.as_ref().is_some_and(|editor| editor.leaving.is_some()) {
            return match hit {
                Some(EditHit::Leave(choice)) => self.leave_with(choice),
                _ => self.redraw(),
            };
        }
        if let Some(editor) = &mut self.editor {
            // A number being typed is taken when anything else is pressed.
            if editor.editing.is_some() && !matches!(hit, Some(EditHit::Field(field)) if Some(field) == editor.editing.as_ref().map(|e| e.0)) {
                editor.commit_field();
            }
            // The colors beyond the eight close on a press outside them.
            if editor.flyout.is_some() && !matches!(hit, Some(EditHit::Square | EditHit::HueStrip | EditHit::Color(_) | EditHit::More | EditHit::Pipette | EditHit::Field(Field::Hex) | EditHit::Inside)) {
                editor.flyout = None;
            }
        }
        // A click anywhere but on a tool closes the menu of its ways, once
        // the click has done what it does.
        let close_menu = !matches!(hit, Some(EditHit::Tool(_)));
        // The pipette takes the color under it, from the picture itself.
        if hit.is_none() && self.editor.as_ref().is_some_and(|editor| editor.tool == Tool::Pipette) {
            let p = self.editor.as_ref().map(|editor| editor.place.to_picture((x as f32, y as f32))).unwrap_or_default();
            if let Some(color) = self.picture_color(p) {
                remember_color(color);
                self.edit_option(EditHit::Color(color));
            }
            if let Some(editor) = &mut self.editor {
                editor.tool = editor.before_pipette;
            }
            return self.redraw();
        }
        match (hit, handle) {
            (Some(hit @ (EditHit::Color(_) | EditHit::Filled(_) | EditHit::Heads(_) | EditHit::Dash(_) | EditHit::Face(_) | EditHit::Toggle(_) | EditHit::BadgeLook(_) | EditHit::Lens(_))), _) => self.edit_option(hit),
            (Some(EditHit::Save(way)), _) => return self.save_edit(way),
            (Some(EditHit::Clipboard), _) => return self.copy_edit(),
            (Some(hit), _) if hit != EditHit::Inside || handle.is_none() => {
                if let Some(editor) = &mut self.editor {
                    match hit {
                        // The tool in hand clicked again: the ways it draws.
                        EditHit::Tool(tool) if editor.tool == tool && !tool_options(tool).is_empty() => {
                            let left = editor.hits.iter().find(|(hit, _)| *hit == EditHit::Tool(tool)).map_or(0, |(_, place)| place[0]);
                            editor.tool_menu = if editor.tool_menu.is_some() { None } else { Some((tool, left)) };
                        }
                        EditHit::Tool(tool) => {
                            editor.set_tool(tool);
                            editor.tool_menu = None;
                        }
                        EditHit::Pipette => editor.set_tool(Tool::Pipette),
                        EditHit::More => {
                            editor.flyout = if editor.flyout.is_some() { None } else { Some(annotate::to_hsv(editor.subject().map_or(editor.style.color, |at| editor.doc.shapes[at].color))) };
                            editor.flyout_at = None;
                        }
                        EditHit::Square | EditHit::HueStrip => {
                            editor.drag = Some(EditDrag::Pick { hue: hit == EditHit::HueStrip, before: editor.doc.clone() });
                            pick_color(editor, hit == EditHit::HueStrip, (x as i32, y as i32));
                        }
                        EditHit::Undo => editor.undo(),
                        EditHit::Redo => editor.redo(),
                        EditHit::Reset => editor.reset(),
                        EditHit::Field(Field::Hex) => {
                            let color = editor.subject().map_or(editor.style.color, |at| editor.doc.shapes[at].color);
                            editor.editing = Some((Field::Hex, format!("{color:06X}"), true));
                        }
                        EditHit::Field(field) => {
                            if editor.editing.as_ref().is_none_or(|editing| editing.0 != field) {
                                let start = editor.field(field).unwrap_or(0.0);
                                editor.drag = Some(EditDrag::Scrub { field, x, start, before: editor.doc.clone(), moved: false });
                            }
                        }
                        EditHit::Aspect(aspect) => editor.shape_crop(aspect),
                        EditHit::ApplyCrop => editor.apply_crop(),
                        EditHit::ResetCrop => {
                            if editor.tool == Tool::Crop {
                                editor.crop_draft = Some(editor.whole());
                            } else {
                                editor.snap();
                                editor.doc.crop = None;
                            }
                        }
                        EditHit::Emoji(which) => editor.place_emoji(which),
                        _ => {}
                    }
                }
            }
            (_, Some(handle)) => {
                if let Some(editor) = &mut self.editor {
                    grab(editor, handle, (x as f32, y as f32));
                }
            }
            (None, None) => {
                if let Some(editor) = &mut self.editor {
                    editor_press(editor, (x as f32, y as f32), adding);
                }
            }
            _ => {}
        }
        if close_menu {
            if let Some(editor) = &mut self.editor {
                editor.tool_menu = None;
            }
        }
        self.redraw();
    }

    pub(super) fn editor_motion(&mut self) {
        let (x, y) = self.mouse;
        let shift = self.modifiers.shift_key();
        let free = self.modifiers.control_key();
        // Alt, as in Figma: from the middle rather than from a corner.
        let middle = self.modifiers.alt_key();
        let Some(editor) = &mut self.editor else { return };
        let p = editor.place.to_picture((x as f32, y as f32));
        let (w, h) = (editor.native.0 as f32, editor.native.1 as f32);
        let scale = editor.place.scale;
        let one = editor.one();
        // What a drag rests against is found afresh at each move.
        if editor.drag.is_some() {
            editor.guides = (Vec::new(), Vec::new());
        }
        match editor.drag.clone() {
            Some(EditDrag::Draw { from }) => {
                // A box or a line being drawn rests its end on edges.
                let last = editor.doc.shapes.len().checked_sub(1);
                let spans = last.is_some_and(|at| editor.doc.shapes[at].tool.spans());
                let p = if spans && !free { editor.rest(p, (true, true), last) } else { p };
                if let Some(shape) = editor.doc.shapes.last_mut() {
                    match shape.tool {
                        Tool::Pen | Tool::Marker => {
                            // A point every pixel and a half on screen.
                            if shape.points.last().is_none_or(|last| (last.0 - p.0).hypot(last.1 - p.1) * scale >= 1.5) {
                                shape.points.push(p);
                            }
                        }
                        // A counter dragged from grows a tail to the pointer.
                        Tool::Counter => {
                            shape.points.truncate(1);
                            shape.points.push(p);
                        }
                        // Shift: a square, a circle, a line at one of eight
                        // angles; Alt: about where the drag began.
                        tool => {
                            let to = match (shift, tool.is_line()) {
                                (true, true) => level(from, p),
                                (true, false) => square(from, p),
                                (false, _) => p,
                            };
                            let start = if middle { (2.0 * from.0 - to.0, 2.0 * from.1 - to.1) } else { from };
                            (shape.points[0], shape.points[1]) = (start, to);
                        }
                    }
                }
            }
            Some(EditDrag::Move { from, before, was_selected, .. }) => {
                // Shift keeps it to across or down, whichever it has gone
                // further.
                let (mut dx, mut dy) = (p.0 - from.0, p.1 - from.1);
                if shift {
                    if dx.abs() >= dy.abs() { dy = 0.0 } else { dx = 0.0 }
                }
                editor.move_selected(&before, (dx, dy), free);
                editor.drag = Some(EditDrag::Move { from, before, moved: true, was_selected });
            }
            Some(EditDrag::Marquee { from, adding, .. }) => editor.drag = Some(EditDrag::Marquee { from, to: p, adding }),
            Some(EditDrag::Grip { grip, frame, from, before }) => {
                // Square to the picture, the side taken rests on an edge.
                let upright = one.is_some_and(|at| before.shapes[at].angle == 0.0);
                let p = if upright && !free { editor.rest(p, (grip.0 != 0, grip.1 != 0), one) } else { p };
                // Turned, the drag is taken as along its own sides.
                let turned = one.map(|at| before.shapes[at].clone());
                let (p, from) = match &turned {
                    Some(shape) => (shape.unturn(p), shape.unturn(from)),
                    None => (p, from),
                };
                let [l, t, r, b] = frame;
                let size = ((r - l).max(1.0), (b - t).max(1.0));
                let centre = ((l + r) / 2.0, (t + b) / 2.0);
                // What stays put: the side or corner across from the one
                // dragged, or with Alt the middle, the drag then counting
                // twice as it pulls both sides.
                let mut still = match middle {
                    true => centre,
                    false => (if grip.0 < 0 { r } else { l }, if grip.1 < 0 { b } else { t }),
                };
                let reach = if middle { 2.0 } else { 1.0 };
                let pull = |g: i8, d: f32, size: f32| if g == 0 { 1.0 } else { (size + f32::from(g) * d * reach) / size };
                let mut times = (pull(grip.0, p.0 - from.0, size.0), pull(grip.1, p.1 - from.1, size.1));
                // Shift: in proportion; from a side, the other way about
                // its middle.
                if shift {
                    match grip {
                        (0, _) => (times.0, still.0) = (times.1.abs(), centre.0),
                        (_, 0) => (times.1, still.1) = (times.0.abs(), centre.1),
                        _ => {
                            let both = times.0.abs().max(times.1.abs());
                            times = (both.copysign(times.0), both.copysign(times.1));
                        }
                    }
                }
                let keep = |t: f32| if t.abs() < 0.02 { 0.02f32.copysign(t) } else { t };
                if let (Some(at), Some(was)) = (one, turned) {
                    let mut shape = was.clone();
                    shape.scale(still, (keep(times.0), keep(times.1)));
                    // Its middle moves as it is sized, and it turns about
                    // that: put back where what stays put was on screen.
                    let (want, now) = (was.returned(still), shape.returned(still));
                    shape.translate(want.0 - now.0, want.1 - now.1);
                    editor.doc.shapes[at] = shape;
                }
            }
            Some(EditDrag::Rotate { centre, from, before }) => {
                let by = (p.1 - centre.1).atan2(p.0 - centre.0) - from;
                if let Some(at) = one {
                    let mut shape = before.shapes[at].clone();
                    // Shift: to a whole fifteen degrees.
                    let step = 15f32.to_radians();
                    let by = match shift {
                        true => ((shape.angle + by) / step).round() * step - shape.angle,
                        false => by,
                    };
                    shape.turn(by, centre);
                    editor.doc.shapes[at] = shape;
                }
            }
            Some(EditDrag::End { end, before, .. }) => {
                editor.drag = Some(EditDrag::End { end, before, moved: true });
                if let Some(shape) = one.and_then(|at| editor.doc.shapes.get_mut(at)) {
                    shape.points[end] = if shift && shape.tool.is_line() { level(shape.points[1 - end], p) } else { p };
                }
            }
            Some(EditDrag::Bend { before, .. }) => {
                editor.drag = Some(EditDrag::Bend { before, moved: true });
                if let Some(shape) = one.and_then(|at| editor.doc.shapes.get_mut(at)) {
                    shape.bend_to(p, 4.0 / scale);
                }
            }
            Some(EditDrag::Round { corner, .. }) => {
                if let Some(shape) = one.and_then(|at| editor.doc.shapes.get_mut(at)) {
                    let [l, t, r, b] = shape.frame();
                    let (cx, cy) = (if corner.0 < 0 { l } else { r }, if corner.1 < 0 { t } else { b });
                    let most = (r - l).min(b - t) / 2.0;
                    shape.radius = (p.0 - cx).abs().min((p.1 - cy).abs()).clamp(0.0, most);
                }
            }
            Some(EditDrag::Zoom { rim, .. }) => {
                if let Some(shape) = one.and_then(|at| editor.doc.shapes.get_mut(at)) {
                    shape.zoom_to(rim, p);
                }
            }
            Some(EditDrag::Crop { from }) => {
                let p = if free { p } else { editor.rest(p, (true, true), None) };
                let to = (p.0.clamp(0.0, w), p.1.clamp(0.0, h));
                // Shift with no shape chosen: a square.
                let to = match editor.aspect.or(shift.then_some(1.0)) {
                    // Kept to its shape, from where it was begun.
                    Some(ratio) => {
                        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                        let wide = dx.abs().max(dy.abs() * ratio);
                        let (wide, high) = ((wide).min(if dx < 0.0 { from.0 } else { w - from.0 }), 0.0);
                        let high = (wide / ratio).min(if dy < 0.0 { from.1 } else { h - from.1 }).max(high);
                        let wide = high * ratio;
                        (from.0 + wide.copysign(dx), from.1 + high.copysign(dy))
                    }
                    None => to,
                };
                // Alt: about where it was begun, as far as the picture goes.
                // Both ways shrunk alike, so that a shape it is kept to holds.
                let draft = if middle {
                    let (dx, dy) = ((to.0 - from.0).abs(), (to.1 - from.1).abs());
                    let room = (from.0.min(w - from.0) / dx.max(1e-3)).min(from.1.min(h - from.1) / dy.max(1e-3)).min(1.0);
                    let (dx, dy) = (dx * room, dy * room);
                    [from.0 - dx, from.1 - dy, from.0 + dx, from.1 + dy]
                } else {
                    [from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1)]
                };
                editor.crop_draft = Some(draft);
            }
            Some(EditDrag::CropGrip { grip, start }) => {
                // The side or corner taken rests on an edge of the picture's.
                let p = if free { p } else { editor.rest(p, (grip.0 != 0, grip.1 != 0), None) };
                editor.pull_crop(grip, start, p, shift, middle);
            }
            Some(EditDrag::CropMove { from, start }) => {
                // Moved inside the picture, resting against its edges and
                // middle when near them.
                let [l, t, r, b] = start;
                let (dx, dy) = (within(p.0 - from.0, -l, w - r), within(p.1 - from.1, -t, h - b));
                let moved = [l + dx, t + dy, r + dx, b + dy];
                let (gx, gy, guides) = if free { (0.0, 0.0, (Vec::new(), Vec::new())) } else { snap_box(moved, &[editor.whole()], (&editor.edges.0, &editor.edges.1), SNAP / scale) };
                editor.guides = guides;
                editor.crop_draft = Some([moved[0] + gx, moved[1] + gy, moved[2] + gx, moved[3] + gy]);
            }
            Some(EditDrag::Scrub { field, x: from, start, before, .. }) => {
                let step = editor.step(field);
                let value = start + (x - from) as f32 * step * if shift { 10.0 } else { 1.0 };
                editor.doc = before.clone();
                editor.set_field(field, value);
                editor.drag = Some(EditDrag::Scrub { field, x: from, start, before, moved: true });
            }
            Some(EditDrag::Pick { hue, .. }) => pick_color(editor, hue, (x as i32, y as i32)),
            Some(EditDrag::Caret) => {
                if let Some(at) = editor.typing {
                    editor.caret = editor.doc.shapes[at].caret_at(p);
                }
            }
            // In the box about what is unsaved, the choice under the
            // pointer is the one Enter would take, as in the delete box.
            None if editor.leaving.is_some() => {
                if let Some(EditHit::Leave(choice)) = editor.hit_at(x as i32, y as i32) {
                    editor.leaving = Some(choice);
                }
                self.set_edit_cursor(CursorIcon::Default);
                return self.redraw();
            }
            None => {
                // The pointer says what a press would do, and with the
                // arrow in hand the mark under it is outlined.
                let over_stage = inside(editor.stage, (x as i32, y as i32));
                let hit = editor.hit_at(x as i32, y as i32);
                let on_handle = editor.handle_at(x as i32, y as i32);
                let hovered = if over_stage && hit.is_none() && editor.tool == Tool::Select {
                    let reach = 6.0 / scale;
                    editor.doc.shapes.iter().rposition(|shape| shape.hit(p, reach))
                } else {
                    None
                };
                editor.hovered = hovered;
                let in_crop = editor.crop_draft.is_some_and(|[l, t, r, b]| [l, t, r, b] != editor.whole() && p.0 >= l && p.0 <= r && p.1 >= t && p.1 <= b);
                let icon = match (over_stage && hit.is_none(), editor.tool) {
                    _ if matches!(hit, Some(EditHit::Field(field)) if field != Field::Hex) => CursorIcon::EwResize,
                    _ if on_handle.is_some() => match on_handle {
                        Some(Handle::Crop(gx, gy) | Handle::Grip(gx, gy)) => match (gx, gy) {
                            (0, _) => CursorIcon::NsResize,
                            (_, 0) => CursorIcon::EwResize,
                            (a, b) if a == b => CursorIcon::NwseResize,
                            _ => CursorIcon::NeswResize,
                        },
                        // No cursor says turn; a hand to take it by is nearest.
                        Some(Handle::Rotate) => CursorIcon::Grab,
                        _ => CursorIcon::Pointer,
                    },
                    (false, _) => CursorIcon::Default,
                    (true, Tool::Crop) if in_crop => CursorIcon::Move,
                    (true, Tool::Select) if hovered.is_some() => CursorIcon::Move,
                    (true, Tool::Select | Tool::Emoji) => CursorIcon::Default,
                    (true, Tool::Text | Tool::Badge) => CursorIcon::Text,
                    (true, _) => CursorIcon::Crosshair,
                };
                self.set_edit_cursor(icon);
                return self.redraw();
            }
        }
        self.redraw();
    }

    fn editor_release(&mut self) {
        let Some(editor) = &mut self.editor else { return };
        editor.guides = (Vec::new(), Vec::new());
        match editor.drag.take() {
            Some(EditDrag::Draw { .. }) => {
                let last = editor.doc.shapes.len() - 1;
                let shape = &mut editor.doc.shapes[last];
                // A counter's tail too short to point anywhere goes.
                if shape.tool == Tool::Counter && shape.points.len() > 1 && (shape.points[1].0 - shape.points[0].0).hypot(shape.points[1].1 - shape.points[0].1) < shape.counter_radius() * 1.4 {
                    shape.points.truncate(1);
                }
                if shape.is_slight() {
                    editor.doc.shapes.pop();
                    editor.past.pop();
                } else if matches!(editor.tool, Tool::Pen | Tool::Marker | Tool::Counter) {
                    // These come several at a time: the tool stays.
                } else {
                    if shape.tool == Tool::Zoom {
                        shape.place_zoom();
                    }
                    editor.made(last);
                }
            }
            Some(EditDrag::Move { from, before, moved, was_selected }) => {
                if moved {
                    editor.settle(before);
                } else if was_selected {
                    // A click on words already picked out goes to typing,
                    // the caret where the click was.
                    if let Some(at) = editor.one().filter(|at| matches!(editor.doc.shapes[*at].tool, Tool::Text | Tool::Badge)) {
                        editor.snap();
                        let caret = editor.doc.shapes[at].caret_at(from);
                        editor.start_typing(at, Some(caret));
                    }
                }
            }
            Some(EditDrag::Marquee { from, to, adding }) => editor.pick_within(from, to, adding),
            Some(EditDrag::Grip { before, .. } | EditDrag::Round { before, .. } | EditDrag::Zoom { before, .. } | EditDrag::Rotate { before, .. }) => editor.settle(before),
            Some(EditDrag::End { end, before, moved }) => {
                if moved {
                    editor.settle(before);
                } else if let Some(at) = editor.one().filter(|at| editor.doc.shapes[*at].tool.is_line()) {
                    // A click on an end puts a head there, or takes it off.
                    editor.snap();
                    let shape = &mut editor.doc.shapes[at];
                    shape.heads[end] = !shape.heads[end];
                    if shape.tool == Tool::Arrow && shape.heads != [false, false] {
                        editor.style.heads = shape.heads;
                    }
                }
            }
            Some(EditDrag::Bend { before, moved }) => {
                if moved {
                    editor.settle(before);
                } else if let Some(at) = editor.one() {
                    // A click on the bend straightens the line.
                    if editor.doc.shapes[at].bend.is_some() {
                        editor.snap();
                        editor.doc.shapes[at].bend = None;
                    }
                }
            }
            Some(EditDrag::Crop { .. }) => {
                // A click and no drag: the crop as it was.
                let small = editor.crop_draft.is_some_and(|[l, t, r, b]| (r - l) * editor.place.scale < 8.0 || (b - t) * editor.place.scale < 8.0);
                if small {
                    editor.crop_draft = Some(editor.doc.crop.unwrap_or(editor.whole()));
                }
            }
            Some(EditDrag::CropGrip { .. } | EditDrag::CropMove { .. }) => {}
            Some(EditDrag::Scrub { field, before, moved, .. }) => {
                if moved {
                    editor.settle(before);
                } else {
                    // A click and no drag: the number to type over.
                    let now = editor.field(field).map(shown).unwrap_or_default();
                    editor.editing = Some((field, now, true));
                }
            }
            Some(EditDrag::Caret) => {}
            Some(EditDrag::Pick { before, .. }) => {
                remember_color(editor.style.color);
                editor.settle(before);
            }
            None => return,
        }
        self.redraw();
    }

    fn discard_edit(&mut self) {
        self.toast("Discarded");
        self.close_editor();
    }

    /// One of the choices on leaving with something unsaved, by its place
    /// in `LEAVE`.
    fn leave_with(&mut self, choice: usize) {
        if let Some(editor) = &mut self.editor {
            editor.leaving = None;
        }
        match choice {
            1 => self.discard_edit(),
            2 => self.save_edit(SaveAs::Copy),
            3 => self.save_edit(SaveAs::Overwrite),
            _ => self.redraw(),
        }
    }

    /// Saves over the file, or opens the box naming the copy.
    pub(super) fn save_edit(&mut self, way: SaveAs) {
        let Some(editor) = &mut self.editor else { return };
        editor.finish_typing();
        editor.commit_field();
        if editor.tool == Tool::Crop {
            editor.apply_crop();
        }
        if !editor.dirty() {
            return self.toast("Nothing to save");
        }
        let path = self.files[editor.id].clone();
        match way {
            SaveAs::Overwrite => {
                if Format::of(&path).is_none() {
                    let ext = path.extension().unwrap_or_default().to_string_lossy().to_uppercase();
                    return self.toast(format!("Not over a {ext} · save a copy"));
                }
                self.write_edit(path, true);
            }
            SaveAs::Copy => {
                let id = editor.id;
                let name = copy_name(&path, editor.doc.radius > 0.0);
                let chars: Vec<char> = name.chars().collect();
                // The name but not its extension picked out, to type over.
                let stem = chars.iter().rposition(|c| *c == '.').unwrap_or(chars.len());
                let mut field = TextField::new(chars, stem);
                field.anchor = Some(0);
                self.prompt = Some(Prompt { id, kind: PromptKind::EditCopy, field });
                self.redraw();
            }
        }
    }

    /// Enter in the box naming the copy.
    pub(super) fn finish_edit_copy(&mut self, typed: &str) {
        let Some(editor) = &self.editor else { return self.prompt = None };
        let path = &self.files[editor.id];
        let typed = typed.trim();
        if typed.is_empty() {
            return;
        }
        let mut target = path.parent().map(|dir| dir.join(typed)).unwrap_or_else(|| PathBuf::from(typed));
        if target.extension().is_none() {
            let fallback = copy_name(path, editor.doc.radius > 0.0);
            target.set_extension(Path::new(&fallback).extension().unwrap_or_default());
        }
        if target.exists() {
            return;
        }
        if Format::of(&target).is_none() {
            return self.toast("JXL, AVIF, JPEG, PNG or WebP");
        }
        self.prompt = None;
        self.write_edit(target, false);
    }

    /// What `typed` in the box naming the copy would come to.
    pub(super) fn edit_copy_hint(&self, typed: &str) -> Option<String> {
        let editor = self.editor.as_ref()?;
        let path = &self.files[editor.id];
        let target = path.parent()?.join(typed.trim());
        if target.exists() {
            return Some(format!("{} exists", target.file_name()?.to_string_lossy()));
        }
        let jpeg = Format::of(&target) == Some(Format::Jpeg);
        if jpeg && editor.doc.radius > 0.0 {
            return Some("Enter to save  ·  JPEG · corners → white".to_string());
        }
        None
    }

    /// Writes the picture as edited on another thread, leaving Edit at
    /// once: the marks are kept until it is known to have worked.
    /// The picture as edited, onto the clipboard, from another thread; Edit
    /// stays open, the marks as they are.
    pub(super) fn copy_edit(&mut self) {
        let Some(editor) = &mut self.editor else { return };
        editor.finish_typing();
        editor.commit_field();
        if editor.tool == Tool::Crop {
            editor.apply_crop();
        }
        let (id, doc) = (editor.id, editor.doc.clone());
        let source = self.files[id].clone();
        let transform = self.transforms.get(&id).copied().unwrap_or_default();
        // A scripted test run must not replace the user's clipboard.
        if self.snapshot.is_some() {
            return self.toast("Copied");
        }
        self.toast("Copying…");
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = proxy.send_event(Event::EditCopied(copy_edited(&source, transform, &doc)));
        });
    }

    pub(super) fn edit_copied(&mut self, copied: Result<(u32, u32), String>) {
        match copied {
            Ok((w, h)) => self.toast(format!("Copied  ·  {w} × {h}")),
            Err(error) => self.toast(format!("Not copied · {error}")),
        }
    }

    fn write_edit(&mut self, target: PathBuf, over: bool) {
        let Some(editor) = self.editor.take() else { return };
        let (id, doc) = (editor.id, editor.doc.clone());
        let source = self.files[id].clone();
        if self.dry(|_| vec![format!("save the edit of {} as {}", source.display(), target.display())], "would save") {
            self.editor = Some(editor);
            return;
        }
        self.edit_stash = Some(editor);
        self.set_edit_cursor(CursorIcon::Default);
        let transform = self.transforms.get(&id).copied().unwrap_or_default();
        let settings = self.options.convert;
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.toast(format!("Saving → {name}…"));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = write_edited(&source, &target, over, transform, &doc, &settings);
            let _ = proxy.send_event(Event::EditSaved(Saved { id, target, over, result }));
        });
        self.redraw();
    }

    pub(super) fn edit_saved(&mut self, saved: Saved) {
        let Saved { id, target, over, result } = saved;
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let (size, backup) = match result {
            Ok(done) => done,
            Err(error) => {
                eprintln!("jog: cannot save {}: {error}", target.display());
                return self.toast(format!("Not saved · {error}  ·  e → back to it"));
            }
        };
        if self.edit_stash.as_ref().is_some_and(|editor| editor.id == id) {
            self.edit_stash = None;
        }
        println!("[edit] {}", target.display());
        if over {
            // What was turned on screen is turned in the file now.
            self.transforms.remove(&id);
            self.loader.set_transform(id, Transform::default());
            self.reload_file(id);
            if let Some(backup) = backup {
                self.did(Undo::Replaced { id, path: target, backup });
            }
            self.toast_with(format!("{name}  ·  {}  ·  saved over", meta::human_size(size)), vec![("Undo".into(), ToastLink::Undo)]);
        } else {
            self.made(target.clone(), format!("{name}  ·  {}", meta::human_size(size)));
            self.show_path(&target);
        }
    }

    /// The file is not what it was: what was decoded of it goes.
    pub(super) fn reload_file(&mut self, id: usize) {
        self.loader.invalidate(id);
        if id == self.id() {
            self.meta = None;
            self.meta_asked = None;
        }
        self.request();
        self.redraw();
    }

    /// Undo of a save over a file: the original put back.
    pub(super) fn restore_original(&mut self, id: usize, path: &Path, backup: &Path) {
        match put_back(backup, path) {
            Ok(()) => {
                println!("[undo] {} put back", path.display());
                self.toast(format!("Undone  ·  {} as it was", self.name_of(id)));
                self.reload_file(id);
            }
            Err(error) => self.toast(format!("Not undone · {error}")),
        }
    }

    pub(super) fn draw_editor(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        let frame = self.source();
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        let pointed = self
            .editor
            .as_ref()
            .filter(|editor| editor.tool == Tool::Pipette && inside(editor.stage, mouse))
            .and_then(|editor| self.picture_color(editor.place.to_picture((mouse.0 as f32, mouse.1 as f32))));
        let Some(editor) = &mut self.editor else { return };
        let name = self.files[editor.id].file_name().unwrap_or_default().to_string_lossy().into_owned();
        let format = Format::of(&self.files[editor.id]);
        let (width, height) = (canvas.width, canvas.height);
        let pad = px as i32;
        let line = Text::line_height(px);
        let small = (px * 5 / 6).max(11);
        let small_line = Text::line_height(small);
        canvas.fill(0, 0, width, height, GRID_COLOR);
        editor.hits.clear();
        editor.handles.clear();
        editor.tips.clear();
        editor.reach = (pad / 3).max(5);

        // The panel down the right, the bar along the bottom, and the
        // picture in what is left.
        let side = (px as i32 * 17).min(width / 3);
        let button = line + pad;
        let bar_h = button + pad / 2;
        let top = pad + line + pad / 2;
        let stage = [pad, top, width - side - pad * 3, height - top - bar_h - pad * 2];
        editor.stage = stage;
        let region = editor.region();
        let (rw, rh) = (region[2] - region[0], region[3] - region[1]);
        let scale = (stage[2] as f32 / rw).min(stage[3] as f32 / rh).min(4.0);
        let (pw, ph) = ((rw * scale).round() as i32, (rh * scale).round() as i32);
        let (px0, py0) = (stage[0] + (stage[2] - pw) / 2, stage[1] + (stage[3] - ph) / 2);
        editor.place = Place { x: px0 as f32 - region[0] * scale, y: py0 as f32 - region[1] * scale, scale };
        let place = editor.place;
        let cropping = editor.tool == Tool::Crop;
        let radius = if cropping { 0.0 } else { editor.doc.radius * scale };
        if let Some(frame) = frame.filter(|_| pw > 0 && ph > 0) {
            if radius > 0.0 {
                checker(canvas, [px0, py0, pw, ph]);
            }
            let fx = frame.width as f64 / f64::from(editor.native.0.max(1));
            let fy = frame.height as f64 / f64::from(editor.native.1.max(1));
            let crop = [f64::from(region[0]) * fx, f64::from(region[1]) * fy, f64::from(rw) * fx, f64::from(rh) * fy];
            let behind = if radius > 0.0 { canvas.copy(px0, py0, pw, ph) } else { Vec::new() };
            let _ = render::draw_crop(canvas.all(), width as usize, &frame.pixels, (frame.width, frame.height), crop, [px0, py0, pw, ph]);
            annotate::paint(&editor.doc.shapes, canvas.all(), width as usize, place, [px0, py0, pw, ph], None);
            if radius > 0.0 {
                round_corners(canvas, [px0, py0, pw, ph], radius, &behind);
            }
        }
        // The crop being set: the rest dimmed, its edge, its handles.
        if cropping {
            draw_crop_box(editor, canvas, text, [px0, py0, pw, ph], small, pad);
        }
        // The mark under the pointer, faintly; those picked out, outlined,
        // and the one picked out alone with its handles.
        if let Some(shape) = editor.hovered.filter(|at| !editor.selected.contains(at)).and_then(|at| editor.doc.shapes.get(at)) {
            let [x, y, w, h] = on_screen(place, shape.bounds(), 3);
            canvas.round_outline(x, y, w, h, 2, 1, accent_soft());
        }
        if editor.one().is_none() {
            for at in editor.selected.clone() {
                let [x, y, w, h] = on_screen(place, editor.doc.shapes[at].bounds(), 3);
                canvas.round_outline(x, y, w, h, 2, 1, accent());
            }
        }
        if let Some(at) = editor.typing.or(editor.one()) {
            let shape = editor.doc.shapes[at].clone();
            draw_handles(editor, canvas, &shape, pad);
            if editor.typing == Some(at) {
                draw_caret(canvas, place, &shape, editor.caret, editor.selection());
            }
        }
        // The edges what is moved rests against.
        for x in editor.guides.0.clone() {
            let (sx, _) = place.to_screen((x, 0.0));
            canvas.fill(sx.round() as i32, py0, 1, ph, accent_soft());
        }
        for y in editor.guides.1.clone() {
            let (_, sy) = place.to_screen((0.0, y));
            canvas.fill(px0, sy.round() as i32, pw, 1, accent_soft());
        }
        // A box being dragged out to pick out what it touches.
        if let Some(EditDrag::Marquee { from, to, .. }) = editor.drag {
            let rect = [from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1)];
            let [x, y, w, h] = on_screen(place, rect, 0);
            canvas.round_fill(x, y, w, h, 0, accent(), 40);
            canvas.round_outline(x, y, w, h, 0, 1, accent());
        }
        // The color the pipette is over, beside it.
        if let Some(color) = pointed {
            let words = format!("#{color:06X}");
            let (x, y) = (mouse.0 + pad, mouse.1 + pad);
            let w = text.width(&words, small) + pad * 2 + small_line;
            canvas.round_fill(x, y, w, small_line + pad / 2, pad / 3, 0x000000, 220);
            canvas.round_fill(x + pad / 3, y + pad / 4, small_line, small_line, small_line / 2, color, 256);
            text.draw(canvas, x + pad / 2 + small_line + pad / 3, y + pad / 4, small, TEXT_COLOR, &words);
        }

        // The heading: what is being edited, and whether it is saved.
        let (ow, oh) = editor.out_size();
        let kind = format.map_or_else(|| self.files[editor.id].extension().unwrap_or_default().to_string_lossy().to_uppercase(), |format| format.name().to_string());
        let heading = format!("{}  Edit", commands::icons::EDIT);
        text.draw(canvas, pad, pad / 2, px, TEXT_COLOR, &heading);
        let about = format!("{name}  ·  {ow} × {oh}  ·  {kind}{}", if editor.dirty() { "  ·  unsaved" } else { "" });
        let hx = pad + text.width(&heading, px) + pad;
        text.draw(canvas, hx, pad / 2 + (line - small_line) / 2, small, DIM_TEXT_COLOR, &about);

        // The tools along the bottom, then undo, redo and reset.
        let bar_y = height - pad - bar_h;
        let history = [(EditHit::Undo, commands::icons::UNDO, "Undo  Ctrl+Z", !editor.past.is_empty()), (EditHit::Redo, commands::icons::REDO, "Redo  Ctrl+Shift+Z", !editor.future.is_empty()), (EditHit::Reset, commands::icons::REFRESH, "Reset  as opened", editor.dirty())];
        let tools = Tool::BAR.iter().filter(|tool| tool.is_some()).count() as i32;
        let gaps = Tool::BAR.iter().filter(|tool| tool.is_none()).count() as i32 + 1;
        let bar_w = (tools + history.len() as i32) * button + gaps * pad / 2 + pad / 2;
        draw::panel(canvas, pad, bar_y, bar_w, bar_h, pad / 2);
        editor.hits.push((EditHit::Inside, [pad, bar_y, bar_w, bar_h]));
        let mut x = pad + pad / 4;
        let divider = |canvas: &mut Canvas, x: i32| canvas.fill(x + pad / 4 - 1, bar_y + pad / 2, 1, bar_h - pad, MENU_EDGE_COLOR);
        let mut emoji_at = 0;
        for tool in Tool::BAR {
            let Some(tool) = tool else {
                divider(canvas, x);
                x += pad / 2;
                continue;
            };
            let place = [x, bar_y + pad / 4, button, button];
            let on = editor.tool == tool;
            bar_button(canvas, text, place, tool_icon(tool), on, true, inside(place, mouse), px, pad);
            editor.hits.push((EditHit::Tool(tool), place));
            // A corner marked where clicking it again shows its ways.
            let more = !tool_options(tool).is_empty();
            if more {
                let (cx, cy) = (x + button - pad / 3, place[1] + button - pad / 3);
                for step in 0..4 {
                    canvas.fill(cx - step, cy - 3 + step, step + 1, 1, if on { on_accent() } else { DIM_TEXT_COLOR });
                }
            }
            let tip = if more && on { format!("{}  ·  again for its ways", tool.name()) } else { format!("{}  {}", tool.name(), tool.key().to_uppercase()) };
            editor.tips.push((place, tip));
            if tool == Tool::Emoji {
                emoji_at = x;
            }
            x += button;
        }
        divider(canvas, x);
        x += pad / 2;
        for (hit, icon, tip, able) in history {
            let place = [x, bar_y + pad / 4, button, button];
            bar_button(canvas, text, place, icon, false, able, inside(place, mouse), px, pad);
            editor.hits.push((hit, place));
            editor.tips.push((place, tip.to_string()));
            x += button;
        }

        // The ways out at the other end, alike: the two saves, and the
        // clipboard, which leaves Edit open.
        let dirty = editor.dirty();
        let mut right = width - side - pad * 2;
        let clipboard = format!("{}  Clipboard", commands::icons::CLIPBOARD);
        let ways = [
            (EditHit::Save(SaveAs::Overwrite), "Overwrite", "Ctrl+S", dirty && format.is_some()),
            (EditHit::Save(SaveAs::Copy), "Save copy", "Ctrl+Shift+S", dirty),
            (EditHit::Clipboard, clipboard.as_str(), "Ctrl+C  ·  as a PNG", true),
        ];
        for (hit, label, tip, able) in ways {
            let w = text.width(label, px) + pad * 2;
            let place = [right - w, bar_y, w, bar_h];
            let (ground, ink) = match able {
                true => (if inside(place, mouse) { PLACEHOLDER_COLOR } else { MENU_COLOR }, TEXT_COLOR),
                false => (MENU_COLOR, DIM_TEXT_COLOR),
            };
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 2, ground, 256);
            canvas.round_outline(place[0], place[1], place[2], place[3], pad / 2, 1, MENU_EDGE_COLOR);
            text.draw(canvas, place[0] + pad, bar_y + (bar_h - line) / 2, px, ink, label);
            editor.hits.push((hit, place));
            editor.tips.push((place, tip.to_string()));
            right -= w + pad / 2;
        }

        let color_row = draw_panel(editor, canvas, text, [width - side - pad, top, side, height - top - pad], px, format, mouse);

        // The colors beyond the eight, beside the panel.
        // Beside the panel, or under the pointer a right click was made at.
        let flyout_place = match editor.flyout_at {
            Some((x, y)) => Some([x + pad * 15 / 2, y + pad / 2]),
            None => color_row.map(|row| [width - side - pad * 2, row]),
        };
        if let (Some(hsv), Some(at)) = (editor.flyout, flyout_place) {
            draw_flyout(editor, canvas, text, hsv, at, px, mouse);
        }
        // The ways the tool in hand draws, over its button.
        if let Some((tool, left)) = editor.tool_menu {
            draw_tool_menu(editor, canvas, text, tool, [left, bar_y - pad / 2], px, mouse);
        }
        // The emoji to pick from, over the bar.
        if editor.picker.is_some() {
            draw_picker(editor, canvas, text, emoji_at, bar_y - pad / 2, px, mouse);
        }

        // Escape with something unsaved: a box in the middle, over the rest
        // dimmed, as the one for deleting is. Enter takes the choice picked
        // out, Keep editing to begin with; each has a key of its own.
        if let Some(focus) = editor.leaving {
            canvas.shade(0, 0, width, height, 150);
            editor.tips.clear();
            let (sign, sign_px) = (commands::icons::EDIT, px * 3 / 2);
            let sign_w = text.width(sign, sign_px) + pad;
            let title = "Unsaved edits";
            let button_h = line + pad;
            let widths: Vec<i32> = LEAVE.iter().map(|(word, _)| text.width(word, px) + pad * 2).collect();
            let buttons_w = widths.iter().sum::<i32>() + pad / 2 * (LEAVE.len() as i32 - 1);
            let words = text.width(title, px).max(text.width(&name, small));
            let box_w = ((sign_w + words).max(buttons_w) + pad * 3).min(width);
            let box_h = line + small_line + button_h + pad * 4;
            let (bx, by) = ((width - box_w) / 2, (height - box_h) / 2);
            draw::panel(canvas, bx, by, box_w, box_h, pad);
            editor.hits.push((EditHit::Inside, [bx, by, box_w, box_h]));
            text.draw(canvas, bx + pad * 3 / 2, by + pad * 3 / 2, sign_px, MARK_COLOR, sign);
            let wx = bx + pad * 3 / 2 + sign_w;
            text.draw(canvas, wx, by + pad * 3 / 2, px, TEXT_COLOR, title);
            text.draw(canvas, wx, by + pad * 3 / 2 + line, small, DIM_TEXT_COLOR, &name);
            // The buttons at the right, as the last thing read.
            let mut x = bx + box_w - pad * 3 / 2 - buttons_w;
            let y = by + box_h - pad * 3 / 2 - button_h;
            for (at, ((word, key), w)) in LEAVE.iter().zip(widths).enumerate() {
                // Over a file jog cannot write, only a copy.
                let able = at != 3 || format.is_some();
                let place = [x, y, w, button_h];
                let on = at == focus && able;
                let ground = match (on, at) {
                    (true, 1) => DANGER_COLOR,
                    (true, _) => accent(),
                    (false, _) => PLACEHOLDER_COLOR,
                };
                canvas.round_fill(x, y, w, button_h, pad / 2, ground, 256);
                if !on {
                    canvas.round_outline(x, y, w, button_h, pad / 2, 1, MENU_EDGE_COLOR);
                }
                let ink = match (on, able) {
                    (true, _) if at == 1 => 0xFFFFFF,
                    (true, _) => on_accent(),
                    (false, true) => TEXT_COLOR,
                    (false, false) => DIM_TEXT_COLOR,
                };
                let ww = text.width(word, px);
                text.draw(canvas, x + (w - ww) / 2, y + pad / 2, px, ink, word);
                if able {
                    editor.hits.push((EditHit::Leave(at), place));
                    editor.tips.push((place, key.to_string()));
                }
                x += w + pad / 2;
            }
        }

        // What the button under the pointer is, and its key.
        if editor.drag.is_none() && editor.tool_menu.is_none() {
            if let Some((place, tip)) = editor.tips.iter().find(|(place, _)| inside(*place, mouse)) {
                let w = text.width(tip, small) + pad;
                let h = small_line + pad / 2;
                let x = (place[0] + place[2] / 2 - w / 2).min(width - w - pad / 2).max(pad / 2);
                let y = place[1] - h - pad / 3;
                canvas.round_fill(x, y, w, h, pad / 3, 0x000000, 210);
                text.draw(canvas, x + pad / 2, y + pad / 4, small, TEXT_COLOR, tip);
            }
        }
    }
}

/// The ways a tool can draw, as its menu lists them, a `None` where a line
/// parts one kind of choice from the next.
fn tool_options(tool: Tool) -> Vec<Option<(&'static str, EditHit)>> {
    let dashes = [Some(("Solid", EditHit::Dash(Dash::Solid))), Some(("Dashed", EditHit::Dash(Dash::Dashed))), Some(("Dotted", EditHit::Dash(Dash::Dotted)))];
    match tool {
        Tool::Arrow => [Some(("One head", EditHit::Heads([false, true]))), Some(("A head at each end", EditHit::Heads([true, true]))), Some(("No heads", EditHit::Heads([false, false]))), None].into_iter().chain(dashes).collect(),
        Tool::Line => dashes.to_vec(),
        Tool::Rect | Tool::Oval => [Some(("Outlined", EditHit::Filled(false))), Some(("Filled", EditHit::Filled(true))), None].into_iter().chain(dashes).collect(),
        Tool::Zoom => vec![Some(("Round", EditHit::Lens(false))), Some(("Square", EditHit::Lens(true)))],
        Tool::Text => FACES.iter().enumerate().map(|(at, name)| Some((*name, EditHit::Face(at)))).collect(),
        Tool::Badge => BADGE_NAMES.iter().enumerate().map(|(at, name)| Some((*name, EditHit::BadgeLook(at)))).collect(),
        Tool::Crop => ASPECTS.iter().map(|(name, ratio)| Some((*name, EditHit::Aspect(*ratio)))).collect(),
        _ => Vec::new(),
    }
}

/// `value` kept between `low` and `high`, or at `low` when there is no
/// room between them, as `clamp` would panic at: a crop at the edge of
/// the picture, or a sum a rounding off.
fn within(value: f32, low: f32, high: f32) -> f32 {
    value.min(high).max(low)
}

fn inside([l, t, w, h]: [i32; 4], (x, y): (i32, i32)) -> bool {
    (l..l + w).contains(&x) && (t..t + h).contains(&y)
}

/// A rectangle of the picture, on screen, `more` pixels larger all round.
fn on_screen(place: Place, [l, t, r, b]: [f32; 4], more: i32) -> [i32; 4] {
    let (l, t) = place.to_screen((l, t));
    let (r, b) = place.to_screen((r, b));
    [l.floor() as i32 - more, t.floor() as i32 - more, (r - l).ceil() as i32 + more * 2, (b - t).ceil() as i32 + more * 2]
}

/// The end of a line kept to a multiple of 45 degrees from `from`.
fn level(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = dx.hypot(dy);
    let angle = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    (from.0 + length * angle.cos(), from.1 + length * angle.sin())
}

/// The far corner of a box kept square, or a circle round.
fn square(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let side = dx.abs().max(dy.abs());
    (from.0 + side.copysign(dx), from.1 + side.copysign(dy))
}

/// The crop being set: what falls outside it dimmed, its edge, a bracket
/// at each corner and a bar on each side to take it by, and its size.
fn draw_crop_box(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, [px0, py0, pw, ph]: [i32; 4], small: u32, pad: i32) {
    let draft = editor.crop_draft.unwrap_or(editor.whole());
    let [l, t, w, h] = on_screen(editor.place, draft, 0);
    let (r, b) = (l + w, t + h);
    for [x, y, w, h] in [[px0, py0, pw, t - py0], [px0, b, pw, py0 + ph - b], [px0, t, l - px0, b - t], [r, t, px0 + pw - r, b - t]] {
        canvas.shade(x, y, w, h, 90);
    }
    canvas.round_outline(l, t, w, h, 0, 1, 0xFFFFFF);
    let (arm, thick) = ((pad * 3 / 2).min(w / 3).min(h / 3), 3);
    for (gx, gy) in [(-1i8, -1i8), (1, -1), (1, 1), (-1, 1)] {
        let (cx, cy) = (if gx < 0 { l } else { r }, if gy < 0 { t } else { b });
        let (hx, vy) = (if gx < 0 { cx - thick + 1 } else { cx - arm }, if gy < 0 { cy - thick + 1 } else { cy - arm });
        canvas.fill(hx.min(cx - thick + 1).max(if gx < 0 { cx - thick + 1 } else { cx - arm }), if gy < 0 { cy - thick + 1 } else { cy - 1 }, arm + thick - 1, thick, 0xFFFFFF);
        canvas.fill(if gx < 0 { cx - thick + 1 } else { cx - 1 }, vy, thick, arm + thick - 1, 0xFFFFFF);
        editor.handles.push((Handle::Crop(gx, gy), (cx, cy)));
    }
    for (gx, gy) in [(0i8, -1i8), (1, 0), (0, 1), (-1, 0)] {
        let (cx, cy) = (l + w * (i32::from(gx) + 1) / 2, t + h * (i32::from(gy) + 1) / 2);
        if gx == 0 {
            canvas.fill(cx - arm / 2, cy - 1, arm, thick, 0xFFFFFF);
        } else {
            canvas.fill(cx - 1, cy - arm / 2, thick, arm, 0xFFFFFF);
        }
        editor.handles.push((Handle::Crop(gx, gy), (cx, cy)));
    }
    let small_line = Text::line_height(small);
    let label = format!("{} × {}", (draft[2] - draft[0]).round(), (draft[3] - draft[1]).round());
    let lw = text.width(&label, small) + pad;
    let ly = if t - small_line - pad / 2 >= py0 { t - small_line - pad / 2 } else { b + pad / 3 };
    canvas.round_fill(l + (w - lw) / 2, ly, lw, small_line + pad / 3, pad / 3, 0x000000, 200);
    text.draw(canvas, l + (w - lw) / 2 + pad / 2, ly + pad / 6, small, TEXT_COLOR, &label);
}

/// Where the next letter typed goes: after the last of the last line.
/// What of the words is picked out, washed over in the accent, and the
/// caret: both turned as the words are.
fn draw_caret(canvas: &mut Canvas, place: Place, shape: &Shape, caret: usize, selection: Option<(usize, usize)>) {
    let screen = |p: Point| {
        let (x, y) = place.to_screen(shape.returned(p));
        (x.round() as i32, y.round() as i32)
    };
    if let Some((from, to)) = selection {
        // A band for each line it runs over.
        let mut start = 0;
        for line in shape.text.split('\n') {
            let end = start + line.chars().count();
            let (a, b) = (from.max(start), to.min(end));
            if a < b {
                let ((x0, y0), height) = shape.caret_spot(a);
                let ((x1, _), _) = shape.caret_spot(b);
                let corners = [(x0, y0), (x1, y0), (x1, y0 + height), (x0, y0 + height)].map(|p| place.to_screen(shape.returned(p)));
                let (xs, ys) = (corners.map(|p| p.0), corners.map(|p| p.1));
                let lo = (xs.iter().copied().fold(f32::MAX, f32::min).floor() as i32, ys.iter().copied().fold(f32::MAX, f32::min).floor() as i32);
                let hi = (xs.iter().copied().fold(f32::MIN, f32::max).ceil() as i32, ys.iter().copied().fold(f32::MIN, f32::max).ceil() as i32);
                for y in lo.1.max(0)..hi.1.min(canvas.height) {
                    for x in lo.0.max(0)..hi.0.min(canvas.width) {
                        let q = shape.unturn(place.to_picture((x as f32 + 0.5, y as f32 + 0.5)));
                        if q.0 >= x0 && q.0 <= x1 && q.1 >= y0 && q.1 <= y0 + height {
                            canvas.round_fill(x, y, 1, 1, 0, accent(), 110);
                        }
                    }
                }
            }
            start = end + 1;
        }
    }
    let ((x, y), height) = shape.caret_spot(caret);
    let (top, bottom) = (screen((x, y)), screen((x, y + height)));
    line(canvas, top, bottom, accent());
    line(canvas, (top.0 + 1, top.1), (bottom.0 + 1, bottom.1), accent());
}

#[allow(clippy::too_many_arguments)]
fn bar_button(canvas: &mut Canvas, text: &mut Text, place: [i32; 4], icon: &str, on: bool, able: bool, over: bool, px: u32, pad: i32) {
    let line = Text::line_height(px);
    if on || (over && able) {
        canvas.round_fill(place[0], place[1], place[2], place[3], pad / 2, if on { accent() } else { PLACEHOLDER_COLOR }, 256);
    }
    let ink = match (on, able) {
        (true, _) => on_accent(),
        (false, true) => TEXT_COLOR,
        (false, false) => 0x555555,
    };
    let iw = text.width(icon, px);
    text.draw(canvas, place[0] + (place[2] - iw) / 2, place[1] + (place[3] - line) / 2, px, ink, icon);
}

/// The handles of the mark picked out: grips at its corners and sides,
/// dots at a line's ends and middle, inside a rectangle's corners the
/// dots that round them, and a zoom's two.
fn draw_handles(editor: &mut Editor, canvas: &mut Canvas, shape: &Shape, pad: i32) {
    let place = editor.place;
    let grip = (pad / 5).max(3);
    let ring = |canvas: &mut Canvas, (x, y): (i32, i32), r: i32| {
        canvas.round_fill(x - r - 1, y - r - 1, (r + 1) * 2, (r + 1) * 2, r + 1, accent(), 256);
        canvas.round_fill(x - r, y - r, r * 2, r * 2, r, 0xFFFFFF, 256);
    };
    let screen = |p: Point| {
        let (x, y) = place.to_screen(p);
        (x.round() as i32, y.round() as i32)
    };
    let [x, y, w, h] = on_screen(place, shape.bounds(), 3);
    match shape.tool {
        Tool::Arrow | Tool::Line => {
            for end in 0..2 {
                let at = screen(shape.points[end]);
                ring(canvas, at, grip + 1);
                editor.handles.push((Handle::End(end), at));
            }
            let (a, b) = (screen(shape.points[0]), screen(shape.points[1]));
            if ((a.0 - b.0).abs() + (a.1 - b.1).abs()) >= pad * 3 || shape.bend.is_some() {
                let at = screen(shape.middle());
                ring(canvas, at, grip);
                editor.handles.push((Handle::Bend, at));
            }
        }
        Tool::Counter => {
            canvas.round_outline(x, y, w, h, 2, 1, accent());
            if let Some(tip) = shape.points.get(1) {
                let at = screen(*tip);
                ring(canvas, at, grip + 1);
                editor.handles.push((Handle::End(1), at));
            }
        }
        Tool::Zoom => {
            for rect in [shape.frame(), shape.zoomed()] {
                let [x, y, w, h] = on_screen(place, rect, 2);
                canvas.round_outline(x, y, w, h, 2, 1, accent());
            }
            let [middle, rim] = shape.zoom_handles();
            ring(canvas, screen(middle), grip + 1);
            ring(canvas, screen(rim), grip + 1);
            editor.handles.push((Handle::ZoomMove, screen(middle)));
            editor.handles.push((Handle::ZoomRim, screen(rim)));
        }
        _ => {
            // Its box, turned as it is, a little larger than it: where each
            // grip of it is on screen.
            let more = 3.0 / place.scale;
            let [l, t, r, b] = shape.local_bounds();
            let (l, t, r, b) = (l - more, t - more, r + more, b + more);
            let at = |gx: i8, gy: i8| screen(shape.returned((l + (r - l) * f32::from(gx + 1) / 2.0, t + (b - t) * f32::from(gy + 1) / 2.0)));
            let corners = [at(-1, -1), at(1, -1), at(1, 1), at(-1, 1)];
            for side in 0..4 {
                line(canvas, corners[side], corners[(side + 1) % 4], accent());
            }
            if editor.typing.is_some() {
                return;
            }
            // A badge is as large as its words: no grips.
            if shape.tool != Tool::Badge {
                for (gx, gy) in [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)] {
                    // Words keep their proportions: their corners only.
                    if shape.tool.lettered() && (gx == 0 || gy == 0) {
                        continue;
                    }
                    let spot = at(gx, gy);
                    canvas.fill(spot.0 - grip - 1, spot.1 - grip - 1, (grip + 1) * 2, (grip + 1) * 2, accent());
                    canvas.fill(spot.0 - grip + 1, spot.1 - grip + 1, (grip - 1) * 2, (grip - 1) * 2, 0xFFFFFF);
                    editor.handles.push((Handle::Grip(gx, gy), spot));
                }
            }
            // Just outside each corner, as in Figma, a place to take it by
            // and turn it; after the grips, which win where they meet.
            if shape.turns() {
                let (cx, cy) = screen(shape.centre());
                for corner in corners {
                    let (dx, dy) = ((corner.0 - cx) as f32, (corner.1 - cy) as f32);
                    let far = dx.hypot(dy).max(1.0);
                    let out = pad as f32 * 0.9;
                    editor.handles.push((Handle::Rotate, (corner.0 + (dx / far * out) as i32, corner.1 + (dy / far * out) as i32)));
                }
            }
            // Corners to round, where there is room for them, on a box that
            // is square to the picture.
            if matches!(shape.tool, Tool::Rect | Tool::Spotlight) && shape.angle == 0.0 && w.min(h) >= pad * 3 {
                let [l, t, r, b] = shape.frame();
                let inset = (shape.radius * place.scale).max(pad as f32).min(w.min(h) as f32 / 2.0 - 2.0);
                for (cx, cy) in [(-1i8, -1i8), (1, -1), (1, 1), (-1, 1)] {
                    let (sx, sy) = place.to_screen((if cx < 0 { l } else { r }, if cy < 0 { t } else { b }));
                    let at = ((sx - f32::from(cx) * inset).round() as i32, (sy - f32::from(cy) * inset).round() as i32);
                    ring(canvas, at, grip - 1);
                    editor.handles.push((Handle::Round(cx, cy), at));
                }
            }
        }
    }
}

/// A line a pixel wide from `a` to `b`.
fn line(canvas: &mut Canvas, a: (i32, i32), b: (i32, i32), color: u32) {
    let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).max(1);
    for step in 0..=steps {
        let x = a.0 + (b.0 - a.0) * step / steps;
        let y = a.1 + (b.1 - a.1) * step / steps;
        canvas.fill(x, y, 1, 1, color);
    }
}

/// A press on a handle of the mark picked out, or of the crop.
fn grab(editor: &mut Editor, handle: Handle, at: Point) {
    if let Handle::Crop(gx, gy) = handle {
        let start = editor.crop_draft.unwrap_or(editor.whole());
        editor.drag = Some(EditDrag::CropGrip { grip: (gx, gy), start });
        return;
    }
    let Some(index) = editor.one() else { return };
    let p = editor.place.to_picture(at);
    let before = editor.doc.clone();
    editor.drag = Some(match handle {
        Handle::Grip(gx, gy) => EditDrag::Grip { grip: (gx, gy), frame: editor.doc.shapes[index].local_bounds(), from: p, before },
        Handle::Rotate => {
            let centre = editor.doc.shapes[index].centre();
            EditDrag::Rotate { centre, from: (p.1 - centre.1).atan2(p.0 - centre.0), before }
        }
        Handle::End(end) => EditDrag::End { end, before, moved: false },
        Handle::Bend => EditDrag::Bend { before, moved: false },
        Handle::Round(cx, cy) => EditDrag::Round { corner: (cx, cy), before },
        Handle::ZoomMove => EditDrag::Zoom { rim: false, before },
        Handle::ZoomRim => EditDrag::Zoom { rim: true, before },
        Handle::Crop(..) => return,
    });
}

/// A press on the picture, with the tool in hand; `adding` with Shift or
/// Ctrl, which adds to what is picked out rather than starting afresh.
fn editor_press(editor: &mut Editor, at: Point, adding: bool) {
    if !inside(editor.stage, (at.0 as i32, at.1 as i32)) {
        editor.finish_typing();
        return;
    }
    editor.leaving = None;
    // In the words being typed: the caret there, or with Shift what is
    // picked out grown to there; a drag from it picks out what it goes over.
    let p = editor.place.to_picture(at);
    if let Some(typing) = editor.typing.filter(|typing| editor.doc.shapes[*typing].hit(p, 4.0 / editor.place.scale)) {
        let caret = editor.doc.shapes[typing].caret_at(p);
        editor.put_caret(caret, adding);
        if editor.anchor.is_none() {
            editor.anchor = Some(caret);
        }
        editor.drag = Some(EditDrag::Caret);
        return;
    }
    editor.finish_typing();
    let reach = 6.0 / editor.place.scale;
    match editor.tool {
        Tool::Select | Tool::Emoji => {
            editor.picker = None;
            editor.tool = Tool::Select;
            let hit = editor.doc.shapes.iter().rposition(|shape| shape.hit(p, reach));
            // What is picked out is taken hold of anywhere in its box.
            let held = editor.selected.iter().any(|at| {
                let [l, t, r, b] = editor.doc.shapes[*at].bounds();
                p.0 >= l - reach && p.0 <= r + reach && p.1 >= t - reach && p.1 <= b + reach
            });
            match (hit, adding) {
                (Some(at), true) => {
                    if let Some(found) = editor.selected.iter().position(|chosen| *chosen == at) {
                        editor.selected.remove(found);
                    } else {
                        editor.selected.push(at);
                    }
                }
                _ if held && !adding => {
                    let was_selected = editor.one().is_some();
                    editor.drag = Some(EditDrag::Move { from: p, before: editor.doc.clone(), moved: false, was_selected });
                }
                (Some(at), false) => {
                    editor.selected = vec![at];
                    editor.drag = Some(EditDrag::Move { from: p, before: editor.doc.clone(), moved: false, was_selected: false });
                }
                (None, adding) => editor.drag = Some(EditDrag::Marquee { from: p, to: p, adding }),
            }
        }
        Tool::Crop => {
            // Inside the crop it moves; outside, or while it is all of the
            // picture, a new one is dragged out.
            let draft = editor.crop_draft.unwrap_or(editor.whole());
            let [l, t, r, b] = draft;
            if draft != editor.whole() && p.0 >= l && p.0 <= r && p.1 >= t && p.1 <= b {
                editor.drag = Some(EditDrag::CropMove { from: p, start: draft });
            } else {
                let [_, _, w, h] = editor.whole();
                let from = (p.0.clamp(0.0, w), p.1.clamp(0.0, h));
                editor.crop_draft = Some([from.0, from.1, from.0, from.1]);
                editor.drag = Some(EditDrag::Crop { from });
            }
        }
        Tool::Text | Tool::Badge => {
            // On words already there, more of them; elsewhere, new ones.
            let tool = editor.tool;
            let on = editor.doc.shapes.iter().rposition(|shape| shape.tool == tool && shape.hit(p, reach));
            editor.snap();
            match on {
                // The caret where the click was.
                Some(at) => {
                    let caret = editor.doc.shapes[at].caret_at(p);
                    editor.start_typing(at, Some(caret));
                }
                None => {
                    let stroke = editor.default_stroke(tool);
                    // The pointer in the middle of the first line.
                    let mut shape = editor.fresh(tool, (p.0, p.1 - stroke * 0.65));
                    // A badge says what it is for, picked out to be typed over.
                    let caret = if tool == Tool::Badge {
                        shape.text = "Label".to_string();
                        None
                    } else {
                        Some(0)
                    };
                    editor.doc.shapes.push(shape);
                    editor.start_typing(editor.doc.shapes.len() - 1, caret);
                }
            }
        }
        Tool::Pipette => {}
        tool => {
            editor.snap();
            let shape = editor.fresh(tool, p);
            editor.doc.shapes.push(shape);
            editor.drag = Some(EditDrag::Draw { from: p });
            editor.selected.clear();
        }
    }
}

/// The square of the color picker, or its strip of hues, at the pointer.
fn pick_color(editor: &mut Editor, hue: bool, (x, y): (i32, i32)) {
    let target = if hue { EditHit::HueStrip } else { EditHit::Square };
    let Some([l, t, w, h]) = editor.hits.iter().find(|(hit, _)| *hit == target).map(|(_, place)| *place) else { return };
    let Some((mut hh, mut s, mut v)) = editor.flyout else { return };
    let across = ((x - l) as f32 / w.max(1) as f32).clamp(0.0, 1.0);
    if hue {
        hh = across * 360.0;
    } else {
        s = across;
        v = 1.0 - ((y - t) as f32 / h.max(1) as f32).clamp(0.0, 1.0);
    }
    editor.flyout = Some((hh, s, v));
    editor.recolor(annotate::from_hsv((hh, s, v)));
}

/// The panel down the right, as Figma has one: where and how large the
/// mark picked out is, its color, its line, what is its own; then the
/// picture's crop and corners; then the keys. Where the color row is, for
/// the colors beyond the eight to open beside.
#[allow(clippy::too_many_arguments)]
fn draw_panel(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, [ix, top, side, high]: [i32; 4], px: u32, format: Option<Format>, mouse: (i32, i32)) -> Option<i32> {
    let pad = px as i32;
    let line = Text::line_height(px);
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    draw::panel(canvas, ix, top, side, high, pad / 2);
    editor.hits.push((EditHit::Inside, [ix, top, side, high]));
    let inner = ix + pad;
    let inner_w = side - pad * 2;
    let half = (inner_w - pad / 2) / 2;
    let mut y = top + pad;
    let subject = editor.subject().map(|at| editor.doc.shapes[at].clone());
    let kind = subject.as_ref().map_or(editor.tool, |shape| shape.tool);
    let heading = |canvas: &mut Canvas, text: &mut Text, y: i32, words: &str| text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, words);
    let marks = !matches!(kind, Tool::Select | Tool::Crop) || editor.selected.len() > 1;
    let words = matches!(kind, Tool::Text | Tool::Badge);
    let mut color_row = None;
    // A row of two numbers side by side.
    let pair = |editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, y: i32, a: (&str, Field), b: (&str, Field)| {
        number(editor, canvas, text, [inner, y, half, line], a.0, a.1, small, mouse);
        number(editor, canvas, text, [inner + half + pad / 2, y, half, line], b.0, b.1, small, mouse);
    };

    if marks {
        let title = if editor.selected.len() > 1 { format!("{} picked out", editor.selected.len()) } else { kind.name().to_string() };
        text.draw(canvas, inner, y, px, TEXT_COLOR, &title);
        y += line + pad / 2;
    }
    // Where it is and how large, for one with a box.
    if editor.subject_box().is_some() && editor.typing.is_none() {
        pair(editor, canvas, text, y, ("X", Field::X), ("Y", Field::Y));
        y += line + pad / 3;
        pair(editor, canvas, text, y, ("W", Field::W), ("H", Field::H));
        y += line + pad / 3;
        // How far it is turned, for those that turn by an angle.
        if subject.as_ref().is_some_and(Shape::turnable) {
            number(editor, canvas, text, [inner, y, half, line], commands::icons::ROTATE_CW, Field::Angle, small, mouse);
            editor.tips.push(([inner, y, half, line], "Turn  ·  or drag just outside a corner".to_string()));
            y += line + pad / 3;
        }
        y += pad * 2 / 3;
    }
    let colored = marks && !matches!(kind, Tool::Blur | Tool::Spotlight | Tool::Emoji | Tool::Image);
    if colored {
        heading(canvas, text, y, "Color");
        y += small_line + pad / 3;
        color_row = Some(y);
        let color = subject.as_ref().map_or(editor.style.color, |shape| shape.color);
        let swatch = (inner_w - pad / 3 * 8) / 9;
        for (at, c) in COLORS.iter().enumerate() {
            let sx = inner + at as i32 * (swatch + pad / 3);
            swatch_cell(canvas, [sx, y, swatch, swatch], *c, *c == color);
            editor.hits.push((EditHit::Color(*c), [sx, y, swatch, swatch]));
        }
        // The ninth opens the rest, and shows a color from among them.
        let sx = inner + 8 * (swatch + pad / 3);
        let other = !COLORS.contains(&color);
        swatch_cell(canvas, [sx, y, swatch, swatch], if other { color } else { MENU_COLOR }, other || editor.flyout.is_some());
        if !other {
            let plus = "+";
            let pw = text.width(plus, small);
            text.draw(canvas, sx + (swatch - pw) / 2, y + (swatch - small_line) / 2, small, TEXT_COLOR, plus);
        }
        editor.hits.push((EditHit::More, [sx, y, swatch, swatch]));
        editor.tips.push(([sx, y, swatch, swatch], "More colors".to_string()));
        y += swatch + pad / 2;
        // The color in hex, and how opaque.
        let hex_w = inner_w - half - pad / 2;
        canvas.round_fill(inner, y, hex_w, line, line / 4, 0x161616, 256);
        canvas.round_fill(inner + pad / 3, y + pad / 3, line - pad * 2 / 3, line - pad * 2 / 3, 3, color, 256);
        let editing_hex = editor.editing.as_ref().filter(|editing| editing.0 == Field::Hex).map(|editing| editing.1.clone());
        let hex = editing_hex.clone().unwrap_or_else(|| format!("{color:06X}"));
        let hx = inner + line;
        text.draw(canvas, hx, y + (line - small_line) / 2, small, TEXT_COLOR, &hex);
        if editing_hex.is_some() {
            canvas.round_outline(inner, y, hex_w, line, line / 4, 1, accent());
            let cw = text.width(&hex, small);
            canvas.fill(hx + cw + 1, y + (line - small_line) / 2, 2, small_line, accent());
        }
        editor.hits.push((EditHit::Field(Field::Hex), [inner, y, hex_w, line]));
        number(editor, canvas, text, [inner + hex_w + pad / 2, y, half, line], "%", Field::Opacity, small, mouse);
        y += line + pad;
    }
    // The line: how wide, and solid, dashed or dotted, with its dashes'.
    if marks && !matches!(kind, Tool::Emoji | Tool::Image) {
        let (label, letter) = match kind {
            Tool::Text | Tool::Badge => ("Text", "Size"),
            Tool::Counter => ("Counter", "Size"),
            Tool::Blur => ("Blur", "Cell"),
            Tool::Spotlight => ("Spotlight", "Edge"),
            _ => ("Stroke", "Width"),
        };
        if !matches!(kind, Tool::Spotlight) {
            heading(canvas, text, y, label);
            y += small_line + pad / 3;
            number(editor, canvas, text, [inner, y, half, line], letter, Field::Stroke, small, mouse);
            if matches!(kind, Tool::Rect | Tool::Oval) {
                let filled = subject.as_ref().map_or(editor.style.filled, |shape| shape.filled);
                let cells = segments(canvas, text, [inner + half + pad / 2, y, half, line], &["Line", "Fill"], Some(usize::from(filled)), small, mouse);
                editor.hits.extend(cells.into_iter().enumerate().map(|(at, place)| (EditHit::Filled(at == 1), place)));
            }
            y += line + pad / 3;
        }
        let dashing = kind.is_line() || (matches!(kind, Tool::Rect | Tool::Oval) && !subject.as_ref().map_or(editor.style.filled, |shape| shape.filled));
        if dashing {
            let dash = subject.as_ref().map_or(editor.style.dash, |shape| shape.dash);
            y = dashes(editor, canvas, text, [inner, y, inner_w, line], dash, small, mouse) - pad / 2 + pad / 3;
            if dash != Dash::Solid {
                let first = if dash == Dash::Dotted { "Dot" } else { "Dash" };
                pair(editor, canvas, text, y, (first, Field::DashLength), ("Gap", Field::Gap));
                y += line + pad / 3;
            }
        }
        y += pad * 2 / 3;
    }
    if matches!(kind, Tool::Rect | Tool::Spotlight | Tool::Image) && editor.one().is_some() {
        heading(canvas, text, y, "Corners");
        y += small_line + pad / 3;
        number(editor, canvas, text, [inner, y, half, line], "R", Field::Radius, small, mouse);
        y += line + pad;
    }
    if kind.is_line() {
        heading(canvas, text, y, "Ends");
        y += small_line + pad / 3;
        let heads = subject.as_ref().map_or(if kind == Tool::Arrow { editor.style.heads } else { [false, false] }, |shape| shape.heads);
        let choices = [[false, false], [false, true], [true, true]];
        let cells = segments(canvas, text, [inner, y, inner_w, line], &["", "", ""], choices.iter().position(|c| *c == heads), small, mouse);
        for (cell, choice) in cells.iter().zip(choices) {
            preview(canvas, *cell, choice, Dash::Solid, pad);
            editor.hits.push((EditHit::Heads(choice), *cell));
        }
        y += line + pad / 3;
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Click an end  ·  drag the middle");
        y += small_line + pad;
    }
    if words {
        heading(canvas, text, y, "Font");
        y += small_line + pad / 3;
        let face = subject.as_ref().map_or(editor.style.face, |shape| shape.face);
        let cells = segments(canvas, text, [inner, y, inner_w, line], &[""; 5], Some(face), small, mouse);
        let width = canvas.width as usize;
        for (at, cell) in cells.iter().enumerate() {
            // Each face shows itself.
            let size = small as f32 * 1.1;
            let look = Look { face: at, bold: false, italic: false };
            let w = lettering::width(look, "Aa", size);
            let ink = if at == face { TEXT_COLOR } else { DIM_TEXT_COLOR };
            let clip = [cell[0], cell[1], cell[0] + cell[2], cell[1] + cell[3]];
            let spot = (cell[0] as f32 + (cell[2] as f32 - w) / 2.0, cell[1] as f32 + (cell[3] as f32 - lettering::line_height(size)) / 2.0);
            lettering::draw(canvas.all(), width, clip, spot, size, ink, 256, look, "Aa", None);
            editor.hits.push((EditHit::Face(at), *cell));
            editor.tips.push((*cell, format!("{}  G", FACES[at])));
        }
        y += line + pad / 3;
    }
    if kind == Tool::Text {
        let style = subject.as_ref().map_or(editor.style.text, |shape| shape.style);
        let flags = [(BOLD, "B", "Bold  Ctrl+B"), (ITALIC, "I", "Italic  Ctrl+I"), (HIGHLIGHT, "H", "Highlight"), (BOXED, "A", "On a patch"), (BULLETS, "•", "Bullets")];
        let cells = segments(canvas, text, [inner, y, inner_w, line], &flags.map(|(_, label, _)| label), None, small, mouse);
        for ((flag, label, tip), cell) in flags.iter().zip(cells) {
            if style & flag != 0 {
                canvas.round_fill(cell[0], cell[1], cell[2], cell[3], line / 4, accent(), 256);
                let lw = text.width(label, small);
                text.draw(canvas, cell[0] + (cell[2] - lw) / 2, cell[1] + (line - small_line) / 2, small, on_accent(), label);
            }
            editor.hits.push((EditHit::Toggle(*flag), cell));
            editor.tips.push((cell, tip.to_string()));
        }
        y += line + pad;
    } else if words {
        y += pad * 2 / 3;
    }
    if kind == Tool::Badge {
        heading(canvas, text, y, "Look");
        y += small_line + pad / 3;
        let look = subject.as_ref().map_or(editor.style.badge, |shape| shape.look);
        let color = subject.as_ref().map_or(editor.style.color, |shape| shape.color);
        let (across, gap) = (3, pad / 3);
        let cell_w = (inner_w - gap * (across - 1)) / across;
        let cell_h = line + pad / 2;
        let width = canvas.width as usize;
        for at in 0..BADGE_NAMES.len() {
            let (column, row) = (at as i32 % across, at as i32 / across);
            let cell = [inner + column * (cell_w + gap), y + row * (cell_h + gap), cell_w, cell_h];
            if at == look || inside(cell, mouse) {
                canvas.round_fill(cell[0], cell[1], cell[2], cell[3], pad / 3, if at == look { PLACEHOLDER_COLOR } else { 0x202020 }, 256);
            }
            // Each look as it would be, with its name on it.
            let size = small as f32 * 0.8;
            let mut sample = Shape::new(Tool::Badge, color, size, (0.0, 0.0));
            sample.look = at;
            sample.text = BADGE_NAMES[at].to_string();
            let [l, t, r, b] = sample.bounds();
            sample.points[0] = (cell[0] as f32 + (cell[2] as f32 - (r - l)) / 2.0, cell[1] as f32 + (cell[3] as f32 - (b - t)) / 2.0);
            annotate::paint(&[sample], canvas.all(), width, Place::WHOLE, cell, None);
            editor.hits.push((EditHit::BadgeLook(at), cell));
        }
        y += (BADGE_NAMES.len() as i32 + across - 1) / across * (cell_h + gap) + pad;
    }
    if kind == Tool::Zoom {
        heading(canvas, text, y, "Lens");
        y += small_line + pad / 3;
        let square = subject.as_ref().map_or(editor.style.square, |shape| shape.filled);
        let cells = segments(canvas, text, [inner, y, half, line], &["Round", "Square"], Some(usize::from(square)), small, mouse);
        editor.hits.extend(cells.into_iter().enumerate().map(|(at, place)| (EditHit::Lens(at == 1), place)));
        if editor.one().is_some() {
            number(editor, canvas, text, [inner + half + pad / 2, y, half, line], "×", Field::Times, small, mouse);
        }
        y += line + pad;
    }

    // The picture's own: its crop and its corners.
    if marks {
        canvas.fill(inner, y, inner_w, 1, MENU_EDGE_COLOR);
        y += pad;
    }
    text.draw(canvas, inner, y, px, TEXT_COLOR, if kind == Tool::Crop { "Crop" } else { "Picture" });
    y += line + pad / 2;
    if kind == Tool::Crop {
        // Kept to a shape, or free; its size; and done.
        let chosen = ASPECTS.iter().position(|(_, ratio)| match (ratio, editor.aspect) {
            (None, None) => true,
            (Some(0.0), Some(now)) => (now - editor.native.0 as f32 / editor.native.1 as f32).abs() < 1e-3,
            (Some(ratio), Some(now)) => (ratio - now).abs() < 1e-3,
            _ => false,
        });
        let cells = segments(canvas, text, [inner, y, inner_w, line], &ASPECTS.map(|(name, _)| name), chosen, small, mouse);
        editor.hits.extend(cells.into_iter().zip(ASPECTS).map(|(place, (_, ratio))| (EditHit::Aspect(ratio), place)));
        y += line + pad / 3;
        pair(editor, canvas, text, y, ("W", Field::CropW), ("H", Field::CropH));
        y += line + pad / 2;
        let apply = "Crop  ⏎";
        let w = half;
        canvas.round_fill(inner, y, w, line, line / 4, accent(), 256);
        let aw = text.width(apply, small);
        text.draw(canvas, inner + (w - aw) / 2, y + (line - small_line) / 2, small, on_accent(), apply);
        editor.hits.push((EditHit::ApplyCrop, [inner, y, w, line]));
        let reset = "All of it";
        let rx = inner + half + pad / 2;
        canvas.round_fill(rx, y, half, line, line / 4, if inside([rx, y, half, line], mouse) { PLACEHOLDER_COLOR } else { 0x161616 }, 256);
        let rw = text.width(reset, small);
        text.draw(canvas, rx + (half - rw) / 2, y + (line - small_line) / 2, small, TEXT_COLOR, reset);
        editor.hits.push((EditHit::ResetCrop, [rx, y, half, line]));
        y += line + pad / 2;
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Drag inside to move  ·  Esc cancels");
        y += small_line + pad;
    } else {
        heading(canvas, text, y, "Crop");
        let (ow, oh) = editor.out_size();
        let crop_words = if editor.doc.crop.is_some() { format!("{ow} × {oh}") } else { "none  ·  X".to_string() };
        let cw = text.width(&crop_words, small);
        text.draw(canvas, inner + inner_w - cw, y, small, TEXT_COLOR, &crop_words);
        y += small_line + pad / 3;
        if editor.doc.crop.is_some() {
            let label = "Reset crop";
            let w = text.width(label, small) + pad;
            canvas.round_fill(inner, y, w, small_line + pad / 3, pad / 3, PLACEHOLDER_COLOR, 256);
            text.draw(canvas, inner + pad / 2, y + pad / 6, small, TEXT_COLOR, label);
            editor.hits.push((EditHit::ResetCrop, [inner, y, w, small_line + pad / 3]));
            y += small_line + pad;
        }
    }
    heading(canvas, text, y, "Corners");
    y += small_line + pad / 3;
    number(editor, canvas, text, [inner, y, half, line], "R", Field::Corners, small, mouse);
    y += line + pad / 2;
    if editor.doc.radius > 0.0 && format == Some(Format::Jpeg) {
        text.draw(canvas, inner, y, small, MARK_COLOR, "JPEG · no clear corners");
        y += small_line;
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Overwrite → white  ·  Copy → PNG");
        y += small_line;
    }
    let keys = [("Drag a number", "or click to type"), ("Shift+click", "pick out more"), ("Ctrl+D", "duplicate  ·  Del remove"), ("c d f g", "color, dash, fill, font")];
    let key_w = keys.iter().map(|(key, _)| text.width(key, small)).max().unwrap_or(0) + pad / 2;
    let mut y = y.max(top + high - pad - keys.len() as i32 * small_line);
    for (key, what) in keys {
        text.draw(canvas, inner, y, small, TEXT_COLOR, key);
        text.draw(canvas, inner + key_w, y, small, DIM_TEXT_COLOR, what);
        y += small_line;
    }
    color_row
}

/// A swatch of a color, ringed when it is the one in use.
fn swatch_cell(canvas: &mut Canvas, [x, y, w, _]: [i32; 4], color: u32, on: bool) {
    if on {
        canvas.round_fill(x - 2, y - 2, w + 4, w + 4, w / 2 + 2, TEXT_COLOR, 256);
        canvas.round_fill(x, y, w, w, w / 2, MENU_COLOR, 256);
    }
    let inset = if on { 2 } else { 0 };
    canvas.round_fill(x + inset, y + inset, w - inset * 2, w - inset * 2, w / 2, color, 256);
    // The darkest would be lost on the panel.
    if color < 0x404040 {
        canvas.round_outline(x + inset, y + inset, w - inset * 2, w - inset * 2, w / 2, 1, MENU_EDGE_COLOR);
    }
}

/// A number of the panel: its letter, and its value, dragged sideways to
/// change it or clicked to type it.
#[allow(clippy::too_many_arguments)]
fn number(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, place: [i32; 4], label: &str, field: Field, small: u32, mouse: (i32, i32)) {
    let [x, y, w, h] = place;
    let value = editor.field(field);
    let typed = editor.editing.as_ref().filter(|editing| editing.0 == field).map(|editing| (editing.1.clone(), editing.2));
    let over = inside(place, mouse);
    canvas.round_fill(x, y, w, h, h / 4, if over || typed.is_some() { 0x202020 } else { 0x161616 }, 256);
    let small_line = Text::line_height(small);
    let ty = y + (h - small_line) / 2;
    let lx = x + small as i32 / 2;
    text.draw(canvas, lx, ty, small, DIM_TEXT_COLOR, label);
    let vx = lx + text.width(label, small).max(small as i32) + small as i32 / 2;
    match typed {
        Some((typed, fresh)) => {
            canvas.round_outline(x, y, w, h, h / 4, 1, accent());
            let tw = text.width(&typed, small);
            // A number about to be typed over is picked out.
            if fresh {
                canvas.round_fill(vx - 2, ty, tw + 4, small_line, 2, accent(), 120);
            }
            text.draw(canvas, vx, ty, small, TEXT_COLOR, &typed);
            canvas.fill(vx + tw + 1, ty, 2, small_line, accent());
        }
        None => {
            let words = value.map(shown).unwrap_or_else(|| "–".to_string());
            text.draw(canvas, vx, ty, small, TEXT_COLOR, &words);
        }
    }
    editor.hits.push((EditHit::Field(field), place));
}

/// The colors beyond the eight, to the left of where `at` says: a square
/// of saturation and value, a strip of hues, a palette, those taken
/// lately, and the pipette.
fn draw_flyout(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, (hue, saturation, value): (f32, f32, f32), [right, top]: [i32; 2], px: u32, mouse: (i32, i32)) {
    let pad = px as i32;
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    let w = pad * 15;
    let inner_w = w - pad * 2;
    let swatch = (inner_w - pad / 3 * 7) / 8;
    let recent: Vec<u32> = RECENT.lock().map(|recent| recent.clone()).unwrap_or_default();
    let square_h = inner_w * 2 / 3;
    let rows = 3 + i32::from(!recent.is_empty());
    let h = pad + square_h + pad / 2 + pad + pad / 2 + (swatch + pad / 3) * rows + small_line + pad;
    let (x, y) = ((right - w).min(canvas.width - w - pad).max(pad), top.min(canvas.height - h - pad).max(pad));
    draw::panel(canvas, x, y, w, h, pad / 2);
    editor.hits.push((EditHit::Inside, [x, y, w, h]));
    let (inner, mut row) = (x + pad, y + pad);
    // Saturation across, value down, at the hue chosen.
    let pixels_w = canvas.width as usize;
    {
        let pixels = canvas.all();
        for sy in 0..square_h {
            let v = 1.0 - sy as f32 / square_h.max(1) as f32;
            for sx in 0..inner_w {
                let s = sx as f32 / inner_w.max(1) as f32;
                let (px_, py_) = (inner + sx, row + sy);
                if px_ >= 0 && py_ >= 0 && (px_ as usize) < pixels_w {
                    if let Some(pixel) = pixels.get_mut(py_ as usize * pixels_w + px_ as usize) {
                        *pixel = annotate::from_hsv((hue, s, v));
                    }
                }
            }
        }
    }
    let (mx, my) = (inner + (saturation * inner_w as f32) as i32, row + ((1.0 - value) * square_h as f32) as i32);
    canvas.round_outline(mx - 6, my - 6, 12, 12, 6, 2, 0xFFFFFF);
    editor.hits.push((EditHit::Square, [inner, row, inner_w, square_h]));
    row += square_h + pad / 2;
    // The hues, end to end.
    let strip_h = pad * 2 / 3;
    for sx in 0..inner_w {
        canvas.fill(inner + sx, row, 1, strip_h, annotate::from_hsv((sx as f32 / inner_w as f32 * 360.0, 1.0, 1.0)));
    }
    let hx = inner + (hue / 360.0 * inner_w as f32) as i32;
    canvas.round_outline(hx - 3, row - 2, 6, strip_h + 4, 2, 2, 0xFFFFFF);
    editor.hits.push((EditHit::HueStrip, [inner, row - 2, inner_w, strip_h + 4]));
    row += strip_h + pad / 2 + pad / 2;
    let now = annotate::from_hsv((hue, saturation, value));
    for (n, color) in PALETTE.iter().enumerate() {
        let (column, line) = ((n % 8) as i32, (n / 8) as i32);
        let cell = [inner + column * (swatch + pad / 3), row + line * (swatch + pad / 3), swatch, swatch];
        swatch_cell(canvas, cell, *color, *color == now);
        editor.hits.push((EditHit::Color(*color), cell));
    }
    row += 3 * (swatch + pad / 3);
    if !recent.is_empty() {
        for (n, color) in recent.iter().enumerate() {
            let cell = [inner + n as i32 * (swatch + pad / 3), row, swatch, swatch];
            swatch_cell(canvas, cell, *color, *color == now);
            editor.hits.push((EditHit::Color(*color), cell));
        }
        row += swatch + pad / 3;
    }
    // The pipette, and what is under the pointer.
    let label = format!("{}  Pick from the picture   I", commands::icons::PIPETTE);
    let place = [inner - pad / 3, row, inner_w + pad * 2 / 3, small_line + pad / 3];
    if inside(place, mouse) || editor.tool == Tool::Pipette {
        canvas.round_fill(place[0], place[1], place[2], place[3], pad / 3, PLACEHOLDER_COLOR, 256);
    }
    text.draw(canvas, inner, row + pad / 6, small, TEXT_COLOR, &label);
    editor.hits.push((EditHit::Pipette, place));
}

/// The menu of a tool's ways, its bottom at `bottom`: a line each, the
/// ones in use ticked, as hotshot's.
fn draw_tool_menu(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, tool: Tool, [left, bottom]: [i32; 2], px: u32, mouse: (i32, i32)) {
    let pad = px as i32;
    let line = Text::line_height(px);
    let row = line + pad / 2;
    let options = tool_options(tool);
    let ticked = |editor: &Editor, hit: EditHit| match hit {
        EditHit::Heads(heads) => editor.style.heads == heads,
        EditHit::Dash(dash) => editor.style.dash == dash,
        EditHit::Filled(filled) => editor.style.filled == filled,
        EditHit::Lens(square) => editor.style.square == square,
        EditHit::Face(face) => editor.style.face == face,
        EditHit::BadgeLook(look) => editor.style.badge == look,
        EditHit::Aspect(ratio) => match (ratio, editor.aspect) {
            (None, None) => true,
            (Some(0.0), Some(now)) => (now - editor.native.0 as f32 / editor.native.1 as f32).abs() < 1e-3,
            (Some(ratio), Some(now)) => (ratio - now).abs() < 1e-3,
            _ => false,
        },
        _ => false,
    };
    let tick = text.width(commands::icons::CHECK, px) + pad / 2;
    let widest = options.iter().flatten().map(|(label, _)| text.width(label, px)).max().unwrap_or(0);
    let w = tick + widest + pad * 2;
    let gaps = options.iter().filter(|option| option.is_none()).count() as i32;
    let h = row * (options.len() as i32 - gaps) + gaps * pad / 2 + pad / 2;
    let (x, y) = (left.min(canvas.width - w - pad).max(pad), (bottom - h).max(pad));
    draw::panel(canvas, x, y, w, h, pad / 2);
    editor.hits.push((EditHit::Inside, [x, y, w, h]));
    let mut at = y + pad / 4;
    for option in options {
        let Some((label, hit)) = option else {
            canvas.fill(x + pad / 2, at + pad / 4 - 1, w - pad, 1, MENU_EDGE_COLOR);
            at += pad / 2;
            continue;
        };
        let place = [x + pad / 4, at, w - pad / 2, row];
        if inside(place, mouse) {
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 3, PLACEHOLDER_COLOR, 256);
        }
        if ticked(editor, hit) {
            text.draw(canvas, x + pad * 3 / 4, at + (row - line) / 2, px, accent_soft(), commands::icons::CHECK);
        }
        text.draw(canvas, x + pad * 3 / 4 + tick, at + (row - line) / 2, px, TEXT_COLOR, label);
        editor.hits.push((hit, place));
        at += row;
    }
}

/// Solid, dashed and dotted, each drawn as it would look; where the next
/// row goes.
#[allow(clippy::too_many_arguments)]
fn dashes(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, place: [i32; 4], dash: Dash, small: u32, mouse: (i32, i32)) -> i32 {
    let pad = small as i32;
    let choices = [Dash::Solid, Dash::Dashed, Dash::Dotted];
    let cells = segments(canvas, text, place, &["", "", ""], choices.iter().position(|d| *d == dash), small, mouse);
    for (cell, choice) in cells.iter().zip(choices) {
        preview(canvas, *cell, [false, false], choice, pad);
        editor.hits.push((EditHit::Dash(choice), *cell));
    }
    place[1] + place[3] + pad / 2
}

/// A row of choices side by side, the one chosen filled in; where each is.
#[allow(clippy::too_many_arguments)]
fn segments(canvas: &mut Canvas, text: &mut Text, [x, y, w, h]: [i32; 4], labels: &[&str], chosen: Option<usize>, small: u32, mouse: (i32, i32)) -> Vec<[i32; 4]> {
    let gap = 3;
    let n = labels.len() as i32;
    let cell = (w - gap * (n - 1)) / n;
    canvas.round_fill(x, y, w, h, h / 4, 0x161616, 256);
    labels
        .iter()
        .enumerate()
        .map(|(at, label)| {
            let place = [x + at as i32 * (cell + gap), y, cell, h];
            let on = chosen == Some(at);
            if on || inside(place, mouse) {
                canvas.round_fill(place[0], place[1], place[2], place[3], h / 4, if on { PLACEHOLDER_COLOR } else { 0x202020 }, 256);
            }
            if !label.is_empty() {
                let lw = text.width(label, small);
                text.draw(canvas, place[0] + (cell - lw) / 2, y + (h - Text::line_height(small)) / 2, small, if on { TEXT_COLOR } else { DIM_TEXT_COLOR }, label);
            }
            place
        })
        .collect()
}

/// A short line in a cell, with heads and dashes as a choice would draw it.
fn preview(canvas: &mut Canvas, [x, y, w, h]: [i32; 4], heads: [bool; 2], dash: Dash, pad: i32) {
    let mut shape = Shape::new(Tool::Line, TEXT_COLOR, 2.0, ((x + pad / 2) as f32, (y + h / 2) as f32));
    shape.points[1] = ((x + w - pad / 2) as f32, (y + h / 2) as f32);
    shape.heads = heads;
    shape.set_dash(dash);
    let width = canvas.width as usize;
    annotate::paint(&[shape], canvas.all(), width, Place::WHOLE, [x, y, w, h], None);
}

/// The emoji picker: what is typed to find one, the emoji found, and the
/// name of the one under the pointer.
fn draw_picker(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, left: i32, bottom: i32, px: u32, mouse: (i32, i32)) {
    let pad = px as i32;
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    let cell = Text::line_height(px) + pad;
    let found = editor.emoji_found();
    let rows = found.len().div_ceil(EMOJI_ACROSS);
    let Some(picker) = &mut editor.picker else { return };
    picker.scroll = picker.scroll.min(rows.saturating_sub(EMOJI_DOWN));
    let (w, h) = (cell * EMOJI_ACROSS as i32 + pad, pad / 2 + small_line + pad + cell * EMOJI_DOWN as i32 + small_line + pad);
    let (x, y) = (left.min(canvas.width - w - pad), bottom - h);
    draw::panel(canvas, x, y, w, h, pad / 2);
    editor.hits.push((EditHit::Inside, [x, y, w, h]));
    // What is typed, in a field.
    let field = [x + pad / 2, y + pad / 2, w - pad, small_line + pad / 2];
    canvas.round_fill(field[0], field[1], field[2], field[3], pad / 3, 0x161616, 256);
    let (words, ink) = if picker.query.is_empty() { ("Type to search", DIM_TEXT_COLOR) } else { (picker.query.as_str(), TEXT_COLOR) };
    text.draw(canvas, field[0] + pad / 2, field[1] + pad / 4, small, ink, words);
    let caret = field[0] + pad / 2 + if picker.query.is_empty() { 0 } else { text.width(&picker.query, small) + 1 };
    canvas.fill(caret, field[1] + pad / 4, 2, small_line, accent());
    let grid_y = field[1] + field[3] + pad / 2;
    let list = annotate::emoji_list();
    let side = (cell - pad / 2).max(8) as u32;
    let mut caption = if found.is_empty() { "None by that name".to_string() } else { format!("{} emoji  ·  Enter places the first", found.len()) };
    let width = canvas.width as usize;
    let scroll = picker.scroll;
    for (n, which) in found.iter().skip(scroll * EMOJI_ACROSS).take(EMOJI_ACROSS * EMOJI_DOWN).enumerate() {
        let (column, row) = ((n % EMOJI_ACROSS) as i32, (n / EMOJI_ACROSS) as i32);
        let place = [x + pad / 2 + column * cell, grid_y + row * cell, cell, cell];
        if inside(place, mouse) {
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 3, PLACEHOLDER_COLOR, 256);
            caption = list[*which].1.to_string();
        }
        if let Some(picture) = annotate::emoji(list[*which].0, side) {
            let at = (place[0] + (cell - picture.0) / 2, place[1] + (cell - picture.1) / 2);
            annotate::stamp(canvas.all(), width, [place[0], place[1], place[0] + cell, place[1] + cell], &picture, at, 256, None);
        }
        editor.hits.push((EditHit::Emoji(*which), place));
    }
    text.draw(canvas, x + pad / 2, grid_y + cell * EMOJI_DOWN as i32 + pad / 4, small, DIM_TEXT_COLOR, &caption);
}

fn tool_icon(tool: Tool) -> &'static str {
    use commands::icons::*;
    match tool {
        Tool::Select => SELECT,
        Tool::Rect => RECT,
        Tool::Oval => OVAL,
        Tool::Arrow => ARROW_UP_RIGHT,
        Tool::Line => LINE,
        Tool::Pen => PENCIL,
        Tool::Marker => MARKER,
        Tool::Text => TYPE,
        Tool::Counter => COUNTER,
        Tool::Badge => BADGE,
        Tool::Emoji => STICKER,
        Tool::Blur => BLUR,
        Tool::Spotlight => SPOTLIGHT,
        Tool::Zoom => ZOOM_IN,
        Tool::Crop => CROP,
        Tool::Pipette => PIPETTE,
        Tool::Image => IMAGES,
    }
}

/// Squares of two grays, where the picture will be clear.
fn checker(canvas: &mut Canvas, [x, y, w, h]: [i32; 4]) {
    let cell = 8;
    canvas.fill(x, y, w, h, 0x2A2E38);
    for row in (0..h).step_by(cell) {
        let first = if (row / cell as i32) % 2 == 0 { 0 } else { cell as i32 };
        for column in (first..w).step_by(cell * 2) {
            canvas.fill(x + column, y + row, (cell as i32).min(w - column), (cell as i32).min(h - row), 0x353A46);
        }
    }
}

/// The corners of the picture at `at` rounded off, `behind` showing
/// where they were.
fn round_corners(canvas: &mut Canvas, at: [i32; 4], radius: f32, behind: &[u32]) {
    let [x, y, w, h] = at;
    let r = radius.min(w as f32 / 2.0).min(h as f32 / 2.0);
    let reach = r.ceil() as i32 + 1;
    let (width, height) = (canvas.width, canvas.height);
    let pixels = canvas.all();
    for (cx, cy) in [(0, 0), (w - reach, 0), (0, h - reach), (w - reach, h - reach)] {
        for row in cy.max(0)..(cy + reach).min(h) {
            for column in cx.max(0)..(cx + reach).min(w) {
                let (sx, sy) = (column as f32 + 0.5 - w as f32 / 2.0, row as f32 + 0.5 - h as f32 / 2.0);
                let (qx, qy) = (sx.abs() - w as f32 / 2.0 + r, sy.abs() - h as f32 / 2.0 + r);
                let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
                let keep = (0.5 - outside).clamp(0.0, 1.0);
                if keep < 1.0 {
                    let (px, py) = (x + column, y + row);
                    if px < 0 || py < 0 || px >= width || py >= height {
                        continue;
                    }
                    let at = py as usize * width as usize + px as usize;
                    let under = behind[(row * w + column) as usize];
                    pixels[at] = ui::mix(under, pixels[at], (keep * 256.0) as u32);
                }
            }
        }
    }
}

/// The name a copy is offered under: `-edit` after the name, then
/// `-edit-2` and on, and PNG for a JPEG whose corners are clear now.
fn copy_name(path: &Path, rounded: bool) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = path.extension().unwrap_or_default().to_string_lossy().into_owned();
    let ext = match Format::of(path) {
        Some(Format::Jpeg) if rounded => "png".to_string(),
        Some(_) => ext,
        None => "png".to_string(),
    };
    (1..)
        .map(|n| if n == 1 { format!("{stem}-edit.{ext}") } else { format!("{stem}-edit-{n}.{ext}") })
        .find(|name| !path.with_file_name(name).exists())
        .unwrap_or_default()
}

/// The picture decoded afresh, drawn on, and written: over the file, the
/// original kept aside for Undo, or as a new one. How large it came to,
/// and where the original went.
fn write_edited(source: &Path, target: &Path, over: bool, transform: Transform, doc: &Doc, settings: &convert::Settings) -> Result<(u64, Option<PathBuf>), String> {
    let format = Format::of(target).ok_or("not a format jog writes")?;
    let out = render_edited(source, transform, doc)?;
    let encoded = convert::encode(&DynamicImage::ImageRgba8(out), None, format, settings).map_err(|error| error.to_string())?;
    let size = encoded.bytes.len() as u64;
    if !over {
        // Never over a file that is there.
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(target).map_err(|error| error.to_string())?;
        use std::io::Write;
        if let Err(error) = file.write_all(&encoded.bytes) {
            let _ = std::fs::remove_file(target);
            return Err(error.to_string());
        }
        return Ok((size, None));
    }
    let backup = keep_aside(target).map_err(|error| format!("original not kept · {error}"))?;
    let temp = beside(target, "jog-edit");
    std::fs::write(&temp, &encoded.bytes).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::rename(&temp, target) {
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_file(&backup);
        return Err(error.to_string());
    }
    Ok((size, Some(backup)))
}

/// The picture as edited, at full size: the file decoded afresh, turned as
/// it is shown, with the marks drawn on it, cropped and its corners rounded.
fn render_edited(source: &Path, transform: Transform, doc: &Doc) -> Result<image::RgbaImage, String> {
    let image = convert::turned(crate::decode::open(source).map_err(|error| error.to_string())?, transform);
    Ok(annotate::render(&image.to_rgba8(), &doc.shapes, doc.crop, doc.radius))
}

/// The picture as edited, on the clipboard as a PNG; how large it is.
fn copy_edited(source: &Path, transform: Transform, doc: &Doc) -> Result<(u32, u32), String> {
    let out = render_edited(source, transform, doc)?;
    let size = out.dimensions();
    let encoded = convert::encode(&DynamicImage::ImageRgba8(out), None, Format::Png, &convert::Settings::default()).map_err(|error| error.to_string())?;
    if !crate::ops::set_clipboard_image(&encoded.bytes) {
        return Err("no xclip or wl-copy".to_string());
    }
    Ok(size)
}

/// A hidden name in the same folder, to write to and then move over.
fn beside(path: &Path, what: &str) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".{name}.{what}"))
}

/// A copy of the file in jog's state folder, for Undo to put back; those
/// older than a week go as each is made.
fn keep_aside(path: &Path) -> std::io::Result<PathBuf> {
    let dir = crate::positions::state_dir().ok_or_else(|| std::io::Error::other("no state folder"))?.join("edited");
    std::fs::create_dir_all(&dir)?;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let old = entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|at| at.elapsed().is_ok_and(|age| age > BACKUPS_KEPT));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |at| at.as_millis());
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let backup = dir.join(format!("{stamp}-{name}"));
    std::fs::copy(path, &backup)?;
    Ok(backup)
}

/// The original laid back where the edit was saved, by way of a copy
/// beside it, so that what is there is only ever replaced whole.
fn put_back(backup: &Path, path: &Path) -> std::io::Result<()> {
    let temp = beside(path, "jog-undo");
    std::fs::copy(backup, &temp)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    let _ = std::fs::remove_file(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_offered_a_name_that_is_free() {
        let dir = std::env::temp_dir().join(format!("jog-edit-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("lake.jpg");
        std::fs::write(&photo, b"").unwrap();
        assert_eq!(copy_name(&photo, false), "lake-edit.jpg");
        assert_eq!(copy_name(&photo, true), "lake-edit.png");
        std::fs::write(dir.join("lake-edit.jpg"), b"").unwrap();
        assert_eq!(copy_name(&photo, false), "lake-edit-2.jpg");
        assert_eq!(copy_name(&dir.join("anim.gif"), false), "anim-edit.png");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_over_the_original_is_kept_and_put_back() {
        let dir = std::env::temp_dir().join(format!("jog-edit-over-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("JOG_STATE", dir.join("state"));
        let photo = dir.join("p.png");
        image::RgbImage::from_pixel(40, 30, image::Rgb([200, 10, 10])).save(&photo).unwrap();
        let before = std::fs::read(&photo).unwrap();
        let doc = Doc { shapes: Vec::new(), crop: Some([0.0, 0.0, 20.0, 30.0]), radius: 0.0 };
        let (_, backup) = write_edited(&photo, &photo, true, Transform::default(), &doc, &convert::Settings::default()).unwrap();
        assert_eq!(image::open(&photo).unwrap().width(), 20);
        put_back(&backup.unwrap(), &photo).unwrap();
        assert_eq!(std::fs::read(&photo).unwrap(), before);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn shape_at(l: f32, t: f32, r: f32, b: f32) -> Shape {
        let mut shape = Shape::new(Tool::Rect, 0, 2.0, (l, t));
        shape.points[1] = (r, b);
        shape
    }

    #[test]
    fn a_mark_moved_near_another_rests_against_its_edge() {
        let mut editor = Editor::new(0, (1000, 1000));
        editor.doc.shapes = vec![shape_at(100.0, 100.0, 200.0, 200.0), shape_at(400.0, 130.0, 500.0, 230.0)];
        editor.selected = vec![0];
        let before = editor.doc.clone();
        // Its right edge three pixels short of the other's left edge.
        editor.move_selected(&before, (194.0, 0.0), false);
        let right = editor.doc.shapes[0].bounds()[2];
        let left = editor.doc.shapes[1].bounds()[0];
        assert!((right - left).abs() < 0.01);
        assert!(!editor.guides.0.is_empty());
        // With Ctrl, it goes where it is put.
        editor.move_selected(&before, (194.0, 0.0), true);
        assert!((editor.doc.shapes[0].bounds()[2] - left).abs() > 1.0);
    }

    #[test]
    fn a_box_dragged_out_picks_out_what_it_touches() {
        let mut editor = Editor::new(0, (1000, 1000));
        editor.doc.shapes = vec![shape_at(100.0, 100.0, 200.0, 200.0), shape_at(400.0, 400.0, 500.0, 500.0), shape_at(800.0, 800.0, 900.0, 900.0)];
        editor.pick_within((450.0, 450.0), (50.0, 50.0), false);
        assert_eq!(editor.selected, vec![0, 1]);
        editor.pick_within((850.0, 850.0), (860.0, 860.0), true);
        assert_eq!(editor.selected, vec![0, 1, 2]);
        editor.pick_within((850.0, 850.0), (860.0, 860.0), false);
        assert_eq!(editor.selected, vec![2]);
    }

    #[test]
    fn numbers_set_where_a_mark_is_how_large_and_how_it_is_drawn() {
        let mut editor = Editor::new(0, (1000, 1000));
        editor.doc.shapes = vec![shape_at(100.0, 100.0, 200.0, 150.0)];
        editor.selected = vec![0];
        editor.set_field(Field::X, 300.0);
        editor.set_field(Field::W, 50.0);
        assert_eq!(editor.doc.shapes[0].frame(), [300.0, 100.0, 350.0, 150.0]);
        editor.set_field(Field::Stroke, 7.5);
        editor.set_field(Field::Opacity, 50.0);
        assert_eq!((editor.doc.shapes[0].stroke, editor.doc.shapes[0].opacity), (7.5, 128));
        // And what is drawn next takes them.
        assert_eq!(editor.fresh(Tool::Rect, (0.0, 0.0)).stroke, 7.5);
    }

    #[test]
    fn a_crop_kept_to_a_shape_stays_that_shape_and_inside() {
        let mut editor = Editor::new(0, (1600, 900));
        editor.place.scale = 1.0;
        editor.crop_draft = Some(editor.whole());
        editor.shape_crop(Some(1.0));
        let [l, t, r, b] = editor.crop_draft.unwrap();
        assert_eq!((r - l, b - t), (900.0, 900.0));
        // Its corner taken far outside: as large as fits, still square.
        editor.pull_crop((1, 1), [l, t, r, b], (5000.0, 5000.0), false, false);
        let [l, t, r, b] = editor.crop_draft.unwrap();
        assert!((r - l - (b - t)).abs() < 0.5 && r <= 1600.0 && b <= 900.0 && l >= 0.0 && t >= 0.0);
    }

    #[test]
    fn the_edges_of_a_card_and_a_divider_are_found() {
        // A light page, a dark card from (100, 80) to (300, 200), and a
        // rule across at 250; and a little noise that is no edge.
        let (w, h) = (400usize, 300usize);
        let mut pixels = vec![0xF0F0F0u32; w * h];
        for y in 80..200 {
            pixels[y * w + 100..y * w + 300].fill(0x202020);
        }
        pixels[250 * w..251 * w].fill(0x404040);
        pixels[10 * w + 10] = 0x000000;
        let (down, across) = find_edges(&pixels, w, h);
        for x in [100.0, 300.0] {
            assert!(down.contains(&x), "{x} in {down:?}");
        }
        // The rule, a pixel high, is one line, not two.
        for y in [80.0, 200.0, 250.0] {
            assert!(across.contains(&y), "{y} in {across:?}");
        }
        assert!(!across.contains(&251.0));
        assert!(!down.contains(&10.0) && !across.contains(&10.0));
    }

    #[test]
    fn words_are_typed_where_the_caret_is_and_over_what_is_picked_out() {
        let mut editor = Editor::new(0, (1000, 1000));
        let mut words = Shape::new(Tool::Text, 0, 20.0, (0.0, 0.0));
        words.text = "one\nthree".to_string();
        editor.doc.shapes = vec![words];
        editor.start_typing(0, Some(3));
        editor.insert(" two");
        assert_eq!(editor.doc.shapes[0].text, "one two\nthree");
        // Down from the end of the first line: as far along the second.
        let below = editor.caret_for(NamedKey::ArrowDown, false);
        assert_eq!(below, 13);
        editor.put_caret(below, false);
        editor.erase(false);
        assert_eq!(editor.doc.shapes[0].text, "one two\nthre");
        // Picked out from the start of the line to here, typed over.
        editor.put_caret(editor.caret_for(NamedKey::Home, true), true);
        assert_eq!(editor.selected_text().as_deref(), Some("thre"));
        editor.insert("3");
        assert_eq!(editor.doc.shapes[0].text, "one two\n3");
        // All of it picked out, as a badge starts.
        editor.start_typing(0, None);
        editor.erase(true);
        assert_eq!(editor.doc.shapes[0].text, "");
    }

    #[test]
    fn shift_and_an_arrow_grow_what_is_picked_out_a_letter_at_a_time() {
        let mut editor = Editor::new(0, (1000, 1000));
        let mut words = Shape::new(Tool::Text, 0, 20.0, (0.0, 0.0));
        words.text = "abcdef".to_string();
        editor.doc.shapes = vec![words];
        editor.start_typing(0, Some(4));
        for _ in 0..2 {
            let to = editor.caret_for(NamedKey::ArrowLeft, true);
            editor.put_caret(to, true);
        }
        assert_eq!(editor.selected_text().as_deref(), Some("cd"));
        // Without Shift, the arrow goes to the start of what was picked out.
        let to = editor.caret_for(NamedKey::ArrowRight, false);
        editor.put_caret(to, false);
        assert_eq!((editor.caret, editor.selection()), (4, None));
    }

    #[test]
    fn a_click_puts_the_caret_by_the_nearest_letter() {
        let mut words = Shape::new(Tool::Text, 0, 40.0, (0.0, 0.0));
        words.text = "abc\ndefgh".to_string();
        let ((x, y), height) = words.caret_spot(6);
        assert_eq!(words.caret_at((x + 1.0, y + height / 2.0)), 6);
        assert_eq!(words.caret_at((-50.0, 5.0)), 0);
        assert_eq!(words.caret_at((5000.0, 5000.0)), 9);
    }

    #[test]
    fn colors_go_to_hue_saturation_and_value_and_back() {
        for color in COLORS.iter().chain(PALETTE.iter()) {
            assert_eq!(annotate::from_hsv(annotate::to_hsv(*color)), *color, "{color:06X}");
        }
    }
}
