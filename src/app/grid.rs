//! The grid of thumbnails.

use super::*;

impl App {

    /// How the grid is laid out in a window of this size.
    pub(super) fn grid_layout(&self, window: (u32, u32)) -> GridLayout {
        GridLayout::for_window(window, self.grid_columns)
    }

    /// Shows more or fewer pictures across the grid, making each smaller
    /// or larger.
    pub(super) fn zoom_grid(&mut self, larger: bool) {
        let layout = self.grid_layout(self.window_size());
        let columns = (layout.columns + if larger { -1 } else { 1 }).clamp(GRID_COLUMNS.0, GRID_COLUMNS.1);
        self.grid_columns = Some(columns);
        // Start afresh around the cursor: the rows are not what they were.
        let layout = self.grid_layout(self.window_size());
        if let Some(grid) = &mut self.grid {
            *grid = Grid { moved: grid.moved, ..Grid::at(grid.cursor, &layout) };
        }
        self.toast(format!("{columns} across"));
        self.request();
        self.redraw();
    }

    pub(super) fn toggle_grid(&mut self) {
        let window = self.window_size();
        let layout = self.grid_layout(window);
        let id = self.id();
        // Out of the grid, the picture on view grows from its cell, or from
        // where it was playing there; into the grid it shrinks to its
        // cell, unless it is a video, which plays on in its cell instead.
        let corner = self.pip_place(window).or_else(|| self.pip_small(window));
        // A video playing in its cell grows from the frame it had got to.
        let live = self.pip_live.take().filter(|_| self.grid.is_some()).map(Arc::new);
        let stays = self.grid.is_none() && self.video.is_some();
        self.grow = match &self.grid {
            Some(_) => corner.or_else(|| self.grid_cell(self.pos, &layout, window)).map(|cell| Grow { id, cell, since: Instant::now(), opening: true, picture: live.clone() }),
            // A video shrinks to its cell as the frame it has got to, and
            // plays on there.
            None if stays => Some(Grow { id, cell: [0; 4], since: Instant::now(), opening: false, picture: self.video_frame(window) }),
            None => Some(Grow { id, cell: [0; 4], since: Instant::now(), opening: false, picture: None }),
        };
        self.pip = stays;
        self.grid = match self.grid {
            Some(_) => None,
            None => Some(Grid::at(self.pos, &layout)),
        };
        self.ring_snap.set(true);
        self.menu = None;
        self.drag = None;
        self.request();
        self.sync_video();
        self.apply_video_view();
        self.redraw();
    }

    /// Where the picture of the file at `pos` is in the grid, if it is on
    /// the page showing.
    pub(super) fn grid_cell(&self, pos: usize, layout: &GridLayout, window: (u32, u32)) -> Option<[i32; 4]> {
        let area = self.grid_area(layout, pos, window.1);
        let place = match self.loader.thumb(self.list[pos]) {
            Some(thumb) => Self::grid_picture(area, (thumb.width, thumb.height), false),
            None => area,
        };
        (place[0] >= 0 && place[0] + place[2] <= window.0 as i32).then_some(place)
    }

    /// The frame the video on view is showing, as it is in the window: to
    /// shrink into its cell of the grid.
    pub(super) fn video_frame(&self, window: (u32, u32)) -> Option<Arc<Frame>> {
        let video = self.video.as_ref().filter(|video| video.showing)?;
        let native = self.size_of(video.id)?;
        // Only the picture, without the bars beside it.
        let place = self.layout_of(native);
        let (w, h) = (place.width.min(window.0).max(1), place.height.min(window.1).max(1));
        let mut pixels = vec![0; w as usize * h as usize];
        // The frame kept for the window is not the one the player has now.
        self.video_kept.set(None);
        self.player.as_ref()?.render(&mut pixels, w, h).then(|| Arc::new(Frame::still(w, h, native, pixels)))
    }

