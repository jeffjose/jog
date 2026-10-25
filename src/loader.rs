use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::decode::{self, Source, Spec};
use crate::ultrahdr::Hdr;
use crate::files;
use crate::frame::{Frame, Transform};

const MAX_WORKERS: usize = 16;
/// Workers that never take thumbnail jobs, so a burst of thumbnails cannot
/// delay the image the user is waiting for.
const RESERVED_WORKERS: usize = 2;
/// Memory for fitted frames beyond the ones on and around the screen, so
/// that going back to an image rarely decodes it again.
const FITTED_BUDGET: usize = 512 << 20;
/// Memory for thumbnails. They are small, so this holds many hundreds.
const THUMB_BUDGET: usize = 256 << 20;

/// What the viewer currently needs decoded. File ids index the file list.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct Request {
    /// Images to fit to `target`, most urgent first. The first is on screen.
    pub wanted: Vec<usize>,
    pub target: (u32, u32),
    /// Whether images smaller than `target` are scaled up to it, as they
    /// are shown; done here, showing one is a copy rather than a scaling.
    pub enlarge: bool,
    /// Image to also decode at full resolution, for zooming in.
    pub full: Option<usize>,
    /// Images to make thumbnails of, most urgent first.
    pub thumbs: Vec<usize>,
    pub thumb_size: (u32, u32),
    /// Which rendition of Ultra HDR pictures to show.
    pub hdr: Hdr,
    /// Videos to be picked up where they were left, by how many seconds
    /// in, in milliseconds: their stills are the keyframes playback
    /// begins on there.
    pub starts: Vec<(usize, u64)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Job {
    Fitted(usize),
    Full(usize),
    Thumb(usize),
}

impl Job {
    fn id(self) -> usize {
        match self {
            Job::Fitted(id) | Job::Full(id) | Job::Thumb(id) => id,
        }
    }
}

/// How a frame came to be and what it cost, for the debug overlay.
#[derive(Clone, Copy)]
pub struct Made {
    /// What it was made from when not from the file: `"from decoded"` or
    /// `"from frame"`. Empty when the file was read.
    pub how: &'static str,
    /// Time spent reading and decoding the file.
    pub read: Duration,
    /// Time spent scaling and turning what was decoded.
    pub fit: Duration,
    pub at: Instant,
    /// What the file's first bytes say it is, and whether it is an Ultra
    /// HDR photo, as found while it was read; nothing for a thumbnail.
    pub format: Option<&'static str>,
    pub ultra_hdr: bool,
}

/// What the loader is holding, for the debug overlay.
pub struct Stats {
    /// The frame for the image asked about, and its full-resolution one.
    pub fitted: Option<Made>,
    pub full: Option<Made>,
    /// Count and bytes of the fitted frames and of the thumbnails kept.
    pub fitted_kept: (usize, usize),
    pub thumbs_kept: (usize, usize),
    /// Jobs being worked on right now.
    pub busy: usize,
}

/// A decode result and the size and transform it was made for. Failures are
/// kept too, so a broken file is not retried on every request.
struct Slot {
    frame: Result<Arc<Frame>, String>,
    size: (u32, u32),
    enlarge: bool,
    hdr: Hdr,
    transform: Transform,
    made: Made,
    /// When it was last asked for. The lowest goes first when room is needed.
    used: u64,
}

impl Slot {
    fn bytes(&self) -> usize {
        self.frame.as_ref().map_or(0, |frame| frame.pixels.len() * 4)
    }
}

/// How a job gets done: from the file, or from something already decoded.
enum Work {
    Decode(PathBuf),
    /// Fit and turn an image that is decoded at full resolution.
    FromSource(Arc<Source>),
    /// Shrink a frame that is already fitted and turned.
    FromFrame(Arc<Frame>),
}

