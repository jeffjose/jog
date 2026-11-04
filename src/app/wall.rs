//! The wall: the gallery packed edge to edge, without names, every video on
//! it playing at once. A page is a screenful of columns, each stretched to
//! the window's height. It turns as a wave from the top left corner: the
//! tiles lift away and the next page's rise into their place.

use super::*;

/// Between tiles.
const GAP: i32 = 3;
const RADIUS: i32 = 3;
/// How long a page stays before it turns by itself.
pub(super) const WALL_HOLD: Duration = Duration::from_secs(8);
/// The turn, lift and rise: a tile leaves over half of `SPAN`, rising a
/// little as it fades, and the next arrives over 60% of it from a little
/// below. Each waits `STAGGER` for every step it is from the top left
/// corner, and the arrivals begin with 40% of the leaving still to go.
const SPAN: f64 = 0.55;
const STAGGER: f64 = 0.020;
const OVERLAP: f64 = 0.4;
/// A shape taken for a picture whose size is not known yet.
const UNKNOWN_RATIO: f64 = 1.0;
/// An off-screen run is silent: the wall turns no sound on in one.
static SILENT: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("JOG_SNAPSHOT").is_some());

pub(super) struct Wall {
    /// The list position of the page's first tile.
    pub(super) start: usize,
    /// Where the pages gone forward through began, to go back to them.
    back: Vec<usize>,
    /// How many columns, once changed by hand.
    columns: Option<i32>,
    /// Whether the pages turn by themselves, and when the next one does.
    pub(super) auto: bool,
    pub(super) next_at: Option<Instant>,
    turn: Option<Turn>,
    /// One player for each video on the page, up to what the cores allow.
    players: Vec<TilePlayer>,
    /// The player whose sound is on.
    heard: Option<usize>,
    /// When `--debug` last said how the players keep up.
    told: Instant,
}

/// A page turning: the tiles of the page leaving, from when, and which way.
struct Turn {
    tiles: Vec<Tile>,
    since: Instant,
    forward: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tile {
    pub(super) pos: usize,
    pub(super) place: [i32; 4],
    /// How far into the wave from the top left corner: column plus row.
    step: u32,
}

struct TilePlayer {
    trailers: Trailers,
    id: Option<usize>,
    frame: Option<Frame>,
    /// Frames taken since the last `--debug` line about the wall.
    frames: u32,
}

impl Wall {
    pub(super) fn at(start: usize) -> Wall {
        Wall { start, back: Vec::new(), columns: kept_columns(), auto: false, next_at: None, turn: None, players: Vec::new(), heard: None, told: Instant::now() }
    }
}

/// How many columns the wall was last set to, kept between runs in the
/// state folder; read from there once, the first time it is wanted.
static COLUMNS: std::sync::Mutex<Option<Option<i32>>> = std::sync::Mutex::new(None);

fn columns_file() -> Option<PathBuf> {
    crate::positions::state_dir().map(|dir| dir.join("wall-columns"))
}

fn kept_columns() -> Option<i32> {
    let mut kept = COLUMNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *kept.get_or_insert_with(|| {
        let text = std::fs::read_to_string(columns_file()?).ok()?;
        text.trim().parse().ok().filter(|columns| (2..=16).contains(columns))
    })
}

/// Keeps the columns for the next wall and the next run; the file is
/// written on a thread of its own.
fn keep_columns(columns: i32) {
    *COLUMNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Some(columns));
    std::thread::spawn(move || {
        let Some(file) = columns_file() else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, format!("{columns}\n"));
    });
}

