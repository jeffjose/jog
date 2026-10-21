//! Edit: the picture on view drawn on, cropped and its corners rounded,
//! then saved over itself or as a copy beside it. The marks stay
//! editable until it is saved: picked out with the arrow, moved, sized
//! by their handles, restyled from the panel. Saving is done on another
//! thread, from the file decoded again at full size.

use image::DynamicImage;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use crate::annotate::{self, Dash, Place, Point, Shape, Tool, COLORS, OPACITIES};

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

/// How the next mark is drawn; the panel sets it along with the mark
/// picked out, as hotshot does.
#[derive(Clone, Copy, Debug)]
struct Style {
    color: usize,
    size: usize,
    opacity: usize,
    filled: bool,
    heads: [bool; 2],
    dash: Dash,
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
    style: Style,
    selected: Option<usize>,
    /// The mark under the pointer, with the arrow in hand.
    hovered: Option<usize>,
    /// The words being typed into, by the shape's place.
    typing: Option<usize>,
    /// The crop being dragged out, before Enter takes it.
    crop_draft: Option<[f32; 4]>,
    drag: Option<EditDrag>,
    picker: Option<Picker>,
    /// Escape with something unsaved: the bar that asks what to do.
    leaving: bool,
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
    Draw,
    Move { last: Point, before: Doc, moved: bool, was_selected: bool },
    Grip { grip: (i8, i8), frame: [f32; 4], from: Point, before: Doc },
    End { end: usize, before: Doc, moved: bool },
    Bend { before: Doc, moved: bool },
    Round { corner: (i8, i8), before: Doc },
    Crop { from: Point },
    Slider(Slider),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Slider {
    /// The picture's corners.
    Picture,
    /// The corners of the rectangle or spotlight picked out.
    Shape,
}

/// The handles of the mark picked out.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    /// A corner or the middle of a side, by which way it pulls.
    Grip(i8, i8),
    End(usize),
    Bend,
    Round(i8, i8),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum EditHit {
    Tool(Tool),
    Undo,
    Redo,
    Reset,
    Color(usize),
    Opacity(usize),
    Size(usize),
    Filled(bool),
    Heads([bool; 2]),
    Dash(Dash),
    Slider(Slider),
    ResetCrop,
    Save(SaveAs),
    Discard,
    KeepEditing,
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
            style: Style { color: 0, size: 1, opacity: 3, filled: false, heads: [false, true], dash: Dash::Solid },
            selected: None,
            hovered: None,
            typing: None,
            crop_draft: None,
            drag: None,
            picker: None,
            leaving: false,
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

    /// Kept for Undo before a change.
    fn snap(&mut self) {
        self.past.push(self.doc.clone());
        self.future.clear();
    }

    fn undo(&mut self) {
        self.finish_typing();
        if let Some(doc) = self.past.pop() {
            self.future.push(std::mem::replace(&mut self.doc, doc));
            self.selected = None;
        }
    }

    fn redo(&mut self) {
        self.finish_typing();
        if let Some(doc) = self.future.pop() {
            self.past.push(std::mem::replace(&mut self.doc, doc));
            self.selected = None;
        }
    }

    /// Everything back as it was opened, which Undo can still take back.
    fn reset(&mut self) {
        self.finish_typing();
        if self.dirty() {
            self.snap();
            self.doc = self.opened.clone();
            self.selected = None;
            self.crop_draft = None;
        }
    }

    fn stroke(&self, tool: Tool, size: usize) -> f32 {
        match tool {
            Tool::Text => annotate::text_sizes(self.native)[size],
            Tool::Emoji => annotate::text_sizes(self.native)[size] * 2.0,
            _ => annotate::widths(self.native)[size],
        }
    }

    /// The mark the panel speaks of: the one picked out or being typed.
    fn subject(&self) -> Option<usize> {
        self.typing.or(self.selected).filter(|at| *at < self.doc.shapes.len())
    }

    /// A new mark of `tool` at `at`, drawn as the panel says.
    fn fresh(&self, tool: Tool, at: Point) -> Shape {
        let mut shape = Shape::new(tool, COLORS[self.style.color], self.stroke(tool, self.style.size), at);
        shape.opacity = OPACITIES[self.style.opacity];
        if matches!(tool, Tool::Rect | Tool::Oval) {
            shape.filled = self.style.filled;
        }
        if tool == Tool::Arrow {
            shape.heads = self.style.heads;
        }
        if tool.is_line() {
            shape.dash = self.style.dash;
        }
        shape
    }

    /// Changes the mark picked out, kept for Undo if it changed.
    fn restyle(&mut self, change: impl Fn(&mut Shape)) {
        let Some(at) = self.subject() else { return };
        let mut shape = self.doc.shapes[at].clone();
        change(&mut shape);
        if shape != self.doc.shapes[at] {
            self.snap();
            self.doc.shapes[at] = shape;
        }
    }

    /// The part of the picture on show: all of it while cropping, so the
    /// crop can be dragged anywhere.
    fn region(&self) -> [f32; 4] {
        let whole = [0.0, 0.0, self.native.0 as f32, self.native.1 as f32];
        match (self.tool, self.doc.crop) {
            (Tool::Crop, _) | (_, None) => whole,
            (_, Some(crop)) => crop,
        }
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
        w.min(h) as f32 / 4.0
    }

    fn set_tool(&mut self, tool: Tool) {
        self.finish_typing();
        if self.tool == Tool::Crop && tool != Tool::Crop {
            self.crop_draft = None;
        }
        if tool == Tool::Crop {
            self.crop_draft = self.doc.crop;
        }
        if tool != Tool::Select {
            self.selected = None;
        }
        self.picker = (tool == Tool::Emoji).then(Picker::default);
        self.tool = tool;
    }

    /// A mark just made: picked out, with the arrow in hand to adjust it.
    fn made(&mut self, at: usize) {
        self.selected = Some(at);
        self.tool = Tool::Select;
        self.picker = None;
    }

    /// Words typed and done with: left, and picked out, if there are any;
    /// gone if not.
    fn finish_typing(&mut self) {
        let Some(at) = self.typing.take() else { return };
        if self.doc.shapes.get(at).is_some_and(|shape| shape.text.trim().is_empty()) {
            self.doc.shapes.remove(at);
            // A text that came to nothing leaves nothing to undo.
            if self.past.last() == Some(&self.doc) {
                self.past.pop();
            }
            self.selected = None;
        } else if at < self.doc.shapes.len() {
            self.made(at);
        }
    }

    fn apply_crop(&mut self) {
        if let Some(draft) = self.crop_draft.take() {
            if Some(draft) != self.doc.crop {
                self.snap();
                self.doc.crop = Some(draft);
                self.doc.radius = self.doc.radius.min(self.max_radius());
            }
        }
        self.tool = Tool::Select;
    }

    fn hit_at(&self, x: i32, y: i32) -> Option<EditHit> {
        let inside = |[l, t, w, h]: [i32; 4]| (l..l + w).contains(&x) && (t..t + h).contains(&y);
        self.hits.iter().rev().find(|(_, place)| inside(*place)).map(|(hit, _)| *hit)
    }

    fn handle_at(&self, x: i32, y: i32) -> Option<Handle> {
        self.handles.iter().find(|(_, (hx, hy))| (hx - x).abs() <= self.reach && (hy - y).abs() <= self.reach).map(|(handle, _)| *handle)
    }

    fn slider_value(&self, slider: Slider, x: f64) -> f32 {
        let Some([l, _, w, _]) = self.hits.iter().find(|(hit, _)| *hit == EditHit::Slider(slider)).map(|(_, place)| *place) else { return 0.0 };
        let along = ((x as f32 - l as f32) / w.max(1) as f32).clamp(0.0, 1.0);
        (along * self.slider_most(slider)).round()
    }

    fn slider_most(&self, slider: Slider) -> f32 {
        match slider {
            Slider::Picture => self.max_radius(),
            Slider::Shape => self.subject().map_or(0.0, |at| {
                let [l, t, r, b] = self.doc.shapes[at].frame();
                (r - l).min(b - t) / 2.0
            }),
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
        let side = self.stroke(Tool::Emoji, self.style.size);
        let mut shape = self.fresh(Tool::Emoji, ((l + r - side) / 2.0, (t + b - side) / 2.0));
        shape.text = emoji.to_string();
        self.snap();
        self.doc.shapes.push(shape);
        self.made(self.doc.shapes.len() - 1);
    }
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
        self.redraw();
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
        // Words being typed take every key but the ones that end them.
        if let Some(at) = editor.typing.filter(|_| !control) {
            let shape = &mut editor.doc.shapes[at];
            match key {
                Key::Named(NamedKey::Escape | NamedKey::Enter) => editor.finish_typing(),
                Key::Named(NamedKey::Backspace) => drop(shape.text.pop()),
                Key::Named(NamedKey::Space) => shape.text.push(' '),
                Key::Character(typed) => shape.text.push_str(typed),
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
            Key::Character("s" | "S") if control => {
                let way = if shift { SaveAs::Copy } else { SaveAs::Overwrite };
                return self.save_edit(way);
            }
            Key::Character("z" | "Z") if control && shift => editor.redo(),
            Key::Character("z") if control => editor.undo(),
            Key::Character("y") if control => editor.redo(),
            Key::Character("d") if control => {
                if let Some(at) = editor.selected {
                    let mut copy = editor.doc.shapes[at].clone();
                    let step = 12.0 / editor.place.scale;
                    copy.translate(step, step);
                    editor.snap();
                    editor.doc.shapes.push(copy);
                    editor.made(editor.doc.shapes.len() - 1);
                }
            }
            // Over or under the rest, one step or all the way.
            Key::Character(way @ ("]" | "[" | "}" | "{")) if control => {
                if let Some(at) = editor.selected {
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
                        editor.selected = Some(to);
                    }
                }
            }
            _ if control => return,
            // The bar asking about what is unsaved has keys of its own.
            Key::Character("d" | "D") if editor.leaving => return self.discard_edit(),
            Key::Named(NamedKey::Escape) if editor.leaving => editor.leaving = false,
            Key::Named(NamedKey::Escape) => {
                if editor.tool == Tool::Crop && editor.crop_draft != editor.doc.crop {
                    editor.crop_draft = editor.doc.crop;
                } else if editor.selected.is_some() {
                    editor.selected = None;
                } else if editor.tool != Tool::Select {
                    editor.set_tool(Tool::Select);
                } else if editor.dirty() {
                    editor.leaving = true;
                } else {
                    return self.close_editor();
                }
            }
            Key::Named(NamedKey::Enter) if editor.tool == Tool::Crop => editor.apply_crop(),
            // A text picked out: Enter goes back to typing it.
            Key::Named(NamedKey::Enter) => {
                if let Some(at) = editor.selected.filter(|at| editor.doc.shapes[*at].tool == Tool::Text) {
                    editor.snap();
                    editor.typing = Some(at);
                }
            }
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                if let Some(at) = editor.selected.take() {
                    editor.snap();
                    editor.doc.shapes.remove(at);
                }
            }
            Key::Named(arrow @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown)) => {
                if let Some(at) = editor.selected {
                    let step = if shift { 10.0 } else { 1.0 } / editor.place.scale;
                    let (dx, dy) = match arrow {
                        NamedKey::ArrowLeft => (-step, 0.0),
                        NamedKey::ArrowRight => (step, 0.0),
                        NamedKey::ArrowUp => (0.0, -step),
                        _ => (0.0, step),
                    };
                    editor.snap();
                    editor.doc.shapes[at].translate(dx, dy);
                }
            }
            Key::Character(digit @ ("1" | "2" | "3" | "4")) => {
                let size = digit.parse::<usize>().unwrap_or(1) - 1;
                return self.edit_option(EditHit::Size(size));
            }
            // Through the colors, as hotshot's C does.
            Key::Character("c") => {
                let color = (editor.style.color + 1) % COLORS.len();
                return self.edit_option(EditHit::Color(color));
            }
            Key::Character("f") => {
                let filled = !editor.style.filled;
                return self.edit_option(EditHit::Filled(filled));
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
    /// to the mark picked out.
    fn edit_option(&mut self, hit: EditHit) {
        let Some(editor) = &mut self.editor else { return };
        match hit {
            EditHit::Color(color) => {
                editor.style.color = color;
                editor.restyle(|shape| shape.color = COLORS[color]);
            }
            EditHit::Opacity(opacity) => {
                editor.style.opacity = opacity;
                editor.restyle(|shape| shape.opacity = OPACITIES[opacity]);
            }
            EditHit::Size(size) => {
                editor.style.size = size;
                if let Some(at) = editor.subject() {
                    let stroke = editor.stroke(editor.doc.shapes[at].tool, size);
                    editor.restyle(|shape| shape.stroke = stroke);
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
                editor.restyle(|shape| {
                    if shape.tool.is_line() {
                        shape.dash = dash;
                    }
                });
            }
            _ => {}
        }
        self.redraw();
    }

    pub(super) fn editor_wheel(&mut self, lines: f64) {
        let Some(picker) = self.editor.as_mut().and_then(|editor| editor.picker.as_mut()) else { return };
        let rows = (lines.abs().ceil() as usize).max(1) * 2;
        picker.scroll = if lines > 0.0 { picker.scroll.saturating_sub(rows) } else { picker.scroll + rows };
        self.redraw();
    }

    pub(super) fn editor_mouse(&mut self, pressed: bool) {
        let (x, y) = self.mouse;
        if !pressed {
            return self.editor_release();
        }
        let mut text = self.text.take().unwrap_or_else(Text::load);
        let hit = self.editor.as_ref().and_then(|editor| editor.hit_at(x as i32, y as i32));
        let handle = self.editor.as_ref().and_then(|editor| editor.handle_at(x as i32, y as i32));
        match (hit, handle) {
            (Some(hit @ (EditHit::Color(_) | EditHit::Opacity(_) | EditHit::Size(_) | EditHit::Filled(_) | EditHit::Heads(_) | EditHit::Dash(_))), _) => self.edit_option(hit),
            (Some(EditHit::Save(way)), _) => {
                self.text = Some(text);
                return self.save_edit(way);
            }
            (Some(EditHit::Discard), _) => {
                self.text = Some(text);
                return self.discard_edit();
            }
            (Some(hit), handle) if hit != EditHit::Inside || handle.is_none() => {
                if let Some(editor) = &mut self.editor {
                    match hit {
                        EditHit::Tool(tool) => editor.set_tool(tool),
                        EditHit::Undo => editor.undo(),
                        EditHit::Redo => editor.redo(),
                        EditHit::Reset => editor.reset(),
                        EditHit::Slider(slider) => {
                            editor.finish_typing();
                            editor.snap();
                            let value = editor.slider_value(slider, x);
                            match slider {
                                Slider::Picture => editor.doc.radius = value,
                                Slider::Shape => {
                                    if let Some(at) = editor.subject() {
                                        editor.doc.shapes[at].radius = value;
                                    }
                                }
                            }
                            editor.drag = Some(EditDrag::Slider(slider));
                        }
                        EditHit::ResetCrop => {
                            editor.snap();
                            editor.doc.crop = None;
                            editor.crop_draft = None;
                        }
                        EditHit::KeepEditing => editor.leaving = false,
                        EditHit::Emoji(which) => editor.place_emoji(which),
                        // Somewhere on the bars that is none of their buttons.
                        EditHit::Inside => {}
                        _ => {}
                    }
                }
            }
            (_, Some(handle)) => {
                if let Some(editor) = &mut self.editor {
                    grab(editor, handle, (x as f32, y as f32), &mut text);
                }
            }
            (None, None) => {
                if let Some(editor) = &mut self.editor {
                    editor_press(editor, (x as f32, y as f32), &mut text);
                }
            }
            _ => {}
        }
        self.text = Some(text);
        self.redraw();
    }

    pub(super) fn editor_motion(&mut self) {
        let (x, y) = self.mouse;
        let shift = self.modifiers.shift_key();
        let mut text = self.text.take().unwrap_or_else(Text::load);
        let Some(editor) = &mut self.editor else {
            self.text = Some(text);
            return;
        };
        let p = editor.place.to_picture((x as f32, y as f32));
        let (w, h) = (editor.native.0 as f32, editor.native.1 as f32);
        let scale = editor.place.scale;
        let selected = editor.selected;
        match editor.drag.clone() {
            Some(EditDrag::Draw) => {
                if let Some(shape) = editor.doc.shapes.last_mut() {
                    match shape.tool {
                        Tool::Pen | Tool::Marker => {
                            // A point every pixel and a half on screen.
                            if shape.points.last().is_none_or(|last| (last.0 - p.0).hypot(last.1 - p.1) * scale >= 1.5) {
                                shape.points.push(p);
                            }
                        }
                        Tool::Arrow | Tool::Line if shift => shape.points[1] = level(shape.points[0], p),
                        _ => shape.points[1] = p,
                    }
                }
            }
            Some(EditDrag::Move { last, before, was_selected, .. }) => {
                let (dx, dy) = (p.0 - last.0, p.1 - last.1);
                editor.drag = Some(EditDrag::Move { last: p, before, moved: true, was_selected });
                if let Some(at) = selected {
                    editor.doc.shapes[at].translate(dx, dy);
                }
            }
            Some(EditDrag::Grip { grip, frame, from, before }) => {
                let [l, t, r, b] = frame;
                let size = ((r - l).max(1.0), (b - t).max(1.0));
                let still = (if grip.0 < 0 { r } else { l }, if grip.1 < 0 { b } else { t });
                let pull = |g: i8, d: f32, size: f32| if g == 0 { 1.0 } else { (size + f32::from(g) * d) / size };
                let mut times = (pull(grip.0, p.0 - from.0, size.0), pull(grip.1, p.1 - from.1, size.1));
                // Shift, or a corner of words: in proportion.
                if shift && grip.0 != 0 && grip.1 != 0 {
                    let both = times.0.abs().max(times.1.abs());
                    times = (both.copysign(times.0), both.copysign(times.1));
                }
                let keep = |t: f32| if t.abs() < 0.02 { 0.02f32.copysign(t) } else { t };
                if let Some(at) = selected {
                    let mut shape = before.shapes[at].clone();
                    shape.scale(still, (keep(times.0), keep(times.1)));
                    editor.doc.shapes[at] = shape;
                }
            }
            Some(EditDrag::End { end, before, .. }) => {
                editor.drag = Some(EditDrag::End { end, before, moved: true });
                if let Some(shape) = selected.and_then(|at| editor.doc.shapes.get_mut(at)) {
                    shape.points[end] = if shift { level(shape.points[1 - end], p) } else { p };
                }
            }
            Some(EditDrag::Bend { before, .. }) => {
                editor.drag = Some(EditDrag::Bend { before, moved: true });
                if let Some(shape) = selected.and_then(|at| editor.doc.shapes.get_mut(at)) {
                    shape.bend_to(p, 4.0 / scale);
                }
            }
            Some(EditDrag::Round { corner, .. }) => {
                if let Some(shape) = selected.and_then(|at| editor.doc.shapes.get_mut(at)) {
                    let [l, t, r, b] = shape.frame();
                    let (cx, cy) = (if corner.0 < 0 { l } else { r }, if corner.1 < 0 { t } else { b });
                    let most = (r - l).min(b - t) / 2.0;
                    shape.radius = (p.0 - cx).abs().min((p.1 - cy).abs()).clamp(0.0, most);
                }
            }
            Some(EditDrag::Crop { from }) => {
                let to = (p.0.clamp(0.0, w), p.1.clamp(0.0, h));
                editor.crop_draft = Some([from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1)]);
            }
            Some(EditDrag::Slider(slider)) => {
                let value = editor.slider_value(slider, x);
                match slider {
                    Slider::Picture => editor.doc.radius = value,
                    Slider::Shape => {
                        if let Some(at) = editor.subject() {
                            editor.doc.shapes[at].radius = value;
                        }
                    }
                }
            }
            None => {
                // The pointer says what a press would do, and with the
                // arrow in hand the mark under it is outlined.
                let [l, t, sw, sh] = editor.stage;
                let over_stage = (l..l + sw).contains(&(x as i32)) && (t..t + sh).contains(&(y as i32));
                let on_button = editor.hit_at(x as i32, y as i32).is_some();
                let on_handle = editor.handle_at(x as i32, y as i32).is_some();
                let hovered = if over_stage && !on_button && editor.tool == Tool::Select {
                    let reach = 6.0 / scale;
                    editor.doc.shapes.iter().rposition(|shape| shape.hit(p, reach, &mut text))
                } else {
                    None
                };
                editor.hovered = hovered;
                let icon = match (over_stage && !on_button, editor.tool) {
                    _ if on_handle => CursorIcon::Pointer,
                    (false, _) => CursorIcon::Default,
                    (true, Tool::Select) if hovered.is_some() => CursorIcon::Move,
                    (true, Tool::Select | Tool::Emoji) => CursorIcon::Default,
                    (true, Tool::Text) => CursorIcon::Text,
                    (true, _) => CursorIcon::Crosshair,
                };
                self.text = Some(text);
                self.set_edit_cursor(icon);
                return self.redraw();
            }
        }
        self.text = Some(text);
        self.redraw();
    }

    fn editor_release(&mut self) {
        let Some(editor) = &mut self.editor else { return };
        match editor.drag.take() {
            Some(EditDrag::Draw) => {
                let last = editor.doc.shapes.len() - 1;
                if editor.doc.shapes[last].is_slight() {
                    editor.doc.shapes.pop();
                    editor.past.pop();
                } else if matches!(editor.tool, Tool::Pen | Tool::Marker | Tool::Counter) {
                    // These come several at a time: the tool stays.
                } else {
                    editor.made(last);
                }
            }
            Some(EditDrag::Move { before, moved, was_selected, .. }) => {
                if moved {
                    editor.past.push(before);
                    editor.future.clear();
                } else if was_selected {
                    // A click on words already picked out goes to typing.
                    if let Some(at) = editor.selected.filter(|at| editor.doc.shapes[*at].tool == Tool::Text) {
                        editor.snap();
                        editor.typing = Some(at);
                    }
                }
            }
            Some(EditDrag::Grip { before, .. } | EditDrag::Round { before, .. }) => {
                if before != editor.doc {
                    editor.past.push(before);
                    editor.future.clear();
                }
            }
            Some(EditDrag::End { end, before, moved }) => {
                if moved {
                    editor.past.push(before);
                    editor.future.clear();
                } else if let Some(at) = editor.selected {
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
                    editor.past.push(before);
                    editor.future.clear();
                } else if let Some(at) = editor.selected {
                    // A click on the bend straightens the line.
                    if editor.doc.shapes[at].bend.is_some() {
                        editor.snap();
                        editor.doc.shapes[at].bend = None;
                    }
                }
            }
            Some(EditDrag::Crop { .. }) => {
                let small = editor.crop_draft.is_some_and(|[l, t, r, b]| r - l < 8.0 || b - t < 8.0);
                if small {
                    editor.crop_draft = editor.doc.crop;
                }
            }
            Some(EditDrag::Slider(_)) => {
                // A slider let go where it was taken up undoes nothing.
                if editor.past.last() == Some(&editor.doc) {
                    editor.past.pop();
                }
            }
            None => return,
        }
        self.redraw();
    }

    fn discard_edit(&mut self) {
        self.toast("Discarded");
        self.close_editor();
    }

    /// Saves over the file, or opens the box naming the copy.
    pub(super) fn save_edit(&mut self, way: SaveAs) {
        let Some(editor) = &mut self.editor else { return };
        editor.finish_typing();
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
    fn write_edit(&mut self, target: PathBuf, over: bool) {
        let Some(editor) = self.editor.take() else { return };
        let (id, doc) = (editor.id, editor.doc.clone());
        let source = self.files[id].clone();
        if self.dry(|_| vec![format!("save the edit of {} as {}", source.display(), target.display())], "would save") {
            self.editor = Some(editor);
            return;
        }
        self.edit_last = if over { SaveAs::Overwrite } else { SaveAs::Copy };
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
        let last = self.edit_last;
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
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
        let side = (px as i32 * 16).min(width / 3);
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
            annotate::paint(&editor.doc.shapes, canvas.all(), width as usize, place, [px0, py0, pw, ph], text, None);
            if radius > 0.0 {
                round_corners(canvas, [px0, py0, pw, ph], radius, &behind);
            }
        }
        // The crop being chosen: the rest dimmed, and its size.
        if cropping {
            let draft = editor.crop_draft.unwrap_or([0.0, 0.0, editor.native.0 as f32, editor.native.1 as f32]);
            let (l, t) = place.to_screen((draft[0], draft[1]));
            let (r, b) = place.to_screen((draft[2], draft[3]));
            let [l, t, r, b] = [l.round() as i32, t.round() as i32, r.round() as i32, b.round() as i32];
            for [x, y, w, h] in [[px0, py0, pw, t - py0], [px0, b, pw, py0 + ph - b], [px0, t, l - px0, b - t], [r, t, px0 + pw - r, b - t]] {
                canvas.shade(x, y, w, h, 96);
            }
            canvas.round_outline(l - 1, t - 1, r - l + 2, b - t + 2, 0, 1, 0xFFFFFF);
            let label = format!("{} × {}  ·  Enter → crop", (draft[2] - draft[0]).round(), (draft[3] - draft[1]).round());
            let lw = text.width(&label, small) + pad;
            let ly = (t - small_line - pad / 2).max(py0);
            canvas.round_fill(l, ly, lw, small_line + pad / 3, pad / 3, 0x000000, 200);
            text.draw(canvas, l + pad / 2, ly + pad / 6, small, TEXT_COLOR, &label);
        }
        // The mark under the pointer, faintly; the one picked out, with its
        // handles.
        if let Some(shape) = editor.hovered.filter(|at| Some(*at) != editor.selected).and_then(|at| editor.doc.shapes.get(at)) {
            let [x, y, w, h] = on_screen(place, shape.bounds(text), 3);
            canvas.round_outline(x, y, w, h, 2, 1, accent_soft());
        }
        if let Some(at) = editor.subject() {
            let shape = editor.doc.shapes[at].clone();
            draw_handles(editor, canvas, text, &shape, pad);
            if editor.typing == Some(at) {
                let size = (shape.stroke * scale).round().max(1.0) as u32;
                let (sx, sy) = place.to_screen(shape.points[0]);
                let caret = sx as i32 + text.width(&shape.text, size) + 1;
                canvas.fill(caret, sy as i32, 2.max(size as i32 / 12), Text::line_height(size), accent());
            }
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
            editor.tips.push((place, format!("{}  {}", tool.name(), tool.key().to_uppercase())));
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

        // The two saves at the other end, the one used last filled in.
        let dirty = editor.dirty();
        let mut right = width - side - pad * 2;
        for (way, label, tip) in [(SaveAs::Overwrite, "Overwrite", "Ctrl+S"), (SaveAs::Copy, "Copy", "Ctrl+Shift+S")] {
            let able = dirty && (way == SaveAs::Copy || format.is_some());
            let w = text.width(label, px) + pad * 2;
            let place = [right - w, bar_y, w, bar_h];
            let primary = way == last && able;
            let (ground, ink) = match (primary, able) {
                (true, _) => (accent(), on_accent()),
                (false, true) => (if inside(place, mouse) { PLACEHOLDER_COLOR } else { MENU_COLOR }, TEXT_COLOR),
                (false, false) => (MENU_COLOR, DIM_TEXT_COLOR),
            };
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 2, ground, 256);
            if !primary {
                canvas.round_outline(place[0], place[1], place[2], place[3], pad / 2, 1, MENU_EDGE_COLOR);
            }
            text.draw(canvas, place[0] + pad, bar_y + (bar_h - line) / 2, px, ink, label);
            editor.hits.push((EditHit::Save(way), place));
            editor.tips.push((place, tip.to_string()));
            right -= w + pad / 2;
        }

        draw_panel(editor, canvas, text, [width - side - pad, top, side, height - top - pad], px, format, mouse);

        // The emoji to pick from, over the bar.
        if editor.picker.is_some() {
            draw_picker(editor, canvas, text, emoji_at, bar_y - pad / 2, px, mouse);
        }

        // Escape with something unsaved: what to do with it.
        if editor.leaving {
            let choices = [("Overwrite", "Ctrl+S", EditHit::Save(SaveAs::Overwrite)), ("Copy", "Ctrl+Shift+S", EditHit::Save(SaveAs::Copy)), ("Discard", "D", EditHit::Discard), ("Keep editing", "Esc", EditHit::KeepEditing)];
            let label = "Unsaved";
            let widths: Vec<i32> = choices.iter().map(|(word, key, _)| text.width(word, small) + text.width(key, small) + pad * 2).collect();
            let total = text.width(label, small) + pad * 2 + widths.iter().sum::<i32>();
            let bar = [stage[0] + (stage[2] - total) / 2, top, total, small_line + pad];
            draw::panel(canvas, bar[0], bar[1], bar[2], bar[3], pad / 2);
            text.draw(canvas, bar[0] + pad, bar[1] + pad / 2, small, MARK_COLOR, label);
            let mut x = bar[0] + pad * 2 + text.width(label, small);
            for ((word, key, hit), w) in choices.into_iter().zip(widths) {
                let place = [x - pad / 2, bar[1] + pad / 4, w, bar[3] - pad / 2];
                if inside(place, mouse) {
                    canvas.round_fill(place[0], place[1], place[2], place[3], pad / 3, PLACEHOLDER_COLOR, 256);
                }
                text.draw(canvas, x, bar[1] + pad / 2, small, TEXT_COLOR, word);
                let ww = text.width(word, small);
                text.draw(canvas, x + ww + pad / 3, bar[1] + pad / 2, small, DIM_TEXT_COLOR, key);
                editor.hits.push((hit, place));
                x += w;
            }
        }

        // What the button under the pointer is, and its key.
        if editor.drag.is_none() {
            if let Some((place, tip)) = editor.tips.iter().find(|(place, _)| inside(*place, mouse)) {
                let w = text.width(tip, small) + pad;
                let h = small_line + pad / 2;
                let x = (place[0] + place[2] / 2 - w / 2).clamp(pad / 2, width - w - pad / 2);
                let y = place[1] - h - pad / 3;
                canvas.round_fill(x, y, w, h, pad / 3, 0x000000, 210);
                text.draw(canvas, x + pad / 2, y + pad / 4, small, TEXT_COLOR, tip);
            }
        }
    }
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
/// dots at a line's ends and middle, and inside a rectangle's corners
/// the dots that round them.
fn draw_handles(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, shape: &Shape, pad: i32) {
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
            let [x, y, w, h] = on_screen(place, shape.bounds(text), 3);
            canvas.round_outline(x, y, w, h, 2, 1, accent());
        }
        _ => {
            let [x, y, w, h] = on_screen(place, shape.bounds(text), 3);
            canvas.round_outline(x, y, w, h, 2, 1, accent());
            if editor.typing.is_some() {
                return;
            }
            for (gx, gy) in [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)] {
                // Words keep their proportions: their corners only.
                if shape.tool.lettered() && (gx == 0 || gy == 0) {
                    continue;
                }
                let at = (x + w * (gx + 1) / 2, y + h * (gy + 1) / 2);
                canvas.fill(at.0 - grip - 1, at.1 - grip - 1, (grip + 1) * 2, (grip + 1) * 2, accent());
                canvas.fill(at.0 - grip + 1, at.1 - grip + 1, (grip - 1) * 2, (grip - 1) * 2, 0xFFFFFF);
                editor.handles.push((Handle::Grip(gx as i8, gy as i8), at));
            }
            // Corners to round, where there is room for them.
            if matches!(shape.tool, Tool::Rect | Tool::Spotlight) && w.min(h) >= pad * 3 {
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

/// A press on a handle of the mark picked out.
fn grab(editor: &mut Editor, handle: Handle, at: Point, text: &mut Text) {
    let Some(index) = editor.selected else { return };
    let p = editor.place.to_picture(at);
    let before = editor.doc.clone();
    editor.drag = Some(match handle {
        Handle::Grip(gx, gy) => EditDrag::Grip { grip: (gx, gy), frame: editor.doc.shapes[index].bounds(text), from: p, before },
        Handle::End(end) => EditDrag::End { end, before, moved: false },
        Handle::Bend => EditDrag::Bend { before, moved: false },
        Handle::Round(cx, cy) => EditDrag::Round { corner: (cx, cy), before },
    });
}

/// A press on the picture, with the tool in hand.
fn editor_press(editor: &mut Editor, at: Point, text: &mut Text) {
    let [l, t, w, h] = editor.stage;
    if !(l as f32..(l + w) as f32).contains(&at.0) || !(t as f32..(t + h) as f32).contains(&at.1) {
        editor.finish_typing();
        return;
    }
    editor.leaving = false;
    editor.finish_typing();
    let p = editor.place.to_picture(at);
    let reach = 6.0 / editor.place.scale;
    match editor.tool {
        Tool::Select | Tool::Emoji => {
            editor.picker = None;
            editor.tool = Tool::Select;
            // What is picked out is taken hold of anywhere in its box.
            let held = editor.selected.filter(|at| {
                let [l, t, r, b] = editor.doc.shapes[*at].bounds(text);
                p.0 >= l - reach && p.0 <= r + reach && p.1 >= t - reach && p.1 <= b + reach
            });
            let was_selected = held.is_some();
            editor.selected = held.or_else(|| editor.doc.shapes.iter().rposition(|shape| shape.hit(p, reach, text)));
            if editor.selected.is_some() {
                editor.drag = Some(EditDrag::Move { last: p, before: editor.doc.clone(), moved: false, was_selected });
            }
        }
        Tool::Crop => {
            let (w, h) = (editor.native.0 as f32, editor.native.1 as f32);
            let from = (p.0.clamp(0.0, w), p.1.clamp(0.0, h));
            editor.crop_draft = Some([from.0, from.1, from.0, from.1]);
            editor.drag = Some(EditDrag::Crop { from });
        }
        Tool::Text => {
            // On words already there, more of them; elsewhere, new ones.
            let on = editor.doc.shapes.iter().rposition(|shape| shape.tool == Tool::Text && shape.hit(p, reach, text));
            editor.snap();
            let at = on.unwrap_or_else(|| {
                let stroke = editor.stroke(Tool::Text, editor.style.size);
                // The pointer in the middle of the first line.
                let shape = editor.fresh(Tool::Text, (p.0, p.1 - stroke * 0.65));
                editor.doc.shapes.push(shape);
                editor.doc.shapes.len() - 1
            });
            editor.typing = Some(at);
            editor.selected = None;
        }
        Tool::Counter => {
            editor.snap();
            let shape = editor.fresh(Tool::Counter, p);
            editor.doc.shapes.push(shape);
            editor.drag = Some(EditDrag::Draw);
        }
        tool => {
            editor.snap();
            let shape = editor.fresh(tool, p);
            editor.doc.shapes.push(shape);
            editor.drag = Some(EditDrag::Draw);
            editor.selected = None;
        }
    }
}

/// The panel down the right: how the mark picked out, or the next one, is
/// drawn; then the picture's own corners and crop; then the keys.
#[allow(clippy::too_many_arguments)]
fn draw_panel(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, [ix, top, side, high]: [i32; 4], px: u32, format: Option<Format>, mouse: (i32, i32)) {
    let pad = px as i32;
    let line = Text::line_height(px);
    let small = (px * 5 / 6).max(11);
    let small_line = Text::line_height(small);
    draw::panel(canvas, ix, top, side, high, pad / 2);
    editor.hits.push((EditHit::Inside, [ix, top, side, high]));
    let inner = ix + pad;
    let inner_w = side - pad * 2;
    let mut y = top + pad;
    let subject = editor.subject().map(|at| editor.doc.shapes[at].clone());
    let kind = subject.as_ref().map_or(editor.tool, |shape| shape.tool);
    let heading = |canvas: &mut Canvas, text: &mut Text, y: i32, words: &str| text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, words);
    let marks = !matches!(kind, Tool::Select | Tool::Crop);

    if marks {
        text.draw(canvas, inner, y, px, TEXT_COLOR, kind.name());
        y += line + pad / 2;
    }
    let colored = marks && !matches!(kind, Tool::Blur | Tool::Spotlight | Tool::Emoji);
    if colored {
        let color = subject.as_ref().and_then(|shape| COLORS.iter().position(|c| *c == shape.color)).unwrap_or(editor.style.color);
        let swatch = (inner_w - pad / 3 * 7) / 8;
        for (at, c) in COLORS.iter().enumerate() {
            let sx = inner + at as i32 * (swatch + pad / 3);
            if at == color {
                canvas.round_fill(sx - 2, y - 2, swatch + 4, swatch + 4, swatch / 2 + 2, TEXT_COLOR, 256);
                canvas.round_fill(sx, y, swatch, swatch, swatch / 2, MENU_COLOR, 256);
            }
            let inset = if at == color { 2 } else { 0 };
            canvas.round_fill(sx + inset, y + inset, swatch - inset * 2, swatch - inset * 2, swatch / 2, *c, 256);
            // The near-black one would be lost on the panel.
            if *c < 0x404040 {
                canvas.round_outline(sx + inset, y + inset, swatch - inset * 2, swatch - inset * 2, swatch / 2, 1, MENU_EDGE_COLOR);
            }
            editor.hits.push((EditHit::Color(at), [sx, y, swatch, swatch]));
        }
        y += swatch + pad;
    }
    if marks && !matches!(kind, Tool::Blur | Tool::Spotlight) {
        heading(canvas, text, y, "Opacity");
        y += small_line + pad / 3;
        let opacity = subject.as_ref().and_then(|shape| OPACITIES.iter().position(|o| *o == shape.opacity)).unwrap_or(editor.style.opacity);
        let cells = segments(canvas, text, [inner, y, inner_w, line], &["25%", "50%", "75%", "100%"], Some(opacity), small, mouse);
        editor.hits.extend(cells.into_iter().enumerate().map(|(at, place)| (EditHit::Opacity(at), place)));
        y += line + pad;
    }
    if marks {
        let words = match kind {
            Tool::Text | Tool::Emoji | Tool::Counter => "Size",
            Tool::Blur => "Strength",
            _ => "Width",
        };
        heading(canvas, text, y, words);
        y += small_line + pad / 3;
        let size = subject.as_ref().map_or(editor.style.size, |shape| (0..4).find(|at| (editor.stroke(shape.tool, *at) - shape.stroke).abs() < 0.01).unwrap_or(usize::MAX));
        let cells = segments(canvas, text, [inner, y, inner_w, line], &["1", "2", "3", "4"], Some(size), small, mouse);
        editor.hits.extend(cells.into_iter().enumerate().map(|(at, place)| (EditHit::Size(at), place)));
        y += line + pad;
    }
    if matches!(kind, Tool::Rect | Tool::Oval) {
        heading(canvas, text, y, "Fill");
        y += small_line + pad / 3;
        let filled = subject.as_ref().map_or(editor.style.filled, |shape| shape.filled);
        let cells = segments(canvas, text, [inner, y, inner_w, line], &["Outline", "Filled"], Some(usize::from(filled)), small, mouse);
        editor.hits.extend(cells.into_iter().enumerate().map(|(at, place)| (EditHit::Filled(at == 1), place)));
        y += line + pad;
    }
    if let Some(shape) = subject.as_ref().filter(|shape| matches!(shape.tool, Tool::Rect | Tool::Spotlight)) {
        let value = format!("{} px", shape.radius.round());
        y = slider(editor, canvas, text, [inner, y, inner_w], "Corners", &value, shape.radius / editor.slider_most(Slider::Shape).max(1.0), Slider::Shape, small, pad);
    }
    if kind.is_line() {
        heading(canvas, text, y, "Ends");
        y += small_line + pad / 3;
        let heads = subject.as_ref().map_or(if kind == Tool::Arrow { editor.style.heads } else { [false, false] }, |shape| shape.heads);
        let choices = [[false, false], [false, true], [true, true]];
        let cells = segments(canvas, text, [inner, y, inner_w, line], &["", "", ""], choices.iter().position(|c| *c == heads), small, mouse);
        for (cell, choice) in cells.iter().zip(choices) {
            preview(canvas, text, *cell, choice, Dash::Solid, pad);
            editor.hits.push((EditHit::Heads(choice), *cell));
        }
        y += line + pad / 2;
        let dash = subject.as_ref().map_or(editor.style.dash, |shape| shape.dash);
        let dashes = [Dash::Solid, Dash::Dashed, Dash::Dotted];
        let cells = segments(canvas, text, [inner, y, inner_w, line], &["", "", ""], dashes.iter().position(|d| *d == dash), small, mouse);
        for (cell, dash) in cells.iter().zip(dashes) {
            preview(canvas, text, *cell, [false, false], dash, pad);
            editor.hits.push((EditHit::Dash(dash), *cell));
        }
        y += line + pad;
    }
    if kind == Tool::Arrow || kind == Tool::Line {
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Click an end  ·  drag the middle");
        y += small_line + pad;
    }

    // The picture's own: its corners and its crop.
    if marks {
        canvas.fill(inner, y, inner_w, 1, MENU_EDGE_COLOR);
        y += pad;
    }
    text.draw(canvas, inner, y, px, TEXT_COLOR, "Picture");
    y += line + pad / 2;
    let value = format!("{} px", editor.doc.radius.round());
    let along = editor.doc.radius / editor.max_radius().max(1.0);
    y = slider(editor, canvas, text, [inner, y, inner_w], "Corners", &value, along, Slider::Picture, small, pad);
    let (ow, oh) = editor.out_size();
    heading(canvas, text, y, "Crop");
    let crop_words = if editor.doc.crop.is_some() { format!("{ow} × {oh}") } else { "none".to_string() };
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
    } else {
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "X  ·  drag  ·  Enter");
        y += small_line + pad / 2;
    }
    if editor.doc.radius > 0.0 && format == Some(Format::Jpeg) {
        y += pad / 2;
        text.draw(canvas, inner, y, small, MARK_COLOR, "JPEG · no clear corners");
        y += small_line;
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Overwrite → white  ·  Copy → PNG");
        y += small_line;
    }
    let keys = [("Ctrl+S", "overwrite  ·  Shift copy"), ("Ctrl+D", "duplicate  ·  Del remove"), ("1–4  c  d  f", "size, color, dash, fill"), ("Esc", "leave")];
    let key_w = keys.iter().map(|(key, _)| text.width(key, small)).max().unwrap_or(0) + pad / 2;
    let mut y = y.max(top + high - pad - keys.len() as i32 * small_line);
    for (key, what) in keys {
        text.draw(canvas, inner, y, small, TEXT_COLOR, key);
        text.draw(canvas, inner + key_w, y, small, DIM_TEXT_COLOR, what);
        y += small_line;
    }
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
fn preview(canvas: &mut Canvas, text: &mut Text, [x, y, w, h]: [i32; 4], heads: [bool; 2], dash: Dash, pad: i32) {
    let mut shape = Shape::new(Tool::Line, TEXT_COLOR, 2.0, ((x + pad / 2) as f32, (y + h / 2) as f32));
    shape.points[1] = ((x + w - pad / 2) as f32, (y + h / 2) as f32);
    shape.heads = heads;
    shape.dash = dash;
    let width = canvas.width as usize;
    annotate::paint(&[shape], canvas.all(), width, Place::WHOLE, [x, y, w, h], text, None);
}

/// A slider with its name and value above it; where the next row goes.
#[allow(clippy::too_many_arguments)]
fn slider(editor: &mut Editor, canvas: &mut Canvas, text: &mut Text, [x, y, w]: [i32; 3], name: &str, value: &str, along: f32, which: Slider, small: u32, pad: i32) -> i32 {
    let small_line = Text::line_height(small);
    text.draw(canvas, x, y, small, DIM_TEXT_COLOR, name);
    let vw = text.width(value, small);
    text.draw(canvas, x + w - vw, y, small, TEXT_COLOR, value);
    let y = y + small_line + pad / 2;
    let along = along.clamp(0.0, 1.0);
    let track = (pad / 3).max(4);
    canvas.pill(x, y, w, track, PLACEHOLDER_COLOR, 256, w);
    canvas.pill(x, y, w, track, accent(), 256, (along * w as f32) as i32);
    let knob = pad;
    let kx = x + (along * (w - knob) as f32) as i32;
    canvas.round_fill(kx, y + track / 2 - knob / 2, knob, knob, knob / 2, TEXT_COLOR, 256);
    editor.hits.push((EditHit::Slider(which), [x, y - pad / 2, w, track + pad]));
    y + track + pad * 3 / 2
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
    for (n, which) in found.iter().skip(picker.scroll * EMOJI_ACROSS).take(EMOJI_ACROSS * EMOJI_DOWN).enumerate() {
        let (column, row) = ((n % EMOJI_ACROSS) as i32, (n / EMOJI_ACROSS) as i32);
        let place = [x + pad / 2 + column * cell, grid_y + row * cell, cell, cell];
        if inside(place, mouse) {
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 3, PLACEHOLDER_COLOR, 256);
            caption = list[*which].1.to_string();
        }
        if let Some(picture) = annotate::emoji(list[*which].0, side) {
            let at = (place[0] + (cell - picture.0) / 2, place[1] + (cell - picture.1) / 2);
            annotate::stamp(canvas.all(), width, place_clip(place), &picture, at, 256, None);
        }
        editor.hits.push((EditHit::Emoji(*which), place));
    }
    text.draw(canvas, x + pad / 2, grid_y + cell * EMOJI_DOWN as i32 + pad / 4, small, DIM_TEXT_COLOR, &caption);
}

/// A rectangle as the clip `stamp` takes: left, top, right, bottom.
fn place_clip([x, y, w, h]: [i32; 4]) -> [i32; 4] {
    [x, y, x + w, y + h]
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
        Tool::Emoji => STICKER,
        Tool::Blur => BLUR,
        Tool::Spotlight => SPOTLIGHT,
        Tool::Crop => CROP,
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
    let image = convert::turned(crate::decode::open(source).map_err(|error| error.to_string())?, transform);
    let out = annotate::render(&image.to_rgba8(), &doc.shapes, doc.crop, doc.radius);
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
}