    /// The picture that grows out of a cell of the grid or shrinks into
    /// one: for a video the frame its cell shows, since the frame it opens
    /// on is as often as not black, and otherwise the picture as it is on
    /// view, or failing that the one from its cell.
    pub(super) fn grow_picture(&self, id: usize) -> Option<Arc<Frame>> {
        let shown = self.shown.as_ref().filter(|(shown, _)| *shown == id && !files::is_video(&self.files[id]));
        shown.map(|(_, frame)| frame.clone()).or_else(|| self.loader.thumb(id))
    }

    /// The picture growing out of its cell, and where it has got to on its
    /// way to its place in the window; nothing once it is there.
    pub(super) fn growing(&mut self) -> Option<(Arc<Frame>, [i32; 4])> {
        let grow = self.grow.as_ref().filter(|grow| grow.opening && self.options.animations)?;
        let picture = grow.picture.clone().or_else(|| self.grow_picture(grow.id)).filter(|_| grow.since.elapsed() < GROW && grow.id == self.id());
        let Some(picture) = picture else {
            self.grow = None;
            return None;
        };
        let full = self.layout_of(picture.native);
        let full = [full.x, full.y, full.width as i32, full.height as i32];
        let part = Glide { from: 0.0, to: 1.0, since: grow.since }.part(GROW);
        self.animating.set(true);
        Some((picture, std::array::from_fn(|side| (f64::from(grow.cell[side]) + f64::from(full[side] - grow.cell[side]) * part).round() as i32)))
    }

    /// Draws the picture that was on view shrinking into its cell, over
    /// the grid that has just come up.
    pub(super) fn draw_shrinking(&mut self, canvas: &mut Canvas, layout: &GridLayout) {
        let Some(grow) = self.grow.as_ref().filter(|grow| !grow.opening && self.options.animations) else { return };
        let window = (canvas.width as u32, canvas.height as u32);
        let pos = self.list.iter().position(|id| *id == grow.id);
        let frame = grow.picture.clone().or_else(|| self.grow_picture(grow.id));
        let (Some(pos), Some(frame), true) = (pos, frame, grow.since.elapsed() < GROW) else {
            self.grow = None;
            return;
        };
        let Some(cell) = self.grid_cell(pos, layout, window) else { return };
        let full = self.layout_of(frame.native);
        let full = [full.x, full.y, full.width as i32, full.height as i32];
        let part = Glide { from: 0.0, to: 1.0, since: grow.since }.part(GROW);
        let [x, y, w, h]: [i32; 4] = std::array::from_fn(|side| (f64::from(full[side]) + f64::from(cell[side] - full[side]) * part).round() as i32);
        canvas.image_fast(&frame, x, y, w, h);
        self.animating.set(true);
    }

    /// Moves the grid cursor by a number of images, stopping at either end
    /// of the list. The page on show follows the cursor.
    pub(super) fn move_cursor(&mut self, by: i64) {
        let last = self.list.len() as i64 - 1;
        let layout = self.grid_layout(self.window_size());
        let Some(grid) = &mut self.grid else { return };
        grid.cursor = (grid.cursor as i64).saturating_add(by).clamp(0, last) as usize;
        grid.moved = true;
        // At once, so that what happens next finds the right page on show.
        grid.follow(&layout);
        self.request();
        self.redraw();
    }

    /// Puts the grid cursor on the thumbnail under the pointer, if any.
    pub(super) fn point_cursor(&mut self) -> bool {
        let layout = self.grid_layout(self.window_size());
        let (len, (x, y)) = (self.list.len(), self.mouse);
        let Some(grid) = &mut self.grid else { return false };
        // Where the grid is sliding to, not where it is: a cell picked
        // out mid-slide must not pull the grid back.
        let Some(pos) = layout.cell_at(x, y, grid.target).filter(|pos| *pos < len) else {
            return false;
        };
        if pos != grid.cursor {
            (grid.cursor, grid.moved) = (pos, true);
            self.redraw();
        }
        true
    }