/// Packs pictures of these shapes, each its width over its height, into
/// `columns` columns filling `size`. Each goes to the shortest column; a
/// column is done once one more would overshoot the bottom by more than
/// stopping leaves it short, and a done column is stretched to the full
/// height, its tiles cropped to fit. As many are taken as fill the page,
/// each given its place and its step into the wave, in order.
pub(super) fn pack(ratios: impl Iterator<Item = f64>, size: (i32, i32), columns: i32) -> Vec<([i32; 4], u32)> {
    struct Column {
        items: Vec<(usize, f64)>,
        height: f64,
        done: bool,
    }
    let (width, height) = (size.0.max(1), f64::from(size.1.max(1)));
    let columns = columns.max(1);
    let column_w = f64::from(width - GAP * (columns - 1)).max(1.0) / f64::from(columns);
    let mut cols: Vec<Column> = (0..columns).map(|_| Column { items: Vec::new(), height: 0.0, done: false }).collect();
    let mut ratios = ratios.peekable();
    let mut taken = 0;
    while let Some(&ratio) = ratios.peek() {
        let shortest = (0..cols.len()).filter(|&c| !cols[c].done).min_by(|&a, &b| cols[a].height.total_cmp(&cols[b].height));
        let Some(c) = shortest else { break };
        let col = &mut cols[c];
        let tall = column_w / ratio.clamp(0.05, 20.0);
        let top = col.height + if col.items.is_empty() { 0.0 } else { f64::from(GAP) };
        let over = top + tall - height;
        if over <= 0.0 || over < height - col.height || col.items.is_empty() {
            col.items.push((taken, tall));
            col.height = top + tall;
            col.done = over > 0.0 || col.height >= height * 0.97;
            taken += 1;
            ratios.next();
        } else {
            // It goes to another column; this one is as full as it gets.
            col.done = true;
        }
    }
    let mut places = vec![([0; 4], 0); taken];
    for (c, col) in cols.iter().enumerate() {
        let left = (c as f64 * (column_w + f64::from(GAP))).round() as i32;
        let right = ((c + 1) as f64 * (column_w + f64::from(GAP)) - f64::from(GAP)).round() as i32;
        let gaps = f64::from(GAP) * col.items.len().saturating_sub(1) as f64;
        let natural: f64 = col.items.iter().map(|(_, tall)| tall).sum();
        // The last page may not fill its columns; they are left as they are.
        let scale = if col.done { ((height - gaps) / natural.max(1.0)).max(0.01) } else { 1.0 };
        let mut y = 0.0_f64;
        for (row, &(index, tall)) in col.items.iter().enumerate() {
            let (top, bottom) = (y.round() as i32, (y + tall * scale).round() as i32);
            places[index] = ([left, top, right - left, (bottom - top).max(1)], (c + row) as u32);
            y += tall * scale + f64::from(GAP);
        }
    }
    places
}

fn ease_in(t: f64) -> f64 {
    t * t * t
}

fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// When, from its start, a turn has the next page's tiles begin to arrive,
/// and when it is over.
fn turn_times(leaving: &[Tile], arriving: &[Tile]) -> (f64, f64) {
    let last = |tiles: &[Tile]| f64::from(tiles.iter().map(|tile| tile.step).max().unwrap_or(0));
    let gone = SPAN * 0.5 + last(leaving) * STAGGER;
    let arrive = gone * (1.0 - OVERLAP);
    (arrive, arrive + SPAN * 0.6 + last(arriving) * STAGGER)
}

impl App {
    pub(super) fn wall(&self) -> Option<&Wall> {
        self.grid.as_ref()?.wall.as_ref()
    }

    fn wall_mut(&mut self) -> Option<&mut Wall> {
        self.grid.as_mut()?.wall.as_mut()
    }

    fn wall_columns(&self, wall: &Wall, window: (u32, u32)) -> i32 {
        wall.columns.unwrap_or((window.0 as i32 / 260).clamp(3, 12))
    }

    /// A file's shape, width over height, from its thumbnail or its size.
    fn wall_ratio(&self, id: usize) -> f64 {
        let size = self.loader.thumb(id).map(|thumb| (thumb.width, thumb.height)).or_else(|| self.size_of(id));
        size.filter(|(w, h)| *w > 0 && *h > 0).map_or(UNKNOWN_RATIO, |(w, h)| f64::from(w) / f64::from(h))
    }

    /// The tiles of the page from `start`.
    fn wall_page(&self, wall: &Wall, start: usize, window: (u32, u32)) -> Vec<Tile> {
        let columns = self.wall_columns(wall, window);
        let ratios = self.list.iter().skip(start).map(|&id| self.wall_ratio(id));
        let size = (window.0 as i32, window.1 as i32);
        pack(ratios, size, columns).into_iter().enumerate().map(|(at, (place, step))| Tile { pos: start + at, place, step }).collect()
    }

    /// The tiles of the page on show.
    pub(super) fn wall_tiles(&self, window: (u32, u32)) -> Vec<Tile> {
        self.wall().map_or_else(Vec::new, |wall| self.wall_page(wall, wall.start, window))
    }

