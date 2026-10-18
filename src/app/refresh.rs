//! Refreshing the list, and watching its folders for files coming and going.

use super::*;

impl App {

    /// Starts or stops watching the directories. A thread lists them once
    /// a second and reports when what it finds has changed.
    pub fn set_watch(&mut self, on: bool) {
        if let Some(watching) = self.watch.take() {
            watching.store(false, atomic::Ordering::Relaxed);
        }
        if on {
            let watching = Arc::new(AtomicBool::new(true));
            self.watch = Some(watching.clone());
            let (paths, proxy) = (self.options.paths.clone(), self.proxy.clone());
            let listing = files::Listing {
                recursive: self.options.listing.recursive,
                filelist: self.options.listing.filelist.clone(),
                ..files::Listing::default()
            };
            std::thread::spawn(move || {
                let mut known = files::relist(&paths, &listing);
                while watching.load(atomic::Ordering::Relaxed) {
                    std::thread::sleep(WATCH_EVERY);
                    let listed = files::relist(&paths, &listing);
                    if listed != known && watching.load(atomic::Ordering::Relaxed) {
                        known.clone_from(&listed);
                        if proxy.send_event(Event::Listed(listed)).is_err() {
                            return;
                        }
                    }
                }
            });
            // Whatever changed before the watching began.
            let listed = files::relist(&self.options.paths, &self.options.listing);
            self.take_listing(listed, true);
        }
        self.toast(if on { "Watching" } else { "Not watching" });
    }

    /// Brings the list up to date with `listed`, the files now in the
    /// directories: those not seen before join it where the order puts
    /// them, and those no longer on disk leave it. Marks, the place in the
    /// list and a kept search all stay. When watching, a file arriving
    /// just past the end being viewed is gone to.
    pub(super) fn take_listing(&mut self, listed: Vec<PathBuf>, watching: bool) {
        self.take_listing_as(listed, if watching { Taking::Watching } else { Taking::Refreshing });
    }

    pub(super) fn take_listing_as(&mut self, listed: Vec<PathBuf>, taking: Taking) {
        let watching = taking != Taking::Refreshing;
        // Not under an open search box, whose own lists would go stale.
        if self.palette.is_some() {
            return;
        }
        // What a triage decided is looked up again for the files as they
        // are numbered now.
        self.verdicts.borrow_mut().clear();
        let seen: HashSet<&Path> = self.files.iter().map(PathBuf::as_path).collect();
        let fresh: Vec<PathBuf> = listed.iter().filter(|path| !seen.contains(path.as_path())).cloned().collect();
        let exists = |id: &usize| self.files[*id].exists();
        let total = self.filter.as_ref().map_or(&self.list, |(_, all)| all);
        let gone: HashSet<usize> = total.iter().copied().filter(|id| !exists(id)).collect();
        // With every file gone there would be nothing to show: leave them.
        let gone = if gone.len() == total.len() && fresh.is_empty() { HashSet::new() } else { gone };
        if fresh.is_empty() && gone.is_empty() {
            if !watching {
                self.toast("Nothing new");
            }
            return;
        }

        let (viewed, focus) = (self.id(), self.list[self.focus()]);
        let (was_first, was_last) = (self.pos == 0, self.pos + 1 == self.list.len());
        let first_new = self.files.len();
        let query = self.filter.as_ref().map(|(typed, _)| Query::parse(typed));
        for path in fresh.iter().cloned() {
            let id = self.files.len();
            self.loader.add_file(path.clone());
            self.files.push(path);
            // Among a kept search's matches only if it matches too.
            let matches = query.as_ref().is_none_or(|query| self.score(query, id, false).is_some());
            if let Some((_, all)) = &mut self.filter {
                all.push(id);
            }
            if matches {
                self.list.push(id);
            }
        }
        self.sizes.clear();

        // Leave what has gone out, unless that would leave nothing.
        let keep = |list: &mut Vec<usize>| {
            if list.iter().any(|id| !gone.contains(id)) {
                list.retain(|id| !gone.contains(id));
            }
        };
        keep(&mut self.list);
        if let Some((_, all)) = &mut self.filter {
            keep(all);
        }
        self.marks.retain(|id| !gone.contains(id));

        // Put the newcomers where the order has them. A shuffled list has
        // no such place, so there they stay at the end.
        let (sort, reversed, shuffled) = (self.sort, self.reversed, self.shuffled);
        let place: HashMap<&Path, usize> = listed.iter().enumerate().map(|(at, path)| (path.as_path(), at)).collect();
        let order = |ids: &mut Vec<usize>, files: &[PathBuf]| {
            match sort {
                _ if shuffled => return,
                Some(sort) => files::sort_ids(ids, files, sort),
                None => ids.sort_by_key(|id| place.get(files[*id].as_path()).copied().unwrap_or(usize::MAX)),
            }
            if reversed {
                ids.reverse();
            }
        };
        order(&mut self.list, &self.files);
        if let Some((_, all)) = &mut self.filter {
            order(all, &self.files);
        }

        // Stay on the file that was on view, or where it was if it went.
        let at = |id: usize, fallback: usize| self.list.iter().position(|&other| other == id).unwrap_or(fallback);
        let last = self.list.len() - 1;
        let (mut pos, cursor) = (at(viewed, self.pos.min(last)), at(focus, self.focus().min(last)));
        let is_new = |pos: usize| self.list[pos] >= first_new;
        if taking == Taking::Watching && self.grid.is_none() {
            if was_last && is_new(last) {
                pos = last;
            } else if was_first && is_new(0) && sort == Some(Sort::Newest) && !reversed {
                pos = 0;
            }
        }
        let moved = self.list[pos] != viewed;
        self.pos = pos;
        if let Some(grid) = &mut self.grid {
            grid.cursor = cursor;
        }
        if moved {
            self.error = None;
            self.reset_view();
        }

        let mut parts = Vec::new();
        if !fresh.is_empty() {
            parts.push(format!("{} new", fresh.len()));
        }
        if !gone.is_empty() {
            parts.push(format!("{} gone", gone.len()));
        }
        if self.options.verbose {
            fresh.iter().for_each(|path| eprintln!("jog: new file {}", path.display()));
            eprintln!("jog: refreshed: {}, {} files now", parts.join(", "), self.total());
        }
        if taking != Taking::Adopting {
            self.toast(format!("{}  ·  {}", if watching { "Watch" } else { "Refreshed" }, parts.join("  ·  ")));
        }
        self.request();
        self.refresh();
        self.redraw();
    }

}