#[derive(Default)]
struct State {
    /// Path of each file id. Renaming a file changes its entry.
    files: Vec<PathBuf>,
    request: Request,
    transforms: HashMap<usize, Transform>,
    fitted: HashMap<usize, Slot>,
    thumbs: HashMap<usize, Slot>,
    full: Option<(usize, Slot)>,
    /// The decoded file behind the image on view, kept so that zooming in
    /// or turning it needs no second decode.
    source: Option<(usize, Arc<Source>)>,
    busy: HashSet<Job>,
    /// Counts requests, to tell which slots were asked for most recently.
    clock: u64,
}

impl State {
    fn transform(&self, id: usize) -> Transform {
        self.transforms.get(&id).copied().unwrap_or_default()
    }

    /// Whether a slot serves `id` at `size`. A thumbnail at least as large
    /// as wanted will do: it is drawn shrunk to its cell anyway.
    fn is_current(&self, slot: Option<&Slot>, id: usize, size: (u32, u32), at_least: bool) -> bool {
        let fits = |s: &Slot| match at_least {
            true => s.size.0 >= size.0 && s.size.1 >= size.1,
            false => s.size == size && s.enlarge == self.request.enlarge && s.hdr == self.request.hdr,
        };
        slot.is_some_and(|s| s.frame.is_err() || (fits(s) && s.transform == self.transform(id)))
    }

    fn needs(&self, job: Job) -> bool {
        if self.busy.contains(&job) {
            return false;
        }
        match job {
            Job::Fitted(id) => !self.is_current(self.fitted.get(&id), id, self.request.target, false),
            Job::Thumb(id) => {
                // A thumbnail of an image whose fitted frame is on its way
                // waits for that frame, to be shrunk from it in no time.
                let fitted_coming = self.request.wanted.contains(&id)
                    && !self.is_current(self.fitted.get(&id), id, self.request.target, false);
                !fitted_coming && !self.is_current(self.thumbs.get(&id), id, self.request.thumb_size, true)
            }
            Job::Full(id) => {
                let slot = self.full.as_ref().filter(|(full_id, _)| *full_id == id).map(|(_, s)| s);
                let failed = self.fitted.get(&id).is_some_and(|s| s.frame.is_err());
                !failed && !self.is_current(slot, id, (0, 0), false)
            }
        }
    }

    fn next_job(&self, thumbs_allowed: bool) -> Option<Job> {
        let request = &self.request;
        let fitted = request.wanted.iter().map(|&id| Job::Fitted(id));
        let full = request.full.map(Job::Full);
        let thumbs = request.thumbs.iter().filter(|_| thumbs_allowed).map(|&id| Job::Thumb(id));
        // The on-screen image first, then its full-resolution version, then
        // the neighbors, then thumbnails.
        let mut jobs = fitted.clone().take(1).chain(full).chain(fitted.skip(1)).chain(thumbs);
        jobs.find(|&job| self.needs(job))
    }

    fn source_for(&self, id: usize) -> Option<Arc<Source>> {
        // One decoded for the ordinary rendition alone has no gain map to
        // make another from.
        let usable = |source: &Source| self.request.hdr == Hdr::Sdr || source.looked;
        self.source.as_ref().filter(|(source_id, source)| *source_id == id && usable(source)).map(|(_, source)| source.clone())
    }