    /// The list position of the tile under a window position.
    pub(super) fn wall_tile_at(&self, x: f64, y: f64) -> Option<usize> {
        let (x, y) = (x as i32, y as i32);
        let tiles = self.wall_tiles(self.window_size());
        tiles.iter().find(|tile| (tile.place[0]..tile.place[0] + tile.place[2]).contains(&x) && (tile.place[1]..tile.place[1] + tile.place[3]).contains(&y)).map(|tile| tile.pos)
    }

    /// Where a list position's tile is, when it is on the page showing.
    pub(super) fn wall_place(&self, pos: usize) -> Option<[i32; 4]> {
        self.wall_tiles(self.window_size()).into_iter().find(|tile| tile.pos == pos).map(|tile| tile.place)
    }

    /// The positions whose thumbnails the wall wants: the page on show,
    /// then as many again after it and before it, to lay those out.
    pub(super) fn wall_wanted(&self) -> Vec<usize> {
        let Some(wall) = self.wall() else { return Vec::new() };
        let shown = self.wall_tiles(self.window_size()).len().max(8);
        let len = self.list.len();
        let (start, end) = (wall.start.min(len), (wall.start + shown).min(len));
        (start..end).chain(end..(end + 2 * shown).min(len)).chain(start.saturating_sub(shown)..start).collect()
    }

    /// Opens the wall, or closes it: from the grid it takes the grid's
    /// place on the same files; closed, it leaves for the file under its
    /// pointer as the grid does.
    pub(super) fn toggle_wall(&mut self) {
        if self.wall().is_some() {
            return self.flip_grid();
        }
        let from_grid = self.grid.is_some();
        if !from_grid {
            self.flip_grid();
        }
        let window = self.window_size();
        let layout = self.grid_layout(window);
        let Some(cursor) = self.grid.as_ref().map(|grid| grid.cursor) else { return };
        // From the grid, its page goes on in the wall; from a file, the
        // wall begins with it.
        let mut wall = Wall::at(if from_grid { cursor / layout.per_page() * layout.per_page() } else { cursor });
        if self.wall_page(&wall, wall.start, window).last().is_none_or(|tile| tile.pos < cursor) {
            wall.start = cursor;
        }
        if let Some(grid) = &mut self.grid {
            grid.wall = Some(wall);
        }
        // The video that was on view stops: everything on the wall plays.
        self.pip = false;
        self.trailer_due = None;
        self.request();
        self.sync_video();
        self.apply_video_view();
        self.redraw();
    }

    /// From the wall to the grid, on the same file.
    pub(super) fn wall_to_grid(&mut self) {
        let layout = self.grid_layout(self.window_size());
        if let Some(grid) = &mut self.grid {
            grid.wall = None;
            *grid = Grid { moved: true, ..Grid::at(grid.cursor, &layout) };
        }
        self.ring_snap.set(true);
        self.request();
        self.redraw();
    }

    /// More columns on the wall, or fewer.
    pub(super) fn zoom_wall(&mut self, larger: bool) {
        let window = self.window_size();
        let Some(wall) = self.wall() else { return };
        let columns = (self.wall_columns(wall, window) + if larger { -1 } else { 1 }).clamp(2, 16);
        if let Some(wall) = self.wall_mut() {
            wall.columns = Some(columns);
            wall.back.clear();
        }
        keep_columns(columns);
        self.toast(format!("{columns} across"));
        self.request();
        self.redraw();
    }

    /// The pages turning by themselves, or not.
    pub(super) fn wall_autoplay(&mut self) {
        let Some(wall) = self.wall_mut() else { return };
        wall.auto = !wall.auto;
        wall.next_at = wall.auto.then(|| Instant::now() + WALL_HOLD);
        let on = wall.auto;
        self.toast(if on { format!("Pages turn · every {} s", WALL_HOLD.as_secs()) } else { "Pages stay".to_string() });
        self.redraw();
    }