    /// Leaves the grid to view the image under the cursor.
    pub(super) fn open_from_grid(&mut self) {
        // The picture under the cursor grows out of its cell.
        let window = self.window_size();
        let layout = self.grid_layout(window);
        if let Some(cursor) = self.grid.as_ref().map(|grid| grid.cursor) {
            let id = self.list[cursor];
            self.grow = self.grid_cell(cursor, &layout, window).map(|cell| Grow { id, cell, since: Instant::now(), opening: true, picture: None });
        }
        let Some(grid) = self.grid.take() else { return };
        self.pip = false;
        self.go_to(grid.cursor, grid.cursor >= self.pos);
        self.request();
        self.sync_video();
        self.apply_video_view();
        self.redraw();
    }

    /// Moves the grid cursor one cell. Sideways the rows run on from page
    /// to page: going right from the end of a row lands at the start of
    /// the same row on the next page, which slides in, and left likewise.
    /// Up and down stay within the page.
    pub(super) fn step_cursor(&mut self, dx: i64, dy: i64) {
        let layout = self.grid_layout(self.window_size());
        let Some(grid) = &self.grid else { return };
        let (columns, per_page) = (i64::from(layout.columns), layout.per_page() as i64);
        let (cursor, last) = (grid.cursor as i64, self.list.len() as i64 - 1);
        let column = cursor % per_page % columns;
        let mut target = cursor + dx + dy * columns;
        if dx > 0 && column == columns - 1 {
            // Nothing beyond the last page; a short one may not reach
            // this row, in which case its last picture is taken.
            if (cursor / per_page + 1) * per_page > last {
                return;
            }
            target = (cursor + per_page - (columns - 1)).min(last);
        } else if dx < 0 && column == 0 {
            target = cursor - per_page + (columns - 1);
        }
        let same_page = target.div_euclid(per_page) == cursor / per_page;
        if (0..=last).contains(&target) && (dy == 0 || same_page) {
            self.move_cursor(target - cursor);
        }
    }

    /// Shuffles the viewing order. Where the list can be seen, in the grid,
    /// the filmstrip or the file list, the file in hand stays where it is
    /// and everything before and after it changes. With only the one file
    /// to be seen, another takes its place and is shown.
    pub(super) fn shuffle(&mut self) {
        (self.sort, self.reversed, self.shuffled) = (None, false, true);
        let held = self.list[self.focus()];
        let stays = self.grid.is_some() || self.strip_visible() || self.show_list;
        let pos = self.pos;
        let mut rng = std::mem::replace(&mut self.rng, Rng::from_clock());
        self.reorder(|ids, _| {
            let place = ids.iter().position(|id| *id == held);
            rng.shuffle(ids);
            let (Some(place), Some(at)) = (place, ids.iter().position(|id| *id == held)) else { return };
            if stays {
                // Back to the place it had, whatever landed there going
                // where the shuffle had put it.
                ids.swap(place, at);
            } else if place == at {
                // Out of the place it had, so that another has it.
                let other = (place + 1) % ids.len();
                ids.swap(place, other);
            }
        });
        self.rng = rng;
        if !stays {
            // `reorder` followed the file; this goes back to its place.
            self.go_to(pos, true);
        }
    }

    /// Puts the list in a different order, and with it the files a kept
    /// search is hiding. The file on view stays on view, and the grid's
    /// cursor stays on its file.
    pub(super) fn reorder(&mut self, mut order: impl FnMut(&mut Vec<usize>, &[PathBuf])) {
        let (viewed, focus) = (self.id(), self.list[self.focus()]);
        order(&mut self.list, &self.files);
        if let Some((_, all)) = &mut self.filter {
            order(all, &self.files);
        }
        let at = |id: usize| self.list.iter().position(|&other| other == id).unwrap_or(0);
        let (viewed, focus) = (at(viewed), at(focus));
        self.pos = viewed;
        if let Some(grid) = &mut self.grid {
            grid.cursor = focus;
        }
        self.request();
        self.refresh();
        self.redraw();
    }

}