    /// Decides what a job is for and the cheapest way to do it. The last
    /// value says whether to keep the decoded file for later jobs on it.
    fn plan(&self, job: Job, background: u32) -> (Spec, Work, bool) {
        let id = job.id();
        let target = match job {
            Job::Fitted(_) => Some(self.request.target),
            Job::Thumb(_) => Some(self.request.thumb_size),
            Job::Full(_) => None,
        };
        let transform = self.transform(id);
        // Only a fitted frame is ever enlarged; the flag is kept on a
        // full-resolution one all the same, so that it counts as current.
        let enlarge = !matches!(job, Job::Thumb(_)) && self.request.enlarge;
        // A thumbnail is the ordinary rendition, whatever is on view.
        let thumbnail = matches!(job, Job::Thumb(_));
        let hdr = if thumbnail { Hdr::Sdr } else { self.request.hdr };
        // A video picked up where it was left: its still is the keyframe
        // playback begins on there.
        let start = (!thumbnail).then(|| self.request.starts.iter().find(|(at, _)| *at == id).map(|(_, ms)| *ms as f64 / 1000.0)).flatten();
        let spec = Spec { target, enlarge, transform, background, thumbnail, hdr, start };

        // A thumbnail is made from the fitted frame only when that is the
        // ordinary rendition.
        let fitted = self.fitted.get(&id).filter(|slot| slot.transform == transform && slot.hdr == Hdr::Sdr);
        let fitted = fitted.and_then(|slot| slot.frame.as_ref().ok()).cloned();
        // A video's fitted frame is its opening one, which is often black;
        // its thumbnail is taken from further in instead.
        let own_still = matches!(job, Job::Thumb(_)) && files::is_video(&self.files[id]);
        let work = match (job, fitted, self.source_for(id)) {
            (Job::Thumb(_), Some(frame), _) if !own_still => Work::FromFrame(frame),
            (_, _, Some(source)) if !own_still => Work::FromSource(source),
            (_, _, Some(source)) => Work::FromSource(source),
            _ => Work::Decode(self.files[id].clone()),
        };
        // Keep the file behind the image on view, unless it is a video,
        // whose still is all the frame there is, or a RAW file, whose
        // preview is chosen for the size wanted.
        let path = &self.files[id];
        let on_view = self.request.wanted.first() == Some(&id) && !matches!(job, Job::Thumb(_));
        let keep = on_view && !files::is_video(path) && !files::is_raw(path);
        (spec, work, keep)
    }

    /// Stores a finished decode unless the user has turned the image since.
    fn store(&mut self, job: Job, mut slot: Slot) -> bool {
        let id = job.id();
        if slot.transform != self.transform(id) {
            return false;
        }
        slot.used = self.clock;
        match job {
            Job::Fitted(_) => {
                self.fitted.insert(id, slot);
                let protected = self.request.wanted.clone();
                trim(&mut self.fitted, FITTED_BUDGET, &protected);
            }
            Job::Thumb(_) => {
                self.thumbs.insert(id, slot);
                let protected = self.request.thumbs.clone();
                trim(&mut self.thumbs, THUMB_BUDGET, &protected);
            }
            Job::Full(_) if self.request.full == Some(id) => self.full = Some((id, slot)),
            Job::Full(_) => return false,
        }
        true
    }
}

/// Drops the least recently wanted slots until the cache fits its budget.
/// Slots for `protected` ids stay whatever the budget says.
fn trim(cache: &mut HashMap<usize, Slot>, budget: usize, protected: &[usize]) {
    let mut total: usize = cache.values().map(Slot::bytes).sum();
    while total > budget {
        let oldest = cache
            .iter()
            .filter(|(id, _)| !protected.contains(id))
            .min_by_key(|(_, slot)| slot.used)
            .map(|(id, _)| *id);
        let Some(id) = oldest else { return };
        total -= cache.remove(&id).map_or(0, |slot| slot.bytes());
    }
}

struct Shared {
    background: u32,
    state: Mutex<State>,
    wake: Condvar,
    /// Whether each job is reported on the terminal as it finishes.
    debug: AtomicBool,
    /// The job for the image on view while its scaling is on the pool,
    /// which it has to itself: the neighbors' scaling waits, on `calm`,
    /// rather than take its turn in the way of what is being waited for.
    urgent: Mutex<Option<Job>>,
    calm: Condvar,
}

/// Decodes the current image and its neighbors on background threads so
/// that stepping through a directory never waits on a decode.
pub struct Loader {
    shared: Arc<Shared>,
    /// How many files can be decoded at once.
    workers: usize,
}