    /// Turns the page, forward or back. At the end, a page turning by
    /// itself goes round to the first.
    pub(super) fn turn_wall(&mut self, forward: bool) {
        let window = self.window_size();
        let len = self.list.len();
        let Some(wall) = self.wall() else { return };
        let leaving = self.wall_page(wall, wall.start, window);
        let end = leaving.last().map_or(wall.start, |tile| tile.pos + 1);
        let start = match forward {
            true if end < len => Some(end),
            true if wall.auto && wall.start > 0 => Some(0),
            true => None,
            false if wall.start == 0 => None,
            false => {
                // The page it came from, or as many as fill a page before it.
                let gone_back = wall.back.last().copied().filter(|at| *at < wall.start);
                Some(gone_back.unwrap_or_else(|| {
                    let ratios = self.list[..wall.start].iter().rev().map(|&id| self.wall_ratio(id));
                    let count = pack(ratios, (window.0 as i32, window.1 as i32), self.wall_columns(wall, window)).len();
                    wall.start.saturating_sub(count.max(1))
                }))
            }
        };
        let Some(start) = start else {
            if let Some(wall) = self.wall_mut() {
                wall.next_at = None;
                wall.auto = false;
            }
            return;
        };
        let pointer = self.mouse;
        if let Some(wall) = self.wall_mut() {
            match forward {
                true if start == 0 => wall.back.clear(),
                true => wall.back.push(wall.start),
                false => drop(wall.back.pop()),
            }
            wall.turn = Some(Turn { tiles: leaving, since: Instant::now(), forward });
            wall.start = start;
            wall.next_at = wall.auto.then(|| Instant::now() + Duration::from_secs_f64(SPAN * 2.0) + WALL_HOLD);
        }
        if let Some(grid) = &mut self.grid {
            grid.cursor = start.min(len.saturating_sub(1));
        }
        // The tile now under the pointer is the one picked out.
        if self.wall_tile_at(pointer.0, pointer.1).is_some() {
            self.point_cursor();
        }
        self.request();
        self.redraw();
    }

    /// Keeps the page on show holding the cursor, after it has been moved
    /// some other way than by turning the page.
    pub(super) fn wall_follow(&mut self) {
        let window = self.window_size();
        let (Some(wall), Some(cursor)) = (self.wall(), self.grid.as_ref().map(|grid| grid.cursor)) else { return };
        let tiles = self.wall_page(wall, wall.start, window);
        let on_page = tiles.first().is_some_and(|tile| tile.pos <= cursor) && tiles.last().is_some_and(|tile| tile.pos >= cursor);
        if !on_page {
            if let Some(wall) = self.wall_mut() {
                (wall.start, wall.turn) = (cursor, None);
                wall.back.clear();
            }
        }
    }

    /// The keys that mean something else on the wall: whether this was one.
    pub(super) fn wall_key(&mut self, key: &Key<&str>, event_loop: &ActiveEventLoop) -> bool {
        if self.wall().is_none() {
            return false;
        }
        let ctrl = self.modifiers.control_key();
        match key {
            Key::Named(NamedKey::Escape) => self.toggle_wall(),
            Key::Named(NamedKey::Tab) => self.execute(Command::GridMenu, event_loop),
            Key::Named(NamedKey::Space) => self.wall_autoplay(),
            Key::Named(NamedKey::PageDown | NamedKey::ArrowRight | NamedKey::ArrowDown) => self.turn_wall(true),
            Key::Named(NamedKey::PageUp | NamedKey::ArrowLeft | NamedKey::ArrowUp | NamedKey::Backspace) => self.turn_wall(false),
            Key::Named(NamedKey::Home) => {
                if let Some(wall) = self.wall_mut() {
                    (wall.start, wall.turn) = (0, None);
                    wall.back.clear();
                }
                self.request();
                self.redraw();
            }
            Key::Character("+" | "=") if ctrl => self.zoom_wall(true),
            Key::Character("-" | "_") if ctrl => self.zoom_wall(false),
            Key::Character("G") => self.toggle_wall(),
            Key::Character("g") if !ctrl => self.wall_to_grid(),
            // Titles are the grid's; the wall has none.
            Key::Character("i") => {}
            _ => return false,
        }
        true
    }

