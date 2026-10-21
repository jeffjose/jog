//! Edit: the picture on view drawn on, cropped and its corners rounded,
//! then saved over itself or as a copy beside it. The marks stay
//! editable until it is saved; saving is done on another thread, from
//! the file decoded again at full size.

use image::DynamicImage;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use crate::annotate::{self, Place, Point, Shape, Tool, COLORS};

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
    color: usize,
    size: usize,
    selected: Option<usize>,
    /// The words being typed into, by the shape's place.
    typing: Option<usize>,
    /// The crop being dragged out, before Enter takes it.
    crop_draft: Option<[f32; 4]>,
    drag: Option<EditDrag>,
    /// Escape with something unsaved: the bar that asks what to do.
    leaving: bool,
    /// As last drawn: where the picture is, the part of it on show, and
    /// what there is to click.
    place: Place,
    picture_at: [i32; 4],
    stage: [i32; 4],
    slider: [i32; 4],
    hits: Vec<(EditHit, [i32; 4])>,
    cursor: CursorIcon,
}

#[derive(Clone, Debug)]
enum EditDrag {
    Draw,
    Move { last: Point, before: Doc, moved: bool },
    Crop { from: Point },
    Corners,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum EditHit {
    Tool(Tool),
    Color(usize),
    Size(usize),
    Corners,
    ResetCrop,
    Save(SaveAs),
    Discard,
    KeepEditing,
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
            color: 0,
            size: 1,
            selected: None,
            typing: None,
            crop_draft: None,
            drag: None,
            leaving: false,
            place: Place::WHOLE,
            picture_at: [0; 4],
            stage: [0; 4],
            slider: [0; 4],
            hits: Vec::new(),
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

    fn stroke(&self, tool: Tool) -> f32 {
        match tool {
            Tool::Text => annotate::text_sizes(self.native)[self.size],
            _ => annotate::widths(self.native)[self.size],
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
        self.tool = tool;
    }

    /// Words typed and done with: left if there are any, gone if not.
    fn finish_typing(&mut self) {
        let Some(at) = self.typing.take() else { return };
        if self.doc.shapes.get(at).is_some_and(|shape| shape.text.trim().is_empty()) {
            self.doc.shapes.remove(at);
            // A text that came to nothing leaves nothing to undo.
            if self.past.last() == Some(&self.doc) {
                self.past.pop();
            }
            self.selected = None;
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

    fn radius_at(&self, x: f64) -> f32 {
        let [l, _, w, _] = self.slider;
        let along = ((x as f32 - l as f32) / w.max(1) as f32).clamp(0.0, 1.0);
        (along * self.max_radius()).round()
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
        match key {
            Key::Character("s" | "S") if control => {
                let way = if shift { SaveAs::Copy } else { SaveAs::Overwrite };
                return self.save_edit(way);
            }
            Key::Character("z" | "Z") if control && shift => editor.redo(),
            Key::Character("z") if control => editor.undo(),
            Key::Character("y") if control => editor.redo(),
            _ if control => return,
            // The bar asking about what is unsaved has keys of its own.
            Key::Character("d" | "D") if editor.leaving => return self.discard_edit(),
            Key::Named(NamedKey::Escape) if editor.leaving => editor.leaving = false,
            Key::Named(NamedKey::Escape) => {
                if editor.tool == Tool::Crop && editor.crop_draft != editor.doc.crop {
                    editor.crop_draft = editor.doc.crop;
                } else if editor.tool == Tool::Crop {
                    editor.tool = Tool::Select;
                } else if editor.selected.is_some() {
                    editor.selected = None;
                } else if editor.dirty() {
                    editor.leaving = true;
                } else {
                    return self.close_editor();
                }
            }
            Key::Named(NamedKey::Enter) if editor.tool == Tool::Crop => editor.apply_crop(),
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                if let Some(at) = editor.selected.take() {
                    editor.snap();
                    editor.doc.shapes.remove(at);
                }
            }
            Key::Character(digit @ ("1" | "2" | "3" | "4")) => {
                let size = digit.parse::<usize>().unwrap_or(1) - 1;
                self.set_edit_size(size);
            }
            // Through the colors, as hotshot's C does.
            Key::Character("c") => {
                let color = (editor.color + 1) % COLORS.len();
                self.set_edit_color(color);
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

    fn set_edit_color(&mut self, color: usize) {
        let Some(editor) = &mut self.editor else { return };
        editor.color = color;
        if let Some(at) = editor.selected.or(editor.typing) {
            if editor.doc.shapes[at].color != COLORS[color] {
                editor.snap();
                editor.doc.shapes[at].color = COLORS[color];
            }
        }
        self.redraw();
    }

    fn set_edit_size(&mut self, size: usize) {
        let Some(editor) = &mut self.editor else { return };
        editor.size = size;
        if let Some(at) = editor.selected.or(editor.typing) {
            let stroke = editor.stroke(editor.doc.shapes[at].tool);
            if editor.doc.shapes[at].stroke != stroke {
                editor.snap();
                editor.doc.shapes[at].stroke = stroke;
            }
        }
        self.redraw();
    }

    pub(super) fn editor_mouse(&mut self, pressed: bool) {
        let (x, y) = self.mouse;
        if !pressed {
            return self.editor_release();
        }
        let mut text = self.text.take().unwrap_or_else(Text::load);
        let hit = self.editor.as_ref().and_then(|editor| editor.hit_at(x as i32, y as i32));
        match hit {
            Some(EditHit::Tool(tool)) => {
                if let Some(editor) = &mut self.editor {
                    editor.set_tool(tool);
                }
            }
            Some(EditHit::Color(color)) => self.set_edit_color(color),
            Some(EditHit::Size(size)) => self.set_edit_size(size),
            Some(EditHit::Corners) => {
                if let Some(editor) = &mut self.editor {
                    editor.finish_typing();
                    editor.snap();
                    editor.doc.radius = editor.radius_at(x);
                    editor.drag = Some(EditDrag::Corners);
                }
            }
            Some(EditHit::ResetCrop) => {
                if let Some(editor) = &mut self.editor {
                    editor.snap();
                    editor.doc.crop = None;
                    editor.crop_draft = None;
                }
            }
            Some(EditHit::Save(way)) => {
                self.text = Some(text);
                return self.save_edit(way);
            }
            Some(EditHit::Discard) => {
                self.text = Some(text);
                return self.discard_edit();
            }
            Some(EditHit::KeepEditing) => {
                if let Some(editor) = &mut self.editor {
                    editor.leaving = false;
                }
            }
            None => {
                if let Some(editor) = &mut self.editor {
                    editor_press(editor, (x as f32, y as f32), &mut text);
                }
            }
        }
        self.text = Some(text);
        self.redraw();
    }

    pub(super) fn editor_motion(&mut self) {
        let (x, y) = self.mouse;
        let Some(editor) = &mut self.editor else { return };
        let p = editor.place.to_picture((x as f32, y as f32));
        let (w, h) = (editor.native.0 as f32, editor.native.1 as f32);
        match &mut editor.drag {
            Some(EditDrag::Draw) => {
                if let Some(shape) = editor.doc.shapes.last_mut() {
                    match shape.tool {
                        Tool::Pen | Tool::Marker => shape.points.push(p),
                        _ => {
                            let last = shape.points.len() - 1;
                            shape.points[last] = p;
                        }
                    }
                }
            }
            Some(EditDrag::Move { last, moved, .. }) => {
                let (dx, dy) = (p.0 - last.0, p.1 - last.1);
                *last = p;
                *moved = true;
                if let Some(at) = editor.selected {
                    editor.doc.shapes[at].translate(dx, dy);
                }
            }
            Some(EditDrag::Crop { from }) => {
                let to = (p.0.clamp(0.0, w), p.1.clamp(0.0, h));
                editor.crop_draft = Some([from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1)]);
            }
            Some(EditDrag::Corners) => editor.doc.radius = editor.radius_at(x),
            None => {
                // The pointer says what a press will do: draw, or click.
                let [l, t, sw, sh] = editor.stage;
                let over_stage = (l..l + sw).contains(&(x as i32)) && (t..t + sh).contains(&(y as i32));
                let on_button = editor.hit_at(x as i32, y as i32).is_some();
                let icon = match (over_stage && !on_button, editor.tool) {
                    (false, _) | (true, Tool::Select) => CursorIcon::Default,
                    (true, Tool::Text) => CursorIcon::Text,
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
        match editor.drag.take() {
            Some(EditDrag::Draw) => {
                if editor.doc.shapes.last().is_some_and(Shape::is_slight) {
                    editor.doc.shapes.pop();
                    editor.past.pop();
                } else {
                    editor.selected = Some(editor.doc.shapes.len() - 1);
                }
            }
            Some(EditDrag::Move { before, moved, .. }) => {
                if moved {
                    editor.past.push(before);
                    editor.future.clear();
                }
            }
            Some(EditDrag::Crop { .. }) => {
                let small = editor.crop_draft.is_some_and(|[l, t, r, b]| r - l < 8.0 || b - t < 8.0);
                if small {
                    editor.crop_draft = editor.doc.crop;
                }
            }
            Some(EditDrag::Corners) => {
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

        // The inspector down the right, the bar along the bottom, and the
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
        editor.picture_at = [px0, py0, pw, ph];
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
            annotate::paint(&editor.doc.shapes, canvas.all(), width as usize, editor.place, [px0, py0, pw, ph], text, None);
            if radius > 0.0 {
                round_corners(canvas, [px0, py0, pw, ph], radius, &behind);
            }
        }
        // The crop being chosen: the rest dimmed, thirds, and its size.
        if cropping {
            let draft = editor.crop_draft.unwrap_or([0.0, 0.0, editor.native.0 as f32, editor.native.1 as f32]);
            let (l, t) = editor.place.to_screen((draft[0], draft[1]));
            let (r, b) = editor.place.to_screen((draft[2], draft[3]));
            let [l, t, r, b] = [l.round() as i32, t.round() as i32, r.round() as i32, b.round() as i32];
            for [x, y, w, h] in [[px0, py0, pw, t - py0], [px0, b, pw, py0 + ph - b], [px0, t, l - px0, b - t], [r, t, px0 + pw - r, b - t]] {
                canvas.shade(x, y, w, h, 96);
            }
            canvas.round_outline(l - 1, t - 1, r - l + 2, b - t + 2, 0, 1, 0xFFFFFF);
            for k in 1..3 {
                canvas.fill(l + (r - l) * k / 3, t, 1, b - t, 0x808080);
                canvas.fill(l, t + (b - t) * k / 3, r - l, 1, 0x808080);
            }
            let label = format!("{} × {}  ·  Enter → crop", (draft[2] - draft[0]).round(), (draft[3] - draft[1]).round());
            let lw = text.width(&label, small) + pad;
            let ly = (t - small_line - pad / 2).max(py0);
            canvas.round_fill(l, ly, lw, small_line + pad / 3, pad / 3, 0x000000, 200);
            text.draw(canvas, l + pad / 2, ly + pad / 6, small, TEXT_COLOR, &label);
        }
        // What is picked out, and where the words being typed go on.
        if let Some(shape) = editor.selected.or(editor.typing).and_then(|at| editor.doc.shapes.get(at)) {
            let [l, t, r, b] = shape.bounds(text);
            let (l, t) = editor.place.to_screen((l, t));
            let (r, b) = editor.place.to_screen((r, b));
            let reach = 4;
            let [x, y, w, h] = [l as i32 - reach, t as i32 - reach, (r - l) as i32 + reach * 2, (b - t) as i32 + reach * 2];
            canvas.round_outline(x, y, w, h, 2, 1, accent());
            if let Some(at) = editor.typing {
                let shape = &editor.doc.shapes[at];
                let size = (shape.stroke * scale).round().max(1.0) as u32;
                let (sx, sy) = editor.place.to_screen(shape.points[0]);
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

        // The tools, along the bottom.
        let bar_y = height - pad - bar_h;
        let tools = Tool::BAR.iter().filter(|tool| tool.is_some()).count() as i32;
        let gaps = Tool::BAR.iter().filter(|tool| tool.is_none()).count() as i32;
        let tools_w = tools * button + gaps * pad / 2 + pad / 2;
        draw::panel(canvas, pad, bar_y, tools_w, bar_h, pad / 2);
        let mut x = pad + pad / 4;
        let mouse = (self.mouse.0 as i32, self.mouse.1 as i32);
        for tool in Tool::BAR {
            let Some(tool) = tool else {
                canvas.fill(x + pad / 4 - 1, bar_y + pad / 2, 1, bar_h - pad, MENU_EDGE_COLOR);
                x += pad / 2;
                continue;
            };
            let place = [x, bar_y + pad / 4, button, button];
            let on = editor.tool == tool;
            let over = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
            if on || over {
                canvas.round_fill(place[0], place[1], place[2], place[3], pad / 2, if on { accent() } else { PLACEHOLDER_COLOR }, 256);
            }
            let icon = tool_icon(tool);
            let iw = text.width(icon, px);
            text.draw(canvas, x + (button - iw) / 2, place[1] + (button - line) / 2, px, if on { on_accent() } else { TEXT_COLOR }, icon);
            let key = tool.key().to_uppercase();
            let tiny = (px * 2 / 3).max(9);
            let kw = text.width(&key, tiny);
            text.draw(canvas, x + button - kw - 3, place[1] + button - Text::line_height(tiny), tiny, if on { on_accent() } else { DIM_TEXT_COLOR }, &key);
            editor.hits.push((EditHit::Tool(tool), place));
            x += button;
        }

        // The two saves, at the other end, the one used last filled in.
        let dirty = editor.dirty();
        let jpeg = format == Some(Format::Jpeg);
        let rounded = editor.doc.radius > 0.0;
        let over_note = match (format, rounded && jpeg) {
            (None, _) => "copy only".to_string(),
            (_, true) => "corners → white".to_string(),
            _ => "Ctrl+S".to_string(),
        };
        let copy_note = if rounded && jpeg { "→ PNG  ·  Ctrl+Shift+S" } else { "Ctrl+Shift+S" };
        let mut right = width - side - pad * 2;
        for (way, label, note) in [(SaveAs::Overwrite, "Overwrite", over_note.as_str()), (SaveAs::Copy, "Save as copy", copy_note)] {
            let w = text.width(label, px).max(text.width(note, small)) + pad * 2;
            let place = [right - w, bar_y, w, bar_h];
            let primary = way == last;
            let over = (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1);
            let (ground, ink, dim) = match (primary, dirty) {
                (true, true) => (accent(), on_accent(), on_accent()),
                _ => (if over && dirty { PLACEHOLDER_COLOR } else { MENU_COLOR }, if dirty { TEXT_COLOR } else { DIM_TEXT_COLOR }, DIM_TEXT_COLOR),
            };
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 2, ground, 256);
            if !primary || !dirty {
                canvas.round_outline(place[0], place[1], place[2], place[3], pad / 2, 1, MENU_EDGE_COLOR);
            }
            let ty = bar_y + (bar_h - line - small_line + pad / 4) / 2;
            text.draw(canvas, place[0] + pad, ty, px, ink, label);
            let warn = note.contains('→') || note == "copy only";
            text.draw(canvas, place[0] + pad, ty + line - pad / 4, small, if warn && !(primary && dirty) { MARK_COLOR } else { dim }, note);
            editor.hits.push((EditHit::Save(way), place));
            right -= w + pad / 2;
        }

        // The inspector: color, size, the picture's corners and crop.
        let ix = width - side - pad;
        draw::panel(canvas, ix, top, side, height - top - pad, pad / 2);
        let inner = ix + pad;
        let inner_w = side - pad * 2;
        let mut y = top + pad;
        let selected = editor.selected.or(editor.typing).and_then(|at| editor.doc.shapes.get(at));
        let heading = selected.map_or("Style", |shape| shape.tool.name());
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, heading);
        y += small_line + pad / 3;
        let color = selected.and_then(|shape| COLORS.iter().position(|c| *c == shape.color)).unwrap_or(editor.color);
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
            if annotate::ink_on(*c) == 0xFFFFFF && *c < 0x404040 {
                canvas.round_outline(sx + inset, y + inset, swatch - inset * 2, swatch - inset * 2, swatch / 2, 1, MENU_EDGE_COLOR);
            }
            editor.hits.push((EditHit::Color(at), [sx, y, swatch, swatch]));
        }
        y += swatch + pad;
        let words = selected.is_some_and(|shape| shape.tool == Tool::Text) || (selected.is_none() && editor.tool == Tool::Text);
        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, if words { "Text size" } else { "Width" });
        y += small_line + pad / 3;
        let size = selected.map_or(editor.size, |shape| {
            let sizes = if shape.tool == Tool::Text { annotate::text_sizes(editor.native) } else { annotate::widths(editor.native) };
            sizes.iter().position(|s| (s - shape.stroke).abs() < 0.01).unwrap_or(editor.size)
        });
        let cell = (inner_w - 3 * (pad / 4)) / 4;
        for at in 0..4 {
            let place = [inner + at as i32 * (cell + pad / 4), y, cell, line];
            canvas.round_fill(place[0], place[1], place[2], place[3], pad / 3, if at == size { PLACEHOLDER_COLOR } else { 0x161616 }, 256);
            let label = (at + 1).to_string();
            let lw = text.width(&label, small);
            text.draw(canvas, place[0] + (cell - lw) / 2, y + (line - small_line) / 2, small, if at == size { TEXT_COLOR } else { DIM_TEXT_COLOR }, &label);
            editor.hits.push((EditHit::Size(at), place));
        }
        y += line + pad * 3 / 2;

        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Corners");
        let value = format!("{} px", editor.doc.radius.round());
        let vw = text.width(&value, small);
        text.draw(canvas, inner + inner_w - vw, y, small, TEXT_COLOR, &value);
        y += small_line + pad / 2;
        let along = if editor.max_radius() > 0.0 { (editor.doc.radius / editor.max_radius()).clamp(0.0, 1.0) } else { 0.0 };
        let track_h = (pad / 3).max(4);
        canvas.pill(inner, y, inner_w, track_h, PLACEHOLDER_COLOR, 256, inner_w);
        canvas.pill(inner, y, inner_w, track_h, accent(), 256, (along * inner_w as f32) as i32);
        let knob = pad;
        let kx = inner + (along * (inner_w - knob) as f32) as i32;
        canvas.round_fill(kx, y + track_h / 2 - knob / 2, knob, knob, knob / 2, TEXT_COLOR, 256);
        editor.slider = [inner, y - pad / 2, inner_w, track_h + pad];
        editor.hits.push((EditHit::Corners, editor.slider));
        y += track_h + pad * 3 / 2;

        text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Crop");
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
        y += pad / 2;
        if rounded && jpeg {
            text.draw(canvas, inner, y, small, MARK_COLOR, "JPEG · no transparency");
            y += small_line;
            text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "Overwrite → white corners");
            y += small_line;
            text.draw(canvas, inner, y, small, DIM_TEXT_COLOR, "A copy → PNG");
            y += small_line + pad;
        }
        let keys = [("Ctrl+Z", "undo  ·  Shift redo"), ("Del", "remove"), ("1–4  c", "size  ·  color"), ("Esc", "leave")];
        let key_w = keys.iter().map(|(key, _)| text.width(key, small)).max().unwrap_or(0) + pad / 2;
        y = y.max(height - pad * 2 - keys.len() as i32 * small_line);
        for (key, what) in keys {
            text.draw(canvas, inner, y, small, TEXT_COLOR, key);
            text.draw(canvas, inner + key_w, y, small, DIM_TEXT_COLOR, what);
            y += small_line;
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
                if (place[0]..place[0] + place[2]).contains(&mouse.0) && (place[1]..place[1] + place[3]).contains(&mouse.1) {
                    canvas.round_fill(place[0], place[1], place[2], place[3], pad / 3, PLACEHOLDER_COLOR, 256);
                }
                text.draw(canvas, x, bar[1] + pad / 2, small, TEXT_COLOR, word);
                let ww = text.width(word, small);
                text.draw(canvas, x + ww + pad / 3, bar[1] + pad / 2, small, DIM_TEXT_COLOR, key);
                editor.hits.push((hit, place));
                x += w;
            }
        }
    }
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
    let color = COLORS[editor.color];
    match editor.tool {
        Tool::Select => {
            editor.selected = editor.doc.shapes.iter().rposition(|shape| shape.hit(p, reach, text));
            if editor.selected.is_some() {
                editor.drag = Some(EditDrag::Move { last: p, before: editor.doc.clone(), moved: false });
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
                let stroke = editor.stroke(Tool::Text);
                // The pointer in the middle of the first line.
                let mut shape = Shape::new(Tool::Text, color, stroke, (p.0, p.1 - stroke * 0.65));
                shape.text.clear();
                editor.doc.shapes.push(shape);
                editor.doc.shapes.len() - 1
            });
            editor.typing = Some(at);
            editor.selected = None;
        }
        Tool::Counter => {
            editor.snap();
            editor.doc.shapes.push(Shape::new(Tool::Counter, color, editor.stroke(Tool::Counter), p));
            editor.selected = Some(editor.doc.shapes.len() - 1);
        }
        tool => {
            editor.snap();
            editor.doc.shapes.push(Shape::new(tool, color, editor.stroke(tool), p));
            editor.drag = Some(EditDrag::Draw);
            editor.selected = None;
        }
    }
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