impl Loader {
    /// `on_loaded` is called from a worker thread for each job that finishes
    /// (or fails) and is still wanted.
    pub fn new(
        files: Vec<PathBuf>,
        background: u32,
        debug: bool,
        on_loaded: impl Fn(Job) + Send + Sync + 'static,
    ) -> Self {
        let state = Mutex::new(State { files, ..State::default() });
        let shared = Arc::new(Shared {
            background,
            state,
            wake: Condvar::new(),
            debug: AtomicBool::new(debug),
            urgent: Mutex::new(None),
            calm: Condvar::new(),
        });
        let on_loaded = Arc::new(on_loaded);
        let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_WORKERS);

        for index in 0..workers {
            let shared = shared.clone();
            let on_loaded = on_loaded.clone();
            let thumbs_allowed = index >= RESERVED_WORKERS.min(workers - 1);
            std::thread::Builder::new()
                .name("jog-decode".into())
                .spawn(move || {
                    give_way();
                    worker(&shared, &*on_loaded, thumbs_allowed)
                })
                .expect("failed to spawn decode thread");
        }
        Self { shared, workers }
    }

    /// How many files can be decoded at once: how far ahead it is worth
    /// asking for them.
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// Turns the terminal report of each finished job on or off.
    pub fn set_debug(&self, debug: bool) {
        self.shared.debug.store(debug, Ordering::Relaxed);
    }

    /// Replaces what is wanted. What was decoded stays cached, within a
    /// memory budget, so coming back to an image is instant.
    pub fn request(&self, request: Request) {
        let mut state = self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.clock += 1;
        let clock = state.clock;
        for id in request.wanted.iter().chain(&request.thumbs) {
            if let Some(slot) = state.fitted.get_mut(id) {
                slot.used = clock;
            }
            if let Some(slot) = state.thumbs.get_mut(id) {
                slot.used = clock;
            }
        }
        if state.full.as_ref().is_some_and(|(id, _)| Some(*id) != request.full) {
            state.full = None;
        }
        if state.source.as_ref().is_some_and(|(id, _)| !request.wanted.contains(id)) {
            state.source = None;
        }
        // A video whose still was of another start has it made again.
        for (id, seconds) in &request.starts {
            let had = state.request.starts.iter().find(|(at, _)| at == id).map(|(_, was)| *was);
            if had != Some(*seconds) {
                state.fitted.remove(id);
            }
        }
        state.request = request;
        drop(state);
        self.shared.wake.notify_all();
    }

    /// Sets the user's rotation for an image. Its frames are redone.
    pub fn set_transform(&self, id: usize, transform: Transform) {
        self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transforms.insert(id, transform);
        self.shared.wake.notify_all();
    }

    /// Records that a file now lives at `path`. What was decoded stays valid.
    pub fn set_path(&self, id: usize, path: PathBuf) {
        self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).files[id] = path;
    }

    /// Takes on another file, which gets the next id.
    pub fn add_file(&self, path: PathBuf) {
        self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).files.push(path);
    }

    /// Several at once, as a listing brings them in, the lock taken once.
    pub fn add_files(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).files.extend(paths);
    }

    /// Forgets everything decoded for `id`, so it is read from disk again.
    pub fn invalidate(&self, id: usize) {
        let mut state = self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.fitted.remove(&id);
        state.thumbs.remove(&id);
        if state.full.as_ref().is_some_and(|(full_id, _)| *full_id == id) {
            state.full = None;
        }
        if state.source.as_ref().is_some_and(|(source_id, _)| *source_id == id) {
            state.source = None;
        }
        drop(state);
        self.shared.wake.notify_all();
    }

    /// The fitted frame for `id`, the reason it failed, or `None` while it is
    /// still loading. A frame made for another screen size is still returned.
    pub fn fitted(&self, id: usize) -> Option<Result<Arc<Frame>, String>> {
        let state = self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = state.fitted.get(&id)?;
        (slot.frame.is_err() || slot.transform == state.transform(id)).then(|| slot.frame.clone())
    }

    pub fn full(&self, id: usize) -> Option<Arc<Frame>> {
        let state = self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (full_id, slot) = state.full.as_ref()?;
        let usable = *full_id == id && slot.transform == state.transform(id) && slot.hdr == state.request.hdr;
        slot.frame.as_ref().ok().filter(|_| usable).cloned()
    }

    pub fn thumb(&self, id: usize) -> Option<Arc<Frame>> {
        let state = self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = state.thumbs.get(&id)?;
        slot.frame.as_ref().ok().filter(|_| slot.transform == state.transform(id)).cloned()
    }

    /// What the file's first bytes said it was when its frame was made,
    /// and whether it is an Ultra HDR photo; nothing until then.
    pub fn sniffed(&self, id: usize) -> Option<(Option<&'static str>, bool)> {
        let state = self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = state.fitted.get(&id).filter(|slot| slot.frame.is_ok())?;
        Some((slot.made.format, slot.made.ultra_hdr))
    }

    pub fn stats(&self, id: usize) -> Stats {
        let state = self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let kept = |cache: &HashMap<usize, Slot>| (cache.len(), cache.values().map(Slot::bytes).sum());
        Stats {
            fitted: state.fitted.get(&id).map(|slot| slot.made),
            full: state.full.as_ref().filter(|(full_id, _)| *full_id == id).map(|(_, slot)| slot.made),
            fitted_kept: kept(&state.fitted),
            thumbs_kept: kept(&state.thumbs),
            busy: state.busy.len(),
        }
    }
}

