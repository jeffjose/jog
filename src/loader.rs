use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::decode::{self, Frame};

/// How many images to keep decoded in the direction of travel and behind it.
const AHEAD: i64 = 3;
const BEHIND: i64 = 2;
const MAX_WORKERS: usize = 4;

enum Slot {
    /// A frame and the screen size it was fitted to.
    Ready(Arc<Frame>, (u32, u32)),
    Failed(String),
}

#[derive(Default)]
struct State {
    /// Indices worth having decoded, most urgent first.
    wanted: Vec<usize>,
    target: (u32, u32),
    cache: HashMap<usize, Slot>,
    busy: HashSet<usize>,
}

impl State {
    fn next_job(&self) -> Option<usize> {
        self.wanted.iter().copied().find(|i| {
            !self.busy.contains(i)
                && match self.cache.get(i) {
                    None => true,
                    Some(Slot::Ready(_, fitted_to)) => *fitted_to != self.target,
                    Some(Slot::Failed(_)) => false,
                }
        })
    }
}

struct Shared {
    files: Arc<[PathBuf]>,
    state: Mutex<State>,
    wake: Condvar,
}

/// Decodes the current image and its neighbours on background threads so
/// that stepping through a directory never waits on a decode.
pub struct Loader {
    shared: Arc<Shared>,
}

impl Loader {
    /// `on_loaded` is called from a worker thread with the index of each
    /// image that finishes (or fails) decoding.
    pub fn new(files: Arc<[PathBuf]>, on_loaded: impl Fn(usize) + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared { files, state: Mutex::default(), wake: Condvar::new() });
        let on_loaded = Arc::new(on_loaded);
        let debug = std::env::var_os("JOG_DEBUG").is_some();
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());

        for _ in 0..cores.min(MAX_WORKERS) {
            let shared = shared.clone();
            let on_loaded = on_loaded.clone();
            std::thread::Builder::new()
                .name("jog-decode".into())
                .spawn(move || worker(&shared, &*on_loaded, debug))
                .expect("failed to spawn decode thread");
        }
        Self { shared }
    }

    /// Makes `current` the most urgent image and queues its neighbours,
    /// favouring the direction of travel. Everything else is evicted.
    pub fn request(&self, current: usize, forward: bool, target: (u32, u32)) {
        let len = self.shared.files.len() as i64;
        let dir = if forward { 1 } else { -1 };
        let mut wanted = Vec::new();
        for step in 0..=AHEAD.max(BEHIND) {
            for offset in [step, -step] {
                if offset > AHEAD || offset < -BEHIND {
                    continue;
                }
                let index = (current as i64 + offset * dir).rem_euclid(len) as usize;
                if !wanted.contains(&index) {
                    wanted.push(index);
                }
            }
        }

        let mut state = self.shared.state.lock().unwrap();
        state.cache.retain(|index, _| wanted.contains(index));
        state.wanted = wanted;
        state.target = target;
        drop(state);
        self.shared.wake.notify_all();
    }

    /// The decoded frame for `index`, the reason it failed, or `None` while
    /// it is still loading.
    pub fn get(&self, index: usize) -> Option<Result<Arc<Frame>, String>> {
        match self.shared.state.lock().unwrap().cache.get(&index)? {
            Slot::Ready(frame, _) => Some(Ok(frame.clone())),
            Slot::Failed(reason) => Some(Err(reason.clone())),
        }
    }
}

fn worker(shared: &Shared, on_loaded: &(dyn Fn(usize) + Send + Sync), debug: bool) {
    let mut state = shared.state.lock().unwrap();
    loop {
        let Some(index) = state.next_job() else {
            state = shared.wake.wait(state).unwrap();
            continue;
        };
        let target = state.target;
        state.busy.insert(index);
        drop(state);

        let path = &shared.files[index];
        let started = Instant::now();
        let result = decode::load(path, target);
        if debug {
            eprintln!("jog: {:>7.1?} {}", started.elapsed(), path.display());
        }
        let slot = match result {
            Ok(frame) => Slot::Ready(Arc::new(frame), target),
            Err(error) => {
                eprintln!("jog: {}: {error}", path.display());
                Slot::Failed(error.to_string())
            }
        };

        state = shared.state.lock().unwrap();
        state.busy.remove(&index);
        // The user may have moved on while this was decoding.
        if state.wanted.contains(&index) {
            state.cache.insert(index, slot);
            on_loaded(index);
        }
    }
}
