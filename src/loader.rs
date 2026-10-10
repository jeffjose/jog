use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::decode::{self, Spec};
use crate::frame::{Frame, Transform};

const MAX_WORKERS: usize = 8;
/// Workers that never take thumbnail jobs, so a burst of thumbnails cannot
/// delay the image the user is waiting for.
const RESERVED_WORKERS: usize = 2;

/// What the viewer currently needs decoded. File ids index the file list.
#[derive(Default)]
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

/// A decode result and the size and transform it was made for. Failures are
/// kept too, so a broken file is not retried on every request.
struct Slot {
    frame: Result<Arc<Frame>, String>,
    size: (u32, u32),
    transform: Transform,
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
    busy: HashSet<Job>,
}

impl State {
    fn transform(&self, id: usize) -> Transform {
        self.transforms.get(&id).copied().unwrap_or_default()
    }

    fn is_current(&self, slot: Option<&Slot>, id: usize, size: (u32, u32)) -> bool {
        slot.is_some_and(|s| s.frame.is_err() || (s.size == size && s.transform == self.transform(id)))
    }

    fn needs(&self, job: Job) -> bool {
        if self.busy.contains(&job) {
            return false;
        }
        match job {
            Job::Fitted(id) => !self.is_current(self.fitted.get(&id), id, self.request.target),
            Job::Thumb(id) => !self.is_current(self.thumbs.get(&id), id, self.request.thumb_size),
            Job::Full(id) => {
                let slot = self.full.as_ref().filter(|(full_id, _)| *full_id == id).map(|(_, s)| s);
                let failed = self.fitted.get(&id).is_some_and(|s| s.frame.is_err());
                !failed && !self.is_current(slot, id, (0, 0))
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

    fn spec(&self, job: Job, background: u32) -> (usize, Spec) {
        let (id, target) = match job {
            Job::Fitted(id) => (id, Some(self.request.target)),
            Job::Thumb(id) => (id, Some(self.request.thumb_size)),
            Job::Full(id) => (id, None),
        };
        (id, Spec { target, transform: self.transform(id), background })
    }

    /// Stores a finished decode unless the user has moved on from it.
    fn store(&mut self, job: Job, slot: Slot) -> bool {
        let id = match job {
            Job::Fitted(id) | Job::Full(id) | Job::Thumb(id) => id,
        };
        if slot.transform != self.transform(id) {
            return false;
        }
        match job {
            Job::Fitted(_) if self.request.wanted.contains(&id) => {
                self.fitted.insert(id, slot);
            }
            Job::Thumb(_) if self.request.thumbs.contains(&id) => {
                self.thumbs.insert(id, slot);
            }
            Job::Full(_) if self.request.full == Some(id) => self.full = Some((id, slot)),
            _ => return false,
        }
        true
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
    pub fn new(files: Vec<PathBuf>, background: u32, on_loaded: impl Fn(Job) + Send + Sync + 'static) -> Self {
        let state = Mutex::new(State { files, ..State::default() });
        let shared = Arc::new(Shared { background, state, wake: Condvar::new() });
        let on_loaded = Arc::new(on_loaded);
        let debug = std::env::var_os("JOG_DEBUG").is_some();
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

    /// Replaces what is wanted. Anything no longer wanted is evicted.
    pub fn request(&self, request: Request) {
        let mut state = self.shared.state.lock().unwrap();
        state.fitted.retain(|id, _| request.wanted.contains(id));
        state.thumbs.retain(|id, _| request.thumbs.contains(id));
        if state.full.as_ref().is_some_and(|(id, _)| Some(*id) != request.full) {
            state.full = None;
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
        let (id, spec) = state.spec(job, shared.background);
        state.busy.insert(job);
        let path = state.files[id].clone();
        drop(state);

        let started = Instant::now();
        let result = decode::load(&path, spec);
        if debug {
            let kind = match job {
                Job::Fitted(_) => "fit",
                Job::Full(_) => "full",
                Job::Thumb(_) => "thumb",
            };
            eprintln!("jog: {:>7.1?} {kind:<5} {}", started.elapsed(), path.display());
        }
        let frame = result.map(Arc::new).map_err(|error| {
            if matches!(job, Job::Fitted(_)) {
                eprintln!("jog: {}: {error}", path.display());
            }
            error.to_string()
        });
        let slot = Slot { frame, size: spec.target.unwrap_or_default(), transform: spec.transform };

        state = shared.state.lock().unwrap();
        state.busy.remove(&job);
        if state.store(job, slot) {
            on_loaded(job);
        }
    }
}