/// The threads the scaling and turning that end a decode job spread
/// over; reading and decoding the file stays on the job's own thread.
/// They are kept apart from the ones drawing uses: a frame on its way to
/// the screen would otherwise wait its turn behind the pieces of a decode
/// that is only being done ahead of time.
fn decode_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        // Half the cores: the decode threads are each busy with a file
        // already, and on a machine with few cores more threads here
        // only take turns with the ones drawing the window.
        // `JOG_SCALE_THREADS` sets how many, for trying things out.
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let threads = std::env::var("JOG_SCALE_THREADS").ok().and_then(|n| n.parse().ok()).unwrap_or((cores / 2).max(1));
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("jog-scale-{index}"))
            .start_handler(|_| give_way())
            .build()
            .expect("failed to start the scaling threads")
    })
}

/// Lowers the calling thread's claim on the processor. Decoding is done
/// ahead of time on every core at once; the thread drawing the window,
/// and those helping it, are to have a core whenever they want one.
fn give_way() {
    // `JOG_DECODE_NICE` sets by how much, for trying things out.
    let nice = std::env::var("JOG_DECODE_NICE").ok().and_then(|n| n.parse().ok()).unwrap_or(10);
    // SAFETY: a plain system call on the calling thread, which on Linux is
    // what a process id of 0 names here.
    unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) };
}

