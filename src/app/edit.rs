//! Marks, and what is done to files: copy, move, rename, convert, delete, trash, and undo and redo.

use super::*;

impl App {

    pub(super) fn toggle_mark(&mut self) {
        let id = self.list[self.focus()];
        if self.marks.remove(&id) {
            println!("[unmark] {}", self.name_of(id));
        } else {
            self.marks.insert(id);
            println!("[mark] {}", self.name_of(id));
            self.mark_anchor = Some(id);
        }
        self.redraw();
    }

    /// Ctrl+click: marks the file at this place in the list, or takes its
    /// mark away.
    pub(super) fn mark_one(&mut self, pos: usize) {
        let Some(&id) = self.list.get(pos) else { return };
        if !self.marks.remove(&id) {
            self.marks.insert(id);
            self.mark_anchor = Some(id);
        }
        self.redraw();
    }

    /// Shift+click: marks every file from the one marked last (or the one
    /// on view) to this place in the list.
    pub(super) fn mark_range(&mut self, pos: usize) {
        let from = self.mark_anchor.and_then(|id| self.list.iter().position(|&other| other == id)).unwrap_or(self.focus());
        let (low, high) = (from.min(pos), from.max(pos).min(self.list.len().saturating_sub(1)));
        let before = self.marks.len();
        self.marks.extend(self.list[low..=high].iter().copied());
        self.mark_anchor = self.list.get(pos).copied();
        self.toast(format!("{} marked  ·  {} new", self.marks.len(), self.marks.len() - before));
    }

    /// Copies file names, or full paths, to the clipboard and the terminal.
    /// Copies the target files' paths on one line for a shell, each
    /// folder's files folded together with braces: to be pasted after
    /// `rm` or `ls`.
    pub(super) fn copy_for_shell(&mut self) {
        let paths: Vec<PathBuf> = self.targets().into_iter().map(|id| self.files[id].clone()).collect();
        let line = shellpaths::compress(&paths);
        println!("[copy] {line}");
        // Said as it is, and no more of it than fits a corner.
        let shown = match line.chars().count() {
            0..=70 => line.clone(),
            _ => format!("{}…", line.chars().take(68).collect::<String>()),
        };
        // A scripted test run must not replace the user's clipboard.
        if self.snapshot.is_some() || ops::set_clipboard(&line) {
            self.toast(format!("Copied {shown}"));
        } else {
            self.toast("No clipboard tool (xclip, xsel, wl-copy)");
        }
    }

    pub(super) fn copy_text(&mut self, full_path: bool) {
        let lines: Vec<String> = self
            .targets()
            .into_iter()
            .map(|id| match std::path::absolute(&self.files[id]) {
                Ok(path) if full_path => path.to_string_lossy().into_owned(),
                _ => self.name_of(id),
            })
            .collect();
        for line in &lines {
            println!("[copy] {line}");
        }
        let what = match lines.as_slice() {
            [only] => only.clone(),
            many => format!("{} {}", many.len(), if full_path { "paths" } else { "names" }),
        };
        // A scripted test run must not replace the user's clipboard.
        if self.snapshot.is_some() || ops::set_clipboard(&lines.join("\n")) {
            self.toast(format!("Copied {what}"));
        } else {
            self.toast("No clipboard tool (xclip, xsel, wl-copy)");
        }
    }

    /// Copies or moves the target files into the `-o` folder. Files whose
    /// name is already taken there are left alone.
    pub(super) fn send(&mut self, moving: bool, event_loop: &ActiveEventLoop) {
        // With no `-o` folder there is nowhere agreed on: ask for one.
        match self.options.output_dir.clone() {
            Some(dir) => self.send_to(dir, moving, event_loop),
            None => self.ask_folder(moving),
        }
    }