    /// Keeps a player on each video of the page, the sound on for the one
    /// under the pointer, and the turn going. Called on each redraw;
    /// nothing here waits on a player.
    pub(super) fn tend_wall(&mut self) {
        let window = self.window_size();
        let Some(wall) = self.wall() else { return };
        let tiles = self.wall_page(wall, wall.start, window);
        if let Some(turn) = &wall.turn {
            let (_, over) = turn_times(&turn.tiles, &tiles);
            if turn.since.elapsed().as_secs_f64() >= over {
                if let Some(wall) = self.wall_mut() {
                    wall.turn = None;
                }
            } else {
                self.grid_due = Some(Instant::now() + Duration::from_millis(8));
            }
        }
        // A player a core, between 4 and 24: past that, stills.
        let most = std::thread::available_parallelism().map_or(4, |cores| cores.get()).clamp(4, 24);
        let cursor = self.grid.as_ref().map(|grid| grid.cursor);
        let videos: Vec<&Tile> = tiles.iter().filter(|tile| files::is_video(&self.files[self.list[tile.pos]]) && !self.not_playable(self.list[tile.pos])).collect();
        let mut wanted: Vec<(usize, (u32, u32))> = videos.iter().take(most).map(|tile| (self.list[tile.pos], self.wall_bounds(tile))).collect();
        // The one under the pointer plays, whatever else has to give way.
        if let Some(tile) = videos.iter().skip(most).find(|tile| Some(tile.pos) == cursor) {
            wanted.pop();
            wanted.push((self.list[tile.pos], self.wall_bounds(tile)));
        }
        let hovered = self.wall_tile_at(self.mouse.0, self.mouse.1).filter(|_| self.pointer_inside()).map(|pos| self.list[pos]);
        // Off-screen runs and benchmarks are silent, as the main player is.
        let heard_id = hovered.filter(|_| !self.muted && !*SILENT && self.bench.is_none());
        let proxy = self.proxy.clone();
        let debug = self.options.debug;
        let paths: Vec<PathBuf> = wanted.iter().map(|(id, _)| self.files[*id].clone()).collect();
        let Some(wall) = self.wall_mut() else { return };
        while wall.players.len() < wanted.len() {
            let proxy = proxy.clone();
            wall.players.push(TilePlayer { trailers: Trailers::new(true, move |note| proxy.send_event(Event::Trailer(note)).is_ok()), id: None, frame: None, frames: 0 });
        }
        // Those already on a wanted video stay on it; the rest move on.
        let mut free: Vec<usize> = (0..wall.players.len()).filter(|&at| wall.players[at].id.is_none_or(|id| !wanted.iter().any(|(want, _)| *want == id))).collect();
        for ((id, bounds), path) in wanted.iter().zip(paths) {
            if wall.players.iter().any(|player| player.id == Some(*id)) {
                continue;
            }
            let Some(at) = free.pop() else { break };
            let player = &mut wall.players[at];
            player.trailers.play(*id, path, *bounds, None);
            (player.id, player.frame) = (Some(*id), None);
            if wall.heard == Some(at) {
                player.trailers.sound(false);
                wall.heard = None;
            }
        }
        for at in free {
            let player = &mut wall.players[at];
            if player.id.take().is_some() {
                player.trailers.stop();
                player.frame = None;
            }
        }
        let heard = heard_id.and_then(|id| wall.players.iter().position(|player| player.id == Some(id)));
        if heard != wall.heard {
            if let Some(was) = wall.heard {
                wall.players[was].trailers.sound(false);
            }
            if let Some(now) = heard {
                wall.players[now].trailers.sound(true);
            }
            wall.heard = heard;
        }
        // With --debug, every 2 s: how many frames a second each player
        // has given, `-` for one with no video.
        let every = wall.told.elapsed();
        if debug && every >= Duration::from_secs(2) {
            let rates: Vec<String> = wall.players.iter_mut().map(|player| {
                let rate = f64::from(std::mem::take(&mut player.frames)) / every.as_secs_f64();
                if player.id.is_some() { format!("{rate:.0}") } else { "-".to_string() }
            }).collect();
            wall.told = Instant::now();
            eprintln!("jog: wall    {} players, frames a second: {}", rates.len(), rates.join(" "));
        }
    }

    /// Whether the pointer is over the window.
    fn pointer_inside(&self) -> bool {
        let (w, h) = self.window_size();
        (0.0..f64::from(w)).contains(&self.mouse.0) && (0.0..f64::from(h)).contains(&self.mouse.1)
    }

    /// A video the player has crashed on decoding in software: shown by
    /// its thumbnail alone.
    fn not_playable(&self, id: usize) -> bool {
        crate::mpv::crashed_on(&self.files[id]) == Some(crate::mpv::Crash::Software)
    }