fn worker(shared: &Shared, on_loaded: &(dyn Fn(Job) + Send + Sync), thumbs_allowed: bool) {
    let mut state = shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        let Some(job) = state.next_job(thumbs_allowed) else {
            state = shared.wake.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
            continue;
        };
        let (spec, work, keep) = state.plan(job, shared.background);
        // The image on view, or its full-resolution version: what is
        // being waited for.
        let urgent = !matches!(job, Job::Thumb(_)) && state.request.wanted.first() == Some(&job.id());
        state.busy.insert(job);
        let path = state.files[job.id()].clone();
        drop(state);

        let started = Instant::now();
        let how = match &work {
            Work::Decode(_) => "",
            Work::FromSource(_) => "from decoded",
            Work::FromFrame(_) => "from frame",
        };
        // A decoder that panics on a broken file fails that file, not the
        // thread: the rest go on decoding.
        let performed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| perform(shared, (job, urgent), work, spec, keep)));
        let (result, source, read, (format, ultra_hdr)) = performed.unwrap_or_else(|_| (Err("the decoder crashed on it".into()), None, started.elapsed(), (None, false)));
        let took = started.elapsed();
        let made = Made { how, read, fit: took.saturating_sub(read), at: Instant::now(), format, ultra_hdr };
        if shared.debug.load(Ordering::Relaxed) {
            let kind = match job {
                Job::Fitted(_) => "fit",
                Job::Full(_) => "full",
                Job::Thumb(_) => "thumb",
            };
            let how = if how.is_empty() { String::new() } else { format!(" ({how})") };
            eprintln!("jog: {took:>7.1?} {kind:<5} {}{how}", path.display());
        }
        let frame = result.map(Arc::new).map_err(|error| {
            if matches!(job, Job::Fitted(_)) {
                eprintln!("jog: {}: {error}", path.display());
            }
            error.to_string()
        });
        let slot = Slot { frame, size: spec.target.unwrap_or_default(), enlarge: spec.enlarge, hdr: spec.hdr, transform: spec.transform, made, used: 0 };

        state = shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.busy.remove(&job);
        if let Some(source) = source {
            state.source = Some((job.id(), source));
        }
        if state.store(job, slot) {
            on_loaded(job);
        }
        // Thumbnails may have been waiting for this frame.
        if matches!(job, Job::Fitted(_)) {
            shared.wake.notify_all();
        }
    }
}

/// Does a job. Returns the frame and, when asked to keep it and it is
/// larger than the frame wanted, the decoded file; then how long reading
/// and decoding the file took, which is no time when it was not read.
type Performed = (decode::Result<Frame>, Option<Arc<Source>>, Duration, (Option<&'static str>, bool));

fn perform(shared: &Shared, job: (Job, bool), work: Work, spec: Spec, keep: bool) -> Performed {
    match work {
        Work::Decode(path) => {
            let started = Instant::now();
            let source = match decode::read(&path, spec) {
                Ok(source) => source,
                Err(error) => return (Err(error), None, started.elapsed(), (None, false)),
            };
            let read = started.elapsed();
            let sniffed = (source.format, source.ultra_hdr);
            let frame = on_pool(shared, job, || decode::finish(&source, spec));
            // The file is kept when the frame is not its pixels as they
            // are, and so zooming in would want them. An animation is
            // never wanted at full resolution, and would take much more
            // room than a still.
            let scaled = frame.as_ref().is_ok_and(|frame| !frame.is_native());
            let worth_keeping = keep && scaled && source.animation.is_none();
            (frame, worth_keeping.then(|| Arc::new(source)), read, sniffed)
        }
        Work::FromSource(source) => {
            let sniffed = (source.format, source.ultra_hdr);
            (on_pool(shared, job, || decode::finish(&source, spec)), None, Duration::ZERO, sniffed)
        }
        Work::FromFrame(frame) => {
            (on_pool(shared, job, || decode::shrink(&frame, spec.target.unwrap_or_default())), None, Duration::ZERO, (None, false))
        }
    }
}

/// Runs the scaling of a job on the pool. The urgent job, the one for the
/// image on view, has the pool to itself: the others wait for it to be
/// done before they start, since on the pool they would all share the
/// cores alike, and the one waited for would take as long as the lot.
fn on_pool<R: Send>(shared: &Shared, (job, urgent): (Job, bool), work: impl FnOnce() -> R + Send) -> R {
    let mut first = shared.urgent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if urgent {
        *first = Some(job);
    } else {
        while first.is_some() {
            first = shared.calm.wait(first).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    drop(first);
    let result = decode_pool().install(work);
    if urgent {
        let mut first = shared.urgent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Another may have taken over as the one on view meanwhile.
        if *first == Some(job) {
            *first = None;
        }
        drop(first);
        shared.calm.notify_all();
    }
    result
}