    /// Copies or moves the target files into `dir`, made if it is not
    /// there. Files whose name is already taken there are left alone.
    pub(super) fn send_to(&mut self, dir: PathBuf, moving: bool, event_loop: &ActiveEventLoop) {
        let how = if moving { "move" } else { "copy" };
        let said = |app: &Self| app.targets().iter().map(|id| format!("{how}: {} -> {}", app.files[*id].display(), dir.display())).collect();
        if self.dry(said, &format!("would {how} {} to {}", self.targets().len(), dir.display())) {
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.toast(format!("Not usable · {error}"));
            return;
        }

        let (mut sent, mut skipped, mut failed) = (Vec::new(), 0, 0);
        for id in self.targets() {
            let from = self.files[id].clone();
            let Some(name) = from.file_name() else { continue };
            let to = dir.join(name);
            if to.exists() {
                skipped += 1;
                continue;
            }
            let result = if moving { ops::move_file(&from, &to) } else { ops::copy_file(&from, &to) };
            match result {
                Ok(()) => {
                    println!("[{}] {} -> {}", if moving { "move" } else { "copy to" }, from.display(), to.display());
                    let pos = self.list.iter().position(|&other| other == id).unwrap_or(0);
                    sent.push(Moved { id, pos, from, to });
                }
                Err(error) => {
                    eprintln!("jog: {}: {error}", from.display());
                    failed += 1;
                }
            }
        }

        let mut summary = format!("{} {} to {}", if moving { "Moved" } else { "Copied" }, sent.len(), dir.display());
        if skipped > 0 {
            summary.push_str(&format!(", {skipped} already there"));
        }
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed"));
        }
        if sent.is_empty() {
            return self.toast(summary);
        }
        self.toast_with(summary, vec![("Undo".into(), ToastLink::Undo)]);
        if moving {
            let removed: Vec<(usize, usize)> = sent.iter().map(|moved| (moved.id, moved.pos)).collect();
            self.drop_from_list(&removed, event_loop);
            self.did(Undo::Moved(sent));
        } else {
            // The marked images have been dealt with; start a fresh selection.
            self.marks.clear();
            self.did(Undo::Created(sent.into_iter().map(|copy| copy.to).collect()));
        }
    }

    /// Takes files that are gone out of the list, given as `(id, position)`.
    /// The view and the grid cursor stay on the images they were on where
    /// those are still there.
    pub(super) fn drop_from_list(&mut self, removed: &[(usize, usize)], event_loop: &ActiveEventLoop) {
        let gone = |id: &usize| removed.iter().any(|(gone_id, _)| gone_id == id);
        // Where a list position ends up once the removed files are gone.
        let shifted = |pos: usize| pos - removed.iter().filter(|(_, at)| *at < pos).count();
        let (pos, cursor) = (shifted(self.pos), self.grid.as_ref().map(|grid| shifted(grid.cursor)));

        self.list.retain(|id| !gone(id));
        self.marks.retain(|id| !gone(id));
        if let Some((_, all)) = &mut self.filter {
            all.retain(|id| !gone(id));
        }
        if !self.refill(event_loop) {
            return;
        }
        let last = self.list.len() - 1;
        self.pos = pos.min(last);
        if let (Some(grid), Some(cursor)) = (&mut self.grid, cursor) {
            grid.cursor = cursor.min(last);
        }
        self.error = None;
        self.reset_view();
        self.request();
        self.refresh();
    }

    /// Which button of the delete confirmation is at a window position:
    /// `true` for Delete, `false` for Cancel.
    pub(super) fn confirm_button_at(&self, x: f64, y: f64) -> Option<bool> {
        let confirm = self.confirm.as_ref()?;
        let on = |[left, top, width, height]: [i32; 4]| {
            (left..left + width).contains(&(x as i32)) && (top..top + height).contains(&(y as i32))
        };
        confirm.drawn.iter().position(|place| on(*place)).map(|button| button == 1)
    }

    /// What a menu item is called. Copying and moving name the folder
    /// they go to, when `-o` has set one.
    pub(super) fn menu_label(&self, item: &MenuItem) -> String {
        let output = self.options.output_dir.as_deref().map(short_dir);
        Self::label_for(item, output.as_deref())
    }

    pub(super) fn label_for(item: &MenuItem, output: Option<&str>) -> String {
        match (item.command, output) {
            (Command::CopyTo, Some(folder)) => format!("Copy to {folder}"),
            (Command::MoveTo, Some(folder)) => format!("Move to {folder}"),
            _ => item.label.to_string(),
        }
    }

    /// Asks before deleting the target files for good.
    pub(super) fn ask_delete(&mut self) {
        let ids = self.targets();
        let question = match ids.as_slice() {
            [only] => format!("Delete {}?", self.name_of(*only)),
            many => format!("Delete {} marked images?", many.len()),
        };
        self.menu = None;
        self.confirm = Some(Confirm { question, yes: false, ids, drawn: [[0; 4]; 2] });
        self.redraw();
    }

    pub(super) fn confirm_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let Some(confirm) = &mut self.confirm else { return };
        match key {
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::Tab) => confirm.yes = !confirm.yes,
            Key::Character("y" | "Y") => confirm.yes = true,
            Key::Character("n" | "N") | Key::Named(NamedKey::Escape) => self.confirm = None,
            Key::Named(NamedKey::Enter) => {}
            _ => return,
        }
        // Enter takes the highlighted answer; y is a yes in one press.
        if matches!(key, Key::Named(NamedKey::Enter) | Key::Character("y" | "Y")) {
            if let Some(Confirm { yes: true, ids, .. }) = self.confirm.take() {
                self.delete(&ids, event_loop);
            }
        }
        self.redraw();
    }

    /// Deletes files from disk. This cannot be undone.
    pub(super) fn delete(&mut self, ids: &[usize], event_loop: &ActiveEventLoop) {
        if self.dry(|app| ids.iter().map(|id| format!("delete: {}", app.files[*id].display())).collect(), &format!("would delete {}", ids.len())) {
            return;
        }
        let (mut removed, mut failed) = (Vec::new(), 0);
        for &id in ids {
            let Some(pos) = self.list.iter().position(|&other| other == id) else { continue };
            match std::fs::remove_file(&self.files[id]) {
                Ok(()) => {
                    println!("[delete] {}", self.files[id].display());
                    removed.push((id, pos));
                }
                Err(error) => {
                    eprintln!("jog: cannot delete {}: {error}", self.files[id].display());
                    failed += 1;
                }
            }
        }
        let mut summary = format!("Deleted {}", removed.len());
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed"));
        }
        self.toast(summary);
        if !removed.is_empty() {
            self.drop_from_list(&removed, event_loop);
        }
    }

    pub(super) fn undo(&mut self, event_loop: &ActiveEventLoop) {
        if !self.undo.is_empty() && self.dry(|_| vec!["undo the last copy, move, rename or convert".to_string()], "would undo") {
            return;
        }
        match self.undo.pop() {
            None => self.toast("Nothing to undo"),
            Some(Undo::Created(paths)) => {
                // Into the trash, where Redo finds them again; one on
                // another filesystem is removed outright.
                let (mut removed, mut kept) = (0, Vec::new());
                for path in &paths {
                    let gone = match ops::trash(path) {
                        Ok(Some(trashed)) => {
                            kept.push(trashed);
                            true
                        }
                        _ => std::fs::remove_file(path).is_ok(),
                    };
                    if gone {
                        println!("[undo] removed {}", path.display());
                        removed += 1;
                    }
                }
                if !kept.is_empty() {
                    self.redo.push(Redo::Created(kept));
                }
                self.toast(format!("Undone  ·  {removed} removed"));
                // Those the list had taken in leave it.
                let listed: Vec<(usize, usize)> = (self.list.iter().enumerate())
                    .filter(|(_, id)| paths.contains(&self.files[**id]))
                    .map(|(pos, id)| (*id, pos))
                    .collect();
                if !listed.is_empty() {
                    self.drop_from_list(&listed, event_loop);
                }
            }
            Some(Undo::Moved(moved)) => {
                let (viewing, total) = (self.id(), moved.len());
                let mut restored = 0;
                let mut again = Vec::new();
                // In their original order, so each lands where it was.
                for item in moved {
                    if ops::move_file(&item.to, &item.from).is_ok() {
                        println!("[undo] {} -> {}", item.to.display(), item.from.display());
                        self.put_back(&item);
                        again.push(item);
                        restored += 1;
                    } else {
                        eprintln!("jog: cannot move {} back", item.to.display());
                    }
                }
                self.pos = self.list.iter().position(|&id| id == viewing).unwrap_or(self.pos);
                if !again.is_empty() {
                    self.redo.push(Redo::Moved(again));
                }
                self.toast(format!("Undone  ·  {restored} of {total} moved back"));
                self.request();
                self.refresh();
            }
            Some(Undo::Trashed(trashed)) => {
                let mut back = Vec::new();
                for (item, from_trash) in &trashed {
                    match ops::untrash(from_trash) {
                        Ok(()) => {
                            println!("[undo] {} -> {}", from_trash.file.display(), item.from.display());
                            self.put_back(item);
                            back.push(item.id);
                        }
                        Err(error) => eprintln!("jog: cannot bring {} back: {error}", item.from.display()),
                    }
                }
                // On the first one brought back.
                if let Some(pos) = back.first().and_then(|id| self.list.iter().position(|other| other == id)) {
                    self.pos = pos;
                    self.error = None;
                    self.reset_view();
                }
                self.toast(format!("Undone  ·  {} restored", back.len()));
                if !back.is_empty() {
                    self.redo.push(Redo::Trashed(back));
                }
                self.request();
                self.refresh();
            }
            Some(Undo::Renamed { id, from, to }) => {
                if ops::move_file(&to, &from).is_ok() {
                    println!("[undo] {} -> {}", to.display(), from.display());
                    self.set_path(id, from.clone());
                    self.redo.push(Redo::Renamed { id, from, to });
                    self.toast(format!("Undone  ·  {}", self.name_of(id)));
                    self.refresh();
                } else {
                    self.toast("Not undone");
                }
            }
        }
    }

    /// Something done that Undo can take back: what was undone before it
    /// can no longer be redone.
    pub(super) fn did(&mut self, undo: Undo) {
        self.undo.push(undo);
        self.redo.clear();
    }

    /// Does again what Undo last took back.
    pub(super) fn redo(&mut self, event_loop: &ActiveEventLoop) {
        match self.redo.pop() {
            None => self.toast("Nothing to redo"),
            Some(Redo::Created(trashed)) => {
                let mut paths = Vec::new();
                for trashed in &trashed {
                    match ops::untrash(trashed) {
                        Ok(()) => paths.push(trashed.from.clone()),
                        Err(error) => eprintln!("jog: cannot bring {} back: {error}", trashed.from.display()),
                    }
                }
                self.adopt_created(&paths);
                self.toast(format!("Redone  ·  {} back", paths.len()));
                if !paths.is_empty() {
                    self.undo.push(Undo::Created(paths));
                }
            }
            Some(Redo::Moved(moved)) => {
                let (mut gone, mut again) = (Vec::new(), Vec::new());
                for item in moved {
                    let Some(pos) = self.list.iter().position(|&id| id == item.id) else { continue };
                    if ops::move_file(&item.from, &item.to).is_ok() {
                        println!("[redo] {} -> {}", item.from.display(), item.to.display());
                        gone.push((item.id, pos));
                        again.push(Moved { pos, ..item });
                    }
                }
                self.toast(format!("Redone  ·  {} moved", again.len()));
                if !gone.is_empty() {
                    self.drop_from_list(&gone, event_loop);
                    self.undo.push(Undo::Moved(again));
                }
            }
            Some(Redo::Renamed { id, from, to }) => {
                if ops::move_file(&from, &to).is_ok() {
                    println!("[redo] {} -> {}", from.display(), to.display());
                    self.set_path(id, to.clone());
                    self.undo.push(Undo::Renamed { id, from, to });
                    self.toast(format!("Redone  ·  {}", self.name_of(id)));
                    self.refresh();
                } else {
                    self.toast("Not redone");
                }
            }
            Some(Redo::Trashed(ids)) => self.trash_ids(ids, true, event_loop),
        }
    }

    /// A file brought back puts its id back in the list where it was, and
    /// among a kept search's matches next to the one it followed.
    pub(super) fn put_back(&mut self, item: &Moved) {
        let at = item.pos.min(self.list.len());
        if let Some((_, all)) = &mut self.filter {
            let before = at.checked_sub(1).map(|before| self.list[before]);
            let index = before.and_then(|before| all.iter().position(|id| *id == before));
            all.insert(index.map_or(0, |index| index + 1), item.id);
        }
        self.list.insert(at, item.id);
    }

    pub(super) fn set_path(&mut self, id: usize, path: PathBuf) {
        self.loader.set_path(id, path.clone());
        self.files[id] = path;
    }

    /// Converts the target files on another thread; `Event::Converted`
    /// reports back.
    pub(super) fn convert(&mut self, format: Format) {
        self.convert_ids(self.targets(), format, self.options.convert);
    }

    /// Converts these files, each turned and mirrored as it is shown.
    pub(super) fn convert_ids(&mut self, ids: Vec<usize>, format: Format, settings: convert::Settings) {
        let paths: Vec<(PathBuf, Transform)> = ids.into_iter().map(|id| (self.files[id].clone(), self.transforms.get(&id).copied().unwrap_or_default())).collect();
        if self.dry(|_| paths.iter().map(|(path, _)| format!("convert to {}: {}", format.name(), path.display())).collect(), &format!("would convert {}", paths.len())) {
            return;
        }
        self.toast(format!("Converting {} → {}  ·  {}…", paths.len(), format.name(), settings.describe(format)));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let mut done = Converted { created: Vec::new(), failed: 0, what: format.name().to_string(), before: 0, after: 0, how: None, error: None, saving: false };
            let (total, began, started) = (paths.len(), crate::transcode::generation(), Instant::now());
            let _batch = crate::transcode::Batch::begin();
            for (index, (path, transform)) in paths.into_iter().enumerate() {
                // Called off: the rest are not begun.
                if crate::transcode::generation() != began {
                    done.error = Some(crate::transcode::CANCELED_WORD.to_string());
                    break;
                }
                // Of several, how far along, as a video's encode says.
                if total > 1 {
                    let words = format!("{} of {total} → {}  ·  {}", index + 1, format.name(), progress_words(index as f64 / total as f64, started));
                    let _ = proxy.send_event(Event::VideoProgress(words));
                }
                match convert::convert(&path, format, &settings, transform) {
                    Ok((to, how)) => {
                        println!("[convert] {} -> {} ({how})", path.display(), to.display());
                        done.before += std::fs::metadata(&path).map_or(0, |meta| meta.len());
                        done.after += std::fs::metadata(&to).map_or(0, |meta| meta.len());
                        done.created.push(to);
                        done.how = Some(how);
                    }
                    Err(error) => {
                        eprintln!("jog: cannot convert {}: {error}", path.display());
                        done.failed += 1;
                        done.error = Some(short_error(&path, &error.to_string()));
                    }
                }
            }
            let _ = proxy.send_event(Event::Converted(done));
        });
    }

    /// Converts the target videos by ffmpeg, one after another on another
    /// thread, saying how far along each is as it goes.
    pub(super) fn convert_videos(&mut self, target: crate::transcode::Target) {
        let settings = self.options.convert.video;
        // Each turned and mirrored as it is shown.
        let paths: Vec<(PathBuf, Transform)> = self
            .targets()
            .into_iter()
            .filter(|id| files::is_video(&self.files[*id]))
            .map(|id| (self.files[id].clone(), self.transforms.get(&id).copied().unwrap_or_default()))
            .collect();
        if paths.is_empty() {
            return self.toast("No videos to convert");
        }
        if let Some(problem) = target.problem(&settings) {
            return self.toast(problem);
        }
        let what = target.describe(&settings);
        if self.dry(|_| paths.iter().map(|(path, _)| format!("convert to {what}: {}", path.display())).collect(), &format!("would convert {}", paths.len())) {
            return;
        }
        self.toast(format!("Converting {} → {what}…", paths.len()));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let total = paths.len();
            let (began, _batch) = (crate::transcode::generation(), crate::transcode::Batch::begin());
            let mut done = Converted { created: Vec::new(), failed: 0, what: what.clone(), before: 0, after: 0, how: None, error: None, saving: false };
            for (index, (path, transform)) in paths.iter().enumerate() {
                // Called off: the rest are not begun.
                if crate::transcode::generation() != began {
                    done.error = Some(crate::transcode::CANCELED_WORD.to_string());
                    break;
                }
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let mut said = None;
                let started = Instant::now();
                let result = crate::transcode::transcode(path, &target, &settings, *transform, |share| {
                    // A word only when the whole percentage moves on.
                    let percent = (share * 100.0) as u32;
                    if said != Some(percent) {
                        said = Some(percent);
                        let of = if total > 1 { format!(" ({} of {total})", index + 1) } else { String::new() };
                        let _ = proxy.send_event(Event::VideoProgress(format!("{name}{of}  ·  {}", progress_words(share, started))));
                    }
                });
                match result {
                    Ok(to) => {
                        println!("[convert] {} -> {}", path.display(), to.display());
                        done.before += std::fs::metadata(path).map_or(0, |meta| meta.len());
                        done.after += std::fs::metadata(&to).map_or(0, |meta| meta.len());
                        done.created.push(to);
                    }
                    Err(error) => {
                        if error == crate::transcode::CANCELED_WORD {
                            done.error = Some(error);
                            continue;
                        }
                        eprintln!("jog: cannot convert {}: {error}", path.display());
                        done.failed += 1;
                        done.error = Some(short_error(path, &error));
                    }
                }
            }
            let _ = proxy.send_event(Event::Converted(done));
        });
    }

    pub(super) fn converted(&mut self, done: Converted) {
        let canceled = done.error.as_deref() == Some(crate::transcode::CANCELED_WORD);
        if canceled && done.created.is_empty() {
            return self.toast("Canceled");
        }
        // Nothing made: why, rather than a count of nothing.
        if done.created.is_empty() {
            let why = done.error.unwrap_or_else(|| "see the terminal".to_string());
            return self.toast(format!("{} · {why}", if done.saving { "Not saved" } else { "Not converted" }));
        }
        // What was made joins the list beside what it was made from, and
        // the toast goes to it on a click.
        self.adopt_created(&done.created);
        let links = vec![("Show".into(), ToastLink::Show(done.created[0].clone())), ("Undo".into(), ToastLink::Undo)];
        // A section saved: what and how large.
        if done.saving {
            let made = &done.created[0];
            let size = std::fs::metadata(made).map_or(0, |meta| meta.len());
            self.toast_with(format!("{}  ·  {}", made.file_name().unwrap_or_default().to_string_lossy(), meta::human_size(size)), links);
            self.did(Undo::Created(done.created));
            return;
        }
        let mut summary = match done.created.as_slice() {
            [one] => one.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            all => format!("Converted {} to {}", all.len(), done.what),
        };
        if done.before > 0 {
            let change = (done.after as f64 / done.before as f64 - 1.0) * 100.0;
            summary.push_str(&format!("  ·  {} → {}  {change:+.0}%", meta::human_size(done.before), meta::human_size(done.after)));
        }
        // A JPEG repacked rather than encoded says so.
        if done.how.is_some_and(|how| how.contains("repacked")) {
            summary.push_str("  ·  lossless");
        }
        if done.failed > 0 {
            summary.push_str(&format!("  ·  {} failed", done.failed));
        }
        if canceled {
            summary.push_str("  ·  the rest canceled");
        }
        self.toast_with(summary, links);
        self.did(Undo::Created(done.created));
    }

    /// A file jog has just made, a frame or a copy: it joins the list, is
    /// undone by Undo, and the toast that says so goes to it on a click.
    pub(super) fn made(&mut self, path: PathBuf, said: String) {
        self.adopt_created(std::slice::from_ref(&path));
        self.toast_with(said, vec![("Show".into(), ToastLink::Show(path.clone())), ("Undo".into(), ToastLink::Undo)]);
        self.did(Undo::Created(vec![path]));
    }

    /// Files just made, by a convert or a saved section, put in the list
    /// right after the file each was made from: the one in the same folder
    /// whose name starts its name.
    pub(super) fn adopt_created(&mut self, created: &[PathBuf]) {
        let total = self.filter.as_ref().map_or(&self.list, |(_, all)| all);
        let mut listed: Vec<PathBuf> = total.iter().map(|id| self.files[*id].clone()).collect();
        for made in created {
            if listed.contains(made) || !made.exists() {
                continue;
            }
            let stem = made.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let source = listed.iter().rposition(|path| {
                path.parent() == made.parent() && path.file_stem().is_some_and(|own| stem.starts_with(&*own.to_string_lossy()))
            });
            let at = source.map_or(listed.len(), |at| at + 1);
            listed.insert(at, made.clone());
        }
        self.take_listing_as(listed, Taking::Adopting);
    }

    // ---- Rename --------------------------------------------------------

    pub(super) fn start_rename(&mut self) {
        let id = self.list[self.focus()];
        let text: Vec<char> = self.name_of(id).chars().collect();
        // Start typing before the extension.
        let cursor = text.iter().rposition(|c| *c == '.').filter(|at| *at > 0).unwrap_or(text.len());
        self.prompt = Some(Prompt { id, kind: PromptKind::Rename, field: TextField::new(text, cursor) });
        self.menu = None;
        self.redraw();
    }

    /// Opens a box to type the folder to copy or move the target files
    /// into, starting from the one last typed, or the `-o` folder.
    pub(super) fn ask_folder(&mut self, moving: bool) {
        let known = self.last_folder.clone().or_else(|| self.options.output_dir.as_deref().map(short_dir));
        let text: Vec<char> = known.unwrap_or_default().chars().collect();
        let id = self.list[self.focus()];
        self.prompt = Some(Prompt { id, kind: PromptKind::SendTo { moving }, field: TextField::new(text.clone(), text.len()) });
        self.menu = None;
        self.redraw();
    }

    pub(super) fn prompt_key(&mut self, key: Key<&str>, event_loop: &ActiveEventLoop) {
        let Some(prompt) = &mut self.prompt else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.prompt = None,
            // Tab finishes a folder's name, from what is typed before the
            // cursor: as far as the folders it could be agree.
            Key::Named(NamedKey::Tab) => self.complete_prompt(),
            Key::Named(NamedKey::Enter) => match prompt.kind {
                PromptKind::Rename => self.finish_rename(),
                PromptKind::SaveWorkspace => {
                    let typed = prompt.field.string().trim().to_string();
                    self.prompt = None;
                    self.save_space(&typed);
                }
                PromptKind::FitSize => {
                    let typed = prompt.field.string();
                    self.prompt = None;
                    self.fit_size(&typed);
                }
                PromptKind::SectionName { start, end } => {
                    let typed = prompt.field.string();
                    self.prompt = None;
                    self.name_section(start, end, &typed);
                }
                PromptKind::SendTo { moving } => {
                    let typed = prompt.field.string().trim().to_string();
                    self.prompt = None;
                    if !typed.is_empty() {
                        self.last_folder = Some(typed.clone());
                        // `@name` is the folder of that workspace.
                        let folder = match typed.strip_prefix('@') {
                            Some(name) => self.spaces.iter().find(|space| space.name == name).and_then(|space| space.paths.first().cloned()),
                            None => Some(crate::options::expand_home(&typed)),
                        };
                        match folder {
                            Some(folder) => self.send_to(folder, moving, event_loop),
                            None => self.toast(format!("No workspace @{}", &typed[1..])),
                        }
                    }
                }
            },
            key => drop(prompt.field.key(key, self.modifiers)),
        }
        self.redraw();
    }

    /// Tab in a box for a path: finishes a folder's name from what is
    /// typed before the cursor, as far as the folders it could be agree.
    pub(super) fn complete_prompt(&mut self) {
        let Some(prompt) = &self.prompt else { return };
        let base = match prompt.kind {
            PromptKind::Rename => self.files[prompt.id].parent().map(Path::to_path_buf).unwrap_or_default(),
            PromptKind::SendTo { .. } => PathBuf::new(),
            PromptKind::SaveWorkspace | PromptKind::SectionName { .. } | PromptKind::FitSize => return,
        };
        let before: String = prompt.field.text[..prompt.field.cursor].iter().collect();
        if before.starts_with('@') {
            return;
        }
        let Some(done) = complete_folder(&base, &before) else { return self.toast("No match") };
        let Some(prompt) = &mut self.prompt else { return };
        let after = prompt.field.text.split_off(prompt.field.cursor);
        prompt.field.cursor = done.chars().count();
        prompt.field.text = done.chars().chain(after).collect();
        prompt.field.anchor = None;
    }

    /// What Enter would do with what is typed so far, when it is more than
    /// a new name: where a file would go, and whether that folder is to
    /// be made or a file of that name is there.
    pub(super) fn prompt_hint(&self, prompt: &Prompt) -> Option<String> {
        let typed = prompt.field.string();
        let typed = typed.trim();
        match prompt.kind {
            PromptKind::Rename => {
                let from = &self.files[prompt.id];
                let to = ops::rename_target(from, typed)?;
                let here = from.parent().and_then(|dir| dir.canonicalize().ok());
                let dir = to.parent()?;
                let name = to.file_name()?.to_string_lossy();
                let there = to.exists() && to != from.canonicalize().unwrap_or_default();
                if there {
                    return Some(format!("{name} exists"));
                }
                if Some(dir) == here.as_deref() {
                    return None;
                }
                let new = if dir.exists() { "" } else { "  (new folder)" };
                Some(format!("→ {}/{name}{new}", short_dir(dir)))
            }
            PromptKind::SendTo { .. } if !typed.is_empty() && !typed.starts_with('@') => {
                let dir = crate::options::expand_home(typed);
                let shown = short_dir(&dir.canonicalize().unwrap_or_else(|_| dir.clone()));
                Some(if dir.is_dir() { format!("→ {shown}") } else { format!("→ {shown}  (new folder)") })
            }
            _ => None,
        }
    }

    /// With `--dry-run`, says what would have been done, a line for each
    /// thing in the terminal and a word on screen, and returns true:
    /// whoever asked then does none of it.
    pub(super) fn dry(&mut self, lines: impl FnOnce(&Self) -> Vec<String>, summary: &str) -> bool {
        if !self.options.dry_run {
            return false;
        }
        for line in lines(self) {
            println!("[dry run] {line}");
        }
        self.toast(format!("Dry run · {summary}"));
        true
    }

    /// Saves the frame a video is showing as a picture, at the video's own
    /// size and without subtitles: named after the video and how far in it
    /// is, in the folder set for frames, or the one files are copied to,
    /// or else beside the video.
    pub(super) fn save_frame(&mut self) {
        // A frame kept from before a pause is on show: the main player is
        // put on it, and given a moment to get there, so that the frame
        // saved is that one, at the video's own size.
        let kept_at = self.back_shown().map(|(at, _)| at);
        if let Some(at) = kept_at {
            self.leave_back();
            let asked = Instant::now();
            while let Some(player) = self.player.as_ref().filter(|_| asked.elapsed() < Duration::from_secs(3)) {
                if player.number_now("time-pos").is_some_and(|now| (now - at).abs() < 0.008) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // Named for the frame it is: the one kept, by its own time, the
        // player's word for where it is lagging until its events are read.
        let Some((position, _)) = self.video_time() else { return };
        let position = kept_at.unwrap_or(position);
        let path = self.path().to_path_buf();
        let stem = path.file_stem().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
        let whole = position as u64;
        let at = format!("{:02}-{:02}-{:02}.{:03}", whole / 3600, whole / 60 % 60, whole % 60, (position.fract() * 1000.0) as u64);
        let dir = self.options.frame_dir.clone().or_else(|| self.options.output_dir.clone());
        let dir = dir.or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let to = dir.join(format!("{stem}_{at}.{}", self.options.frame_format));
        if self.dry(|_| vec![format!("save the frame: {}", to.display())], "would save the frame") {
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&dir) {
            return self.toast(format!("Not usable · {error}"));
        }
        // The player writes it, and says nothing of how that went.
        let Some(player) = &self.player else { return };
        player.command(&[b"screenshot-to-file", to.as_os_str().as_encoded_bytes(), b"video"]);
        match to.exists() {
            true => {
                println!("[frame] {}", to.display());
                let said = format!("{}  {}", commands::icons::CAMERA, to.file_name().unwrap_or_default().to_string_lossy());
                self.made(to, said);
            }
            false => self.toast(format!("Not saved · {}", to.display())),
        }
    }

}