    /// How large a tile's video is drawn by its player: just covering the tile.
    fn wall_bounds(&self, tile: &Tile) -> (u32, u32) {
        let ratio = self.wall_ratio(self.list[tile.pos]);
        let (w, h) = (f64::from(tile.place[2].max(1)), f64::from(tile.place[3].max(1)));
        let (w, h) = if ratio > w / h { (h * ratio, h) } else { (w, w / ratio) };
        (w.ceil().max(1.0) as u32, h.ceil().max(1.0) as u32)
    }

    /// Takes the frames the wall's players have ready. Whether there were any.
    pub(super) fn take_wall_frames(&mut self) -> bool {
        let Some(wall) = self.wall_mut() else { return false };
        let mut any = false;
        for player in &mut wall.players {
            if let Some((id, frame)) = player.trailers.take() {
                if player.id == Some(id) {
                    player.frame = Some(frame);
                    player.frames += 1;
                    any = true;
                }
            }
        }
        any
    }

    /// Whether the player of a file has its sound on.
    fn wall_heard(&self, id: usize) -> bool {
        self.wall().and_then(|wall| wall.heard.map(|at| wall.players[at].id == Some(id))).unwrap_or(false)
    }

    pub(super) fn draw_wall(&self, canvas: &mut Canvas) {
        let Some(wall) = self.wall() else { return };
        let window = (canvas.width as u32, canvas.height as u32);
        let tiles = self.wall_page(wall, wall.start, window);
        let lift = f64::from((canvas.height / 60).max(12));
        let draw = |canvas: &mut Canvas, tile: &Tile, dy: f64, shown: f64| {
            let id = self.list[tile.pos];
            let [x, y, w, h] = tile.place;
            let y = y + dy.round() as i32;
            let playing = wall.players.iter().find(|player| player.id == Some(id)).and_then(|player| player.frame.as_ref()).filter(|frame| !frame.pixels.is_empty());
            let thumb = self.loader.thumb(id);
            match playing.or(thumb.as_deref()) {
                Some(frame) => canvas.image_cover(frame, x, y, w, h),
                None => canvas.fill(x, y, w, h, PLACEHOLDER_COLOR),
            }
            canvas.round_off(x, y, w, h, RADIUS, GRID_COLOR);
            if self.marks.contains(&id) {
                let dot = (canvas.height / 90).max(8);
                canvas.round_fill(x + dot / 2, y + dot / 2, dot, dot, dot / 2, MARK_COLOR, 256);
            }
            if shown < 1.0 {
                canvas.round_fill(x, y, w, h, RADIUS, GRID_COLOR, ((1.0 - shown) * 256.0) as u32);
            }
        };
        match &wall.turn {
            Some(turn) => {
                let t = turn.since.elapsed().as_secs_f64();
                let (arrive, _) = turn_times(&turn.tiles, &tiles);
                let way = if turn.forward { 1.0 } else { -1.0 };
                for tile in &turn.tiles {
                    let part = ((t - f64::from(tile.step) * STAGGER) / (SPAN * 0.5)).clamp(0.0, 1.0);
                    if part < 1.0 {
                        let eased = ease_in(part);
                        draw(canvas, tile, -way * lift * eased, 1.0 - eased);
                    }
                }
                for tile in &tiles {
                    let part = ((t - arrive - f64::from(tile.step) * STAGGER) / (SPAN * 0.6)).clamp(0.0, 1.0);
                    if part > 0.0 {
                        let eased = ease_out(part);
                        draw(canvas, tile, way * lift * 1.2 * (1.0 - eased), eased);
                    }
                }
            }
            None => {
                for tile in &tiles {
                    draw(canvas, tile, 0.0, 1.0);
                }
            }
        }
        // While the pages turn by themselves, a line along the top fills
        // up until the next turn.
        if let Some(next) = wall.next_at.filter(|_| wall.auto && wall.turn.is_none()) {
            let left = next.saturating_duration_since(Instant::now()).as_secs_f64() / WALL_HOLD.as_secs_f64();
            let thick = (canvas.height / 400).max(2);
            canvas.fill(0, 0, (f64::from(canvas.width) * (1.0 - left.min(1.0))) as i32, thick, accent_soft());
            self.animating.set(true);
        }
    }

