use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::decode::{self, Source, Spec};
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
    /// Image to also decode at full resolution, for zooming in.
    pub full: Option<usize>,
    /// Images to make thumbnails of, most urgent first.
    pub thumbs: Vec<usize>,
    pub thumb_size: (u32, u32),
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

/// A decode result and the size and transform it was made for. Failures are
/// kept too, so a broken file is not retried on every request.
struct Slot {
    frame: Result<Arc<Frame>, String>,
    size: (u32, u32),
    transform: Transform,
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
        let fits = |s: &Slot| if at_least { s.size.0 >= size.0 && s.size.1 >= size.1 } else { s.size == size };
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
        // the neighbours, then thumbnails.
        let mut jobs = fitted.clone().take(1).chain(full).chain(fitted.skip(1)).chain(thumbs);
        jobs.find(|&job| self.needs(job))
    }

    fn source_for(&self, id: usize) -> Option<Arc<Source>> {
        self.source.as_ref().filter(|(source_id, _)| *source_id == id).map(|(_, source)| source.clone())
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
        let spec = Spec { target, transform, background, thumbnail: matches!(job, Job::Thumb(_)) };

        let fitted = self.fitted.get(&id).filter(|slot| slot.transform == transform);
        let fitted = fitted.and_then(|slot| slot.frame.as_ref().ok()).cloned();
        let work = match (job, fitted, self.source_for(id)) {
            (Job::Thumb(_), Some(frame), _) => Work::FromFrame(frame),
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
}

/// Decodes the current image and its neighbours on background threads so
/// that stepping through a directory never waits on a decode.
pub struct Loader {
    shared: Arc<Shared>,
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
        let shared = Arc::new(Shared { background, state, wake: Condvar::new() });
        let on_loaded = Arc::new(on_loaded);
        let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_WORKERS);

        for index in 0..workers {
            let shared = shared.clone();
            let on_loaded = on_loaded.clone();
            let thumbs_allowed = index >= RESERVED_WORKERS.min(workers - 1);
            std::thread::Builder::new()
                .name("jog-decode".into())
                .spawn(move || worker(&shared, &*on_loaded, thumbs_allowed, debug))
                .expect("failed to spawn decode thread");
        }
        Self { shared }
    }

    /// Replaces what is wanted. What was decoded stays cached, within a
    /// memory budget, so coming back to an image is instant.
    pub fn request(&self, request: Request) {
        let mut state = self.shared.state.lock().unwrap();
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
        state.request = request;
        drop(state);
        self.shared.wake.notify_all();
    }

    /// Sets the user's rotation for an image. Its frames are redone.
    pub fn set_transform(&self, id: usize, transform: Transform) {
        self.shared.state.lock().unwrap().transforms.insert(id, transform);
        self.shared.wake.notify_all();
    }

    /// Records that a file now lives at `path`. What was decoded stays valid.
    pub fn set_path(&self, id: usize, path: PathBuf) {
        self.shared.state.lock().unwrap().files[id] = path;
    }

    /// Forgets everything decoded for `id`, so it is read from disk again.
    pub fn invalidate(&self, id: usize) {
        let mut state = self.shared.state.lock().unwrap();
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
        let state = self.shared.state.lock().unwrap();
        let slot = state.fitted.get(&id)?;
        (slot.frame.is_err() || slot.transform == state.transform(id)).then(|| slot.frame.clone())
    }

    pub fn full(&self, id: usize) -> Option<Arc<Frame>> {
        let state = self.shared.state.lock().unwrap();
        let (full_id, slot) = state.full.as_ref()?;
        let usable = *full_id == id && slot.transform == state.transform(id);
        slot.frame.as_ref().ok().filter(|_| usable).cloned()
    }

    pub fn thumb(&self, id: usize) -> Option<Arc<Frame>> {
        let state = self.shared.state.lock().unwrap();
        let slot = state.thumbs.get(&id)?;
        slot.frame.as_ref().ok().filter(|_| slot.transform == state.transform(id)).cloned()
    }
}

fn worker(shared: &Shared, on_loaded: &(dyn Fn(Job) + Send + Sync), thumbs_allowed: bool, debug: bool) {
    let mut state = shared.state.lock().unwrap();
    loop {
        let Some(job) = state.next_job(thumbs_allowed) else {
            state = shared.wake.wait(state).unwrap();
            continue;
        };
        let (spec, work, keep) = state.plan(job, shared.background);
        state.busy.insert(job);
        let path = state.files[job.id()].clone();
        drop(state);

        let started = Instant::now();
        let how = match &work {
            Work::Decode(_) => "",
            Work::FromSource(_) => " (from decoded)",
            Work::FromFrame(_) => " (from frame)",
        };
        let (result, source) = perform(work, spec, keep);
        if debug {
            let kind = match job {
                Job::Fitted(_) => "fit",
                Job::Full(_) => "full",
                Job::Thumb(_) => "thumb",
            };
            eprintln!("jog: {:>7.1?} {kind:<5} {}{how}", started.elapsed(), path.display());
        }
        let frame = result.map(Arc::new).map_err(|error| {
            if matches!(job, Job::Fitted(_)) {
                eprintln!("jog: {}: {error}", path.display());
            }
            error.to_string()
        });
        let slot = Slot { frame, size: spec.target.unwrap_or_default(), transform: spec.transform, used: 0 };

        state = shared.state.lock().unwrap();
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
/// larger than the frame wanted, the decoded file.
fn perform(work: Work, spec: Spec, keep: bool) -> (decode::Result<Frame>, Option<Arc<Source>>) {
    match work {
        Work::Decode(path) => {
            let source = match decode::read(&path, spec) {
                Ok(source) => source,
                Err(error) => return (Err(error), None),
            };
            let frame = decode::finish(&source, spec);
            let larger = spec.target.is_none_or(|(w, h)| source.width() > w || source.height() > h);
            (frame, (keep && larger).then(|| Arc::new(source)))
        }
        Work::FromSource(source) => (decode::finish(&source, spec), None),
        Work::FromFrame(frame) => (decode::shrink(&frame, spec.target.unwrap_or_default()), None),
    }
}