    /// The words on the wall: the name of the file under the pointer, at
    /// the foot of its tile, and a speaker on it while it is heard.
    pub(super) fn draw_wall_labels(&mut self, canvas: &mut Canvas, text: &mut Text, px: u32) {
        self.wall_name = None;
        if self.wall().is_some_and(|wall| wall.turn.is_some()) || !self.pointer_inside() {
            return;
        }
        let Some(pos) = self.wall_tile_at(self.mouse.0, self.mouse.1) else { return };
        let Some([x, y, w, h]) = self.wall_place(pos) else { return };
        let id = self.list[pos];
        canvas.round_outline(x, y, w, h, RADIUS, 2, accent_soft());
        let line = Text::line_height(px);
        let pad = px as i32 / 2;
        let band = (line + pad * 3).min(h);
        canvas.fade_down(x + 2, y + h - band, w - 4, band - 2, 70);
        let name = clipped(text, &title_of(&self.files[id]), w - 2 * pad, px);
        let (left, top) = (x + pad, y + h - line - pad);
        // A click on the name copies it; under the pointer it says so by
        // a pill behind it.
        let place = [left - pad / 2, top - pad / 3, text.width(&name, px) + pad, line + pad * 2 / 3];
        let [px0, py0, pw, ph] = place;
        let over = (px0..px0 + pw).contains(&(self.mouse.0 as i32)) && (py0..py0 + ph).contains(&(self.mouse.1 as i32));
        if over {
            canvas.round_fill(px0, py0, pw, ph, ph / 2, 0x000000, 170);
        }
        text.draw(canvas, left, top, px, TEXT_COLOR, &name);
        self.wall_name = Some(place);
        if self.wall_heard(id) {
            let icon = commands::icons::VOLUME_2;
            let side = line + pad;
            let (left, top) = (x + w - side - pad, y + pad);
            canvas.round_fill(left, top, side, side, side / 2, 0x000000, 170);
            let icon_w = text.width(icon, px);
            text.draw(canvas, left + (side - icon_w) / 2, top + (side - line) / 2, px, TEXT_COLOR, icon);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ratios(shapes: &[f64]) -> impl Iterator<Item = f64> + '_ {
        shapes.iter().copied()
    }

    #[test]
    fn a_page_fills_the_window_edge_to_edge() {
        let shapes: Vec<f64> = (0..200).map(|at| [9.0 / 16.0, 16.0 / 9.0, 1.0, 4.0 / 5.0][at % 4]).collect();
        let places = pack(ratios(&shapes), (1600, 1000), 5);
        assert!(places.len() > 5 && places.len() < 200);
        // Every column reaches the bottom, and its tiles do not overlap.
        let mut columns: Vec<Vec<[i32; 4]>> = vec![Vec::new(); 5];
        for (place, _) in &places {
            let column = columns.iter().position(|col| col.first().is_none_or(|first: &[i32; 4]| first[0] == place[0])).unwrap();
            columns[column].push(*place);
        }
        for column in &columns {
            assert_eq!(column.first().unwrap()[1], 0);
            let last = column.last().unwrap();
            assert!((last[1] + last[3] - 1000).abs() <= 1, "column ends at {}", last[1] + last[3]);
            for pair in column.windows(2) {
                assert_eq!(pair[0][1] + pair[0][3] + GAP, pair[1][1], "{pair:?}");
            }
        }
        // The columns span the width.
        let right = places.iter().map(|(place, _)| place[0] + place[2]).max().unwrap();
        assert_eq!(right, 1600);
    }

    #[test]
    fn the_wave_starts_at_the_top_left() {
        let places = pack(std::iter::repeat(1.0), (900, 900), 3);
        assert_eq!(places[0], ([0, 0, 298, places[0].0[3]], 0));
        assert!(places.iter().filter(|(_, step)| *step == 0).all(|(place, _)| place[0] == 0 && place[1] == 0));
    }

    #[test]
    fn a_short_last_page_is_left_as_it_is() {
        let places = pack(ratios(&[1.0, 1.0]), (1000, 1000), 4);
        assert_eq!(places.len(), 2);
        assert!(places.iter().all(|(place, _)| place[3] == place[2]));
    }

    #[test]
    fn one_very_tall_picture_still_takes_a_column() {
        let places = pack(ratios(&[0.1, 1.0, 1.0]), (300, 300), 1);
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].0, [0, 0, 300, 300]);
    }
}
